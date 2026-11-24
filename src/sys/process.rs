//! Process list through one `NtQuerySystemInformation(SystemProcessInformation)`.
//!
//! The structure is undocumented but stable on 64-bit Windows; every offset is
//! read with bounds checks and the linked list is abandoned as a whole when an
//! entry looks malformed rather than showing garbage. CPU time comes from the
//! same kernel/user counters Task Manager uses, so no per-process handles are
//! opened just to build the list.
//!
//! Only killing a process opens a handle (`OpenProcess` + `TerminateProcess`).

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use super::{ffi, nt_status_ok};

const SYSTEM_PROCESS_INFORMATION: u32 = 5;
/// `STATUS_INFO_LENGTH_MISMATCH`, returned while the probe buffer is too small.
const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC000_0004u32 as i32;
const MIN_ENTRY: usize = 0x68;
const MAX_BUFFER: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug, Default)]
pub struct ProcessEntry {
    pub pid: u32,
    pub parent_pid: u32,
    pub name: String,
    pub threads: u32,
    pub handles: u32,
    pub private_bytes: u64,
    pub create_time: u64,
    pub age_secs: u64,
    /// Percentage of total CPU capacity, like Task Manager's CPU column.
    pub cpu_percent: f32,
}

#[derive(Clone, Default)]
pub struct ProcessList {
    pub entries: Vec<ProcessEntry>,
    pub total_threads: u32,
}

/// Keeps the previous per-PID CPU counters between five-second snapshots.
pub struct ProcessSampler {
    previous: HashMap<u32, (u64, Instant)>,
    logical: usize,
    last_sample: Instant,
    buffer: Vec<u8>,
}

impl ProcessSampler {
    pub fn new() -> Self {
        Self {
            previous: HashMap::new(),
            logical: super::cpu::logical_processors().max(1),
            last_sample: Instant::now(),
            buffer: Vec::new(),
        }
    }

    pub fn sample(&mut self) -> ProcessList {
        let now = Instant::now();
        let elapsed = now
            .duration_since(self.last_sample)
            .as_secs_f64()
            .clamp(0.001, 120.0);
        self.last_sample = now;

        let Some(mut data) = self.query() else {
            return ProcessList::default();
        };
        let (entries, total_threads) = self.parse(&data, elapsed, now);
        data.clear();
        self.buffer = data;
        ProcessList {
            entries,
            total_threads,
        }
    }

    fn query(&mut self) -> Option<Vec<u8>> {
        let mut size = 1usize << 20;
        loop {
            self.buffer.resize(size, 0);
            let mut returned = 0u32;
            let status = unsafe {
                ffi::NtQuerySystemInformation(
                    SYSTEM_PROCESS_INFORMATION,
                    self.buffer.as_mut_ptr().cast(),
                    self.buffer.len() as u32,
                    &mut returned,
                )
            };
            if nt_status_ok(status) {
                let used = (returned as usize).clamp(MIN_ENTRY, self.buffer.len());
                self.buffer.truncate(used);
                return Some(core::mem::take(&mut self.buffer));
            }
            if status != STATUS_INFO_LENGTH_MISMATCH {
                return None;
            }
            let next = (returned as usize).saturating_add(64 * 1024);
            if next > MAX_BUFFER {
                return None;
            }
            size = next.max(size + 256 * 1024);
        }
    }

    fn parse(&mut self, data: &[u8], elapsed: f64, now: Instant) -> (Vec<ProcessEntry>, u32) {
        let mut entries = Vec::with_capacity(512);
        let mut total_threads = 0u32;
        let start = data.as_ptr() as usize;
        let end = start + data.len();
        let now_filetime = filetime_now();
        let mut offset = 0usize;

        while offset + MIN_ENTRY <= data.len() {
            let next = read_u32(data, offset) as usize;
            let threads = read_u32(data, offset + 0x04);
            let private = read_i64(data, offset + 0x08).max(0) as u64;
            let create_time = read_i64(data, offset + 0x20).max(0) as u64;
            let user = read_i64(data, offset + 0x28).max(0) as u64;
            let kernel = read_i64(data, offset + 0x30).max(0) as u64;
            let name_length = read_u16(data, offset + 0x38) as usize;
            let name_pointer = read_usize(data, offset + 0x40);
            let pid = read_usize(data, offset + 0x50) as u32;
            let parent_pid = read_usize(data, offset + 0x58) as u32;
            let handles = read_u32(data, offset + 0x60);

            if pid != 0 && pid != u32::MAX {
                let name = read_name(name_pointer, name_length, start, end)
                    .unwrap_or_else(|| format!("PID {pid}"));
                let ticks = user.saturating_add(kernel);
                let cpu_percent = if elapsed > 0.05 {
                    self.previous
                        .get(&pid)
                        .filter(|(before, _)| ticks >= *before)
                        .map(|(before, _)| {
                            let delta = ticks.saturating_sub(*before) as f64;
                            (delta / (elapsed * 10_000_000.0) * 100.0 / self.logical as f64)
                                .clamp(0.0, 100.0) as f32
                        })
                        .unwrap_or(0.0)
                } else {
                    0.0
                };
                self.previous.insert(pid, (ticks, now));
                total_threads = total_threads.saturating_add(threads);
                entries.push(ProcessEntry {
                    pid,
                    parent_pid,
                    name,
                    threads,
                    handles,
                    private_bytes: private,
                    create_time,
                    age_secs: if create_time > 0 {
                        now_filetime.saturating_sub(create_time) / 10_000_000
                    } else {
                        0
                    },
                    cpu_percent,
                });
            }

            if next == 0 {
                break;
            }
            if next < MIN_ENTRY || offset + next > data.len() {
                // A damaged list must not be shown as a partial, misleading one.
                return (Vec::new(), 0);
            }
            offset += next;
        }

        let seen: HashSet<u32> = entries.iter().map(|entry| entry.pid).collect();
        self.previous.retain(|pid, _| seen.contains(pid));
        (entries, total_threads)
    }
}

fn read_name(pointer: usize, length: usize, start: usize, end: usize) -> Option<String> {
    if pointer == 0 || length == 0 || length > 512 || length % 2 != 0 {
        return None;
    }
    if pointer < start || pointer.saturating_add(length) > end {
        return None;
    }
    let units = unsafe {
        core::slice::from_raw_parts(pointer as *const u16, length / 2)
    };
    let text = String::from_utf16_lossy(units).trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn filetime_now() -> u64 {
    const UNIX_EPOCH_FILETIME: u64 = 116_444_736_000_000_000;
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(duration) => {
            UNIX_EPOCH_FILETIME
                + duration.as_secs().saturating_mul(10_000_000)
                + duration.subsec_nanos() as u64 / 100
        }
        Err(_) => 0,
    }
}

fn read_u16(data: &[u8], offset: usize) -> u16 {
    u16::from_ne_bytes([data[offset], data[offset + 1]])
}

fn read_u32(data: &[u8], offset: usize) -> u32 {
    u32::from_ne_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

fn read_i64(data: &[u8], offset: usize) -> i64 {
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&data[offset..offset + 8]);
    i64::from_ne_bytes(bytes)
}

fn read_usize(data: &[u8], offset: usize) -> usize {
    let mut bytes = [0u8; core::mem::size_of::<usize>()];
    bytes.copy_from_slice(&data[offset..offset + core::mem::size_of::<usize>()]);
    usize::from_ne_bytes(bytes)
}

/// Best-effort executable path for icons / the details panel.
pub fn image_path(pid: u32) -> Option<String> {
    let handle = unsafe {
        ffi::OpenProcess(ffi::PROCESS_QUERY_LIMITED_INFORMATION, 0, pid)
    };
    if handle.is_null() {
        return None;
    }
    let mut buffer = [0u16; 1024];
    let mut size = buffer.len() as u32;
    let ok = unsafe {
        ffi::QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut size)
    };
    unsafe { ffi::CloseHandle(handle) };
    if ok == 0 || size == 0 {
        return None;
    }
    let path = super::from_wide(&buffer[..size as usize]);
    if path.is_empty() {
        None
    } else {
        Some(path)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KillError {
    Idle,
    SelfProcess,
    AccessDenied,
    NotFound,
    Other(u32),
}

/// Terminate one process.
pub fn kill(pid: u32) -> Result<(), KillError> {
    if pid == u32::MAX || pid == 0 {
        return Err(KillError::Idle);
    }
    if pid == std::process::id() {
        return Err(KillError::SelfProcess);
    }
    let handle = unsafe {
        ffi::OpenProcess(
            ffi::PROCESS_TERMINATE | ffi::PROCESS_QUERY_LIMITED_INFORMATION,
            0,
            pid,
        )
    };
    if handle.is_null() {
        return Err(win_error());
    }
    let ok = unsafe { ffi::TerminateProcess(handle, 1) };
    let error = if ok == 0 {
        Some(win_error())
    } else {
        None
    };
    unsafe { ffi::CloseHandle(handle) };
    match error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn win_error() -> KillError {
    let code = unsafe { ffi::GetLastError() };
    match code {
        5 => KillError::AccessDenied,
        87 | 1168 => KillError::NotFound,
        other => KillError::Other(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_list_is_plausible() {
        let mut sampler = ProcessSampler::new();
        let list = sampler.sample();
        if list.entries.is_empty() {
            return; // unavailable in the test environment
        }
        assert!(list.entries.len() > 5);
        assert!(list.total_threads > 0);
        let system = list.entries.iter().any(|entry| entry.pid == 4);
        assert!(system || list.entries.iter().any(|entry| !entry.name.is_empty()));
        for entry in &list.entries {
            assert!((0.0..=100.0).contains(&entry.cpu_percent), "{entry:?}");
        }
    }

    #[test]
    fn second_sample_has_cpu_deltas() {
        let mut sampler = ProcessSampler::new();
        let first = sampler.sample();
        if first.entries.is_empty() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(120));
        let second = sampler.sample();
        assert!(!second.entries.is_empty());
        assert!(second
            .entries
            .iter()
            .all(|entry| entry.cpu_percent.is_finite()));
    }

    #[test]
    fn refuses_to_kill_idle() {
        assert!(kill(0).is_err());
    }
}

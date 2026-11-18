//! CPU time, per-core time, current clock and the model string.
//!
//! Two syscalls per tick (`GetSystemTimes`, one `NtQuerySystemInformation`)
//! plus a CPUID for the brand, which is read once at startup.

use super::ffi;
use super::{failed, nt_status_ok, read_i64};

/// Raw scheduler counters, in 100 ns ticks. `kernel` already includes `idle`,
/// which is why usage is `(kernel + user - idle_delta) / (kernel + user)`.
#[derive(Clone, Copy, Debug, Default)]
pub struct CpuTimes {
    pub idle: u64,
    pub kernel: u64,
    pub user: u64,
}

impl CpuTimes {
    pub fn total(self) -> u64 {
        self.kernel + self.user
    }

    pub fn busy(self) -> u64 {
        self.total().saturating_sub(self.idle)
    }
}

pub fn total() -> Option<CpuTimes> {
    let mut idle = ffi::FILETIME::default();
    let mut kernel = ffi::FILETIME::default();
    let mut user = ffi::FILETIME::default();
    let ok = unsafe { ffi::GetSystemTimes(&mut idle, &mut kernel, &mut user) };
    if failed(ok) {
        return None;
    }
    Some(CpuTimes {
        idle: idle.ticks(),
        kernel: kernel.ticks(),
        user: user.ticks(),
    })
}

/// Reusable buffer for the per-core query.
///
/// `NtQuerySystemInformation(SystemProcessorPerformanceInformation)` only
/// accepts a buffer of *exactly* `logical_cpus * sizeof(struct)` bytes — one
/// byte more or less and the kernel answers `STATUS_INFO_LENGTH_MISMATCH`. So
/// the required size is discovered once (zero-length probe reports it) and the
/// allocation is then kept exactly that size for every later tick.
#[derive(Default)]
pub struct PerCpuBuffer {
    data: Vec<u8>,
    required: u32,
}

impl PerCpuBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Per-core scheduler counters; `None` when the buffer cannot be sized.
    pub fn read(&mut self, logical: usize) -> Option<Vec<CpuTimes>> {
        if logical == 0 {
            return None;
        }
        if self.required == 0 {
            let mut probe = [0u8; 8];
            let mut required: u32 = 0;
            unsafe {
                ffi::NtQuerySystemInformation(
                    ffi::SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION,
                    probe.as_mut_ptr().cast(),
                    probe.len() as u32,
                    &mut required,
                );
            }
            let minimum = core::mem::size_of::<CpuTimes>() as u32 * logical as u32;
            if required == 0 || required < minimum {
                return None;
            }
            self.required = required;
            self.data = vec![0u8; required as usize];
        }

        let mut returned: u32 = 0;
        let status = unsafe {
            ffi::NtQuerySystemInformation(
                ffi::SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION,
                self.data.as_mut_ptr().cast(),
                self.required,
                &mut returned,
            )
        };
        if !nt_status_ok(status) {
            return None;
        }

        let stride = returned as usize / logical;
        if stride < 40 || stride > self.required as usize {
            return None;
        }

        let mut cores = Vec::with_capacity(logical);
        for index in 0..logical {
            let offset = index * stride;
            if offset + stride > self.required as usize {
                return None;
            }
            let base = unsafe { self.data.as_ptr().add(offset) };
            let idle = unsafe { read_i64(base, 0) };
            let kernel = unsafe { read_i64(base, 8) };
            let user = unsafe { read_i64(base, 16) };
            if idle < 0 || kernel < 0 || user < 0 {
                return None;
            }
            cores.push(CpuTimes {
                idle: idle as u64,
                kernel: kernel as u64,
                user: user as u64,
            });
        }
        Some(cores)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Frequency {
    pub current_mhz: u32,
    pub max_mhz: u32,
}

/// Per-core current clock plus the package maximum.
///
/// `CallNtPowerInformation(ProcessorInformation)` returns one array of six
/// `u32` per logical processor, 24 bytes each.
pub fn frequencies(logical: usize) -> Option<(Vec<u32>, u32)> {
    if logical == 0 {
        return None;
    }
    let mut buffer = vec![0u32; logical * 6];
    let bytes = (buffer.len() * 4) as u32;
    let status = unsafe {
        ffi::CallNtPowerInformation(
            11, // ProcessorInformation
            core::ptr::null_mut(),
            0,
            buffer.as_mut_ptr().cast(),
            bytes,
        )
    };
    if status < 0 {
        return None;
    }

    let mut cores = Vec::with_capacity(logical);
    let mut max = 0u32;
    for chunk in buffer.chunks_exact(6) {
        cores.push(chunk[2]);
        max = max.max(chunk[1]);
    }
    if max == 0 {
        return None;
    }
    Some((cores, max))
}

/// Average of [`frequencies`], for the headline number.
pub fn frequency(logical: usize) -> Option<Frequency> {
    if logical == 0 {
        return None;
    }
    let mut buffer = vec![0u32; logical * 6];
    let bytes = (buffer.len() * 4) as u32;
    let status = unsafe {
        ffi::CallNtPowerInformation(
            11, // ProcessorInformation
            core::ptr::null_mut(),
            0,
            buffer.as_mut_ptr().cast(),
            bytes,
        )
    };
    if status < 0 {
        return None;
    }

    let mut sum = 0u64;
    let mut max = 0u32;
    for chunk in buffer.chunks_exact(6) {
        let chunk_max = chunk[1];
        let current = chunk[2];
        sum += current as u64;
        max = max.max(chunk_max);
    }
    if max == 0 {
        return None;
    }
    Some(Frequency {
        current_mhz: (sum / logical as u64) as u32,
        max_mhz: max,
    })
}

pub fn logical_processors() -> usize {
    let count = unsafe { ffi::GetActiveProcessorCount(ffi::ALL_PROCESSOR_GROUPS) };
    count.max(1) as usize
}

/// Physical core count; `None` if the relationship array does not add up to
/// the logical count, in which case the UI simply omits the "16C" part.
pub fn physical_cores(logical: usize) -> Option<usize> {
    let mut needed: u32 = 0;
    unsafe { ffi::GetLogicalProcessorInformation(core::ptr::null_mut(), &mut needed) };
    if needed == 0 {
        return None;
    }
    let entry = core::mem::size_of::<ffi::SYSTEM_LOGICAL_PROCESSOR_INFORMATION>();
    let mut buffer = vec![0u8; needed as usize + entry];
    // `ReturnedLength` is in/out: it must carry the buffer size on the way in.
    let mut written: u32 = buffer.len() as u32;
    let ok = unsafe {
        ffi::GetLogicalProcessorInformation(
            buffer.as_mut_ptr() as *mut ffi::SYSTEM_LOGICAL_PROCESSOR_INFORMATION,
            &mut written,
        )
    };
    if failed(ok) || (written as usize) < entry {
        return None;
    }

    let count = written as usize / entry;
    let mut cores = 0usize;
    let mut threads = 0usize;
    for index in 0..count {
        let base = unsafe { buffer.as_ptr().add(index * entry) };
        let mask = unsafe { (base as *const usize).read_unaligned() };
        let relation = unsafe { read_i64(base, 8) as u32 };
        if relation != ffi::RELATION_PROCESSOR_CORE {
            continue;
        }
        cores += 1;
        threads += (mask as u64).count_ones() as usize;
    }
    if cores == 0 || threads != logical {
        return None;
    }
    Some(cores)
}

/// CPUID brand string (leaves 0x8000_0002..4), 48 UTF-8-safe bytes.
pub fn brand() -> String {
    #[cfg(target_arch = "x86_64")]
    {
        use core::arch::x86_64::__cpuid;
        let mut bytes = [0u8; 48];
        for leaf in 0..3u32 {
            let result = __cpuid(0x8000_0002 + leaf);
            let offset = leaf as usize * 16;
            bytes[offset..offset + 4].copy_from_slice(&result.eax.to_le_bytes());
            bytes[offset + 4..offset + 8].copy_from_slice(&result.ebx.to_le_bytes());
            bytes[offset + 8..offset + 12].copy_from_slice(&result.ecx.to_le_bytes());
            bytes[offset + 12..offset + 16].copy_from_slice(&result.edx.to_le_bytes());
        }
        let text = String::from_utf8_lossy(&bytes)
            .replace('\0', " ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if !text.is_empty() {
            return text;
        }
    }
    "Unknown CPU".to_string()
}

/// Percent 0..100 from two samples; `None` when the clock did not advance.
pub fn usage_percent(previous: CpuTimes, current: CpuTimes) -> Option<f32> {
    let total = current.total().saturating_sub(previous.total());
    if total == 0 {
        return None;
    }
    let busy = current.busy().saturating_sub(previous.busy());
    Some((busy as f32 / total as f32 * 100.0).clamp(0.0, 100.0))
}

pub(crate) fn _unused_zeroed() -> ffi::FILETIME {
    ffi::FILETIME::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_is_a_delta_ratio() {
        let a = CpuTimes {
            idle: 100,
            kernel: 300,
            user: 100,
        };
        let b = CpuTimes {
            idle: 150,
            kernel: 400,
            user: 150,
        };
        // delta: idle 50, kernel+user 150, busy 100 -> 66.6%
        assert!((usage_percent(a, b).unwrap() - 66.6667).abs() < 0.01);
    }

    #[test]
    fn stalled_counter_reports_none() {
        let a = CpuTimes::default();
        assert!(usage_percent(a, a).is_none());
    }

    #[test]
    fn total_and_cores_are_readable() {
        let logical = logical_processors();
        assert!(logical > 0);
        let first = total().expect("GetSystemTimes");
        assert!(first.total() > 0);
        let mut buffer = PerCpuBuffer::new();
        std::thread::sleep(std::time::Duration::from_millis(60));
        let cores = buffer.read(logical).expect("per-core counters");
        assert_eq!(cores.len(), logical);
        assert!(cores.iter().any(|c| c.total() > 0));
        let second = total().expect("GetSystemTimes");
        assert!(second.total() >= first.total());
    }

    #[test]
    fn physical_cores_do_not_exceed_logical() {
        let logical = logical_processors();
        if let Some(cores) = physical_cores(logical) {
            assert!(cores > 0 && cores <= logical);
        }
    }
}

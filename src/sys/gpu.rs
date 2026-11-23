//! GPU utilisation and memory through the GPU performance counters.
//!
//! Windows exposes the same numbers Task Manager uses through PDH:
//!
//! * `\GPU Engine(*)\Utilization Percentage` — one instance per process/engine,
//!   named `pid_…_luid_…_phys_…_eng_…_engtype_…`.
//! * `\GPU Adapter Memory(*)\Dedicated Usage` / `Shared Usage` /
//!   `Total Committed` — one instance per LUID.
//!
//! Engine instances can number in the hundreds, so they are read as one
//! formatted array and aggregated by LUID: utilisation is the **maximum** engine
//! value on that adapter (adding processes would cheerfully produce >100 %).
//!
//! The adapter display name comes from `EnumDisplayDevicesW`, which needs no COM.
//! LUID → name is matched by memory footprint order, with a LUID fallback.

use std::collections::BTreeMap;

use super::ffi;
use super::{from_wide, wide};

/// One engine's utilisation on an adapter (3D / copy / decode / …).
#[derive(Clone, Debug, Default)]
pub struct Engine {
    pub name: String,
    pub utilization: f32,
}

/// Live numbers for one graphics adapter.
#[derive(Clone, Debug, Default)]
pub struct Gpu {
    pub name: String,
    /// Stable adapter key such as `0x00000000_0x0000f4ec`.
    pub luid: String,
    pub utilization: f32,
    pub engines: Vec<Engine>,
    pub dedicated_used: u64,
    /// Zero when Windows does not expose a dedicated-limit counter.
    pub dedicated_limit: u64,
    pub shared_used: u64,
    pub total_committed: u64,
}

/// PDH query that keeps its counter buffers and instance list across samples.
pub struct GpuQuery {
    query: isize,
    engines: Counter,
    dedicated: Counter,
    shared: Counter,
    dedicated_limit: Option<Counter>,
    committed: Option<Counter>,
    display_names: Vec<String>,
}

impl GpuQuery {
    pub fn open() -> Option<Self> {
        let mut query: isize = 0;
        if unsafe { ffi::PdhOpenQueryW(core::ptr::null(), 0, &mut query) } != 0 {
            return None;
        }

        let mut engines = 0isize;
        if !add_counter(query, "\\GPU Engine(*)\\Utilization Percentage", &mut engines) {
            unsafe { ffi::PdhCloseQuery(query) };
            return None;
        }
        let mut dedicated = 0isize;
        let mut shared = 0isize;
        if !add_counter(query, "\\GPU Adapter Memory(*)\\Dedicated Usage", &mut dedicated)
            || !add_counter(query, "\\GPU Adapter Memory(*)\\Shared Usage", &mut shared)
        {
            unsafe { ffi::PdhCloseQuery(query) };
            return None;
        }

        let mut dedicated_limit = 0isize;
        let dedicated_limit = add_counter(
            query,
            "\\GPU Adapter Memory(*)\\Dedicated Limit",
            &mut dedicated_limit,
        )
        .then_some(dedicated_limit);

        let mut committed = 0isize;
        let committed = add_counter(
            query,
            "\\GPU Adapter Memory(*)\\Total Committed",
            &mut committed,
        )
        .then_some(committed);

        let query = Self {
            query,
            engines: Counter::new(engines),
            dedicated: Counter::new(dedicated),
            shared: Counter::new(shared),
            dedicated_limit: dedicated_limit.map(Counter::new),
            committed: committed.map(Counter::new),
            display_names: display_names(),
        };
        // Prime the rate-based engine counter; the first sample would be zero.
        unsafe { ffi::PdhCollectQueryData(query.query) };
        Some(query)
    }

    pub fn sample(&mut self) -> Vec<Gpu> {
        if unsafe { ffi::PdhCollectQueryData(self.query) } != 0 {
            return Vec::new();
        }

        let mut adapters: BTreeMap<String, Accum> = BTreeMap::new();
        for (instance, value) in self.engines.read() {
            let Some(luid) = luid_key(&instance) else {
                continue;
            };
            let entry = adapters.entry(luid).or_default();
            let value = value.clamp(0.0, 100.0) as f32;
            entry.utilization = entry.utilization.max(value);
            let engine = engine_type(&instance);
            let slot = entry.engines.entry(engine).or_insert(0.0);
            if value > *slot {
                *slot = value;
            }
        }
        if adapters.is_empty() {
            return Vec::new();
        }

        for (instance, value) in self.dedicated.read() {
            if let Some(luid) = luid_key(&instance) {
                adapters.entry(luid).or_default().dedicated = bytes(value);
            }
        }
        for (instance, value) in self.shared.read() {
            if let Some(luid) = luid_key(&instance) {
                adapters.entry(luid).or_default().shared = bytes(value);
            }
        }
        if let Some(counter) = self.dedicated_limit.as_mut() {
            for (instance, value) in counter.read() {
                if let Some(luid) = luid_key(&instance) {
                    adapters.entry(luid).or_default().limit = bytes(value);
                }
            }
        }
        if let Some(counter) = self.committed.as_mut() {
            for (instance, value) in counter.read() {
                if let Some(luid) = luid_key(&instance) {
                    adapters.entry(luid).or_default().committed = bytes(value);
                }
            }
        }

        // Sort by memory footprint, then utilisation: the adapter that carries
        // the desktop usually has the largest footprint.
        let mut rows: Vec<(String, Accum)> = adapters
            .into_iter()
            .filter(|(_, row)| {
                row.dedicated + row.shared >= 1024 * 1024
                    || row.utilization > 0.05
                    || row.limit > 0
            })
            .collect();
        rows.sort_by(|a, b| {
            let left = a.1.dedicated + a.1.shared + a.1.committed;
            let right = b.1.dedicated + b.1.shared + b.1.committed;
            right
                .cmp(&left)
                .then_with(|| b.1.utilization.total_cmp(&a.1.utilization))
                .then_with(|| a.0.cmp(&b.0))
        });

        rows.into_iter()
            .enumerate()
            .map(|(index, (luid, row))| {
                let name = self
                    .display_names
                    .get(index)
                    .cloned()
                    .unwrap_or_else(|| format!("GPU {}", short_luid(&luid)));
                let mut engines: Vec<Engine> = row
                    .engines
                    .into_iter()
                    .map(|(name, utilization)| Engine { name, utilization })
                    .collect();
                engines.sort_by(|a, b| b.utilization.total_cmp(&a.utilization));
                engines.truncate(8);
                Gpu {
                    name,
                    luid,
                    utilization: row.utilization,
                    engines,
                    dedicated_used: row.dedicated,
                    dedicated_limit: row.limit,
                    shared_used: row.shared,
                    total_committed: row.committed,
                }
            })
            .collect()
    }
}

impl Drop for GpuQuery {
    fn drop(&mut self) {
        unsafe { ffi::PdhCloseQuery(self.query) };
    }
}

#[derive(Default)]
struct Accum {
    utilization: f32,
    engines: BTreeMap<String, f32>,
    dedicated: u64,
    shared: u64,
    limit: u64,
    committed: u64,
}

struct Counter {
    handle: isize,
    buffer: Vec<u64>,
}

impl Counter {
    fn new(handle: isize) -> Self {
        Self {
            handle,
            buffer: Vec::new(),
        }
    }

    /// Read a multi-instance formatted counter, reusing the buffer.
    fn read(&mut self) -> Vec<(String, f64)> {
        let mut values = Vec::new();
        let mut size: u32 = 0;
        let mut count: u32 = 0;
        let status = unsafe {
            ffi::PdhGetFormattedCounterArrayW(
                self.handle,
                ffi::PDH_FMT_DOUBLE,
                &mut size,
                &mut count,
                core::ptr::null_mut(),
            )
        };
        if (status != ffi::PDH_MORE_DATA && status != 0) || size == 0 || count == 0 {
            return values;
        }

        let words = (size as usize + core::mem::size_of::<u64>() - 1)
            / core::mem::size_of::<u64>();
        if self.buffer.len() < words {
            self.buffer.resize(words, 0);
        }
        let capacity = self.buffer.len() * core::mem::size_of::<u64>();
        if capacity < 1024 {
            self.buffer.resize(128, 0);
        }
        let pointer = self.buffer.as_mut_ptr() as *mut ffi::PDH_FMT_COUNTERVALUE_ITEM_W;
        let mut actual_size = (self.buffer.len() * core::mem::size_of::<u64>()) as u32;
        let mut actual_count = count;
        let status = unsafe {
            ffi::PdhGetFormattedCounterArrayW(
                self.handle,
                ffi::PDH_FMT_DOUBLE,
                &mut actual_size,
                &mut actual_count,
                pointer,
            )
        };
        if status != 0 || actual_count == 0 {
            return values;
        }

        let item = core::mem::size_of::<ffi::PDH_FMT_COUNTERVALUE_ITEM_W>();
        let available = (self.buffer.len() * core::mem::size_of::<u64>() / item) as u32;
        for index in 0..actual_count.min(available) {
            let entry = unsafe { &*pointer.add(index as usize) };
            if entry.FmtValue.CStatus != 0 || !entry.FmtValue.doubleValue.is_finite() {
                continue;
            }
            let name = unsafe { read_wide(entry.szName) };
            if !name.is_empty() {
                values.push((name, entry.FmtValue.doubleValue));
            }
        }
        values
    }
}

fn add_counter(query: isize, path: &str, out: &mut isize) -> bool {
    let path = wide(path);
    (unsafe { ffi::PdhAddEnglishCounterW(query, path.as_ptr(), 0, out) }) == 0
}

fn bytes(value: f64) -> u64 {
    if !value.is_finite() || value <= 0.0 {
        0
    } else {
        value.min(u64::MAX as f64) as u64
    }
}

/// `pid_123_luid_0x00000000_0x0000f4ec_phys_0_eng_3_engtype_3d`
/// → `0x00000000_0x0000f4ec`.
fn luid_key(instance: &str) -> Option<String> {
    let tail = instance.split_once("luid_")?.1;
    let end = tail
        .find("_phys_")
        .or_else(|| tail.find("_eng_"))
        .unwrap_or(tail.len());
    if end == 0 {
        return None;
    }
    Some(tail[..end].to_string())
}

fn engine_type(instance: &str) -> String {
    match instance.split_once("_engtype_") {
        Some((_, tail)) if !tail.trim_matches('_').is_empty() => {
            tail.trim_matches('_').to_string()
        }
        _ => "other".to_string(),
    }
}

fn short_luid(luid: &str) -> String {
    luid.rsplit('_').next().unwrap_or(luid).to_string()
}

fn display_names() -> Vec<String> {
    let mut names = Vec::new();
    let mut index = 0u32;
    loop {
        let mut device = ffi::DISPLAY_DEVICEW::default();
        device.cb = core::mem::size_of::<ffi::DISPLAY_DEVICEW>() as u32;
        if unsafe { ffi::EnumDisplayDevicesW(core::ptr::null(), index, &mut device, 0) } == 0 {
            break;
        }
        index += 1;
        if index > 32 {
            break;
        }
        let name = from_wide(&device.DeviceString).trim().to_string();
        if name.is_empty()
            || name.contains("Microsoft Basic Render")
            || name.contains("Microsoft Remote")
            || names.contains(&name)
        {
            continue;
        }
        names.push(name);
    }
    names
}

unsafe fn read_wide(pointer: *const u16) -> String {
    if pointer.is_null() {
        return String::new();
    }
    let mut len = 0usize;
    while len < 512 && *pointer.add(len) != 0 {
        len += 1;
    }
    String::from_utf16_lossy(core::slice::from_raw_parts(pointer, len))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_luid_and_engine_type() {
        let instance = "pid_1234_luid_0x00000000_0x0000f4ec_phys_0_eng_0_engtype_3d";
        assert_eq!(luid_key(instance).as_deref(), Some("0x00000000_0x0000f4ec"));
        assert_eq!(engine_type(instance), "3d");
        assert_eq!(engine_type("pid_1_luid_0x0_0x1_phys_0_eng_1_engtype_"), "other");
        assert_eq!(short_luid("0x00000000_0x0000f4ec"), "0x0000f4ec");
    }

    #[test]
    fn query_reads_plausible_values() {
        let Some(mut query) = GpuQuery::open() else {
            return; // no GPU counters on this machine
        };
        std::thread::sleep(std::time::Duration::from_millis(300));
        let gpus = query.sample();
        for gpu in &gpus {
            assert!(gpu.utilization.is_finite());
            assert!((0.0..=100.0).contains(&gpu.utilization), "{gpu:?}");
            assert!(!gpu.name.is_empty());
            if gpu.dedicated_limit > 0 {
                assert!(gpu.dedicated_used <= gpu.dedicated_limit || gpu.dedicated_used < 1 << 30);
            }
        }
    }
}

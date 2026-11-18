//! PDH counters: ACPI thermal zones.
//!
//! This is the only remaining PDH user. Disk throughput is read straight from
//! `NtQuerySystemInformation` (see `sys::io`) because opening a PDH
//! `\PhysicalDisk` query loads the performance-counter providers — several MB
//! of working set and a few hundred milliseconds of startup — for numbers a
//! single syscall already provides.
//!
//! The thermal-zone counter itself is served through ACPI/WMI and costs
//! milliseconds per collect, while temperature moves slowly, so it is polled on
//! its own several-second cadence instead of on every one-second tick.

use super::ffi;
use super::wide;

#[derive(Clone, Debug, Default)]
pub struct Zone {
    pub name: String,
    pub celsius: f32,
}

/// `\PhysicalDisk(_Total)\Disk {Read,Write} Bytes/sec`.
///
/// Device-level throughput, the same numbers Task Manager's disk graph shows.
/// `SYSTEM_PERFORMANCE_INFORMATION` would be a free syscall, but it counts
/// logical transfers (cache hits included) and sits at a few MB/s even when the
/// device is idle, so it is not a disk-activity reading.
pub struct DiskQuery {
    query: isize,
    read: isize,
    write: isize,
}

impl DiskQuery {
    pub fn open() -> Option<Self> {
        let mut query: isize = 0;
        if unsafe { ffi::PdhOpenQueryW(core::ptr::null(), 0, &mut query) } != 0 {
            return None;
        }
        let mut read = 0isize;
        let mut write = 0isize;
        let read_ok = add_counter(query, "\\PhysicalDisk(_Total)\\Disk Read Bytes/sec", &mut read);
        let write_ok = add_counter(
            query,
            "\\PhysicalDisk(_Total)\\Disk Write Bytes/sec",
            &mut write,
        );
        if !read_ok || !write_ok {
            unsafe { ffi::PdhCloseQuery(query) };
            return None;
        }
        // The first collect only establishes the rate baseline.
        unsafe { ffi::PdhCollectQueryData(query) };
        Some(Self {
            query,
            read,
            write,
        })
    }

    /// `(read bytes/sec, write bytes/sec)`; zeros when the query failed.
    pub fn sample(&mut self) -> (f64, f64) {
        if unsafe { ffi::PdhCollectQueryData(self.query) } != 0 {
            return (0.0, 0.0);
        }
        (counter_value(self.read), counter_value(self.write))
    }
}

impl Drop for DiskQuery {
    fn drop(&mut self) {
        unsafe { ffi::PdhCloseQuery(self.query) };
    }
}

/// `\Thermal Zone Information(*)\High Precision Temperature`.
pub struct ThermalQuery {
    query: isize,
    counter: isize,
    /// `High Precision Temperature` reports tenths of a kelvin.
    tenths: bool,
    buffer: Vec<u8>,
    names: Vec<String>,
}

impl ThermalQuery {
    pub fn open() -> Option<Self> {
        let mut query: isize = 0;
        if unsafe { ffi::PdhOpenQueryW(core::ptr::null(), 0, &mut query) } != 0 {
            return None;
        }

        let mut counter = 0isize;
        let mut tenths = true;
        let mut ok = add_counter(
            query,
            "\\Thermal Zone Information(*)\\High Precision Temperature",
            &mut counter,
        );
        if !ok {
            tenths = false;
            ok = add_counter(
                query,
                "\\Thermal Zone Information(*)\\Temperature",
                &mut counter,
            );
        }
        if !ok {
            unsafe { ffi::PdhCloseQuery(query) };
            return None;
        }

        let mut query = Self {
            query,
            counter,
            tenths,
            buffer: Vec::new(),
            names: Vec::new(),
        };
        // One prime collect so the very first `sample` has data.
        let _ = query.sample();
        Some(query)
    }

    pub fn sample(&mut self) -> Vec<Zone> {
        let mut zones = Vec::with_capacity(2);
        if unsafe { ffi::PdhCollectQueryData(self.query) } != 0 {
            return zones;
        }
        let item = core::mem::size_of::<ffi::PDH_FMT_COUNTERVALUE_ITEM_W>();

        let mut size: u32 = 0;
        let mut count: u32 = 0;
        let status = unsafe {
            ffi::PdhGetFormattedCounterArrayW(
                self.counter,
                ffi::PDH_FMT_DOUBLE,
                &mut size,
                &mut count,
                core::ptr::null_mut(),
            )
        };
        if (status != ffi::PDH_MORE_DATA && status != 0) || size == 0 || count == 0 {
            return zones;
        }
        if self.buffer.len() < size as usize {
            self.buffer.resize(size as usize, 0);
        }

        let buffer = self.buffer.as_mut_ptr() as *mut ffi::PDH_FMT_COUNTERVALUE_ITEM_W;
        let mut actual_size = self.buffer.len() as u32;
        let mut actual_count = count;
        let status = unsafe {
            ffi::PdhGetFormattedCounterArrayW(
                self.counter,
                ffi::PDH_FMT_DOUBLE,
                &mut actual_size,
                &mut actual_count,
                buffer,
            )
        };
        if status != 0 || actual_count == 0 {
            return zones;
        }

        let available = (self.buffer.len() / item) as u32;
        for index in 0..actual_count.min(available) {
            let entry = unsafe { &*buffer.add(index as usize) };
            let raw = unsafe { read_wide_string(entry.szName) };
            if raw.is_empty() {
                continue;
            }
            let mut kelvin = entry.FmtValue.doubleValue;
            if !kelvin.is_finite() {
                continue;
            }
            if self.tenths {
                kelvin /= 10.0;
            }
            // Reject nonsense from drivers that expose other units.
            if !(200.0..=400.0).contains(&kelvin) {
                continue;
            }
            zones.push(Zone {
                name: short_zone_name(&raw, &mut self.names),
                celsius: (kelvin - 273.15) as f32,
            });
        }
        zones
    }
}

impl Drop for ThermalQuery {
    fn drop(&mut self) {
        unsafe { ffi::PdhCloseQuery(self.query) };
    }
}

fn add_counter(query: isize, path: &str, out: &mut isize) -> bool {
    let path = wide(path);
    (unsafe { ffi::PdhAddEnglishCounterW(query, path.as_ptr(), 0, out) }) == 0
}

fn counter_value(counter: isize) -> f64 {
    let mut value = ffi::PDH_FMT_COUNTERVALUE {
        CStatus: 0,
        _pad: 0,
        doubleValue: 0.0,
    };
    let mut kind = 0u32;
    let status = unsafe {
        ffi::PdhGetFormattedCounterValue(counter, ffi::PDH_FMT_DOUBLE, &mut kind, &mut value)
    };
    if status != 0 || value.CStatus != 0 || !value.doubleValue.is_finite() {
        return 0.0;
    }
    value.doubleValue.max(0.0)
}

unsafe fn read_wide_string(pointer: *const u16) -> String {
    if pointer.is_null() {
        return String::new();
    }
    let mut len = 0usize;
    while len < 512 && *pointer.add(len) != 0 {
        len += 1;
    }
    if len == 0 {
        return String::new();
    }
    String::from_utf16_lossy(core::slice::from_raw_parts(pointer, len))
}

/// ACPI instance names look like `\_TZ.THM0`; keep the readable tail.
fn short_zone_name(raw: &str, cache: &mut Vec<String>) -> String {
    if let Some(found) = cache.iter().find(|name| name.as_str() == raw) {
        return found.clone();
    }
    let trimmed = raw.trim_start_matches('\\').replace("_TZ.", "");
    let name = if trimmed.is_empty() {
        raw.to_string()
    } else {
        trimmed
    };
    cache.push(name.clone());
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thermal_zones_read_as_celsius() {
        let mut query = ThermalQuery::open();
        let Some(query) = query.as_mut() else {
            return; // firmware without ACPI thermal zones
        };
        std::thread::sleep(std::time::Duration::from_millis(150));
        let zones = query.sample();
        for zone in &zones {
            assert!(zone.celsius > -40.0 && zone.celsius < 150.0, "{zone:?}");
            assert!(!zone.name.is_empty());
        }
    }

    #[test]
    fn zone_names_are_trimmed() {
        let mut cache = Vec::new();
        assert_eq!(short_zone_name("\\_TZ.THM0", &mut cache), "THM0");
        assert_eq!(short_zone_name("\\_TZ.THM0", &mut cache), "THM0");
    }
}
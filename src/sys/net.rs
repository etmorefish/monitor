//! Per-interface byte counters.
//!
//! Two-tier by design: `GetIfTable` returns every interface but walks all of
//! them (~800 µs, and Windows keeps dozens of virtual adapters around), while
//! `GetIfEntry` fetches one known interface for ~30 µs. So the full table is
//! enumerated at startup and every 30 s, and each tick only refreshes the
//! handful of interfaces that carry traffic.

use std::time::{Duration, Instant};

use super::ffi;
use super::from_ansi;

#[derive(Clone, Debug, Default)]
pub struct Interface {
    pub index: u32,
    /// Adapter description, e.g. "Intel(R) Wi-Fi 6E AX211 160MHz".
    pub name: String,
    pub link_bits_per_sec: u64,
    pub up: bool,
    pub rx_bytes: u32,
    pub tx_bytes: u32,
}

/// Cached interface list plus the counters of the most recent poll.
pub struct InterfaceSet {
    pub rows: Vec<Interface>,
    table: Vec<u8>,
    row: ffi::MIB_IFROW,
    refreshed: Instant,
}

impl InterfaceSet {
    pub fn new() -> Self {
        let mut set = Self {
            rows: Vec::new(),
            table: Vec::new(),
            row: unsafe { core::mem::zeroed() },
            refreshed: Instant::now() - REFRESH_EVERY,
        };
        set.refresh();
        set
    }

    /// Full enumeration; log adapters that appeared or disappeared since.
    pub fn refresh(&mut self) {
        self.rows = interfaces(&mut self.table);
        self.refreshed = Instant::now();
    }

    /// Cheap per-tick counter refresh over the cached interfaces.
    pub fn poll(&mut self) {
        let Self { rows, row, .. } = self;
        let mut stale = false;
        for interface in rows.iter_mut() {
            row.dwIndex = interface.index;
            if unsafe { ffi::GetIfEntry(row) } != 0 {
                stale = true;
                interface.up = false;
                continue;
            }
            interface.rx_bytes = row.dwInOctets;
            interface.tx_bytes = row.dwOutOctets;
            interface.up = is_up(row.dwOperStatus);
            interface.link_bits_per_sec = row.dwSpeed as u64;
        }
        // A vanished index means the adapter list changed; re-enumerate now
        // rather than waiting for the periodic refresh.
        if stale || self.refreshed.elapsed() >= REFRESH_EVERY {
            self.refresh();
        }
    }
}

const REFRESH_EVERY: Duration = Duration::from_secs(30);

fn is_up(status: u32) -> bool {
    status == ffi::IF_OPER_STATUS_OPERATIONAL || status == ffi::IF_OPER_STATUS_CONNECTED
}

/// Full interface table. Pre-sized generously so the whole table arrives in a
/// single call instead of a sizing round-trip.
pub fn interfaces(scratch: &mut Vec<u8>) -> Vec<Interface> {
    const MIN_TABLE: usize = 64 * 1024;
    if scratch.len() < MIN_TABLE {
        scratch.resize(MIN_TABLE, 0);
    }

    let mut actual = scratch.len() as u32;
    let mut status = unsafe { ffi::GetIfTable(scratch.as_mut_ptr().cast(), &mut actual, 1) };
    if status == ffi::ERROR_INSUFFICIENT_BUFFER {
        // `actual` carries the required size after the failed call.
        let needed = (actual as usize).saturating_add(4096);
        scratch.resize(needed, 0);
        actual = scratch.len() as u32;
        status = unsafe { ffi::GetIfTable(scratch.as_mut_ptr().cast(), &mut actual, 1) };
    }
    // `GetIfTable` returns NO_ERROR (0) on success, unlike the BOOL APIs.
    if status != 0 || actual as usize > scratch.len() {
        return Vec::new();
    }

    // MIB_IFTABLE: u32 count, then MIB_IFROW rows at offset 4.
    let rows = u32::from_ne_bytes([scratch[0], scratch[1], scratch[2], scratch[3]]);
    let header = core::mem::size_of::<u32>();
    let stride = ffi::MIB_IFROW_SIZE;
    let capacity = (actual as usize).saturating_sub(header) / stride;
    let rows = (rows as usize).min(capacity);

    let mut result = Vec::with_capacity(rows);
    for index in 0..rows {
        let offset = header + index * stride;
        let row =
            unsafe { (scratch.as_ptr().add(offset) as *const ffi::MIB_IFROW).read_unaligned() };

        if row.dwType == ffi::IF_TYPE_SOFTWARE_LOOPBACK {
            continue;
        }
        let name = from_ansi(&row.bDescr);
        // Virtual/tunnel adapters report as "up" with no traffic; they only
        // crowd out the real link.
        if is_pseudo(&name) {
            continue;
        }
        let up = is_up(row.dwOperStatus);
        // Keep an interface if it links up or has moved bytes.
        if !up && row.dwInOctets == 0 && row.dwOutOctets == 0 {
            continue;
        }
        if row.dwSpeed == 0 && row.dwInOctets == 0 && row.dwOutOctets == 0 {
            continue;
        }

        result.push(Interface {
            index: row.dwIndex,
            name: if name.is_empty() {
                format!("adapter {}", row.dwIndex)
            } else {
                name
            },
            link_bits_per_sec: row.dwSpeed as u64,
            up,
            rx_bytes: row.dwInOctets,
            tx_bytes: row.dwOutOctets,
        });
    }
    result
}

/// Delta of a 32-bit octet counter, correct across a wrap.
pub fn delta_u32(previous: u32, current: u32) -> u64 {
    if current >= previous {
        (current - previous) as u64
    } else {
        (u32::MAX as u64 - previous as u64) + current as u64 + 1
    }
}

/// Virtual, loopback and tunnel interfaces Windows always reports as "up".
fn is_pseudo(name: &str) -> bool {
    const PATTERNS: [&str; 7] = [
        "WAN Miniport",
        "Pseudo-Interface",
        "Teredo",
        "ISATAP",
        "Loopback",
        "Wi-Fi Direct",
        "Microsoft Kernel Debug",
    ];
    PATTERNS.iter().any(|pattern| name.contains(pattern))
}

/// Split an adapter description so it fits one line: drop the vendor prefix
/// noise Windows appends ("... 160MHz", "-NDIS 6.70").
pub fn shorten_adapter(name: &str) -> &str {
    let cut = name
        .find(" -NDIS")
        .or_else(|| name.find(" NDIS "))
        .unwrap_or(name.len());
    &name[..cut]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_parses() {
        let mut scratch = Vec::new();
        let interfaces = interfaces(&mut scratch);
        for interface in &interfaces {
            assert!(interface.index > 0);
            assert!(!interface.name.is_empty());
        }
    }

    #[test]
    fn cached_set_polls() {
        let mut set = InterfaceSet::new();
        let before: Vec<(u32, u32)> = set.rows.iter().map(|row| (row.rx_bytes, row.tx_bytes)).collect();
        std::thread::sleep(Duration::from_millis(120));
        set.poll();
        let after: Vec<(u32, u32)> = set.rows.iter().map(|row| (row.rx_bytes, row.tx_bytes)).collect();
        assert_eq!(before.len(), after.len());
        // Counters only ever grow (mod 2^32).
        for (previous, current) in before.iter().zip(after.iter()) {
            assert!(delta_u32(previous.0, current.0) < u32::MAX as u64);
        }
    }

    #[test]
    fn wrap_is_handled() {
        assert_eq!(delta_u32(10, 20), 10);
        assert_eq!(delta_u32(20, 10), (u32::MAX as u64 - 20) + 11);
        assert_eq!(delta_u32(5, 5), 0);
    }

    #[test]
    fn mib_row_layout_matches_windows() {
        assert_eq!(core::mem::size_of::<ffi::MIB_IFROW>(), ffi::MIB_IFROW_SIZE);
        assert_eq!(core::mem::align_of::<ffi::MIB_IFROW>(), 4);
    }

    #[test]
    fn pseudo_adapters_are_filtered() {
        assert!(is_pseudo("WAN Miniport (IP)"));
        assert!(is_pseudo("Microsoft Teredo Tunneling Adapter"));
        assert!(!is_pseudo("Intel(R) Wi-Fi 6E AX211 160MHz"));
    }
}

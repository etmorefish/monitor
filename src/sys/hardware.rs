//! Static hardware inventory read once at startup.
//!
//! SMBIOS comes straight from `GetSystemFirmwareTable` and disk models from the
//! `disk\Enum` registry key, so this needs no WMI, no COM and no elevation.

use core::ffi::c_void;

use super::disk;

#[link(name = "kernel32")]
extern "system" {
    fn GetSystemFirmwareTable(
        provider: u32,
        table: u32,
        buffer: *mut c_void,
        size: u32,
    ) -> u32;
}

/// 'RSMB' — the raw SMBIOS table provider.
const RSMB: u32 = u32::from_be_bytes(*b"RSMB");
/// Raw SMBIOS data starts after this header.
const SMBIOS_DATA_OFFSET: usize = 8;

#[derive(Clone, Default, Debug)]
pub struct Hardware {
    pub bios_vendor: String,
    pub bios_version: String,
    pub bios_date: String,
    pub board_vendor: String,
    pub board_product: String,
    pub board_version: String,
    pub smbios_version: String,
    /// Sockets the firmware reports in the physical-memory array.
    pub slots_total: u16,
    pub modules: Vec<MemoryModule>,
    pub disks: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct MemoryModule {
    pub slot: String,
    pub size_bytes: u64,
    pub speed_mts: u32,
    pub kind: String,
    pub manufacturer: String,
    pub part: String,
}

impl Hardware {
    /// Total installed memory across populated slots.
    pub fn memory_total(&self) -> u64 {
        self.modules.iter().map(|module| module.size_bytes).sum()
    }

    pub fn slots_used(&self) -> usize {
        self.modules.len()
    }
}

pub fn load() -> Hardware {
    let mut hardware = Hardware::default();
    if let Some(table) = firmware_table() {
        if table.len() >= 3 {
            hardware.smbios_version = format!("{}.{}", table[1], table[2]);
        }
        parse(&table, &mut hardware);
    }
    // Some firmware omits the memory-array structure; trust the populated slots.
    hardware.slots_total = hardware.slots_total.max(hardware.modules.len() as u16);
    hardware.disks = disk::models();
    hardware
}

fn firmware_table() -> Option<Vec<u8>> {
    unsafe {
        let size = GetSystemFirmwareTable(RSMB, 0, core::ptr::null_mut(), 0);
        if size < 16 {
            return None;
        }
        let mut buffer = vec![0u8; size as usize];
        let written =
            GetSystemFirmwareTable(RSMB, 0, buffer.as_mut_ptr().cast(), buffer.len() as u32);
        if written == 0 || written as usize > buffer.len() {
            return None;
        }
        buffer.truncate(written as usize);
        Some(buffer)
    }
}

/// Walk the SMBIOS structure table and fill in what we understand.
fn parse(data: &[u8], hardware: &mut Hardware) {
    let mut offset = SMBIOS_DATA_OFFSET;
    while offset + 4 <= data.len() {
        let kind = data[offset];
        let length = data[offset + 1] as usize;
        if length < 4 || offset + length > data.len() {
            break;
        }
        let formatted = &data[offset..offset + length];

        // Strings are NUL-terminated and the list ends with an extra NUL.
        let mut strings: Vec<String> = Vec::new();
        let mut cursor = offset + length;
        if cursor + 1 < data.len() && data[cursor] == 0 && data[cursor + 1] == 0 {
            cursor += 2;
        } else {
            while cursor < data.len() {
                let start = cursor;
                while cursor < data.len() && data[cursor] != 0 {
                    cursor += 1;
                }
                if cursor >= data.len() {
                    break;
                }
                strings.push(String::from_utf8_lossy(&data[start..cursor]).to_string());
                cursor += 1;
                if cursor < data.len() && data[cursor] == 0 {
                    cursor += 1;
                    break;
                }
            }
        }

        let get_string = |index: u8| -> String {
            if index == 0 {
                return String::new();
            }
            // Firmware pads these with spaces.
            strings
                .get(index as usize - 1)
                .map(|value| value.trim().to_string())
                .unwrap_or_default()
        };
        let byte = |offset: usize| -> Option<u8> { formatted.get(offset).copied() };
        let word = |offset: usize| -> Option<u16> {
            let bytes = formatted.get(offset..offset + 2)?;
            Some(u16::from_le_bytes([bytes[0], bytes[1]]))
        };
        let dword = |offset: usize| -> Option<u32> {
            let bytes = formatted.get(offset..offset + 4)?;
            Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
        };

        match kind {
            // BIOS information
            0 => {
                hardware.bios_vendor = get_string(byte(0x04).unwrap_or(0));
                hardware.bios_version = get_string(byte(0x05).unwrap_or(0));
                hardware.bios_date = get_string(byte(0x08).unwrap_or(0));
            }
            // Baseboard information
            2 => {
                hardware.board_vendor = get_string(byte(0x04).unwrap_or(0));
                hardware.board_product = get_string(byte(0x05).unwrap_or(0));
                hardware.board_version = get_string(byte(0x06).unwrap_or(0));
            }
            // Physical memory array
            16 => {
                if let Some(slots) = word(0x0D) {
                    hardware.slots_total = hardware.slots_total.max(slots);
                }
            }
            // Memory device
            17 => {
                let size_field = word(0x0C).unwrap_or(0);
                let extended = dword(0x1C).unwrap_or(0);
                let size_mb: u64 = if size_field == 0x7FFF {
                    extended as u64
                } else if size_field & 0x8000 != 0 {
                    // Legacy "kilobytes" encoding.
                    (size_field & 0x7FFF) as u64 / 1024
                } else {
                    size_field as u64
                };
                // A zero size means an empty slot; skip it but still advance.
                if size_mb > 0 {
                // SMBIOS 3.3+ carries the real MT/s in Extended Speed.
                let extended_speed = if formatted.len() >= 0x58 {
                    dword(0x54).unwrap_or(0)
                } else {
                    0
                };
                let speed = if extended_speed > 0 {
                    extended_speed
                } else {
                    let configured = word(0x20).unwrap_or(0) as u32;
                    if configured > 0 {
                        configured
                    } else {
                        word(0x15).unwrap_or(0) as u32
                    }
                };

                hardware.modules.push(MemoryModule {
                    slot: get_string(byte(0x10).unwrap_or(0)),
                    size_bytes: size_mb.saturating_mul(1024 * 1024),
                    speed_mts: speed,
                    kind: memory_kind(byte(0x12).unwrap_or(0)),
                    manufacturer: get_string(byte(0x17).unwrap_or(0)),
                    part: get_string(byte(0x1A).unwrap_or(0)),
                });
                }
            }
            _ => {}
        }

        // Never loop in place on malformed data.
        if cursor <= offset {
            break;
        }
        offset = cursor;
    }
}

/// SMBIOS memory-type byte → a name people recognise.
fn memory_kind(code: u8) -> String {
    match code {
        0x12 => "DDR",
        0x13 => "DDR2",
        0x18 => "DDR2",
        0x19 => "DDR3",
        0x1A => "DDR4",
        0x1B => "LPDDR",
        0x1C => "LPDDR2",
        0x1D => "LPDDR3",
        0x1E => "LPDDR4",
        0x20 => "HBM",
        0x21 => "HBM2",
        0x22 => "DDR5",
        0x23 => "LPDDR5",
        0x24 => "HBM3",
        _ => "",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build one SMBIOS structure: header, formatted body, string table.
    fn structure(kind: u8, body: &[u8], strings: &[&str]) -> Vec<u8> {
        let mut out = Vec::new();
        out.push(kind);
        out.push((body.len() + 4) as u8);
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(body);
        if strings.is_empty() {
            out.extend_from_slice(&[0, 0]);
        } else {
            for text in strings {
                out.extend_from_slice(text.as_bytes());
                out.push(0);
            }
            out.push(0);
        }
        out
    }

    fn table(structures: &[Vec<u8>]) -> Vec<u8> {
        let mut out = vec![0u8; SMBIOS_DATA_OFFSET];
        out[4..8].copy_from_slice(&((structures.iter().map(Vec::len).sum::<usize>()) as u32).to_le_bytes());
        for structure in structures {
            out.extend_from_slice(structure);
        }
        out
    }

    #[test]
    fn parses_bios_board_and_memory() {
        // Body offsets are structure offsets minus the 4-byte header.
        let mut bios = vec![0u8; 0x12];
        bios[0x00] = 1; // vendor
        bios[0x01] = 2; // version
        bios[0x04] = 3; // release date

        let mut board = vec![0u8; 0x09];
        board[0x00] = 1;
        board[0x01] = 2;
        board[0x02] = 3;

        let mut array = vec![0u8; 0x0F];
        array[0x09..0x0B].copy_from_slice(&8u16.to_le_bytes());

        let mut device = vec![0u8; 0x58];
        device[0x08..0x0A].copy_from_slice(&16384u16.to_le_bytes()); // 16 GB in MB
        device[0x0C] = 1; // locator
        device[0x0E] = 0x22; // DDR5
        device[0x11..0x13].copy_from_slice(&5600u16.to_le_bytes());
        device[0x13] = 2; // manufacturer
        device[0x16] = 3; // part

        let data = table(&[
            structure(0, &bios, &["LENOVO", "N45ET27W", "05/12/2025"]),
            structure(2, &board, &["LENOVO", "21LU003GCD", "SDK0J40697"]),
            structure(16, &array, &[]),
            structure(17, &device, &["DIMM 0", "Samsung", "K3LKBKB0BM-MGCP"]),
        ]);

        let mut hardware = Hardware::default();
        parse(&data, &mut hardware);

        assert_eq!(hardware.bios_vendor, "LENOVO");
        assert_eq!(hardware.bios_version, "N45ET27W");
        assert_eq!(hardware.board_product, "21LU003GCD");
        assert_eq!(hardware.slots_total, 8);
        assert_eq!(hardware.modules.len(), 1);
        assert_eq!(hardware.modules[0].size_bytes, 16 * 1024 * 1024 * 1024);
        assert_eq!(hardware.modules[0].speed_mts, 5600);
        assert_eq!(hardware.modules[0].kind, "DDR5");
        assert_eq!(hardware.modules[0].manufacturer, "Samsung");
        assert_eq!(hardware.memory_total(), 16 * 1024 * 1024 * 1024);
    }

    #[test]
    fn empty_slots_are_skipped() {
        let mut device = vec![0u8; 0x58];
        device[0x08..0x0A].copy_from_slice(&0u16.to_le_bytes());
        let data = table(&[structure(17, &device, &["DIMM 1"])]);
        let mut hardware = Hardware::default();
        parse(&data, &mut hardware);
        assert!(hardware.modules.is_empty());
    }

    #[test]
    fn truncated_input_does_not_panic() {
        let mut hardware = Hardware::default();
        parse(&[0u8; 8], &mut hardware);
        parse(&[1, 2, 3], &mut hardware);
        parse(&[], &mut hardware);
    }
}
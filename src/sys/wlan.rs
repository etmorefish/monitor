//! Wi-Fi connection details through the native WLAN API.
//!
//! One handle is opened at startup and reused; each sample is a local IPC call
//! (tens of microseconds), so it is polled on the same 5-second cadence as the
//! thermal zones.

use core::ffi::c_void;

#[link(name = "wlanapi")]
extern "system" {
    fn WlanOpenHandle(
        version: u32,
        reserved: *mut c_void,
        negotiated: *mut u32,
        handle: *mut *mut c_void,
    ) -> u32;
    fn WlanCloseHandle(handle: *mut c_void, reserved: *mut c_void) -> u32;
    fn WlanEnumInterfaces(handle: *mut c_void, reserved: *mut c_void, list: *mut *mut c_void) -> u32;
    fn WlanQueryInterface(
        handle: *mut c_void,
        guid: *const Guid,
        opcode: u32,
        reserved: *mut c_void,
        size: *mut u32,
        data: *mut *mut c_void,
        kind: *mut u32,
    ) -> u32;
    fn WlanFreeMemory(pointer: *mut c_void);
}

const WLAN_CLIENT_VERSION: u32 = 2;
const WLAN_INTERFACE_STATE_CONNECTED: u32 = 1;
const WLAN_INTF_OPCODE_CURRENT_CONNECTION: u32 = 7;
const INTERFACE_INFO_SIZE: usize = 532;
const INTERFACE_LIST_OFFSET: usize = 8;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Dot11Ssid {
    length: u32,
    ssid: [u8; 32],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct AssociationAttributes {
    ssid: Dot11Ssid,
    bss_type: u32,
    bssid: [u8; 6],
    _pad: [u8; 2],
    phy_type: u32,
    phy_index: u32,
    signal_quality: u32,
    rx_rate: u32,
    tx_rate: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct SecurityAttributes {
    enabled: i32,
    one_x: i32,
    auth: u32,
    cipher: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ConnectionAttributes {
    state: u32,
    mode: u32,
    profile: [u16; 256],
    association: AssociationAttributes,
    security: SecurityAttributes,
}

#[derive(Clone, Debug, Default)]
pub struct Wifi {
    pub adapter: String,
    pub ssid: String,
    /// 0–100, as reported by the driver.
    pub signal: u8,
    pub rx_kbps: u32,
    pub tx_kbps: u32,
    /// e.g. `802.11ax`.
    pub phy: String,
}

/// Open handle kept for the lifetime of the sampler.
pub struct Wlan {
    handle: *mut c_void,
}

impl Wlan {
    pub fn open() -> Option<Self> {
        let mut negotiated = 0u32;
        let mut handle: *mut c_void = core::ptr::null_mut();
        let status = unsafe {
            WlanOpenHandle(WLAN_CLIENT_VERSION, core::ptr::null_mut(), &mut negotiated, &mut handle)
        };
        if status != 0 || handle.is_null() {
            return None;
        }
        Some(Self { handle })
    }

    /// The first connected interface, if any.
    pub fn current(&self) -> Option<Wifi> {
        let mut list: *mut c_void = core::ptr::null_mut();
        if unsafe { WlanEnumInterfaces(self.handle, core::ptr::null_mut(), &mut list) } != 0
            || list.is_null()
        {
            return None;
        }

        let result = unsafe { self.scan(list) };
        unsafe { WlanFreeMemory(list) };
        result
    }

    unsafe fn scan(&self, list: *mut c_void) -> Option<Wifi> {
        let base = list as *const u8;
        let count = read_u32(base, 0);
        for index in 0..count as usize {
            let entry = base.add(INTERFACE_LIST_OFFSET + index * INTERFACE_INFO_SIZE);
            let state = read_u32(entry, 528);
            if state != WLAN_INTERFACE_STATE_CONNECTED {
                continue;
            }

            let mut guid = Guid::default();
            core::ptr::copy_nonoverlapping(entry, (&mut guid as *mut Guid).cast(), 16);
            let adapter = {
                let mut units = [0u16; 256];
                core::ptr::copy_nonoverlapping(
                    entry.add(16) as *const u16,
                    units.as_mut_ptr(),
                    units.len(),
                );
                let end = units.iter().position(|unit| *unit == 0).unwrap_or(0);
                String::from_utf16_lossy(&units[..end])
            };

            let mut size = 0u32;
            let mut data: *mut c_void = core::ptr::null_mut();
            let mut kind = 0u32;
            let status = WlanQueryInterface(
                self.handle,
                &guid,
                WLAN_INTF_OPCODE_CURRENT_CONNECTION,
                core::ptr::null_mut(),
                &mut size,
                &mut data,
                &mut kind,
            );
            if status != 0 || data.is_null() {
                continue;
            }

            let attributes = &*(data as *const ConnectionAttributes);
            let assoc = attributes.association;
            let ssid_length = assoc.ssid.length.min(32) as usize;
            let ssid = String::from_utf8_lossy(&assoc.ssid.ssid[..ssid_length])
                .trim()
                .to_string();
            let wifi = Wifi {
                adapter,
                ssid,
                signal: assoc.signal_quality.min(100) as u8,
                rx_kbps: assoc.rx_rate,
                tx_kbps: assoc.tx_rate,
                phy: phy_name(assoc.phy_type),
            };
            WlanFreeMemory(data);
            return Some(wifi);
        }
        None
    }
}

impl Drop for Wlan {
    fn drop(&mut self) {
        unsafe { WlanCloseHandle(self.handle, core::ptr::null_mut()) };
    }
}

unsafe fn read_u32(base: *const u8, offset: usize) -> u32 {
    let mut bytes = [0u8; 4];
    core::ptr::copy_nonoverlapping(base.add(offset), bytes.as_mut_ptr(), 4);
    u32::from_ne_bytes(bytes)
}

/// `DOT11_PHY_TYPE` → the marketing name.
fn phy_name(phy_type: u32) -> String {
    match phy_type {
        4 => "802.11a",
        6 => "802.11g",
        7 => "802.11n",
        8 => "802.11ac",
        10 => "802.11ax",
        11 => "802.11be",
        _ => "",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_matches_windows() {
        assert_eq!(core::mem::size_of::<AssociationAttributes>(), 68);
        assert_eq!(core::mem::size_of::<ConnectionAttributes>(), 604);
        assert_eq!(core::mem::size_of::<Guid>(), 16);
    }

    #[test]
    fn phy_names() {
        assert_eq!(phy_name(10), "802.11ax");
        assert_eq!(phy_name(999), "");
    }

    #[test]
    fn opens_on_this_machine() {
        // Returns None on machines without a WLAN service; never panics.
        if let Some(wlan) = Wlan::open() {
            let _ = wlan.current();
        }
    }
}
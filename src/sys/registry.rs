//! A couple of registry reads, so the GUI can follow the Windows theme.

use core::ffi::c_void;

use super::wide;

const HKEY_CURRENT_USER: isize = 0x8000_0001u32 as i32 as isize;
const RRF_RT_REG_DWORD: u32 = 0x0000_0010;

const HKEY_LOCAL_MACHINE: isize = 0x8000_0002u32 as i32 as isize;
const KEY_READ: u32 = 0x0002_0019;
const ERROR_NO_MORE_ITEMS: i32 = 259;

#[link(name = "advapi32")]
extern "system" {
    fn RegGetValueW(
        key: isize,
        sub_key: *const u16,
        value: *const u16,
        flags: u32,
        kind: *mut u32,
        data: *mut c_void,
        size: *mut u32,
    ) -> i32;
    fn RegOpenKeyExW(
        key: isize,
        sub_key: *const u16,
        options: u32,
        access: u32,
        result: *mut isize,
    ) -> i32;
    fn RegEnumValueW(
        key: isize,
        index: u32,
        name: *mut u16,
        name_len: *mut u32,
        reserved: *mut u32,
        kind: *mut u32,
        data: *mut u8,
        data_len: *mut u32,
    ) -> i32;
    fn RegCloseKey(key: isize) -> i32;
}

/// All string values of a key under `HKEY_LOCAL_MACHINE`, in enumeration order.
pub fn values(sub_key: &str) -> Vec<String> {
    let mut result = Vec::new();
    let sub_key = wide(sub_key);
    let mut key: isize = 0;
    let status = unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, sub_key.as_ptr(), 0, KEY_READ, &mut key) };
    if status != 0 {
        return result;
    }

    let mut index = 0u32;
    loop {
        let mut name = [0u16; 64];
        let mut name_len = name.len() as u32;
        let mut data = [0u8; 1024];
        let mut data_len = data.len() as u32;
        let status = unsafe {
            RegEnumValueW(
                key,
                index,
                name.as_mut_ptr(),
                &mut name_len,
                core::ptr::null_mut(),
                core::ptr::null_mut(),
                data.as_mut_ptr(),
                &mut data_len,
            )
        };
        if status == ERROR_NO_MORE_ITEMS || status != 0 {
            break;
        }

        // `REG_SZ` values arrive as NUL-terminated UTF-16.
        let bytes = &data[..(data_len as usize).min(data.len())];
        let mut units = Vec::with_capacity(bytes.len() / 2);
        for pair in bytes.chunks_exact(2) {
            units.push(u16::from_ne_bytes([pair[0], pair[1]]));
        }
        let text = String::from_utf16_lossy(&units);
        let text = text.trim_end_matches('\0').trim();
        if !text.is_empty() {
            result.push(text.to_string());
        }
        index += 1;
    }

    unsafe { RegCloseKey(key) };
    result
}

/// `true` when Windows apps are set to dark mode.
///
/// Reads `AppsUseLightTheme` under the current user's personalisation key:
/// `0` means dark, `1` means light. Returns `None` when the value is missing or
/// unreadable, so the caller can pick its own default.
pub fn system_is_dark() -> Option<bool> {
    let sub_key = wide(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
    let value = wide("AppsUseLightTheme");
    let mut data: u32 = 0;
    let mut size: u32 = core::mem::size_of::<u32>() as u32;

    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            sub_key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            core::ptr::null_mut(),
            (&mut data as *mut u32).cast::<c_void>(),
            &mut size,
        )
    };
    if status != 0 || size != core::mem::size_of::<u32>() as u32 {
        return None;
    }
    Some(data == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_plausible_theme_value() {
        // Acceptable on any Windows install: either a value or no value.
        if let Some(dark) = system_is_dark() {
            let _ = dark;
        }
    }
}

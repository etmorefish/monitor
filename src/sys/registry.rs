//! A couple of registry reads, so the GUI can follow the Windows theme.

use core::ffi::c_void;

use super::wide;

const HKEY_CURRENT_USER: isize = 0x8000_0001u32 as i32 as isize;
const RRF_RT_REG_DWORD: u32 = 0x0000_0010;

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

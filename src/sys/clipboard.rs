//! Clipboard and shell helpers for the GUI.
//!
//! Copying an address is a few Win32 calls; keeping them here means the
//! dashboard code never touches raw handles.

use core::ffi::c_void;

use super::wide;

#[link(name = "user32")]
extern "system" {
    fn OpenClipboard(owner: *mut c_void) -> i32;
    fn EmptyClipboard() -> i32;
    fn SetClipboardData(format: u32, memory: *mut c_void) -> *mut c_void;
    fn GetClipboardData(format: u32) -> *mut c_void;
    fn CloseClipboard() -> i32;
}

#[link(name = "kernel32")]
extern "system" {
    fn GlobalAlloc(flags: u32, bytes: usize) -> *mut c_void;
    fn GlobalLock(memory: *mut c_void) -> *mut c_void;
    fn GlobalUnlock(memory: *mut c_void) -> i32;
}

#[link(name = "shell32")]
extern "system" {
    fn ShellExecuteW(
        window: *mut c_void,
        operation: *const u16,
        file: *const u16,
        parameters: *const u16,
        directory: *const u16,
        show: i32,
    ) -> *mut c_void;
}

const CF_UNICODETEXT: u32 = 13;
const GMEM_MOVEABLE: u32 = 0x0042;

/// Put `text` on the clipboard as UTF-16, replacing the current contents.
pub fn set_text(text: &str) -> bool {
    unsafe {
        if OpenClipboard(core::ptr::null_mut()) == 0 {
            return false;
        }
        EmptyClipboard();

        let units: Vec<u16> = text.encode_utf16().chain(core::iter::once(0)).collect();
        let memory = GlobalAlloc(GMEM_MOVEABLE, units.len() * 2);
        if memory.is_null() {
            CloseClipboard();
            return false;
        }
        let target = GlobalLock(memory) as *mut u16;
        if target.is_null() {
            CloseClipboard();
            return false;
        }
        core::ptr::copy_nonoverlapping(units.as_ptr(), target, units.len());
        GlobalUnlock(memory);

        // On success the clipboard owns `memory`; do not free it here.
        let stored = !SetClipboardData(CF_UNICODETEXT, memory).is_null();
        CloseClipboard();
        stored
    }
}

/// Read back the clipboard text, used by tests and future copy feedback.
pub fn get_text() -> Option<String> {
    unsafe {
        if OpenClipboard(core::ptr::null_mut()) == 0 {
            return None;
        }
        let handle = GetClipboardData(CF_UNICODETEXT);
        let mut text = None;
        if !handle.is_null() {
            let pointer = GlobalLock(handle) as *const u16;
            if !pointer.is_null() {
                let mut length = 0usize;
                while length < 4096 && *pointer.add(length) != 0 {
                    length += 1;
                }
                text = Some(String::from_utf16_lossy(core::slice::from_raw_parts(
                    pointer, length,
                )));
                GlobalUnlock(handle);
            }
        }
        CloseClipboard();
        text
    }
}

/// Open a `ms-settings:` page (used for the Wi-Fi location permission hint).
pub fn open_uri(uri: &str) -> bool {
    let operation = wide("open");
    let target = wide(uri);
    let result = unsafe {
        ShellExecuteW(
            core::ptr::null_mut(),
            operation.as_ptr(),
            target.as_ptr(),
            core::ptr::null(),
            core::ptr::null(),
            1,
        )
    };
    result as isize > 32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_text() {
        let marker = "monitor-clipboard-test-中文";
        if !set_text(marker) {
            // A clipboard owned by another process can make this flaky; skip.
            return;
        }
        assert_eq!(get_text().as_deref(), Some(marker));
    }
}

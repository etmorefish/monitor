//! Entry point for the graphical shell.
//!
//! A GUI binary has no console, so startup errors and panics are reported in a
//! message box instead of disappearing.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use core::ffi::c_void;

fn main() {
    std::panic::set_hook(Box::new(|info| {
        show_error(&format!("minitor 遇到内部错误：\n{info}"));
    }));

    if let Err(error) = minitor::gui::run() {
        show_error(&format!("minitor 无法启动：\n{error}"));
    }
}

fn show_error(message: &str) {
    #[link(name = "user32")]
    extern "system" {
        fn MessageBoxW(
            window: *mut c_void,
            text: *const u16,
            caption: *const u16,
            kind: u32,
        ) -> i32;
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(core::iter::once(0)).collect()
    }

    let text = wide(message);
    let caption = wide("minitor");
    // 0x10 = MB_ICONERROR
    unsafe { MessageBoxW(core::ptr::null_mut(), text.as_ptr(), caption.as_ptr(), 0x10) };
}

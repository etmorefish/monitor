//! System CJK font, memory-mapped instead of copied.
//!
//! `msyh.ttc` is ~19 MB. Reading it into a `Vec` makes that private, committed
//! memory for the whole session. Mapping the file instead turns those pages
//! into file-backed, evictable ones: they stay in the working set, but the
//! private commit of the process no longer carries the font.
//!
//! Falls back to a normal read if mapping is unavailable.

use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::Arc;

use eframe::egui::{self, FontData, FontDefinitions, FontFamily};

#[link(name = "kernel32")]
extern "system" {
    fn CreateFileW(
        path: *const u16,
        access: u32,
        share: u32,
        security: *mut c_void,
        creation: u32,
        flags: u32,
        template: *mut c_void,
    ) -> *mut c_void;
    fn CreateFileMappingW(
        file: *mut c_void,
        attributes: *mut c_void,
        protect: u32,
        high: u32,
        low: u32,
        name: *const u16,
    ) -> *mut c_void;
    fn MapViewOfFile(
        mapping: *mut c_void,
        access: u32,
        high: u32,
        low: u32,
        bytes: usize,
    ) -> *mut c_void;
    fn UnmapViewOfFile(base: *const c_void) -> i32;
    fn GetFileSizeEx(file: *mut c_void, size: *mut i64) -> i32;
    fn CloseHandle(handle: *mut c_void) -> i32;
}

const GENERIC_READ: u32 = 0x8000_0000;
const FILE_SHARE_READ: u32 = 0x0000_0001;
const OPEN_EXISTING: u32 = 3;
const PAGE_READONLY: u32 = 0x02;
const FILE_MAP_READ: u32 = 0x0004;
const INVALID_HANDLE: *mut c_void = -1isize as *mut c_void;

/// A read-only file mapping that exposes its bytes like a slice.
struct Mapped {
    base: NonNull<u8>,
    len: usize,
    mapping: *mut c_void,
    file: *mut c_void,
}

// The mapping is immutable and only read through `AsRef<[u8]>`.
unsafe impl Send for Mapped {}
unsafe impl Sync for Mapped {}

impl AsRef<[u8]> for Mapped {
    fn as_ref(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.base.as_ptr(), self.len) }
    }
}

impl Drop for Mapped {
    fn drop(&mut self) {
        unsafe {
            UnmapViewOfFile(self.base.as_ptr().cast());
            CloseHandle(self.mapping);
            CloseHandle(self.file);
        }
    }
}

impl Mapped {
    fn open(path: &str) -> Option<Self> {
        let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            let file = CreateFileW(
                wide.as_ptr(),
                GENERIC_READ,
                FILE_SHARE_READ,
                std::ptr::null_mut(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            );
            if file == INVALID_HANDLE || file.is_null() {
                return None;
            }

            let mut size: i64 = 0;
            if GetFileSizeEx(file, &mut size) == 0 || size <= 0 {
                CloseHandle(file);
                return None;
            }

            let mapping = CreateFileMappingW(
                file,
                std::ptr::null_mut(),
                PAGE_READONLY,
                0,
                0,
                std::ptr::null(),
            );
            if mapping.is_null() {
                CloseHandle(file);
                return None;
            }

            let base = MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, 0) as *mut u8;
            let Some(base) = NonNull::new(base) else {
                CloseHandle(mapping);
                CloseHandle(file);
                return None;
            };

            Some(Self {
                base,
                len: size as usize,
                mapping,
                file,
            })
        }
    }
}

/// Replace egui's Latin-only defaults with a Windows CJK face.
///
/// Candidates are tried in order; the first readable one wins. When none is
/// available the default fonts stay in place (Chinese shows as boxes, but the
/// shell still runs).
pub fn install(ctx: &egui::Context) {
    const CANDIDATES: [&str; 4] = [
        r"C:\Windows\Fonts\msyh.ttc",
        r"C:\Windows\Fonts\msyh.ttf",
        r"C:\Windows\Fonts\simhei.ttf",
        r"C:\Windows\Fonts\simsun.ttc",
    ];

    for path in CANDIDATES {
        let Some(data) = load(path) else {
            continue;
        };
        let mut fonts = FontDefinitions::default();
        fonts.font_data.insert("cjk".to_owned(), Arc::new(data));
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            fonts
                .families
                .entry(family)
                .or_default()
                .push("cjk".to_owned());
        }
        ctx.set_fonts(fonts);
        return;
    }
}

fn load(path: &str) -> Option<FontData> {
    if let Some(mapped) = Mapped::open(path) {
        let blob: Arc<dyn AsRef<[u8]> + Send + Sync> = Arc::new(mapped);
        return Some(FontData::from_blob(blob, 0));
    }
    // Mapping can fail on locked or unusual filesystems; reading still works.
    std::fs::read(path)
        .ok()
        .map(|bytes| FontData::from_owned(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_font_maps_and_parses() {
        // The font may be absent on non-Windows-CJK installs; only assert when
        // the file is actually there.
        let path = r"C:\Windows\Fonts\msyh.ttc";
        if !std::path::Path::new(path).exists() {
            return;
        }
        let data = load(path).expect("font data");
        assert!(data.bytes().len() > 1024 * 1024, "font should be megabytes");
        assert!(data.bytes().starts_with(b"ttcf") || data.bytes().starts_with(&[0, 1, 0, 0]));
    }
}

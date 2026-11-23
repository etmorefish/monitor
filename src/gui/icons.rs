//! Extract a process executable's Windows icon into an egui image.
//!
//! `SHGetFileInfoW` returns an `HICON`; GDI then renders it into a 32-bit DIB.
//! This is the only GUI-only piece of Win32 code and is called lazily, once per
//! unique executable path, so a process page costs a handful of shell calls.

use core::ffi::c_void;

use eframe::egui::ColorImage;

type HANDLE = *mut c_void;

const SHGFI_ICON: u32 = 0x0000_0100;
const SHGFI_LARGEICON: u32 = 0x0000_0000;
const DIB_RGB_COLORS: u32 = 0;

#[repr(C)]
struct SHFILEINFOW {
    h_icon: HANDLE,
    i_icon: i32,
    attributes: u32,
    display_name: [u16; 260],
    type_name: [u16; 80],
}

#[repr(C)]
struct ICONINFO {
    is_icon: i32,
    x_hotspot: u32,
    y_hotspot: u32,
    hbm_mask: HANDLE,
    hbm_color: HANDLE,
}

#[repr(C)]
#[derive(Default)]
struct BITMAP {
    bm_type: i32,
    bm_width: i32,
    bm_height: i32,
    bm_width_bytes: i32,
    bm_planes: u16,
    bm_bits_pixel: u16,
    bm_bits: *mut c_void,
}

#[repr(C)]
#[derive(Default)]
struct BITMAPINFOHEADER {
    bi_size: u32,
    bi_width: i32,
    bi_height: i32,
    bi_planes: u16,
    bi_bit_count: u16,
    bi_compression: u32,
    bi_size_image: u32,
    bi_x_pels_per_meter: i32,
    bi_y_pels_per_meter: i32,
    bi_clr_used: u32,
    bi_clr_important: u32,
}

#[repr(C)]
#[derive(Default)]
struct BITMAPINFO {
    header: BITMAPINFOHEADER,
    colors: [u32; 4],
}

#[link(name = "shell32")]
extern "system" {
    fn SHGetFileInfoW(
        path: *const u16,
        attributes: u32,
        info: *mut SHFILEINFOW,
        info_size: u32,
        flags: u32,
    ) -> usize;
}

#[link(name = "user32")]
extern "system" {
    fn DestroyIcon(icon: HANDLE) -> i32;
    fn GetIconInfo(icon: HANDLE, info: *mut ICONINFO) -> i32;
}

#[link(name = "gdi32")]
extern "system" {
    fn CreateCompatibleDC(dc: HANDLE) -> HANDLE;
    fn DeleteDC(dc: HANDLE) -> i32;
    fn SelectObject(dc: HANDLE, object: HANDLE) -> HANDLE;
    fn DeleteObject(object: HANDLE) -> i32;
    fn GetObjectW(object: HANDLE, size: i32, out: *mut c_void) -> i32;
    fn GetDIBits(
        dc: HANDLE,
        bitmap: HANDLE,
        start: u32,
        lines: u32,
        bits: *mut c_void,
        info: *mut BITMAPINFO,
        usage: u32,
    ) -> i32;
}

/// Load the large (32 px) shell icon for `path` as RGBA.
pub fn load(path: &str) -> Option<ColorImage> {
    if path.is_empty() {
        return None;
    }
    let path = crate::sys::wide(path);
    let mut info = SHFILEINFOW {
        h_icon: core::ptr::null_mut(),
        i_icon: 0,
        attributes: 0,
        display_name: [0; 260],
        type_name: [0; 80],
    };
    let ok = unsafe {
        SHGetFileInfoW(
            path.as_ptr(),
            0,
            &mut info,
            core::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_LARGEICON,
        )
    };
    if ok == 0 || info.h_icon.is_null() {
        return None;
    }
    let image = unsafe { convert(info.h_icon) };
    unsafe { DestroyIcon(info.h_icon) };
    image
}

unsafe fn convert(icon: HANDLE) -> Option<ColorImage> {
    let mut icon_info = ICONINFO {
        is_icon: 0,
        x_hotspot: 0,
        y_hotspot: 0,
        hbm_mask: core::ptr::null_mut(),
        hbm_color: core::ptr::null_mut(),
    };
    if GetIconInfo(icon, &mut icon_info) == 0 || icon_info.hbm_color.is_null() {
        return None;
    }

    let mut bitmap = BITMAP::default();
    if GetObjectW(
        icon_info.hbm_color,
        core::mem::size_of::<BITMAP>() as i32,
        (&mut bitmap as *mut BITMAP).cast(),
    ) == 0
        || bitmap.bm_width <= 0
        || bitmap.bm_height <= 0
    {
        cleanup(&icon_info);
        return None;
    }
    let width = bitmap.bm_width as u32;
    let height = bitmap.bm_height as u32;
    let dc = CreateCompatibleDC(core::ptr::null_mut());
    if dc.is_null() {
        cleanup(&icon_info);
        return None;
    }

    let mut rgba = vec![0u8; (width * height * 4) as usize];
    let old = SelectObject(dc, icon_info.hbm_color);
    let mut bmi = BITMAPINFO::default();
    bmi.header.bi_size = core::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bmi.header.bi_width = width as i32;
    bmi.header.bi_height = height as i32;
    bmi.header.bi_planes = 1;
    bmi.header.bi_bit_count = 32;
    let lines = GetDIBits(
        dc,
        icon_info.hbm_color,
        0,
        height,
        rgba.as_mut_ptr().cast(),
        &mut bmi,
        DIB_RGB_COLORS,
    );
    SelectObject(dc, old);
    if lines == 0 {
        DeleteDC(dc);
        cleanup(&icon_info);
        return None;
    }
    flip_rows(&mut rgba, width, height);
    let has_alpha = rgba.chunks_exact(4).any(|pixel| pixel[3] != 0);

    if !has_alpha && !icon_info.hbm_mask.is_null() {
        apply_mask(
            dc,
            icon_info.hbm_mask,
            width,
            height,
            &mut rgba,
        );
    } else if !has_alpha {
        for pixel in rgba.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
    }

    // DIB is BGRA; egui wants RGBA.
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }

    DeleteDC(dc);
    cleanup(&icon_info);
    let image = ColorImage::from_rgba_unmultiplied([width as usize, height as usize], &rgba);
    Some(image)
}

unsafe fn flip_rows(rgba: &mut [u8], width: u32, height: u32) {
    let stride = (width * 4) as usize;
    let mut top = 0usize;
    let mut bottom = (height as usize - 1) * stride;
    while top < bottom {
        for index in 0..stride {
            rgba.swap(top + index, bottom + index);
        }
        top += stride;
        bottom -= stride;
    }
}

unsafe fn apply_mask(dc: HANDLE, mask: HANDLE, width: u32, height: u32, rgba: &mut [u8]) {
    let stride = ((width as usize + 31) / 32) * 4;
    let mut bits = vec![0u8; stride * height as usize];
    let old = SelectObject(dc, mask);
    let mut bmi = BITMAPINFO::default();
    bmi.header.bi_size = core::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bmi.header.bi_width = width as i32;
    bmi.header.bi_height = height as i32;
    bmi.header.bi_planes = 1;
    bmi.header.bi_bit_count = 1;
    let lines = GetDIBits(
        dc,
        mask,
        0,
        height,
        bits.as_mut_ptr().cast(),
        &mut bmi,
        DIB_RGB_COLORS,
    );
    SelectObject(dc, old);
    if lines == 0 {
        return;
    }
    for y in 0..height as usize {
        let row = (height as usize - 1 - y) * stride;
        for x in 0..width as usize {
            let transparent = (bits[row + x / 8] >> (7 - x % 8)) & 1 != 0;
            let pixel = (y * width as usize + x) * 4;
            rgba[pixel + 3] = if transparent { 0 } else { 255 };
        }
    }
}

unsafe fn cleanup(info: &ICONINFO) {
    if !info.hbm_color.is_null() {
        DeleteObject(info.hbm_color);
    }
    if !info.hbm_mask.is_null() {
        DeleteObject(info.hbm_mask);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_explorer_icon() {
        let path = r"C:\Windows\explorer.exe";
        if !std::path::Path::new(path).exists() {
            return;
        }
        let image = load(path);
        if let Some(image) = image {
            assert!(image.size[0] >= 16);
            assert!(image.size[1] >= 16);
            assert_eq!(image.pixels.len(), image.size[0] * image.size[1]);
        }
    }
}

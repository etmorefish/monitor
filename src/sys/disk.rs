//! Fixed-volume capacity via `GetLogicalDriveStringsW` + `GetDiskFreeSpaceExW`.

use super::ffi;
use super::{failed, from_wide, wide};

#[derive(Clone, Debug, Default)]
pub struct Volume {
    pub letter: char,
    pub label: String,
    pub filesystem: String,
    pub total: u64,
    pub free: u64,
}

impl Volume {
    pub fn used(&self) -> u64 {
        self.total.saturating_sub(self.free)
    }

    pub fn percent(&self) -> f32 {
        if self.total == 0 {
            return 0.0;
        }
        (self.used() as f64 / self.total as f64 * 100.0) as f32
    }
}

#[link(name = "kernel32")]
extern "system" {
    fn CreateFileW(
        path: *const u16,
        access: u32,
        share: u32,
        security: *mut core::ffi::c_void,
        creation: u32,
        flags: u32,
        template: *mut core::ffi::c_void,
    ) -> *mut core::ffi::c_void;
    fn DeviceIoControl(
        device: *mut core::ffi::c_void,
        code: u32,
        input: *mut core::ffi::c_void,
        input_len: u32,
        output: *mut core::ffi::c_void,
        output_len: u32,
        returned: *mut u32,
        overlapped: *mut core::ffi::c_void,
    ) -> i32;
    fn CloseHandle(handle: *mut core::ffi::c_void) -> i32;
}

const INVALID_HANDLE: *mut core::ffi::c_void = -1isize as *mut core::ffi::c_void;
const OPEN_EXISTING: u32 = 3;
const IOCTL_STORAGE_QUERY_PROPERTY: u32 = 0x002D_1400;
const STORAGE_DEVICE_PROPERTY: u32 = 0;
const PROPERTY_STANDARD_QUERY: u32 = 0;

/// Ask each physical drive for its descriptor; this is the same source Task
/// Manager uses, so the model string is complete (the registry one truncates).
fn device_models() -> Vec<String> {
    let mut models = Vec::new();
    for index in 0..4u32 {
        let path = wide(&format!("\\\\.\\PhysicalDrive{index}"));
        let handle = unsafe {
            CreateFileW(path.as_ptr(), 0, 3, core::ptr::null_mut(), OPEN_EXISTING, 0, core::ptr::null_mut())
        };
        if handle == INVALID_HANDLE || handle.is_null() {
            continue;
        }

        let mut query = [0u8; 12];
        query[0..4].copy_from_slice(&STORAGE_DEVICE_PROPERTY.to_le_bytes());
        query[4..8].copy_from_slice(&PROPERTY_STANDARD_QUERY.to_le_bytes());
        let mut buffer = vec![0u8; 1024];
        let mut returned = 0u32;
        let ok = unsafe {
            DeviceIoControl(
                handle,
                IOCTL_STORAGE_QUERY_PROPERTY,
                query.as_mut_ptr().cast(),
                query.len() as u32,
                buffer.as_mut_ptr().cast(),
                buffer.len() as u32,
                &mut returned,
                core::ptr::null_mut(),
            )
        };
        unsafe { CloseHandle(handle) };
        if ok == 0 || returned < 36 {
            continue;
        }

        let descriptor = &buffer[..returned as usize];
        let offset = |at: usize| -> usize {
            descriptor
                .get(at..at + 4)
                .map(|bytes| u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize)
                .unwrap_or(0)
        };
        let vendor = read_c_string(descriptor, offset(12));
        let product = read_c_string(descriptor, offset(16));
        let model = format!("{vendor} {product}").trim().to_string();
        if model.len() > 2 && !models.contains(&model) {
            models.push(model);
        }
    }
    models
}

fn read_c_string(buffer: &[u8], offset: usize) -> String {
    if offset == 0 || offset >= buffer.len() {
        return String::new();
    }
    let end = buffer[offset..]
        .iter()
        .position(|byte| *byte == 0)
        .map(|length| offset + length)
        .unwrap_or(buffer.len());
    String::from_utf8_lossy(&buffer[offset..end]).trim().to_string()
}

/// Physical disk models, parsed from `HKLM\SYSTEM\CurrentControlSet\Services\disk\Enum`.
///
/// Values look like `SCSI\Disk&Ven_SAMSUNG&Prod_MZVLB512HBJQ-000L7\4&…`;
/// the vendor/product tokens are enough for a model label.
pub fn models() -> Vec<String> {
    let from_devices = device_models();
    if !from_devices.is_empty() {
        return from_devices;
    }
    let mut models: Vec<String> = Vec::new();
    for value in super::registry::values(r"SYSTEM\CurrentControlSet\Services\disk\Enum") {
        let Some(descriptor) = value.split('\\').nth(1) else {
            continue;
        };
        let mut vendor = None;
        let mut product = None;
        for part in descriptor.split('&') {
            if let Some(text) = part.strip_prefix("Ven_") {
                vendor = Some(text);
            }
            if let Some(text) = part.strip_prefix("Prod_") {
                product = Some(text);
            }
        }
        let (Some(vendor), Some(product)) = (vendor, product) else {
            continue;
        };
        let model = format!("{} {}", vendor.trim(), product.trim());
        if model.len() > 4 && !models.contains(&model) {
            models.push(model);
        }
    }
    models
}

/// Mounted fixed/removable volumes, in drive-letter order.
pub fn volumes() -> Vec<Volume> {
    const MAX_DRIVES: usize = 512;
    let mut roots = [0u16; MAX_DRIVES];
    let written = unsafe { ffi::GetLogicalDriveStringsW(MAX_DRIVES as u32, roots.as_mut_ptr()) };
    if written == 0 || written as usize > MAX_DRIVES {
        return Vec::new();
    }

    let mut result = Vec::with_capacity(4);
    let mut start = 0usize;
    for index in 0..written as usize {
        if roots[index] != 0 {
            continue;
        }
        let root = &roots[start..index];
        start = index + 1;
        if root.len() < 3 {
            continue;
        }
        let letter = char::from_u32(root[0] as u32).unwrap_or('?');
        if !letter.is_ascii_alphabetic() {
            continue;
        }

        let mut root_wide = root.to_vec();
        root_wide.push(0);
        let drive_type = unsafe { ffi::GetDriveTypeW(root_wide.as_ptr()) };
        // 2 = removable, 3 = fixed. Network drives are intentionally skipped:
        // polling them every second would show up on the wire.
        if drive_type != ffi::DRIVE_REMOVABLE && drive_type != ffi::DRIVE_FIXED {
            continue;
        }

        let mut free_to_caller = 0u64;
        let mut total = 0u64;
        let mut free = 0u64;
        let ok = unsafe {
            ffi::GetDiskFreeSpaceExW(
                root_wide.as_ptr(),
                &mut free_to_caller,
                &mut total,
                &mut free,
            )
        };
        if failed(ok) || total == 0 {
            continue;
        }

        let mut label = [0u16; 261];
        let mut filesystem = [0u16; 32];
        let mut serial = 0u32;
        let mut max_component = 0u32;
        let mut flags = 0u32;
        unsafe {
            ffi::GetVolumeInformationW(
                root_wide.as_ptr(),
                label.as_mut_ptr(),
                label.len() as u32,
                &mut serial,
                &mut max_component,
                &mut flags,
                filesystem.as_mut_ptr(),
                filesystem.len() as u32,
            )
        };

        result.push(Volume {
            letter,
            label: from_wide(&label),
            filesystem: from_wide(&filesystem),
            total,
            free,
        });
    }
    result
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_system_volume() {
        let volumes = volumes();
        assert!(!volumes.is_empty(), "no fixed volumes found");
        for volume in &volumes {
            assert!(volume.total > 0);
            assert!(volume.free <= volume.total);
            assert!(volume.percent() >= 0.0 && volume.percent() <= 100.0);
        }
        assert!(
            volumes.iter().any(|v| v.letter.is_ascii_alphabetic()),
            "drive letters must parse"
        );
    }
}

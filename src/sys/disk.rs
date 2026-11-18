//! Fixed-volume capacity via `GetLogicalDriveStringsW` + `GetDiskFreeSpaceExW`.

use super::ffi;
use super::{failed, from_wide};

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

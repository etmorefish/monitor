//! Physical memory, commit charge and cache size.

use super::ffi;
use super::{failed, zeroed};

#[derive(Clone, Copy, Debug, Default)]
pub struct Memory {
    /// Bytes of installed physical memory.
    pub total: u64,
    pub available: u64,
    /// Bytes in the standby/modified cache lists, from `SystemCache`.
    pub cache: u64,
    /// Commit charge and its limit (RAM + page file headroom).
    pub commit: u64,
    pub commit_limit: u64,

    pub processes: u32,
    pub threads: u32,
    pub handles: u32,
}

impl Memory {
    pub fn used(&self) -> u64 {
        self.total.saturating_sub(self.available)
    }

    pub fn percent(&self) -> f32 {
        if self.total == 0 {
            return 0.0;
        }
        (self.used() as f64 / self.total as f64 * 100.0) as f32
    }


}

pub fn read() -> Option<Memory> {
    let mut status: ffi::MEMORYSTATUSEX = zeroed();
    status.dwLength = core::mem::size_of::<ffi::MEMORYSTATUSEX>() as u32;
    if failed(unsafe { ffi::GlobalMemoryStatusEx(&mut status) }) {
        return None;
    }

    let mut memory = Memory {
        total: status.ullTotalPhys,
        available: status.ullAvailPhys,

        ..Memory::default()
    };

    // Commit charge, cache and object counts come from the same syscall the
    // Task Manager uses; one call fills all of them.
    let mut performance: ffi::PERFORMANCE_INFORMATION = zeroed();
    performance.cb = core::mem::size_of::<ffi::PERFORMANCE_INFORMATION>() as u32;
    let ok = unsafe {
        ffi::K32GetPerformanceInfo(
            &mut performance,
            core::mem::size_of::<ffi::PERFORMANCE_INFORMATION>() as u32,
        )
    };
    if !failed(ok) {
        let page = performance.PageSize as u64;
        memory.cache = performance.SystemCache as u64 * page;
        memory.commit = performance.CommitTotal as u64 * page;
        memory.commit_limit = performance.CommitLimit as u64 * page;
        memory.processes = performance.ProcessCount;
        memory.threads = performance.ThreadCount;
        memory.handles = performance.HandleCount;
    }
    Some(memory)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_is_plausible() {
        let memory = read().expect("GlobalMemoryStatusEx");
        assert!(memory.total > 1 << 30, "total={}", memory.total);
        assert!(memory.available <= memory.total);
        assert!(memory.percent() > 0.0 && memory.percent() < 100.0);
        assert!(memory.commit_limit >= memory.commit);
        assert!(memory.processes > 0);
    }
}

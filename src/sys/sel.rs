//! The monitor measuring itself, so the UI can show its own footprint.

use super::ffi;
use super::{failed, zeroed};

/// Resident working set in bytes.
pub fn working_set() -> u64 {
    counters().map_or(0, |(_, working_set)| working_set)
}

/// Private committed bytes — the figure Task Manager calls "提交大小", i.e. what
/// this process actually costs the system rather than shared DLL pages.
pub fn private_bytes() -> u64 {
    counters().map_or(0, |(private, _)| private)
}

fn counters() -> Option<(u64, u64)> {
    let mut counters: ffi::PROCESS_MEMORY_COUNTERS_EX = zeroed();
    counters.cb = core::mem::size_of::<ffi::PROCESS_MEMORY_COUNTERS_EX>() as u32;
    let ok = unsafe {
        ffi::K32GetProcessMemoryInfo(ffi::GetCurrentProcess(), &mut counters, counters.cb)
    };
    if failed(ok) {
        None
    } else {
        Some((counters.PrivateUsage as u64, counters.WorkingSetSize as u64))
    }
}

/// Kernel + user time in 100 ns ticks.
pub fn cpu_ticks() -> Option<u64> {
    let mut creation = ffi::FILETIME::default();
    let mut exit = ffi::FILETIME::default();
    let mut kernel = ffi::FILETIME::default();
    let mut user = ffi::FILETIME::default();
    let ok = unsafe {
        ffi::GetProcessTimes(
            ffi::GetCurrentProcess(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    };
    if failed(ok) {
        return None;
    }
    Some(kernel.ticks() + user.ticks())
}

/// Below-normal priority: the sampler never competes with foreground work.
pub fn set_low_priority() -> bool {
    let ok = unsafe {
        ffi::SetPriorityClass(ffi::GetCurrentProcess(), ffi::BELOW_NORMAL_PRIORITY_CLASS)
    };
    !failed(ok)
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn self_stats_are_readable() {
        assert!(working_set() > 0);
        assert!(cpu_ticks().unwrap() > 0);
    }
}

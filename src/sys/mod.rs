//! Thin, allocation-frugal readers over the native Windows APIs.
//!
//! Every function here is called at most once per refresh tick, so each one
//! has to stay in the microsecond range: syscall + one small parse, no COM, no
//! WMI, no spawned processes, no per-tick string formatting.

pub mod cpu;
pub mod disk;
pub mod ffi;
pub mod host;
pub mod mem;
pub mod net;
pub mod pdh;
pub mod registry;
pub mod power;
pub mod sel;

/// NUL-terminated UTF-16, the form every `*W` API expects.
pub(crate) fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(core::iter::once(0)).collect()
}

/// Read a NUL-terminated UTF-16 buffer captured from the OS.
pub(crate) fn from_wide(buffer: &[u16]) -> String {
    let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}

/// `MIB_IFROW.bDescr` is a byte string in the ANSI code page; keep the ASCII
/// part, which is where adapter names live.
pub(crate) fn from_ansi(buffer: &[u8]) -> String {
    let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    buffer[..end].iter().map(|&b| b as char).collect()
}

/// True when a Win32 call reported failure (`0`).
pub(crate) fn failed(result: i32) -> bool {
    result == 0
}

/// Guard for `NtQuerySystemInformation` buffers: the API writes the required
/// length even when it fails, and reading past it must never happen.
pub(crate) fn nt_status_ok(status: i32) -> bool {
    status >= 0
}

unsafe fn ptr_at<T>(base: *const u8, offset: usize) -> *const T {
    base.add(offset) as *const T
}

pub(crate) unsafe fn read_i64(base: *const u8, offset: usize) -> i64 {
    ptr_at::<i64>(base, offset).read_unaligned()
}



pub(crate) fn zeroed<T>() -> T {
    unsafe { core::mem::zeroed() }
}



//! Machine identity and uptime — read once at startup.

use super::ffi;
use super::{from_wide, failed, zeroed};

pub fn computer_name() -> String {
    let mut buffer = [0u16; 256];
    let mut size = buffer.len() as u32;
    let ok = unsafe { ffi::GetComputerNameW(buffer.as_mut_ptr(), &mut size) };
    if failed(ok) {
        return "unknown".to_string();
    }
    let name = from_wide(&buffer);
    if name.is_empty() {
        "unknown".to_string()
    } else {
        name
    }
}

pub fn uptime_seconds() -> u64 {
    (unsafe { ffi::GetTickCount64() }) / 1000
}

/// Local wall clock, already DST-adjusted: `(2026-11-18, 11:05:12)`.
pub fn local_time() -> (String, String) {
    let mut time: ffi::SYSTEMTIME = zeroed();
    unsafe { ffi::GetLocalTime(&mut time) };
    (
        format!("{:04}-{:02}-{:02}", time.wYear, time.wMonth, time.wDay),
        format!("{:02}:{:02}:{:02}", time.wHour, time.wMinute, time.wSecond),
    )
}

/// `2026-11-18T11:05:12` for log lines and JSON.
pub fn local_timestamp() -> String {
    let (date, time) = local_time();
    format!("{date}T{time}")
}

pub fn uptime_human(seconds: u64) -> String {
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3_600;
    let minutes = (seconds % 3_600) / 60;
    if days > 0 {
        format!("{days}天{hours:02}时{minutes:02}分")
    } else if hours > 0 {
        format!("{hours}时{minutes:02}分")
    } else {
        format!("{minutes}分")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_reads() {
        assert!(!computer_name().is_empty());
        assert!(uptime_seconds() > 0);
    }

    #[test]
    fn uptime_formats() {
        assert_eq!(uptime_human(90), "1分");
        assert_eq!(uptime_human(3_600), "1时00分");
        assert_eq!(uptime_human(90_000), "1天01时00分");
    }
}

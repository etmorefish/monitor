//! Hand-written bindings for the handful of Windows APIs `minitor` needs.
//!
//! Written out instead of pulling in the `windows` crate: the monitor touches
//! roughly twenty functions, and a dependency-free binary starts instantly,
//! ships smaller and has no third-party code in a long-running process.

#![allow(
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    clippy::upper_case_acronyms
)]

use core::ffi::c_void;

pub type HANDLE = *mut c_void;
pub type BOOL = i32;
pub type DWORD = u32;
pub type PCWSTR = *const u16;

pub const STD_INPUT_HANDLE: DWORD = -10i32 as DWORD;
pub const STD_OUTPUT_HANDLE: DWORD = -11i32 as DWORD;

pub const ENABLE_PROCESSED_INPUT: DWORD = 0x0001;
pub const ENABLE_LINE_INPUT: DWORD = 0x0002;
pub const ENABLE_ECHO_INPUT: DWORD = 0x0004;

pub const ENABLE_PROCESSED_OUTPUT: DWORD = 0x0001;

pub const ENABLE_VIRTUAL_TERMINAL_PROCESSING: DWORD = 0x0004;

pub const BELOW_NORMAL_PRIORITY_CLASS: DWORD = 0x0000_4000;

pub const ALL_PROCESSOR_GROUPS: u16 = 0xFFFF;

pub const DRIVE_REMOVABLE: DWORD = 2;
pub const DRIVE_FIXED: DWORD = 3;

/// `ERROR_INSUFFICIENT_BUFFER`, returned by several `Get*Table` calls when the
/// supplied buffer is too small; the size out-parameter then carries the need.
pub const ERROR_INSUFFICIENT_BUFFER: DWORD = 122;

// PDH -----------------------------------------------------------------------
pub const PDH_FMT_DOUBLE: DWORD = 0x0000_0200;
/// `PDH_MORE_DATA`: the caller-sized buffer was too small, retry with the
/// length the API wrote back.
pub const PDH_MORE_DATA: DWORD = 0x8000_07D2;

// NT ------------------------------------------------------------------------
pub const SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION: u32 = 8;


// GetLogicalProcessorInformation relationships.
pub const RELATION_PROCESSOR_CORE: DWORD = 0;
pub const RELATION_CACHE: DWORD = 2;

// MIB_IFROW.dwOperStatus
pub const IF_OPER_STATUS_CONNECTED: DWORD = 4;
pub const IF_OPER_STATUS_OPERATIONAL: DWORD = 5;
pub const IF_TYPE_SOFTWARE_LOOPBACK: DWORD = 24;

#[repr(C)]
#[derive(Default, Clone, Copy, Debug)]
pub struct FILETIME {
    pub dwLowDateTime: DWORD,
    pub dwHighDateTime: DWORD,
}

impl FILETIME {
    /// 100-nanosecond ticks, the unit every CPU/IO counter is reported in.
    pub fn ticks(self) -> u64 {
        ((self.dwHighDateTime as u64) << 32) | self.dwLowDateTime as u64
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct MEMORYSTATUSEX {
    pub dwLength: DWORD,
    pub dwMemoryLoad: DWORD,
    pub ullTotalPhys: u64,
    pub ullAvailPhys: u64,
    pub ullTotalPageFile: u64,
    pub ullAvailPageFile: u64,
    pub ullTotalVirtual: u64,
    pub ullAvailVirtual: u64,
    pub ullAvailExtendedVirtual: u64,
}

impl Default for MEMORYSTATUSEX {
    fn default() -> Self {
        unsafe { core::mem::zeroed() }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PERFORMANCE_INFORMATION {
    pub cb: DWORD,
    pub CommitTotal: usize,
    pub CommitLimit: usize,
    pub CommitPeak: usize,
    pub PhysicalTotal: usize,
    pub PhysicalAvailable: usize,
    pub SystemCache: usize,
    pub KernelTotal: usize,
    pub KernelPaged: usize,
    pub KernelNonpaged: usize,
    pub PageSize: usize,
    pub HandleCount: DWORD,
    pub ProcessCount: DWORD,
    pub ThreadCount: DWORD,
}

impl Default for PERFORMANCE_INFORMATION {
    fn default() -> Self {
        unsafe { core::mem::zeroed() }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PROCESS_MEMORY_COUNTERS_EX {
    pub cb: DWORD,
    pub PageFaultCount: DWORD,
    pub PeakWorkingSetSize: usize,
    pub WorkingSetSize: usize,
    pub QuotaPeakPagedPoolUsage: usize,
    pub QuotaPagedPoolUsage: usize,
    pub QuotaPeakNonPagedPoolUsage: usize,
    pub QuotaNonPagedPoolUsage: usize,
    pub PagefileUsage: usize,
    pub PeakPagefileUsage: usize,
    pub PrivateUsage: usize,
}

impl Default for PROCESS_MEMORY_COUNTERS_EX {
    fn default() -> Self {
        unsafe { core::mem::zeroed() }
    }
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct SYSTEM_POWER_STATUS {
    pub ACLineStatus: u8,
    pub BatteryFlag: u8,
    pub BatteryLifePercent: u8,
    pub SystemStatusFlag: u8,
    pub BatteryLifeTime: u32,
    pub BatteryFullLifeTime: u32,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct SYSTEMTIME {
    pub wYear: u16,
    pub wMonth: u16,
    pub wDayOfWeek: u16,
    pub wDay: u16,
    pub wHour: u16,
    pub wMinute: u16,
    pub wSecond: u16,
    pub wMilliseconds: u16,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct COORD {
    pub X: i16,
    pub Y: i16,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct SMALL_RECT {
    pub Left: i16,
    pub Top: i16,
    pub Right: i16,
    pub Bottom: i16,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct CONSOLE_SCREEN_BUFFER_INFO {
    pub dwSize: COORD,
    pub dwCursorPosition: COORD,
    pub wAttributes: u16,
    pub srWindow: SMALL_RECT,
    pub dwMaximumWindowSize: COORD,
}

/// `MIB_IFROW` — 860 bytes, 4-byte aligned. Kept deliberately flat because the
/// legacy `GetIfTable` array is the one network API that needs no COM and no
/// per-interface handles.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct MIB_IFROW {
    pub wszName: [u16; 256],
    pub dwIndex: DWORD,
    pub dwType: DWORD,
    pub dwMtu: DWORD,
    pub dwSpeed: DWORD,
    pub dwPhysAddrLen: DWORD,
    pub bPhysAddr: [u8; 8],
    pub dwAdminStatus: DWORD,
    pub dwOperStatus: DWORD,
    pub dwLastChange: DWORD,
    pub dwInOctets: DWORD,
    pub dwInUcastPkts: DWORD,
    pub dwInNUcastPkts: DWORD,
    pub dwInDiscards: DWORD,
    pub dwInErrors: DWORD,
    pub dwInUnknownProtos: DWORD,
    pub dwOutOctets: DWORD,
    pub dwOutUcastPkts: DWORD,
    pub dwOutNUcastPkts: DWORD,
    pub dwOutDiscards: DWORD,
    pub dwOutErrors: DWORD,
    pub dwOutQLen: DWORD,
    pub dwDescrLen: DWORD,
    pub bDescr: [u8; 256],
}

pub const MIB_IFROW_SIZE: usize = 860;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PDH_FMT_COUNTERVALUE {
    pub CStatus: DWORD,
    pub _pad: DWORD,
    pub doubleValue: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PDH_FMT_COUNTERVALUE_ITEM_W {
    pub szName: *mut u16,
    pub FmtValue: PDH_FMT_COUNTERVALUE,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SYSTEM_LOGICAL_PROCESSOR_INFORMATION {
    pub ProcessorMask: usize,
    pub Relationship: DWORD,
    pub _pad: DWORD,
    pub _union: [u8; 16],
}

#[link(name = "kernel32")]
extern "system" {
    pub fn GetSystemTimes(idle: *mut FILETIME, kernel: *mut FILETIME, user: *mut FILETIME) -> BOOL;
    pub fn GlobalMemoryStatusEx(buffer: *mut MEMORYSTATUSEX) -> BOOL;
    pub fn K32GetPerformanceInfo(info: *mut PERFORMANCE_INFORMATION, cb: DWORD) -> BOOL;
    pub fn K32GetProcessMemoryInfo(
        process: HANDLE,
        counters: *mut PROCESS_MEMORY_COUNTERS_EX,
        cb: DWORD,
    ) -> BOOL;
    pub fn GetProcessTimes(
        process: HANDLE,
        creation: *mut FILETIME,
        exit: *mut FILETIME,
        kernel: *mut FILETIME,
        user: *mut FILETIME,
    ) -> BOOL;
    pub fn GetCurrentProcess() -> HANDLE;
    pub fn SetPriorityClass(process: HANDLE, priority_class: DWORD) -> BOOL;
    pub fn GetTickCount64() -> u64;
    pub fn GetComputerNameW(buffer: *mut u16, size: *mut DWORD) -> BOOL;
    pub fn GetActiveProcessorCount(group: u16) -> DWORD;
    pub fn GetLogicalProcessorInformation(
        buffer: *mut SYSTEM_LOGICAL_PROCESSOR_INFORMATION,
        length: *mut DWORD,
    ) -> BOOL;
    pub fn GetLogicalDriveStringsW(len: DWORD, buffer: *mut u16) -> DWORD;
    pub fn GetDriveTypeW(root: PCWSTR) -> DWORD;
    pub fn GetDiskFreeSpaceExW(
        directory: PCWSTR,
        free_to_caller: *mut u64,
        total: *mut u64,
        free: *mut u64,
    ) -> BOOL;
    pub fn GetVolumeInformationW(
        root: PCWSTR,
        volume_name: *mut u16,
        volume_name_len: DWORD,
        serial: *mut DWORD,
        max_len: *mut DWORD,
        flags: *mut DWORD,
        fs_name: *mut u16,
        fs_name_len: DWORD,
    ) -> BOOL;
    pub fn GetSystemPowerStatus(status: *mut SYSTEM_POWER_STATUS) -> BOOL;
    pub fn GetConsoleMode(console: HANDLE, mode: *mut DWORD) -> BOOL;
    pub fn SetConsoleMode(console: HANDLE, mode: DWORD) -> BOOL;
    pub fn GetConsoleScreenBufferInfo(
        console: HANDLE,
        info: *mut CONSOLE_SCREEN_BUFFER_INFO,
    ) -> BOOL;
    pub fn GetStdHandle(which: DWORD) -> HANDLE;
    pub fn WriteConsoleW(
        console: HANDLE,
        buffer: *const u16,
        chars: DWORD,
        written: *mut DWORD,
        reserved: *mut c_void,
    ) -> BOOL;
    pub fn ReadConsoleW(
        console: HANDLE,
        buffer: *mut u16,
        chars: DWORD,
        read: *mut DWORD,
        control: *mut c_void,
    ) -> BOOL;

    pub fn GetLocalTime(time: *mut SYSTEMTIME);
}

#[link(name = "ntdll")]
extern "system" {
    pub fn NtQuerySystemInformation(
        class: u32,
        info: *mut c_void,
        len: u32,
        returned: *mut u32,
    ) -> i32;
}

#[link(name = "powrprof")]
extern "system" {
    pub fn CallNtPowerInformation(
        level: i32,
        input: *mut c_void,
        input_len: u32,
        output: *mut c_void,
        output_len: u32,
    ) -> i32;
}

#[link(name = "iphlpapi")]
extern "system" {
    pub fn GetIfTable(table: *mut c_void, size: *mut DWORD, order: BOOL) -> DWORD;
    pub fn GetIpAddrTable(table: *mut c_void, size: *mut DWORD, order: BOOL) -> DWORD;
    pub fn GetIfEntry(row: *mut MIB_IFROW) -> DWORD;
    pub fn GetIfTable2(table: *mut *mut c_void) -> DWORD;
    pub fn FreeMibTable(memory: *mut c_void);
}

#[link(name = "pdh")]
extern "system" {
    pub fn PdhOpenQueryW(source: PCWSTR, user_data: usize, query: *mut isize) -> DWORD;
    pub fn PdhAddEnglishCounterW(
        query: isize,
        path: PCWSTR,
        user_data: usize,
        counter: *mut isize,
    ) -> DWORD;
    pub fn PdhCollectQueryData(query: isize) -> DWORD;
    pub fn PdhGetFormattedCounterValue(
        counter: isize,
        format: DWORD,
        kind: *mut DWORD,
        value: *mut PDH_FMT_COUNTERVALUE,
    ) -> DWORD;

    pub fn PdhGetFormattedCounterArrayW(
        counter: isize,
        format: DWORD,
        buffer_size: *mut DWORD,
        item_count: *mut DWORD,
        buffer: *mut PDH_FMT_COUNTERVALUE_ITEM_W,
    ) -> DWORD;
    pub fn PdhCloseQuery(query: isize) -> DWORD;
}

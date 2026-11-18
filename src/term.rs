//! Raw console control: alternate screen, virtual-terminal output, key events.
//!
//! Frames are written with a single `WriteConsoleW` per refresh (UTF-16, so the
//! console code page never matters), positioned with absolute cursor moves and
//! auto-wrap disabled — no scrolling, no tearing, no per-cell syscalls.

use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

use crate::sys::ffi;

const ENTER: &str = "\x1b[?1049h\x1b[?25l\x1b[?7l\x1b[2J\x1b[0m";
const LEAVE: &str = "\x1b[0m\x1b[?7h\x1b[?25h\x1b[?1049l";

pub struct Terminal {
    out: ffi::HANDLE,
    inp: ffi::HANDLE,
    saved_out_mode: u32,
    saved_in_mode: u32,
    buffer: Vec<u16>,
    width: usize,
    height: usize,
    active: bool,
}

impl Terminal {
    /// `None` when stdout is not a console (piped, redirected, CI).
    pub fn enter() -> Option<Self> {
        let out = unsafe { ffi::GetStdHandle(ffi::STD_OUTPUT_HANDLE) };
        let inp = unsafe { ffi::GetStdHandle(ffi::STD_INPUT_HANDLE) };
        if out.is_null() || inp.is_null() {
            return None;
        }

        let mut out_mode: u32 = 0;
        if unsafe { ffi::GetConsoleMode(out, &mut out_mode) } == 0 {
            return None;
        }
        let mut in_mode: u32 = 0;
        if unsafe { ffi::GetConsoleMode(inp, &mut in_mode) } == 0 {
            return None;
        }

        let wanted_out = out_mode
            | ffi::ENABLE_PROCESSED_OUTPUT
            | ffi::ENABLE_VIRTUAL_TERMINAL_PROCESSING;
        unsafe { ffi::SetConsoleMode(out, wanted_out) };

        // Character-at-a-time input: no line buffering, no echo, and Ctrl+C
        // arrives as a key byte so the screen is restored on the way out.
        let wanted_in = in_mode
            & !(ffi::ENABLE_LINE_INPUT | ffi::ENABLE_ECHO_INPUT | ffi::ENABLE_PROCESSED_INPUT);
        unsafe { ffi::SetConsoleMode(inp, wanted_in) };

        let mut terminal = Self {
            out,
            inp,
            saved_out_mode: out_mode,
            saved_in_mode: in_mode,
            buffer: Vec::with_capacity(16 * 1024),
            width: 100,
            height: 30,
            active: true,
        };
        terminal.write_str(ENTER);
        let (width, height) = terminal.query_size();
        terminal.width = width;
        terminal.height = height;
        Some(terminal)
    }

    /// Current window size; refreshed every frame so resize needs no events.
    pub fn size(&mut self) -> (usize, usize) {
        let (width, height) = self.query_size();
        self.width = width;
        self.height = height;
        (width, height)
    }

    fn query_size(&self) -> (usize, usize) {
        let mut info: ffi::CONSOLE_SCREEN_BUFFER_INFO = crate::sys::zeroed();
        let ok = unsafe { ffi::GetConsoleScreenBufferInfo(self.out, &mut info) };
        if ok == 0 {
            return (self.width, self.height);
        }
        let width = (info.srWindow.Right - info.srWindow.Left + 1).max(20) as usize;
        let height = (info.srWindow.Bottom - info.srWindow.Top + 1).max(6) as usize;
        (width, height)
    }

    pub fn draw(&mut self, frame: &str) {
        self.buffer.clear();
        self.buffer.extend(frame.encode_utf16());
        let mut written = 0u32;
        unsafe {
            ffi::WriteConsoleW(
                self.out,
                self.buffer.as_ptr(),
                self.buffer.len() as u32,
                &mut written,
                std::ptr::null_mut(),
            )
        };
    }

    fn write_str(&mut self, text: &str) {
        self.buffer.clear();
        self.buffer.extend(text.encode_utf16());
        let mut written = 0u32;
        unsafe {
            ffi::WriteConsoleW(
                self.out,
                self.buffer.as_ptr(),
                self.buffer.len() as u32,
                &mut written,
                std::ptr::null_mut(),
            )
        };
    }

    pub fn restore(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        self.write_str(LEAVE);
        unsafe {
            ffi::SetConsoleMode(self.out, self.saved_out_mode);
            ffi::SetConsoleMode(self.inp, self.saved_in_mode);
        }
    }

    /// Spawn a dedicated reader thread; it blocks in `ReadConsoleW` and costs
    /// one parked thread, so key handling adds no polling to the main loop.
    pub fn spawn_input(sender: Sender<u8>) {
        let _ = thread::Builder::new()
            .name("minitor-keys".to_string())
            .stack_size(32 * 1024)
            .spawn(move || {
                let input = unsafe { ffi::GetStdHandle(ffi::STD_INPUT_HANDLE) };
                let mut buffer = [0u16; 8];
                loop {
                    let mut read = 0u32;
                    let ok = unsafe {
                        ffi::ReadConsoleW(
                            input,
                            buffer.as_mut_ptr(),
                            buffer.len() as u32,
                            &mut read,
                            std::ptr::null_mut(),
                        )
                    };
                    if ok == 0 {
                        break;
                    }
                    if read == 0 {
                        // Focus/resize notifications carry no characters.
                        thread::sleep(Duration::from_millis(30));
                        continue;
                    }
                    // A multi-character read is an escape sequence (arrows, F-keys);
                    // only plain keys are meaningful here.
                    if read == 1 {
                        let code = buffer[0];
                        if code < 0x80 && sender.send(code as u8).is_err() {
                            break;
                        }
                    }
                }
            });
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        self.restore();
    }
}

/// Visible width of the console, or `default` when stdout is not a console.
pub fn current_width(default: usize) -> usize {
    let out = unsafe { ffi::GetStdHandle(ffi::STD_OUTPUT_HANDLE) };
    if out.is_null() {
        return default;
    }
    let mut info: ffi::CONSOLE_SCREEN_BUFFER_INFO = crate::sys::zeroed();
    if unsafe { ffi::GetConsoleScreenBufferInfo(out, &mut info) } == 0 {
        return default;
    }
    (info.srWindow.Right - info.srWindow.Left + 1).max(20) as usize
}

/// Best-effort restore from a panic hook, where no `Terminal` is reachable.
pub fn emergency_restore() {
    let out = unsafe { ffi::GetStdHandle(ffi::STD_OUTPUT_HANDLE) };
    if out.is_null() {
        return;
    }
    let text: Vec<u16> = LEAVE.encode_utf16().collect();
    let mut written = 0u32;
    unsafe {
        ffi::WriteConsoleW(
            out,
            text.as_ptr(),
            text.len() as u32,
            &mut written,
            std::ptr::null_mut(),
        )
    };
}

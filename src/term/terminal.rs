//! Terminal setup, input reader, size/signal polling, and best-effort cleanup.
//! Unix uses `stty` (no shell) for raw mode; Windows uses console APIs. A
//! panic hook restores state without logging possibly sensitive panic payloads.

use std::io::{self, Read, Write};
#[cfg(windows)]
use std::sync::atomic::AtomicU32;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Mutex, Once, OnceLock};
use std::time::Duration;

use crate::config::Config;
use crate::position::Size;
use crate::term::input::{Event, InputParser, SignalKind};

static RAW_ACTIVE: AtomicBool = AtomicBool::new(false);
static ALT_ACTIVE: AtomicBool = AtomicBool::new(false);
static MOUSE_ACTIVE: AtomicBool = AtomicBool::new(false);
#[cfg(windows)]
static WINDOWS_SAVED_MODE: AtomicU32 = AtomicU32::new(0);
#[cfg(windows)]
static WINDOWS_SAVED_OUTPUT_MODE: AtomicU32 = AtomicU32::new(0);
#[cfg(unix)]
static SAVED_STTY: OnceLock<Mutex<Option<String>>> = OnceLock::new();
static SIGNAL_ONCE: Once = Once::new();
static SIGNAL_RESIZE: AtomicBool = AtomicBool::new(false);
static SIGNAL_TERMINATE: AtomicBool = AtomicBool::new(false);
static SIGNAL_INTERRUPT: AtomicBool = AtomicBool::new(false);
static SIGNAL_CONTINUE: AtomicBool = AtomicBool::new(false);

const ENTER_MOUSE: &[u8] = b"\x1b[?1000h\x1b[?1002h\x1b[?1006h\x1b[?1015h";
const EXIT_MOUSE: &[u8] = b"\x1b[?1000l\x1b[?1002l\x1b[?1006l\x1b[?1015l";
const ENTER_PASTE: &[u8] = b"\x1b[?2004h\x1b[?1004h";
const EXIT_PASTE: &[u8] = b"\x1b[?2004l\x1b[?1004l\x1b[0m\x1b[?25h";
const ENTER_ALT: &[u8] = b"\x1b[?1049h";
const EXIT_ALT: &[u8] = b"\x1b[?1049l";

pub struct Terminal {
    pub size: Size,
    input_rx: Receiver<Vec<u8>>,
    mouse: bool,
    alt_screen: bool,
}

impl Terminal {
    pub fn new(config: &Config) -> io::Result<Self> {
        use std::io::IsTerminal;
        if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Raster needs an interactive terminal; use --dump-screen for a headless frame",
            ));
        }
        install_signal_handlers();
        let (tx, input_rx) = mpsc::channel::<Vec<u8>>();
        std::thread::Builder::new()
            .name("raster-input".into())
            .spawn(move || {
                let mut stdin = io::stdin().lock();
                let mut buffer = [0u8; 4096];
                loop {
                    match stdin.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(n) => {
                            if tx.send(buffer[..n].to_vec()).is_err() {
                                break;
                            }
                        }
                        Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    }
                }
            })
            .map_err(io::Error::other)?;
        let size = current_size();
        Ok(Self {
            size,
            input_rx,
            mouse: config.ui.mouse,
            alt_screen: config.ui.alt_screen,
        })
    }

    pub fn enter(&mut self, mouse: bool, alt_screen: bool) -> io::Result<()> {
        self.mouse = mouse;
        self.alt_screen = alt_screen;
        platform_enter_raw()?;
        RAW_ACTIVE.store(true, Ordering::SeqCst);
        MOUSE_ACTIVE.store(mouse, Ordering::SeqCst);
        ALT_ACTIVE.store(alt_screen, Ordering::SeqCst);
        let mut sequence = Vec::new();
        if alt_screen {
            sequence.extend_from_slice(ENTER_ALT);
        }
        sequence.extend_from_slice(ENTER_PASTE);
        if mouse {
            sequence.extend_from_slice(ENTER_MOUSE);
        }
        sequence.extend_from_slice(b"\x1b[0m\x1b[?25l");
        self.write(&sequence)
    }

    pub fn restore(&mut self) {
        emergency_cleanup();
    }

    pub fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        let mut stdout = io::stdout().lock();
        stdout.write_all(bytes)?;
        stdout.flush()
    }

    pub fn set_mouse(&mut self, enabled: bool) {
        if enabled == self.mouse {
            return;
        }
        self.mouse = enabled;
        MOUSE_ACTIVE.store(enabled, Ordering::SeqCst);
        let _ = self.write(if enabled { ENTER_MOUSE } else { EXIT_MOUSE });
    }

    pub fn current_size(&self) -> Size {
        current_size()
    }

    pub fn poll_events(&mut self, parser: &mut InputParser, timeout: Duration) -> Vec<Event> {
        let mut events = Vec::new();
        match self.input_rx.recv_timeout(timeout) {
            Ok(bytes) => {
                events.extend(parser.feed(&bytes));
                while let Ok(bytes) = self.input_rx.try_recv() {
                    events.extend(parser.feed(&bytes));
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {}
        }
        events.extend(parser.flush());
        let size = self.current_size();
        if size.cols != self.size.cols || size.rows != self.size.rows {
            self.size = size;
            events.push(Event::Resize(size));
        }
        for signal in take_signals() {
            events.push(Event::Signal(signal));
        }
        events.push(Event::Tick);
        events
    }

    pub fn suspend(&mut self) -> io::Result<()> {
        #[cfg(unix)]
        {
            let mouse = self.mouse;
            let alt = self.alt_screen;
            self.restore();
            let pid = std::process::id().to_string();
            let status = std::process::Command::new("kill")
                .arg("-TSTP")
                .arg(&pid)
                .status();
            self.enter(mouse, alt)?;
            SIGNAL_CONTINUE.store(true, Ordering::SeqCst);
            match status {
                Ok(status) if status.success() => Ok(()),
                _ => Err(io::Error::other(
                    "suspend is unavailable; terminal restored",
                )),
            }
        }
        #[cfg(not(unix))]
        {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "job-control suspend is not supported on this platform",
            ))
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        emergency_cleanup();
    }
}

pub fn install_panic_hook() {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        std::panic::set_hook(Box::new(|_panic| {
            emergency_cleanup();
            // Do not print the panic payload: it could contain document content.
            let _ = io::stderr().write_all(b"\nraster: internal error; terminal state restored. Details omitted to protect document contents.\n");
        }));
    });
}

pub fn emergency_cleanup() {
    use std::io::IsTerminal;
    let was_raw = RAW_ACTIVE.swap(false, Ordering::SeqCst);
    let mouse = MOUSE_ACTIVE.swap(false, Ordering::SeqCst);
    let alt = ALT_ACTIVE.swap(false, Ordering::SeqCst);
    if was_raw || io::stdout().is_terminal() {
        let mut sequence = Vec::new();
        if mouse {
            sequence.extend_from_slice(EXIT_MOUSE);
        }
        sequence.extend_from_slice(EXIT_PASTE);
        if alt {
            sequence.extend_from_slice(EXIT_ALT);
        }
        sequence.extend_from_slice(b"\r\n");
        let mut stdout = io::stdout().lock();
        let _ = stdout.write_all(&sequence);
        let _ = stdout.flush();
    }
    if was_raw {
        platform_restore_raw();
    }
}

pub fn current_size() -> Size {
    #[cfg(unix)]
    {
        if let Ok(output) = std::process::Command::new("stty")
            .arg("size")
            .stdin(std::process::Stdio::inherit())
            .output()
        {
            if output.status.success() {
                let text = String::from_utf8_lossy(&output.stdout);
                let parts: Vec<&str> = text.split_whitespace().collect();
                if parts.len() == 2 {
                    if let (Ok(rows), Ok(cols)) =
                        (parts[0].parse::<usize>(), parts[1].parse::<usize>())
                    {
                        if rows > 0 && cols > 0 {
                            return Size::new(cols, rows);
                        }
                    }
                }
            }
        }
    }
    #[cfg(windows)]
    {
        if let Some(size) = windows_console_size() {
            return size;
        }
    }
    let cols = std::env::var("COLUMNS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(80);
    let rows = std::env::var("LINES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(24);
    Size::new(cols.max(1), rows.max(1))
}

pub fn take_signals() -> Vec<SignalKind> {
    let mut out = Vec::new();
    if SIGNAL_RESIZE.swap(false, Ordering::SeqCst) {
        out.push(SignalKind::Resize);
    }
    if SIGNAL_TERMINATE.swap(false, Ordering::SeqCst) {
        out.push(SignalKind::Terminate);
    }
    if SIGNAL_INTERRUPT.swap(false, Ordering::SeqCst) {
        out.push(SignalKind::Interrupt);
    }
    if SIGNAL_CONTINUE.swap(false, Ordering::SeqCst) {
        out.push(SignalKind::Continue);
    }
    out
}

#[cfg(unix)]
fn install_signal_handlers() {
    SIGNAL_ONCE.call_once(|| unsafe {
        let handler = signal_handler as extern "C" fn(i32);
        let pointer = handler as usize;
        signal(2, pointer); // SIGINT
        signal(15, pointer); // SIGTERM
        signal(28, pointer); // SIGWINCH on Linux/macOS/Android
    });
}

#[cfg(not(unix))]
fn install_signal_handlers() {
    SIGNAL_ONCE.call_once(|| {});
}

#[cfg(unix)]
extern "C" {
    fn signal(sig: i32, handler: usize) -> usize;
}

#[cfg(unix)]
extern "C" fn signal_handler(sig: i32) {
    match sig {
        2 => SIGNAL_INTERRUPT.store(true, Ordering::SeqCst),
        15 => SIGNAL_TERMINATE.store(true, Ordering::SeqCst),
        28 => SIGNAL_RESIZE.store(true, Ordering::SeqCst),
        _ => {}
    }
}

#[cfg(unix)]
fn platform_enter_raw() -> io::Result<()> {
    let output = std::process::Command::new("stty")
        .arg("-g")
        .stdin(std::process::Stdio::inherit())
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other("stty -g failed"));
    }
    let saved = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let mut command = std::process::Command::new("stty");
    command
        .args(["raw", "-echo", "-ixon", "-ixoff"])
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if !command.status().map(|s| s.success()).unwrap_or(false) {
        // Some minimal terminals omit -ixoff; raw/-echo/-ixon still disable
        // XON/XOFF output flow control so Ctrl+S and Ctrl+Q reach Raster.
        let fallback = std::process::Command::new("stty")
            .args(["raw", "-echo", "-ixon"])
            .stdin(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        if !fallback.map(|s| s.success()).unwrap_or(false) {
            return Err(io::Error::other("stty raw setup failed"));
        }
    }
    let lock = SAVED_STTY.get_or_init(|| Mutex::new(None));
    if let Ok(mut guard) = lock.lock() {
        *guard = Some(saved);
    }
    Ok(())
}

#[cfg(unix)]
fn platform_restore_raw() {
    let saved = SAVED_STTY
        .get_or_init(|| Mutex::new(None))
        .try_lock()
        .ok()
        .and_then(|guard| guard.clone());
    if let Some(saved) = saved {
        let _ = std::process::Command::new("stty")
            .arg(saved)
            .stdin(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn GetStdHandle(n_std_handle: i32) -> *mut std::ffi::c_void;
    fn GetConsoleMode(handle: *mut std::ffi::c_void, mode: *mut u32) -> i32;
    fn SetConsoleMode(handle: *mut std::ffi::c_void, mode: u32) -> i32;
    fn GetConsoleScreenBufferInfo(
        handle: *mut std::ffi::c_void,
        info: *mut ConsoleScreenBufferInfo,
    ) -> i32;
}

#[cfg(windows)]
#[repr(C)]
#[derive(Copy, Clone, Default)]
struct ConsoleCoord {
    x: i16,
    y: i16,
}
#[cfg(windows)]
#[repr(C)]
#[derive(Copy, Clone, Default)]
struct SmallRect {
    left: i16,
    top: i16,
    right: i16,
    bottom: i16,
}
#[cfg(windows)]
#[repr(C)]
#[derive(Copy, Clone, Default)]
struct ConsoleScreenBufferInfo {
    size: ConsoleCoord,
    cursor: ConsoleCoord,
    attributes: u16,
    window: SmallRect,
    maximum: ConsoleCoord,
}

#[cfg(windows)]
fn platform_enter_raw() -> io::Result<()> {
    const STD_INPUT_HANDLE: i32 = -10;
    const STD_OUTPUT_HANDLE: i32 = -11;
    const ENABLE_LINE_INPUT: u32 = 0x0002;
    const ENABLE_ECHO_INPUT: u32 = 0x0004;
    const ENABLE_PROCESSED_INPUT: u32 = 0x0001;
    const ENABLE_VIRTUAL_TERMINAL_INPUT: u32 = 0x0200;
    const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;
    unsafe {
        let input = GetStdHandle(STD_INPUT_HANDLE);
        let output = GetStdHandle(STD_OUTPUT_HANDLE);
        let mut input_mode = 0u32;
        let mut output_mode = 0u32;
        if GetConsoleMode(input, &mut input_mode) == 0 {
            return Err(io::Error::last_os_error());
        }
        WINDOWS_SAVED_MODE.store(input_mode, Ordering::SeqCst);
        WINDOWS_SAVED_OUTPUT_MODE.store(output_mode, Ordering::SeqCst);
        let raw = (input_mode & !(ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT | ENABLE_PROCESSED_INPUT))
            | ENABLE_VIRTUAL_TERMINAL_INPUT;
        if SetConsoleMode(input, raw) == 0 {
            return Err(io::Error::last_os_error());
        }
        if GetConsoleMode(output, &mut output_mode) != 0 {
            let _ = SetConsoleMode(output, output_mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
        }
    }
    Ok(())
}

#[cfg(windows)]
fn platform_restore_raw() {
    const STD_INPUT_HANDLE: i32 = -10;
    const STD_OUTPUT_HANDLE: i32 = -11;
    let input_mode = WINDOWS_SAVED_MODE.swap(0, Ordering::SeqCst);
    if input_mode != 0 {
        unsafe {
            let _ = SetConsoleMode(GetStdHandle(STD_INPUT_HANDLE), input_mode);
        }
    }
    let output_mode = WINDOWS_SAVED_OUTPUT_MODE.swap(0, Ordering::SeqCst);
    if output_mode != 0 {
        unsafe {
            let _ = SetConsoleMode(GetStdHandle(STD_OUTPUT_HANDLE), output_mode);
        }
    }
}

#[cfg(windows)]
fn windows_console_size() -> Option<Size> {
    const STD_OUTPUT_HANDLE: i32 = -11;
    unsafe {
        let mut info = ConsoleScreenBufferInfo::default();
        if GetConsoleScreenBufferInfo(GetStdHandle(STD_OUTPUT_HANDLE), &mut info) == 0 {
            return None;
        }
        let cols = (info.window.right - info.window.left + 1).max(1) as usize;
        let rows = (info.window.bottom - info.window.top + 1).max(1) as usize;
        Some(Size::new(cols, rows))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_size_is_positive_when_no_tty_is_available() {
        let size = current_size();
        assert!(size.cols > 0 && size.rows > 0);
    }

    #[test]
    fn cleanup_is_idempotent_without_an_entered_terminal() {
        emergency_cleanup();
        emergency_cleanup();
        assert!(!RAW_ACTIVE.load(Ordering::SeqCst));
    }

    #[test]
    fn mouse_sequences_are_balanced() {
        assert!(ENTER_MOUSE.starts_with(b"\x1b[?"));
        assert!(EXIT_MOUSE.ends_with(b"l"));
    }
}

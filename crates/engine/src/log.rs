use std::io::Write;
use std::sync::OnceLock;
use std::time::Instant;

use log::{Level, LevelFilter, Metadata, Record};
use parking_lot::Mutex;

struct Logger {
    start: Instant,
    file: Mutex<Option<std::fs::File>>,
    level: LevelFilter,
}

static LOGGER: OnceLock<Logger> = OnceLock::new();

pub fn init(file: Option<&std::path::Path>, verbose: bool) {
    let file = file.and_then(|p| {
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        std::fs::File::create(p).ok()
    });
    let level = if verbose { LevelFilter::Debug } else { LevelFilter::Info };
    let logger = LOGGER.get_or_init(|| Logger { start: Instant::now(), file: Mutex::new(file), level });
    if log::set_logger(logger).is_ok() {
        log::set_max_level(level);
    }
    enable_ansi();
}

impl log::Log for Logger {
    fn enabled(&self, m: &Metadata) -> bool {
        m.level() <= self.level || (m.level() <= Level::Info && !m.target().starts_with("voxide"))
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        // Third-party crates are noisy at info level, only their warnings matter.
        let target = record.target();
        let ours = target.starts_with("voxide") || target.starts_with("mod:") || target == "vulkan";
        if !ours && record.level() > Level::Warn {
            return;
        }
        let t = self.start.elapsed().as_secs_f32();
        let (color, name) = match record.level() {
            Level::Error => ("\x1b[1;31m", "ERROR"),
            Level::Warn => ("\x1b[33m", "WARN "),
            Level::Info => ("\x1b[32m", "INFO "),
            Level::Debug => ("\x1b[36m", "DEBUG"),
            Level::Trace => ("\x1b[90m", "TRACE"),
        };
        let short = target.strip_prefix("voxide_").unwrap_or(target);
        let line = format!("{t:9.3} {name} [{short}] {}\n", record.args());
        let mut out = std::io::stderr().lock();
        let _ = write!(out, "\x1b[90m{t:9.3}\x1b[0m {color}{name}\x1b[0m \x1b[90m[{short}]\x1b[0m {}\n", record.args());
        if let Some(f) = self.file.lock().as_mut() {
            let _ = f.write_all(line.as_bytes());
        }
    }

    fn flush(&self) {
        if let Some(f) = self.file.lock().as_mut() {
            let _ = f.flush();
        }
    }
}

#[cfg(windows)]
fn enable_ansi() {
    // Older conhost windows only interpret escape codes with this flag set.
    unsafe extern "system" {
        fn GetStdHandle(n: u32) -> *mut core::ffi::c_void;
        fn GetConsoleMode(h: *mut core::ffi::c_void, mode: *mut u32) -> i32;
        fn SetConsoleMode(h: *mut core::ffi::c_void, mode: u32) -> i32;
    }
    const STD_ERROR_HANDLE: u32 = -12i32 as u32;
    unsafe {
        let h = GetStdHandle(STD_ERROR_HANDLE);
        let mut mode = 0;
        if GetConsoleMode(h, &mut mode) != 0 {
            SetConsoleMode(h, mode | 0x4);
        }
    }
}

#[cfg(not(windows))]
fn enable_ansi() {}

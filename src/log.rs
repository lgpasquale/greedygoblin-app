//! The log file, in the app's data directory.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

use chrono::Local;

/// When the log is larger than this at start, it is moved to `.old`.
const MAX_SIZE: u64 = 1024 * 1024;

static FILE: Mutex<Option<File>> = Mutex::new(None);

pub fn init(path: &Path) {
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if fs::metadata(path).is_ok_and(|m| m.len() > MAX_SIZE) {
        let _ = fs::rename(path, path.with_extension("log.old"));
    }
    let file = OpenOptions::new().create(true).append(true).open(path).ok();
    *FILE.lock().unwrap_or_else(|e| e.into_inner()) = file;
}

pub fn write(message: &str) {
    let line = format!("{} {message}\n", Local::now().format("%Y-%m-%d %H:%M:%S"));
    if cfg!(debug_assertions) {
        eprint!("{line}");
    }
    if let Some(file) = FILE.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        let _ = file.write_all(line.as_bytes());
    }
}

#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => {
        $crate::log::write(&format!($($arg)*))
    };
}

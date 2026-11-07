//! The app's own files, and atomic writes.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const APP_DIR: &str = "greedygoblin-app";

/// `%APPDATA%\greedygoblin-app\config.toml` or
/// `$XDG_CONFIG_HOME/greedygoblin-app/config.toml`.
pub fn config_file() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(APP_DIR)
        .join("config.toml")
}

/// `%LOCALAPPDATA%\greedygoblin-app` or `$XDG_DATA_HOME/greedygoblin-app`.
pub fn data_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(APP_DIR)
}

pub fn log_file() -> PathBuf {
    data_dir().join("greedygoblin-app.log")
}

pub fn state_dir() -> PathBuf {
    data_dir().join("state")
}

/// Writes a temporary file next to `path`, then renames it over `path`.
pub fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, contents)?;
    fs::rename(&tmp, path)
}

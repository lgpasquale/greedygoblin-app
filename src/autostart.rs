//! Start at login: a `Run` registry value for the current user on Windows, an
//! XDG autostart entry on Linux.

use auto_launch::{AutoLaunchBuilder, WindowsEnableMode};

const APP_NAME: &str = "GreedyGoblin";

/// Adds or removes the running executable from the programs started at
/// login. Called at each start, so the entry follows the executable.
pub fn apply(enabled: bool) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let launcher = AutoLaunchBuilder::new()
        .set_app_name(APP_NAME)
        .set_app_path(&exe.to_string_lossy())
        .set_windows_enable_mode(WindowsEnableMode::CurrentUser)
        .build()
        .map_err(|e| e.to_string())?;
    if enabled {
        launcher.enable().map_err(|e| e.to_string())
    } else if launcher.is_enabled().map_err(|e| e.to_string())? {
        launcher.disable().map_err(|e| e.to_string())
    } else {
        Ok(())
    }
}

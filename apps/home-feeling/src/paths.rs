//! Where the app keeps the user's Things and its own data.
use std::path::PathBuf;

/// The user's Things, one folder each. Outside the install folder, so
/// removing or updating the app never touches them (UC-INSTALL-02).
pub fn things_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("HOME_FEELING_THINGS_DIR") {
        return dir.into();
    }
    dirs::config_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("home-feeling")
        .join("things")
}

/// Settings, consent answers, and built-in bookkeeping.
pub fn data_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("home-feeling")
}

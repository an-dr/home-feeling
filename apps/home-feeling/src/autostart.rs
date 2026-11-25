//! Starting with the user's session: one value under the per-user `Run` key
//! on Windows, which needs no administrator rights and is the same list the
//! Startup apps page shows.
//!
//! The value is the program to run, in quotes, and `--minimized` when the app
//! should start in the tray.
#[cfg(windows)]
use std::path::PathBuf;

/// The program a login should start: the launcher of the installed copy, which
/// survives updates, and otherwise the running one.
#[cfg(windows)]
fn program() -> Option<PathBuf> {
    crate::installation::startup_exe()
}

#[cfg(windows)]
mod imp {
    use windows_sys::Win32::System::Registry::{
        RegDeleteKeyValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ,
    };

    const KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const NAME: &str = "home-feeling";

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn set(enabled: bool, minimized: bool) -> Result<(), String> {
        let (key, name) = (wide(KEY), wide(NAME));
        if !enabled {
            // SAFETY: plain call with NUL-ended names; a missing value is fine.
            unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr()) };
            return Ok(());
        }
        let program = super::program().ok_or("cannot find this program's file")?;
        let command = format!(
            "\"{}\"{}",
            program.display(),
            if minimized { " --minimized" } else { "" }
        );
        let data = wide(&command);
        // SAFETY: `data` is a NUL-ended UTF-16 string and its size in bytes is given.
        let status = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                REG_SZ,
                data.as_ptr().cast(),
                (data.len() * 2) as u32,
            )
        };
        if status == 0 {
            Ok(())
        } else {
            Err(format!(
                "Windows refused the startup entry (error {status})"
            ))
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn set(_enabled: bool, _minimized: bool) -> Result<(), String> {
        Err("starting with the session is only set up on Windows".into())
    }
}

pub use imp::set;

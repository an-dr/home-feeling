//! Which Things the user turned off, kept in one JSON file.
use std::collections::BTreeSet;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Default, Serialize, Deserialize)]
struct Stored {
    #[serde(default)]
    disabled: BTreeSet<String>,
}

pub struct Settings {
    path: PathBuf,
    stored: Stored,
}

impl Settings {
    pub fn open(path: PathBuf) -> Self {
        let stored = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self { path, stored }
    }

    pub fn enabled(&self, thing: &str) -> bool {
        !self.stored.disabled.contains(thing)
    }

    pub fn set_enabled(&mut self, thing: &str, enabled: bool) -> Result<(), String> {
        if enabled {
            self.stored.disabled.remove(thing);
        } else {
            self.stored.disabled.insert(thing.to_string());
        }
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }
        let text = serde_json::to_vec_pretty(&self.stored).map_err(|err| err.to_string())?;
        std::fs::write(&self.path, text).map_err(|err| err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::Settings;

    #[test]
    fn a_disabled_thing_stays_disabled_after_reopening() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let mut settings = Settings::open(path.clone());
        assert!(settings.enabled("tiler"));
        settings.set_enabled("tiler", false).unwrap();
        assert!(!Settings::open(path).enabled("tiler"));
    }
}

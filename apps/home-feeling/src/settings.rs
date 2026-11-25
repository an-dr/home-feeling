//! Which Things the user turned off and the values they chose for each
//! Thing's settings, kept in one JSON file.
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// The app's own choices, apart from any Thing's.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppSettings {
    /// Start with the user's session.
    #[serde(default)]
    pub autostart: bool,
    /// Start hidden in the tray, not with the window open.
    #[serde(default)]
    pub start_minimized: bool,
}

#[derive(Default, Serialize, Deserialize)]
struct Stored {
    #[serde(default)]
    app: AppSettings,
    #[serde(default)]
    disabled: BTreeSet<String>,
    /// Thing -> setting key -> chosen value; what was never changed is absent.
    #[serde(default)]
    values: BTreeMap<String, BTreeMap<String, serde_json::Value>>,
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
        self.save()
    }

    pub fn app(&self) -> AppSettings {
        self.stored.app
    }

    pub fn set_app(&mut self, app: AppSettings) -> Result<(), String> {
        self.stored.app = app;
        self.save()
    }

    /// What the user chose for `thing`, by setting key.
    pub fn chosen(&self, thing: &str) -> BTreeMap<String, serde_json::Value> {
        self.stored.values.get(thing).cloned().unwrap_or_default()
    }

    pub fn choose(
        &mut self,
        thing: &str,
        key: &str,
        value: serde_json::Value,
    ) -> Result<(), String> {
        self.stored
            .values
            .entry(thing.to_string())
            .or_default()
            .insert(key.to_string(), value);
        self.save()
    }

    /// Forgets every choice for `thing`, so its defaults apply.
    pub fn reset(&mut self, thing: &str) -> Result<(), String> {
        self.stored.values.remove(thing);
        self.save()
    }

    fn save(&self) -> Result<(), String> {
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

    #[test]
    fn the_apps_own_choices_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let mut settings = Settings::open(path.clone());
        assert!(!settings.app().autostart);
        let app = super::AppSettings {
            autostart: true,
            start_minimized: true,
        };
        settings.set_app(app).unwrap();
        assert_eq!(Settings::open(path).app(), app);
    }

    #[test]
    fn chosen_values_are_kept_until_reset() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let mut settings = Settings::open(path.clone());
        settings
            .choose("tiler", "gap", serde_json::json!(12))
            .unwrap();
        assert_eq!(Settings::open(path.clone()).chosen("tiler")["gap"], 12);
        settings.reset("tiler").unwrap();
        assert!(Settings::open(path).chosen("tiler").is_empty());
    }
}

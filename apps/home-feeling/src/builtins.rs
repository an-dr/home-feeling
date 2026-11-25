//! The Things that ship with the app. They are copied into the user's Things
//! folder, where the user owns them: a file the user edited is never
//! overwritten; a newer built-in is offered instead (UC-UPDATE-01).
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::path::Path;

/// Each built-in Thing: its folder name and its files.
const BUILTINS: &[(&str, &[(&str, &str)])] = &[(
    "tiler",
    &[
        (
            "manifest.toml",
            include_str!("../../../things/tiler/manifest.toml"),
        ),
        ("init.lua", include_str!("../../../things/tiler/init.lua")),
    ],
)];

/// The built-in file each user file was last copied from, by relative path.
type Copied = BTreeMap<String, u64>;

fn fingerprint(text: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.replace("\r\n", "\n").hash(&mut hasher);
    hasher.finish()
}

/// Copies missing built-in files and refreshes ones the user never edited.
/// Returns the relative paths of edited files that have a newer built-in,
/// which is written beside them as `<file>.new`.
pub fn install(things_dir: &Path, record: &Path) -> Vec<String> {
    let mut copied: Copied = std::fs::read(record)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    let mut offered = Vec::new();
    for (thing, files) in BUILTINS {
        let dir = things_dir.join(thing);
        if std::fs::create_dir_all(&dir).is_err() {
            continue;
        }
        for (file, text) in *files {
            let key = format!("{thing}/{file}");
            let path = dir.join(file);
            let new = fingerprint(text);
            let current = std::fs::read_to_string(&path).ok().map(|t| fingerprint(&t));
            let untouched = match (current, copied.get(&key)) {
                (None, _) => true,
                (Some(current), Some(last)) => current == *last,
                (Some(current), None) => current == new,
            };
            if current == Some(new) {
                copied.insert(key, new);
            } else if untouched {
                if std::fs::write(&path, text).is_ok() {
                    copied.insert(key, new);
                }
            } else {
                let _ = std::fs::write(dir.join(format!("{file}.new")), text);
                offered.push(key);
            }
        }
    }
    if let Some(parent) = record.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_vec_pretty(&copied) {
        let _ = std::fs::write(record, text);
    }
    offered
}

#[cfg(test)]
mod tests {
    use super::install;

    #[test]
    fn first_run_copies_and_an_edited_file_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let things = dir.path().join("things");
        let record = dir.path().join("builtins.json");
        assert!(install(&things, &record).is_empty());
        let init = things.join("tiler").join("init.lua");
        assert!(init.exists());

        std::fs::write(&init, "-- mine").unwrap();
        let mut copied: std::collections::BTreeMap<String, u64> =
            serde_json::from_slice(&std::fs::read(&record).unwrap()).unwrap();
        copied.insert("tiler/init.lua".into(), 1); // pretend an older built-in
        std::fs::write(&record, serde_json::to_vec(&copied).unwrap()).unwrap();

        let offered = install(&things, &record);
        assert_eq!(std::fs::read_to_string(&init).unwrap(), "-- mine");
        assert_eq!(offered, vec!["tiler/init.lua"]);
        assert!(things.join("tiler").join("init.lua.new").exists());
    }
}

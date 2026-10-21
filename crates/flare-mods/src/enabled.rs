//! `mods.json`: which mods the user turned on. Only the switch is stored; tokens never are.
//!
//! Shape: `{"version":1,"mods":{"notes":true,"other":false}}`. It is written the way the other stores are
//! (temp file and rename, rule 36), and read defensively (rules 40 and 41). Three outcomes, see [`Parsed`]:
//! a DAMAGED file (not JSON, not an object, no `version`, no `mods` object) is no entries and is moved to
//! `mods.json.bad`, so the user's switches are never silently overwritten by a fresh file; a file whose
//! `version` is present and is not exactly 1 (a newer Flare, or `"2"`, or `2.0`) is no entries AND left
//! alone (the store is read-only for the session). An entry whose id is not a legal mod id or whose value
//! is not a boolean is dropped and never written back. A mod with no entry is disabled.
//!
//! The save is synchronous. It is called on a worker (the Settings click hands the change to one), never
//! on the UI thread (rule 34).

use crate::manifest::valid_id;
use flare_settings::atomic::{Opened, open_text, write_atomic};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The file's format version.
pub const VERSION: u64 = 1;

/// The enabled switches, kept in memory and saved on every change.
#[derive(Debug)]
pub struct EnabledStore {
    file: PathBuf,
    mods: BTreeMap<String, bool>,
    /// The file is a version this build does not read, or could not be read nor set aside.
    read_only: bool,
}

impl EnabledStore {
    /// Opens the store. A missing file is no entries; an unreadable or damaged one is no entries and is set
    /// aside; a newer version is no entries and is left alone (read-only).
    pub fn load(file: impl Into<PathBuf>) -> Self {
        let file = file.into();
        let (mods, read_only) = match open_text(&file) {
            // sync-ok: called on a worker
            Opened::Missing => (BTreeMap::new(), false),
            Opened::Text(raw) => match parse(&raw) {
                Parsed::Entries(mods) => (mods, false),
                Parsed::Newer => {
                    log::warn!(
                        "{} is a version this Flare does not read; left alone",
                        file.display()
                    );
                    (BTreeMap::new(), true)
                }
                Parsed::Damaged => {
                    let moved = set_aside(&file);
                    log::warn!(
                        "{} is damaged; {}",
                        file.display(),
                        if moved {
                            "kept as .bad, and the switches start empty"
                        } else {
                            "left alone and not written this session"
                        }
                    );
                    (BTreeMap::new(), !moved)
                }
            },
            Opened::Unreadable { moved_aside } => (BTreeMap::new(), !moved_aside),
        };
        Self {
            file,
            mods,
            read_only,
        }
    }

    /// Whether the mod is switched on. A mod with no entry is off.
    pub fn is_enabled(&self, id: &str) -> bool {
        self.mods.get(id).copied().unwrap_or(false)
    }

    /// The ids switched on, sorted.
    pub fn enabled_ids(&self) -> Vec<String> {
        self.mods
            .iter()
            .filter(|(_, on)| **on)
            .map(|(id, _)| id.clone())
            .collect()
    }

    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// Switches a mod on or off and saves. A refused id, a read-only store or a failed write changes
    /// nothing: the in-memory state goes back to what it was, and the error says why.
    pub fn set(&mut self, id: &str, enabled: bool) -> Result<(), String> {
        if !valid_id(id) {
            return Err(format!("\"{id}\" is not a mod id"));
        }
        if self.read_only {
            return Err(
                "mods.json is a version this Flare does not read; the switch is not saved".into(),
            );
        }
        let before = self.mods.insert(id.to_string(), enabled);
        if let Err(e) = write_atomic(&self.file, &self.to_json()) {
            // sync-ok: called on a worker
            match before {
                Some(v) => {
                    self.mods.insert(id.to_string(), v);
                }
                None => {
                    self.mods.remove(id);
                }
            }
            return Err(format!("mods.json could not be saved: {e}"));
        }
        Ok(())
    }

    /// The file's text for the current entries.
    pub fn to_json(&self) -> String {
        let mods: Map<String, Value> = self
            .mods
            .iter()
            .map(|(id, on)| (id.clone(), json!(on)))
            .collect();
        json!({ "version": VERSION, "mods": mods }).to_string()
    }

    pub fn path(&self) -> &Path {
        &self.file
    }
}

/// What the file's text is.
#[derive(Debug, PartialEq, Eq)]
pub enum Parsed {
    /// Version 1: the valid entries (anything else in it is dropped).
    Entries(BTreeMap<String, bool>),
    /// A `version` that is present and is not exactly 1: a newer Flare's file, or one written as `"2"`.
    /// Read as empty and left alone.
    Newer,
    /// Not JSON, not an object, no `version`, or no `mods` object. Set aside, and the switches start empty.
    Damaged,
}

/// Reads the file's text (see the module doc for the three outcomes).
pub fn parse(raw: &str) -> Parsed {
    let Ok(v) = serde_json::from_str::<Value>(raw) else {
        return Parsed::Damaged;
    };
    let Some(obj) = v.as_object() else {
        return Parsed::Damaged;
    };
    match obj.get("version") {
        None => return Parsed::Damaged,
        Some(ver) if ver.as_u64() != Some(VERSION) => return Parsed::Newer,
        Some(_) => {}
    }
    let Some(mods) = obj.get("mods").and_then(Value::as_object) else {
        return Parsed::Damaged;
    };
    Parsed::Entries(
        mods.iter()
            .filter_map(|(id, on)| Some((id.clone(), on.as_bool()?)))
            .filter(|(id, _)| valid_id(id))
            .collect(),
    )
}

/// Moves a damaged file to `<file>.bad`, replacing an older `.bad`. False when the move fails, and then the
/// store is read-only for the session so the damaged file is not overwritten.
fn set_aside(file: &Path) -> bool {
    let mut bad = file.as_os_str().to_owned();
    bad.push(".bad");
    std::fs::rename(file, PathBuf::from(bad)).is_ok() // sync-ok: called on a worker
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_store_round_trips_through_its_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("mods.json");
        let mut s = EnabledStore::load(&file);
        assert!(!s.is_enabled("notes"));
        s.set("notes", true).unwrap();
        s.set("other-mod", true).unwrap();
        s.set("other-mod", false).unwrap();
        let again = EnabledStore::load(&file);
        assert!(again.is_enabled("notes"));
        assert!(!again.is_enabled("other-mod"));
        assert_eq!(again.enabled_ids(), ["notes"]);
    }

    #[test]
    fn the_file_has_a_version_and_a_stable_shape() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("mods.json");
        let mut s = EnabledStore::load(&file);
        s.set("notes", true).unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(), // sync-ok: test code only
            r#"{"version":1,"mods":{"notes":true}}"#
        );
    }

    #[test]
    fn an_illegal_id_is_refused_and_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("mods.json");
        let mut s = EnabledStore::load(&file);
        assert!(s.set("../evil", true).is_err());
        assert!(s.set("Notes", true).is_err());
        assert!(!file.exists());
    }

    #[test]
    fn a_damaged_file_is_no_entries_and_is_set_aside() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("mods.json");
        std::fs::write(&file, [0xff, 0xfe, 0x00]).unwrap(); // sync-ok: test code only
        let mut s = EnabledStore::load(&file);
        assert!(s.enabled_ids().is_empty());
        assert!(!s.is_read_only());
        assert!(dir.path().join("mods.json.bad").exists());
        s.set("notes", true).unwrap();
        assert!(EnabledStore::load(&file).is_enabled("notes"));
    }

    #[test]
    fn unknown_entries_are_dropped_on_read() {
        let raw = r#"{"version":1,"mods":{
            "notes": true,
            "off-one": false,
            "Bad": true,
            "../up": true,
            "string-value": "yes",
            "number": 1,
            "nul": null
        }}"#;
        let Parsed::Entries(got) = parse(raw) else {
            panic!("version 1 with a mods object is entries");
        };
        assert_eq!(got.get("notes"), Some(&true));
        assert_eq!(got.get("off-one"), Some(&false));
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn damaged_json_or_the_wrong_shape_is_damaged_not_newer() {
        for raw in [
            "",
            "not json",
            "[]",
            "{}",
            r#"{"version":1}"#,
            r#"{"version":1,"mods":5}"#,
            r#"{"version":1,"mods":[true]}"#,
        ] {
            assert_eq!(parse(raw), Parsed::Damaged, "{raw:?}");
        }
    }

    #[test]
    fn a_version_that_is_not_exactly_one_is_newer_and_a_string_one_is_too() {
        for raw in [
            r#"{"version":2,"mods":{}}"#,
            r#"{"version":"2","mods":{}}"#,
            r#"{"version":"1","mods":{}}"#,
            r#"{"version":2.0,"mods":{}}"#,
            r#"{"version":null,"mods":{}}"#,
        ] {
            assert_eq!(parse(raw), Parsed::Newer, "{raw}");
        }
    }

    #[test]
    fn a_newer_version_is_read_as_empty_and_left_read_only() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("mods.json");
        let future = r#"{"version":2,"mods":{"notes":true}}"#;
        std::fs::write(&file, future).unwrap(); // sync-ok: test code only
        let mut s = EnabledStore::load(&file);
        assert!(!s.is_enabled("notes"));
        assert!(s.is_read_only());
        assert!(s.set("notes", true).is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), future); // sync-ok: test code only
    }

    #[test]
    fn a_string_version_is_left_untouched_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("mods.json");
        let odd = r#"{"version":"2","mods":{"notes":true}}"#;
        std::fs::write(&file, odd).unwrap(); // sync-ok: test code only
        let mut s = EnabledStore::load(&file);
        assert!(s.is_read_only());
        assert!(s.set("other", true).is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), odd); // sync-ok: test code only
        assert!(
            !dir.path().join("mods.json.bad").exists(),
            "not set aside either"
        );
    }

    #[test]
    fn damaged_json_is_set_aside_and_the_next_save_writes_a_fresh_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("mods.json");
        std::fs::write(&file, "not json at all").unwrap(); // sync-ok: test code only
        let mut s = EnabledStore::load(&file);
        assert!(s.enabled_ids().is_empty());
        assert!(
            !s.is_read_only(),
            "a damaged file is moved, so the store is writable"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("mods.json.bad")).unwrap(), // sync-ok: test code only
            "not json at all"
        );
        assert!(!file.exists());
        s.set("notes", true).unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(), // sync-ok: test code only
            r#"{"version":1,"mods":{"notes":true}}"#
        );
    }

    #[test]
    fn a_failed_save_puts_the_old_state_back() {
        let dir = tempfile::tempdir().unwrap();
        // A folder that does not exist makes the temp-file write fail; the file is in no folder.
        let file = dir.path().join("gone").join("mods.json");
        let mut s = EnabledStore::load(&file);
        assert!(s.set("notes", true).is_err());
        assert!(!s.is_enabled("notes"));
    }
}

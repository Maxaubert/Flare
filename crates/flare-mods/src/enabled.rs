//! `mods.json`: which mods and plugins the user turned on. Only the switch is stored; tokens never are.
//!
//! Shape: `{"version":1,"mods":{"notes":{"on":true,"permissions":["panel","tools"]},"other":{"on":false}}}`.
//! A protocol 2 plugin's entry is the same, plus what the plugin said it was when it was approved:
//! `{"on":true,"permissions":[...],"author":"...","homepage":"https://...","source":"...","hash":"sha256:..."}`, each
//! of the four only when known. The file keeps its name and version 1 (design section 13), so an older Flare still
//! reads every switch (it ignores the extra keys) and the protocol 1 entries are written exactly as before.
//! The `permissions` are the set the user APPROVED when they switched the mod on; a mod whose manifest later
//! asks for more is turned off by the hub until it is switched on again. An entry that is on but has no
//! approved set (the old `"notes":true` form, or a hand-edited one) reads as NOT enabled: it fails closed,
//! because nobody can say what was approved. A plain `false` is an off entry. It is written the way the other stores are
//! (temp file and rename, rule 36), and read defensively (rules 40 and 41). Three outcomes, see [`Parsed`]:
//! a DAMAGED file (not JSON, not an object, no `version`, no `mods` object) is no entries and is moved to
//! `mods.json.bad`, so the user's switches are never silently overwritten by a fresh file; a file whose
//! `version` is present and is not exactly 1 (a newer Flare, or `"2"`, or `2.0`) is no entries AND left
//! alone (the store is read-only for the session). An entry whose id is not a legal mod id or whose value
//! is not a boolean or an entry object is dropped and never written back. A mod with no entry is disabled.
//!
//! The save is synchronous. It is called on a worker (the Settings click hands the change to one), never
//! on the UI thread (rule 34).

use crate::manifest::valid_id;
use flare_ctl::Permission;
use flare_settings::atomic::{Opened, open_text, write_atomic};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The file's format version.
pub const VERSION: u64 = 1;

/// One mod's saved switch: on or off, and, when it is on, the permissions the user approved.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Entry {
    pub on: bool,
    /// Present for an entry written when the mod was switched on; `None` for an old or hand-made one.
    pub approved: Option<Vec<Permission>>,
    /// Who a protocol 2 plugin said it was when it was approved. Empty for a protocol 1 mod and an off entry.
    pub identity: Identity,
}

/// What a plugin said about itself when the person approved it, kept with the approval: its `author` and
/// `homepage`, where it was installed from (`source`, once install exists) and a hash of its folder's files. A
/// change of author or homepage turns the plugin off until it is approved again.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Identity {
    pub author: Option<String>,
    pub homepage: Option<String>,
    pub source: Option<String>,
    /// `sha256:` and 64 lower-case hex digits (`crate::hash`).
    pub hash: Option<String>,
}

/// The longest identity text kept; a longer one (a hand edit) is dropped on read.
const MAX_IDENTITY_CHARS: usize = 500;

/// The enabled switches, kept in memory and saved on every change.
#[derive(Debug)]
pub struct EnabledStore {
    file: PathBuf,
    mods: BTreeMap<String, Entry>,
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

    /// Whether the mod is switched on: an entry that is on AND holds an approved set. A mod with no entry, or
    /// an on entry without an approved set, is off.
    pub fn is_enabled(&self, id: &str) -> bool {
        self.mods
            .get(id)
            .is_some_and(|e| e.on && e.approved.is_some())
    }

    /// The permissions the user approved for a mod that is on.
    pub fn approved(&self, id: &str) -> Option<Vec<Permission>> {
        self.mods
            .get(id)
            .filter(|e| e.on)
            .and_then(|e| e.approved.clone())
    }

    /// What the plugin said it was when it was approved, for a mod that is on.
    pub fn identity(&self, id: &str) -> Option<Identity> {
        self.mods
            .get(id)
            .filter(|e| e.on && e.approved.is_some())
            .map(|e| e.identity.clone())
    }

    /// The ids switched on (with an approved set), sorted.
    pub fn enabled_ids(&self) -> Vec<String> {
        self.mods
            .keys()
            .filter(|id| self.is_enabled(id))
            .cloned()
            .collect()
    }

    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// Switches a mod on or off and saves. A refused id, a read-only store or a failed write changes
    /// nothing: the in-memory state goes back to what it was, and the error says why.
    /// Switching on takes the permissions the user is approving; off drops them.
    pub fn set(&mut self, id: &str, enabled: bool, approved: &[Permission]) -> Result<(), String> {
        self.set_with(id, enabled, approved, &Identity::default())
    }

    /// [`EnabledStore::set`] for a protocol 2 plugin: switching on keeps `identity` with the approval.
    pub fn set_with(
        &mut self,
        id: &str,
        enabled: bool,
        approved: &[Permission],
        identity: &Identity,
    ) -> Result<(), String> {
        if !valid_id(id) {
            return Err(format!("\"{id}\" is not a mod id"));
        }
        if self.read_only {
            return Err(
                "mods.json is a version this Flare does not read; the switch is not saved".into(),
            );
        }
        let entry = Entry {
            on: enabled,
            approved: enabled.then(|| approved.to_vec()),
            identity: if enabled {
                identity.clone()
            } else {
                Identity::default()
            },
        };
        let before = self.mods.insert(id.to_string(), entry);
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
            .map(|(id, e)| {
                let v = match &e.approved {
                    Some(p) if e.on => {
                        let mut o = Map::new();
                        o.insert("on".into(), Value::Bool(true));
                        o.insert(
                            "permissions".into(),
                            json!(p.iter().map(|p| p.name()).collect::<Vec<_>>()),
                        );
                        let i = &e.identity;
                        for (key, val) in [
                            ("author", &i.author),
                            ("homepage", &i.homepage),
                            ("source", &i.source),
                            ("hash", &i.hash),
                        ] {
                            if let Some(v) = val {
                                o.insert(key.into(), Value::String(v.clone()));
                            }
                        }
                        Value::Object(o)
                    }
                    _ => json!({ "on": false }),
                };
                (id.clone(), v)
            })
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
    Entries(BTreeMap<String, Entry>),
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
            .filter_map(|(id, v)| Some((id.clone(), entry_of(v)?)))
            .filter(|(id, _)| valid_id(id))
            .collect(),
    )
}

/// One entry's value: a bare boolean is the old form (`true` has no approved set, so it reads as not enabled);
/// an object has `on` (a boolean) and, for an on entry, `permissions` (names; any unknown name makes the set
/// unreadable, so the entry fails closed). Anything else is dropped.
fn entry_of(v: &Value) -> Option<Entry> {
    if let Some(on) = v.as_bool() {
        return Some(Entry {
            on,
            ..Entry::default()
        });
    }
    let o = v.as_object()?;
    let on = o.get("on")?.as_bool()?;
    let approved = o
        .get("permissions")
        .and_then(Value::as_array)
        .and_then(|a| {
            a.iter()
                .map(|n| n.as_str().and_then(Permission::parse))
                .collect::<Option<Vec<_>>>()
        });
    let text = |key: &str| {
        o.get(key)
            .and_then(Value::as_str)
            .filter(|s| s.chars().count() <= MAX_IDENTITY_CHARS && !s.chars().any(char::is_control))
            .map(str::to_string)
    };
    let identity = Identity {
        author: text("author"),
        homepage: text("homepage"),
        source: text("source"),
        hash: text("hash").filter(|h| crate::hash::is_hash(h)),
    };
    Some(Entry {
        on,
        approved,
        identity,
    })
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

    const P: &[Permission] = &[Permission::Panel, Permission::Tools];

    #[test]
    fn a_store_round_trips_through_its_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("mods.json");
        let mut s = EnabledStore::load(&file);
        assert!(!s.is_enabled("notes"));
        s.set("notes", true, P).unwrap();
        s.set("other-mod", true, P).unwrap();
        s.set("other-mod", false, P).unwrap();
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
        s.set("notes", true, P).unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(), // sync-ok: test code only
            r#"{"version":1,"mods":{"notes":{"on":true,"permissions":["panel","tools"]}}}"#
        );
    }

    #[test]
    fn an_illegal_id_is_refused_and_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("mods.json");
        let mut s = EnabledStore::load(&file);
        assert!(s.set("../evil", true, P).is_err());
        assert!(s.set("Notes", true, P).is_err());
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
        s.set("notes", true, P).unwrap();
        assert!(EnabledStore::load(&file).is_enabled("notes"));
    }

    #[test]
    fn unknown_entries_are_dropped_on_read() {
        let raw = r#"{"version":1,"mods":{
            "notes": {"on": true, "permissions": ["panel"]},
            "off-one": false,
            "old-form": true,
            "bad-perm": {"on": true, "permissions": ["panel", "root"]},
            "Bad": true,
            "../up": true,
            "string-value": "yes",
            "number": 1,
            "nul": null
        }}"#;
        let Parsed::Entries(got) = parse(raw) else {
            panic!("version 1 with a mods object is entries");
        };
        assert_eq!(
            got.get("notes").unwrap().approved,
            Some(vec![Permission::Panel])
        );
        assert!(!got.get("off-one").unwrap().on);
        assert_eq!(got.get("old-form").unwrap().approved, None);
        assert_eq!(got.get("bad-perm").unwrap().approved, None);
        assert_eq!(got.len(), 4);
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
        assert!(s.set("notes", true, P).is_err());
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
        assert!(s.set("other", true, P).is_err());
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
        s.set("notes", true, P).unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(), // sync-ok: test code only
            r#"{"version":1,"mods":{"notes":{"on":true,"permissions":["panel","tools"]}}}"#
        );
    }

    #[test]
    fn a_failed_save_puts_the_old_state_back() {
        let dir = tempfile::tempdir().unwrap();
        // A folder that does not exist makes the temp-file write fail; the file is in no folder.
        let file = dir.path().join("gone").join("mods.json");
        let mut s = EnabledStore::load(&file);
        assert!(s.set("notes", true, P).is_err());
        assert!(!s.is_enabled("notes"));
    }

    fn ident() -> Identity {
        Identity {
            author: Some("someone".into()),
            homepage: Some("https://example.com/todo".into()),
            source: None,
            hash: Some(format!("sha256:{}", "ab".repeat(32))),
        }
    }

    #[test]
    fn a_plugin_keeps_its_identity_with_the_approval_and_loses_it_when_off() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("mods.json");
        let mut s = EnabledStore::load(&file);
        s.set_with("todo", true, &[Permission::Decorate], &ident())
            .unwrap();
        s.set("notes", true, P).unwrap();
        let text = std::fs::read_to_string(&file).unwrap(); // sync-ok: test code only
        assert_eq!(
            text,
            format!(
                r#"{{"version":1,"mods":{{"notes":{{"on":true,"permissions":["panel","tools"]}},"todo":{{"on":true,"permissions":["decorate"],"author":"someone","homepage":"https://example.com/todo","hash":"sha256:{}"}}}}}}"#,
                "ab".repeat(32)
            ),
            "a protocol 1 entry is written exactly as before; the plugin adds what is known"
        );
        let again = EnabledStore::load(&file);
        assert_eq!(again.identity("todo"), Some(ident()));
        assert_eq!(again.identity("notes"), Some(Identity::default()));
        assert_eq!(again.approved("todo"), Some(vec![Permission::Decorate]));
        let mut again = again;
        again
            .set_with("todo", false, &[Permission::Decorate], &ident())
            .unwrap();
        assert_eq!(again.identity("todo"), None);
        let text = std::fs::read_to_string(&file).unwrap(); // sync-ok: test code only
        assert!(text.contains(r#""todo":{"on":false}"#), "{text}");
    }

    /// The store an older Flare reads: the same file, the same version, and for every entry the two keys it knows.
    #[test]
    fn an_older_reader_sees_every_switch_of_a_store_with_plugins() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("mods.json");
        let mut s = EnabledStore::load(&file);
        s.set_with("todo", true, &[Permission::Panel], &ident())
            .unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap(); // sync-ok: test code only
        assert_eq!(v["version"], 1);
        assert_eq!(v["mods"]["todo"]["on"], true);
        assert_eq!(v["mods"]["todo"]["permissions"], json!(["panel"]));
    }

    #[test]
    fn a_damaged_identity_field_is_dropped_on_read() {
        let raw = format!(
            r#"{{"version":1,"mods":{{"todo":{{"on":true,"permissions":["panel"],"author":5,"homepage":"{}","source":"a\u0007b","hash":"md5:00"}}}}}}"#,
            "x".repeat(501)
        );
        let Parsed::Entries(got) = parse(&raw) else {
            panic!("entries");
        };
        let e = got.get("todo").unwrap();
        assert_eq!(e.identity, Identity::default());
        assert_eq!(e.approved, Some(vec![Permission::Panel]));
    }

    #[test]
    fn an_on_entry_without_an_approved_set_reads_as_not_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("mods.json");
        // The old format, a hand-made object, and a set with an unknown name: none can say what was approved.
        let raw = r#"{"version":1,"mods":{
            "old-form": true,
            "no-set": {"on": true},
            "bad-name": {"on": true, "permissions": ["panel", "root"]},
            "good": {"on": true, "permissions": ["panel"]}
        }}"#;
        std::fs::write(&file, raw).unwrap(); // sync-ok: test code only
        let s = EnabledStore::load(&file);
        assert_eq!(s.enabled_ids(), ["good"]);
        assert!(!s.is_enabled("old-form"));
        assert!(!s.is_enabled("no-set"));
        assert!(!s.is_enabled("bad-name"));
        assert_eq!(s.approved("good"), Some(vec![Permission::Panel]));
        assert_eq!(s.approved("old-form"), None);
    }
}

//! Machine pairing PR 2 (rules 1252 and 1255): the Computer sharing switch and what Flare changed to turn it on.
//!
//! Keys: `flare.machines.sharing` (a bool, default OFF: the switch, kept across restarts once the setup finished),
//! `flare.machines.serverByFlare` (a bool: Flare started or installed the SSH server when sharing turned on, so turning
//! sharing off offers to stop it again) and `flare.machines.adminKeys` (a bool: pairing, PR 3, put a key in Windows'
//! `administrators_authorized_keys`, so turning sharing off needs the elevated clean up) and `flare.machines.firewallRule`
//! (a bool: Windows' setup added Flare's own firewall rules, so turning sharing off removes them, elevated).
//!
//! Import remote sessions PR 2 (rule 1290): `flare.machines.importRemote` (a bool, default ON: the switch "Import
//! sessions started on other machines") and `flare.machines.importHidden` (a JSON object, machine id to the host tab
//! ids hidden here, at most [`IMPORT_HIDDEN_MAX`] per machine, the oldest dropped).

use crate::kv::{PrefStore, bool_pref_get, bool_pref_set};
use std::collections::BTreeMap;

const SHARING: &str = "flare.machines.sharing";
const SERVER_BY_FLARE: &str = "flare.machines.serverByFlare";
const ADMIN_KEYS: &str = "flare.machines.adminKeys";
const FIREWALL_RULE: &str = "flare.machines.firewallRule";
const IMPORT_REMOTE: &str = "flare.machines.importRemote";
const IMPORT_HIDDEN: &str = "flare.machines.importHidden";

/// The most host tab ids the hidden list keeps for one machine; past it the oldest drop.
pub const IMPORT_HIDDEN_MAX: usize = 200;

/// Computer sharing is on (the setup finished). Off by default.
pub fn sharing_on(s: &PrefStore) -> bool {
    bool_pref_get(s, SHARING, false)
}

pub fn set_sharing_on(s: &PrefStore, on: bool) {
    bool_pref_set(s, SHARING, on);
}

/// Flare turned the SSH server on (installed it, or started a stopped one) when sharing turned on.
pub fn server_by_flare(s: &PrefStore) -> bool {
    bool_pref_get(s, SERVER_BY_FLARE, false)
}

pub fn set_server_by_flare(s: &PrefStore, on: bool) {
    bool_pref_set(s, SERVER_BY_FLARE, on);
}

/// A paired computer's key went into Windows' `administrators_authorized_keys` (machine pairing PR 3 sets it).
pub fn admin_keys(s: &PrefStore) -> bool {
    bool_pref_get(s, ADMIN_KEYS, false)
}

pub fn set_admin_keys(s: &PrefStore, on: bool) {
    bool_pref_set(s, ADMIN_KEYS, on);
}

/// Windows' setup added Flare's own firewall rules (rule 1253); turning sharing off removes them (rule 1255).
pub fn firewall_rule(s: &PrefStore) -> bool {
    bool_pref_get(s, FIREWALL_RULE, false)
}

pub fn set_firewall_rule(s: &PrefStore, on: bool) {
    bool_pref_set(s, FIREWALL_RULE, on);
}

/// Sessions started in Flare on a linked machine show here as remote tabs (rule 1290). On by default.
pub fn import_remote_on(s: &PrefStore) -> bool {
    bool_pref_get(s, IMPORT_REMOTE, true)
}

pub fn set_import_remote_on(s: &PrefStore, on: bool) {
    bool_pref_set(s, IMPORT_REMOTE, on);
}

/// The host tabs hidden here ("Hide here only"), machine id to host tab ids, oldest first. Bad JSON, or a value that is
/// not a list of words, reads as nothing hidden; each list keeps its newest [`IMPORT_HIDDEN_MAX`] ids.
pub fn import_hidden(s: &PrefStore) -> BTreeMap<String, Vec<String>> {
    let Some(raw) = s.get(IMPORT_HIDDEN) else {
        return BTreeMap::new();
    };
    let Ok(serde_json::Value::Object(o)) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return BTreeMap::new();
    };
    o.into_iter()
        .filter_map(|(machine, ids)| {
            let ids: Vec<String> = ids
                .as_array()?
                .iter()
                .filter_map(|v| v.as_str().filter(|s| !s.is_empty()).map(str::to_string))
                .collect();
            (!ids.is_empty()).then(|| (machine, newest(ids)))
        })
        .collect()
}

/// Writes the hidden lists, each capped to its newest [`IMPORT_HIDDEN_MAX`] ids; an empty list is not kept, and with
/// none left the key goes.
pub fn set_import_hidden(s: &PrefStore, hidden: &BTreeMap<String, Vec<String>>) {
    let kept: BTreeMap<&String, Vec<String>> = hidden
        .iter()
        .filter(|(_, ids)| !ids.is_empty())
        .map(|(m, ids)| (m, newest(ids.clone())))
        .collect();
    if kept.is_empty() {
        s.remove(IMPORT_HIDDEN);
        return;
    }
    s.set(
        IMPORT_HIDDEN,
        &serde_json::to_string(&kept).unwrap_or_else(|_| "{}".into()),
    );
}

/// The newest [`IMPORT_HIDDEN_MAX`] ids of a list kept oldest first.
fn newest(mut ids: Vec<String>) -> Vec<String> {
    if ids.len() > IMPORT_HIDDEN_MAX {
        ids.drain(..ids.len() - IMPORT_HIDDEN_MAX);
    }
    ids
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sharing_and_what_flare_changed_default_off_and_keep_what_is_set() {
        let s = PrefStore::memory();
        assert!(!sharing_on(&s) && !server_by_flare(&s) && !admin_keys(&s) && !firewall_rule(&s));
        set_sharing_on(&s, true);
        set_server_by_flare(&s, true);
        set_admin_keys(&s, true);
        set_firewall_rule(&s, true);
        assert!(sharing_on(&s) && server_by_flare(&s) && admin_keys(&s) && firewall_rule(&s));
        assert_eq!(s.get("flare.machines.firewallRule").as_deref(), Some("1"));
        assert_eq!(s.get("flare.machines.sharing").as_deref(), Some("1"));
        set_sharing_on(&s, false);
        assert!(!sharing_on(&s));
    }

    /// Rule 1290: the import switch is on until turned off; the hidden list keeps the newest 200 ids per machine and
    /// reads anything it cannot use as nothing hidden.
    #[test]
    fn import_remote_defaults_on_and_the_hidden_list_is_capped_and_reads_bad_json_as_empty() {
        let s = PrefStore::memory();
        assert!(import_remote_on(&s), "on by default");
        set_import_remote_on(&s, false);
        assert!(!import_remote_on(&s));
        assert_eq!(s.get("flare.machines.importRemote").as_deref(), Some("0"));
        set_import_remote_on(&s, true);
        assert!(import_remote_on(&s));

        assert!(import_hidden(&s).is_empty());
        let many: Vec<String> = (0..250).map(|n| format!("t1-{n}")).collect();
        let mut m = BTreeMap::new();
        m.insert("mac".to_string(), many);
        m.insert("pc".to_string(), vec!["t2-1".to_string()]);
        m.insert("gone".to_string(), Vec::new());
        set_import_hidden(&s, &m);
        let back = import_hidden(&s);
        assert_eq!(back.len(), 2, "an empty list is not kept");
        assert_eq!(back["mac"].len(), IMPORT_HIDDEN_MAX);
        assert_eq!(back["mac"][0], "t1-50", "the oldest drop");
        assert_eq!(back["mac"][IMPORT_HIDDEN_MAX - 1], "t1-249");
        assert_eq!(back["pc"], ["t2-1"]);

        s.set("flare.machines.importHidden", "{not json");
        assert!(import_hidden(&s).is_empty());
        s.set("flare.machines.importHidden", "[1,2]");
        assert!(import_hidden(&s).is_empty());
        s.set(
            "flare.machines.importHidden",
            r#"{"mac": "t1", "pc": ["t2", 3, ""]}"#,
        );
        let odd = import_hidden(&s);
        assert_eq!(odd.len(), 1, "a list that is not one reads as nothing");
        assert_eq!(odd["pc"], ["t2"]);

        set_import_hidden(&s, &BTreeMap::new());
        assert_eq!(s.get("flare.machines.importHidden"), None);
    }
}

//! The machines a remote tab can run on (remote sessions PR 5): `machines.json` in the profile, read by the link that
//! starts ssh, by `open.remote` (the agent's tool and `flare open --on`) and, from PR 7, by Settings > Machines.
//!
//! `{"machines":[{"id":"mac","name":"Mac Mini","kind":"mac","ssh":"mac-mini","user":"max","flare":"/Applications/..."}]}`
//!
//! Hand-edited until PR 7's page. A machine whose id, host or user is not a plain word is left out when it is read
//! (no option smuggled into ssh's argv, rule 290); an unknown `kind` reads as no kind and is never written back
//! (rule 40). Saving writes the whole file at once (`write_atomic`, rule 36). Every read and write is a disk call:
//! a worker's, never the UI thread's (rule 34).

use flare_tabs::tabs::Os;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// A machine a remote tab can run on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Machine {
    pub id: String,
    pub name: String,
    /// `mac` or `pc`; anything else is no kind and reads as a Mac's shell (rule 40, never written back).
    pub kind: Option<Os>,
    /// The ssh host (an alias of `~/.ssh/config` or a name).
    pub ssh: String,
    pub user: Option<String>,
    /// Flare's executable on the host; `flare` (on the host's PATH) when not given.
    pub flare: String,
}

/// A word that goes to ssh as one argv item: printable, no space, never read as an option.
pub fn argv_word(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 255
        && !s.starts_with('-')
        && s.chars().all(|c| c.is_ascii_graphic())
}

/// Pure: the machines a `machines.json` names. A machine whose id, host or user is not a plain word is left out.
pub fn parse_machines(raw: &str) -> Vec<Machine> {
    let Ok(v) = serde_json::from_str::<Value>(raw) else {
        return Vec::new();
    };
    let Some(list) = v.get("machines").and_then(Value::as_array) else {
        return Vec::new();
    };
    let s = |m: &Value, k: &str| m.get(k).and_then(Value::as_str).map(str::to_string);
    list.iter()
        .filter_map(|m| {
            let id = s(m, "id").filter(|i| argv_word(i))?;
            let ssh = s(m, "ssh").filter(|h| argv_word(h))?;
            let user = match m.get("user") {
                None | Some(Value::Null) => None,
                Some(_) => Some(s(m, "user").filter(|u| argv_word(u))?),
            };
            Some(Machine {
                name: s(m, "name")
                    .filter(|n| !n.trim().is_empty())
                    .unwrap_or_else(|| id.clone()),
                kind: s(m, "kind").as_deref().and_then(Os::parse),
                flare: s(m, "flare")
                    .filter(|f| !f.trim().is_empty())
                    .unwrap_or_else(|| "flare".to_string()),
                id,
                ssh,
                user,
            })
        })
        .collect()
}

/// Pure: the file's text for `machines`. A machine with no kind is written with none (rule 40: an unknown kind read
/// in is not turned into a guess).
#[cfg_attr(not(test), allow(dead_code))] // PR 7's Settings > Machines saves; held by the tests here until then
pub fn machines_json(machines: &[Machine]) -> String {
    let list: Vec<Value> = machines
        .iter()
        .map(|m| {
            let mut v = json!({"id": m.id, "name": m.name, "ssh": m.ssh, "flare": m.flare});
            if let Some(k) = m.kind {
                v["kind"] = json!(match k {
                    Os::Mac => "mac",
                    Os::Pc => "pc",
                });
            }
            if let Some(u) = &m.user {
                v["user"] = json!(u);
            }
            v
        })
        .collect();
    serde_json::to_string_pretty(&json!({"machines": list})).unwrap_or_default()
}

/// `machines.json` in the profile.
pub fn machines_file(profile: &Path) -> PathBuf {
    profile.join("machines.json")
}

/// BLOCKING (a file read): a worker's call. A missing or broken file is no machine.
pub fn read_machines(path: &Path) -> Vec<Machine> {
    std::fs::read_to_string(path) // sync-ok: a link worker, the control channel's worker or a test, never the UI thread
        .map(|raw| parse_machines(&raw))
        .unwrap_or_default()
}

/// The machines as the app reads them: a port, so a test needs no disk.
pub trait MachinesPort: Send + Sync {
    /// Every machine. BLOCKING for the real one: a worker's call.
    fn list(&self) -> Vec<Machine>;

    /// The machine called `id`.
    fn get(&self, id: &str) -> Option<Machine> {
        self.list().into_iter().find(|m| m.id == id)
    }
}

/// The profile's `machines.json`.
pub struct FileMachines {
    path: PathBuf,
}

impl FileMachines {
    pub fn new(profile: &Path) -> Self {
        Self {
            path: machines_file(profile),
        }
    }

    /// Writes the whole list at once (a temp file, then a rename, rule 36). BLOCKING: a worker's call.
    #[cfg_attr(not(test), allow(dead_code))] // PR 7's Settings > Machines saves; held by the tests here until then
    pub fn save(&self, machines: &[Machine]) -> std::io::Result<()> {
        flare_settings::atomic::write_atomic(&self.path, &machines_json(machines))
    }
}

impl MachinesPort for FileMachines {
    fn list(&self) -> Vec<Machine> {
        read_machines(&self.path)
    }
}

/// Pure: the one line a call naming an unknown machine is refused with: the name, and the names there are.
pub fn unknown_machine(id: &str, known: &[Machine]) -> String {
    if known.is_empty() {
        return format!("no machine called {id}: machines.json in Flare's profile names none");
    }
    let names: Vec<&str> = known.iter().map(|m| m.id.as_str()).collect();
    format!(
        "no machine called {id} in machines.json; the machines are {}",
        names.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mac() -> Machine {
        Machine {
            id: "mac".into(),
            name: "Mac Mini".into(),
            kind: Some(Os::Mac),
            ssh: "mac-mini".into(),
            user: Some("max".into()),
            flare: "/Applications/Flare.app/Contents/MacOS/flare".into(),
        }
    }

    /// Rule 36: a save is one whole file, read back the same; the temp file does not stay.
    #[test]
    fn a_saved_list_reads_back_the_same_and_leaves_no_temp_file() {
        let dir = tempfile::tempdir().expect("tmp");
        let file = FileMachines::new(dir.path());
        assert!(file.list().is_empty(), "no file is no machine");
        let pc = Machine {
            id: "pc".into(),
            name: "PC".into(),
            kind: Some(Os::Pc),
            ssh: "desk".into(),
            user: None,
            flare: "C:\\Program Files\\Flare\\flare.exe".into(),
        };
        file.save(&[mac(), pc.clone()]).expect("save");
        assert_eq!(file.list(), vec![mac(), pc]);
        assert_eq!(file.get("mac"), Some(mac()));
        assert_eq!(file.get("nas"), None);
        let names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["machines.json"]);
    }

    /// Rule 40: a kind this build does not know reads as none and is not written back as a guess.
    #[test]
    fn an_unknown_kind_reads_as_none_and_is_written_as_none() {
        let raw = r#"{"machines":[{"id":"nas","ssh":"nas","kind":"beos"}]}"#;
        let ms = parse_machines(raw);
        assert_eq!(ms[0].kind, None);
        assert!(!machines_json(&ms).contains("kind"));
        assert_eq!(ms[0].name, "nas");
        assert_eq!(ms[0].flare, "flare");
    }

    #[test]
    fn an_unknown_machine_is_refused_with_the_names_there_are() {
        assert_eq!(
            unknown_machine("nas", &[mac()]),
            "no machine called nas in machines.json; the machines are mac"
        );
        assert!(unknown_machine("nas", &[]).contains("names none"));
    }
}

//! The machines a remote tab can run on (remote sessions PR 5): `machines.json` in the profile, read by the link that
//! starts ssh, by `open.remote` (the agent's tool and `flare open --on`) and, from PR 7, by Settings > Machines.
//!
//! `{"machines":[{"id":"mac","name":"Mac Mini","kind":"mac","ssh":"mac-mini","user":"max","flare":"/Applications/..."}]}`
//!
//! Settings > Machines (PR 7, `machines_page`) adds and removes them; a hand edit still works. A machine whose id, host or user is not a plain word is left out when it is read
//! (no option smuggled into ssh's argv, rule 290); an unknown `kind` reads as no kind and is never written back
//! (rule 40). Saving writes the whole file at once (`write_atomic`, rule 36). Every read and write is a disk call:
//! a worker's, never the UI thread's (rule 34).

use flare_tabs::tabs::Os;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// A machine a remote tab can run on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
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
    /// The address ssh connects to, when it is not the `ssh` alias (a typed address, a paired machine; rule 1235).
    pub host: Option<String>,
    /// ssh's port on `host` (`-p`); ssh's own (22, or the config's) when not given.
    pub port: Option<u16>,
    /// Flare's own key for this machine (pairing): ssh offers only it (`IdentitiesOnly`, `-i`).
    pub key: Option<String>,
    /// Flare's own known-hosts file for this machine (pairing): ssh trusts only the host key in it.
    pub known_hosts: Option<String>,
}

impl Machine {
    /// What ssh is pointed at: the address when there is one, else the alias.
    pub fn target(&self) -> &str {
        self.host.as_deref().unwrap_or(&self.ssh)
    }
}

/// A file path that goes to ssh as one argv item or one option's value (a key, a known-hosts file, rule 1235): no
/// control character, never read as an option, none of `"` (it would end ssh's own quoting of the value) or `%`, and no
/// `${` (ssh expands `%` tokens and `${VAR}` in both).
pub fn path_word(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 1024
        && !s.starts_with('-')
        && !s.contains("${")
        && !s.chars().any(|c| c.is_control() || c == '"' || c == '%')
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
            // An optional field that is there but not well formed leaves the machine out (rule 290), never half used.
            let opt = |k: &str, ok: fn(&str) -> bool| match m.get(k) {
                None | Some(Value::Null) => Some(None),
                Some(_) => s(m, k).filter(|v| ok(v)).map(Some),
            };
            let id = s(m, "id").filter(|i| argv_word(i))?;
            let user = opt("user", argv_word)?;
            let host = opt("host", argv_word)?;
            let key = opt("key", path_word)?;
            let known_hosts = opt("known_hosts", path_word)?;
            let port = match m.get("port") {
                None | Some(Value::Null) => None,
                Some(p) => Some(
                    p.as_u64()
                        .and_then(|p| u16::try_from(p).ok())
                        .filter(|p| *p > 0)?,
                ),
            };
            // A machine with an address needs no alias: the address stands in (rule 1235).
            let ssh = s(m, "ssh")
                .or_else(|| host.clone())
                .filter(|h| argv_word(h))?;
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
                host,
                port,
                key,
                known_hosts,
            })
        })
        .collect()
}

/// Pure: the file's text for `machines`. A machine with no kind is written with none (rule 40: an unknown kind read
/// in is not turned into a guess).
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
            for (k, val) in [
                ("user", &m.user),
                ("host", &m.host),
                ("key", &m.key),
                ("known_hosts", &m.known_hosts),
            ] {
                if let Some(val) = val {
                    v[k] = json!(val);
                }
            }
            if let Some(p) = m.port {
                v["port"] = json!(p);
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
    pub fn save(&self, machines: &[Machine]) -> std::io::Result<()> {
        flare_settings::atomic::write_atomic(&self.path, &machines_json(machines))
    }
}

impl MachinesPort for FileMachines {
    fn list(&self) -> Vec<Machine> {
        read_machines(&self.path)
    }
}

/// The folder a + menu row opens a remote shell in (remote sessions PR 7): `remote.open` reads this word as the host's
/// own home folder, which the viewer does not know. The tab shows the host's real folder once its shell reports it.
pub const HOST_HOME: &str = "~";

/// Where Settings > Machines looks for Flare on a host it adds (remote sessions PR 7, rules 1207 and 1208), in this
/// order: on the login shell's PATH, then a PC's two installs (Stable, then Dev) under cmd's `%LOCALAPPDATA%`, the same
/// two under PowerShell's `$env:LOCALAPPDATA` (a PowerShell login shell with a space in the profile folder answers only
/// there), then where the Mac app keeps it. The first that answers `remote.hello` is the one saved.
pub const FLARE_CANDIDATES: [&str; 6] = [
    "flare",
    r"%LOCALAPPDATA%\StableFlare\current\FlareStable.exe",
    r"%LOCALAPPDATA%\Flare\current\Flare.exe",
    r"$env:LOCALAPPDATA\StableFlare\current\FlareStable.exe",
    r"$env:LOCALAPPDATA\Flare\current\Flare.exe",
    "/Applications/Flare.app/Contents/MacOS/flare",
];

/// Pure: how a candidate is quoted when it is probed, in the order to try (rule 1207), since the host's system is not
/// known until it answers. A PC path (a leading `%LOCALAPPDATA%`, `$env:LOCALAPPDATA` or a drive letter) gets the
/// PC's `cmd /c` form only; a `/` path gets the sh form only (no kind); a bare name gets the sh form, then the PC form.
pub fn probe_kinds(flare: &str) -> Vec<Option<Os>> {
    let b = flare.as_bytes();
    let drive = b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':';
    if drive
        || flare.starts_with(crate::remote_link::PC_LOCAL_APP_DATA)
        || flare.starts_with(crate::remote_link::PC_LOCAL_APP_DATA_PWSH)
    {
        vec![Some(Os::Pc)]
    } else if flare.starts_with('/') {
        vec![None]
    } else {
        vec![None, Some(Os::Pc)]
    }
}

/// Pure: the kind a machine is saved with once `flare`, probed in `answered`'s quoting, said hello and named `named`
/// (rule 1208). The kind is both the logo and the quoting every later link uses, so the host's own word is kept only
/// when its quoting is the one that answered; otherwise the quoting that answered is kept. A Windows host whose login
/// shell is Git Bash answers sh's form and names itself a PC: saved as a PC, its links would send `cmd /c` to bash,
/// which rewrites `/c` to `C:/`.
pub fn saved_kind(answered: Option<Os>, named: Option<Os>, flare: &str) -> Option<Os> {
    use crate::remote_link::remote_command;
    named
        .filter(|os| remote_command(Some(*os), flare) == remote_command(answered, flare))
        .or(answered)
}

/// Pure: the system `remote.hello` named (`mac`, `pc`); any other word is no kind (rule 40).
pub fn os_of_hello(word: &str) -> Option<Os> {
    Os::parse(word)
}

/// Pure: an id for a new machine named `name`: its letters and digits in lower case, a dash for the rest, never one
/// of `taken` (`-2`, `-3` and so on after it), always one plain argv word.
pub fn machine_id(name: &str, taken: &[String]) -> String {
    let mut base = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            base.push(c.to_ascii_lowercase());
        } else if !base.ends_with('-') && !base.is_empty() {
            base.push('-');
        }
    }
    let base = base.trim_end_matches('-');
    let base: String = if base.is_empty() { "machine" } else { base }
        .chars()
        .take(40)
        .collect();
    if !taken.contains(&base) {
        return base;
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|id| !taken.contains(id))
        .unwrap_or(base)
}

/// Pure: the one red line a failed link check shows under the add form, or the grey line under a machine's row (owner
/// picks, remote sessions PR 7 and 2026-11-13). A protocol refusal says which side to update; the version line names both protocols, the
/// host's first, whichever end noticed (rule 278). `here` is this Flare's remote protocol.
pub fn failure_line(host: &str, e: &crate::remote_link::LinkError, here: u32) -> String {
    use crate::remote_link::LinkError;
    match e {
        LinkError::Refused(err) if err.code == flare_ctl::code::UNSUPPORTED_PROTOCOL => {
            let nums: Vec<u64> = err
                .message
                .split(|c: char| !c.is_ascii_digit())
                .filter_map(|s| s.parse().ok())
                .collect();
            let there = nums.iter().copied().find(|n| *n != u64::from(here));
            match there {
                Some(t) => format!(
                    "Flare there speaks remote protocol {t}, here {here}. Update {}.",
                    if t < u64::from(here) {
                        "it there"
                    } else {
                        "Flare here"
                    }
                ),
                None => err.message.clone(),
            }
        }
        LinkError::Refused(err) if err.code == flare_ctl::code::HOST_NOT_READY => {
            err.message.clone()
        }
        LinkError::Refused(err) => format!("{host} said: {}", err.message),
        // What ssh said, in plain words when it is one of the cases (rule 1236).
        LinkError::Down(m) => match crate::ssh_problem::sort_down(m) {
            Some(p) => p.line().to_string(),
            None => format!("No Flare answered over SSH on {host}."),
        },
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
            ..Default::default()
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
            ..Default::default()
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

    #[test]
    fn a_new_machines_id_is_a_plain_word_from_its_name_and_never_one_taken() {
        assert_eq!(machine_id("Mac Mini", &[]), "mac-mini");
        assert_eq!(machine_id("  Max's PC!! ", &[]), "max-s-pc");
        assert_eq!(machine_id("---", &[]), "machine");
        assert_eq!(machine_id("Ærlig", &[]), "rlig");
        let taken = vec!["mac-mini".to_string(), "mac-mini-2".to_string()];
        assert_eq!(machine_id("Mac mini", &taken), "mac-mini-3");
        for n in ["Mac Mini", "-x", "a b c", "ÆØÅ"] {
            assert!(argv_word(&machine_id(n, &[])), "{n}");
        }
    }

    /// The owner's one red line: a version refusal says which side to update, whichever end noticed (rule 278).
    #[test]
    fn a_failed_check_is_one_line_that_says_what_to_do() {
        use crate::remote_link::LinkError;
        use flare_ctl::CtlError;
        let refused = |code: &str, msg: &str| LinkError::Refused(CtlError::new(code, msg));
        // The viewer noticed: "remote protocol 2 here, 1 there".
        let older = refused("unsupported_protocol", "remote protocol 2 here, 1 there");
        assert_eq!(
            failure_line("studio", &older, 2),
            "Flare there speaks remote protocol 1, here 2. Update it there."
        );
        // The host noticed, in its own words: "remote protocol 3 here, 2 there".
        let newer = refused("unsupported_protocol", "remote protocol 3 here, 2 there");
        assert_eq!(
            failure_line("studio", &newer, 2),
            "Flare there speaks remote protocol 3, here 2. Update Flare here."
        );
        let not_ready = refused("host_not_ready", "open Flare there once");
        assert_eq!(failure_line("desk", &not_ready, 1), "open Flare there once");
        assert_eq!(
            failure_line("desk", &LinkError::Down("the link ended".into()), 1),
            "No Flare answered over SSH on desk."
        );
        assert_eq!(
            failure_line("desk", &refused("unavailable", "no"), 1),
            "desk said: no"
        );
        // Rule 1236: what ssh said is the line when it is one of the cases.
        let said = |s: &str| LinkError::Down(format!("the link ended; ssh said: {s}"));
        assert_eq!(
            failure_line(
                "desk",
                &said("ssh: connect to host desk port 22: Connection refused"),
                1
            ),
            "Not answering"
        );
        assert_eq!(
            failure_line("desk", &said("max@desk: Permission denied (publickey)."), 1),
            "Not paired with this computer"
        );
        assert_eq!(
            failure_line("desk", &said("Host key verification failed."), 1),
            "Its identity changed. Pair again"
        );
        assert_eq!(
            failure_line(
                "desk",
                &said("ssh: Could not resolve hostname desk: No such host is known."),
                1
            ),
            "Can't find it on the network"
        );
        assert_eq!(
            failure_line("desk", &said("some other words"), 1),
            "No Flare answered over SSH on desk."
        );
        assert_eq!(os_of_hello("mac"), Some(Os::Mac));
        assert_eq!(os_of_hello("other"), None);
    }
}

//! Settings > Machines (remote sessions PR 7): what the page shows and what its buttons do.
//!
//! The page is `flare_settings::page::machines`; this is its door to the disk and the links. Every read and write and
//! every link check runs on a worker (rule 34) and answers with a [`MachinesMsg`] the Settings controller hands back
//! here. The machines are `machines.json` (`crate::machines`, saved whole at once, rule 36); the form's hosts are the
//! plain names of `~/.ssh/config` (`crate::ssh_config`); a check is `remote.hello` over the machine's own ssh command
//! (`remote_link::hello_over`), or over the e2e's loopback, never a real ssh in a test.
//!
//! Adding a machine checks its link first and saves only a machine that answered: Flare is looked for on the host's
//! PATH, then where a PC's installs and the Mac app keep it (`machines::FLARE_CANDIDATES`, each quoted as its path
//! implies, `machines::probe_kinds`), and the host's own `remote.hello` says its system (the logo, and how its login
//! shell is quoted). Every save sends the new list to the window
//! (`UiMsg::Machines`), so the + menu and the tabs' logos follow at once.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use flare_settings::page::machines::{AddForm, LinkStatus, MachineOs, MachineRow, MachinesInfo};
use flare_tabs::tabs::Os;

use crate::appctl::{Post, UiMsg};
use crate::machines::{
    FLARE_CANDIDATES, FileMachines, Machine, MachinesPort, failure_line, machine_id, os_of_hello,
    probe_kinds,
};
use crate::remote_link::LinkError;
use crate::settings_ctl::SettingsMsg;

/// What a host said to a check: its Flare version and its system.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HelloFacts {
    pub version: String,
    pub os: Option<Os>,
}

/// The page's disk and links. BLOCKING, every call: a worker's.
pub trait MachinesIo: Send + Sync {
    fn list(&self) -> Vec<Machine>;
    fn save(&self, list: &[Machine]) -> Result<(), String>;
    /// The plain hosts of `~/.ssh/config`.
    fn ssh_hosts(&self) -> Vec<String>;
    /// `remote.hello` over the machine's link.
    fn probe(&self, m: &Machine) -> Result<HelloFacts, LinkError>;
}

/// The real disk and links: the profile's `machines.json`, the home folder's ssh config, ssh (or under `--e2e` the
/// scenario's loopback command, `FLARE_E2E_REMOTE_COMMAND`).
pub struct RealMachinesIo {
    file: FileMachines,
    home: PathBuf,
    loopback: Option<Vec<String>>,
}

impl RealMachinesIo {
    pub fn new(profile: &std::path::Path, home: PathBuf, loopback: Option<Vec<String>>) -> Self {
        Self {
            file: FileMachines::new(profile),
            home,
            loopback,
        }
    }
}

impl MachinesIo for RealMachinesIo {
    fn list(&self) -> Vec<Machine> {
        self.file.list()
    }

    fn save(&self, list: &[Machine]) -> Result<(), String> {
        self.file
            .save(list)
            .map_err(|e| format!("machines.json could not be saved: {e}"))
    }

    fn ssh_hosts(&self) -> Vec<String> {
        crate::ssh_config::hosts_of_home(&self.home)
    }

    fn probe(&self, m: &Machine) -> Result<HelloFacts, LinkError> {
        let argv = match &self.loopback {
            Some(l) => l.clone(),
            None => crate::remote_link::ssh_argv(&crate::remote_link::ssh_exe(), m).ok_or_else(
                || LinkError::Down(format!("cannot build the ssh command for {}", m.ssh)),
            )?,
        };
        crate::remote_link::hello_over(&argv).map(|info| HelloFacts {
            version: info.flare_version,
            os: os_of_hello(&info.os),
        })
    }
}

/// The scenes' machines (`--scene settings --settings-page machines`): a Mac that answers and a PC that does not, and
/// two ssh hosts. Nothing is read or saved, and no link is started.
pub struct SceneMachinesIo;

impl MachinesIo for SceneMachinesIo {
    fn list(&self) -> Vec<Machine> {
        vec![
            Machine {
                id: "mac-mini".into(),
                name: "Mac Mini".into(),
                kind: Some(Os::Mac),
                ssh: "mac".into(),
                user: None,
                flare: "flare".into(),
            },
            Machine {
                id: "desk".into(),
                name: "Desk".into(),
                kind: Some(Os::Pc),
                ssh: "desk".into(),
                user: None,
                flare: "flare".into(),
            },
        ]
    }
    fn save(&self, _list: &[Machine]) -> Result<(), String> {
        Ok(())
    }
    fn ssh_hosts(&self) -> Vec<String> {
        vec!["mac".into(), "desk".into(), "studio".into()]
    }
    fn probe(&self, m: &Machine) -> Result<HelloFacts, LinkError> {
        if m.ssh == "mac" {
            Ok(HelloFacts {
                version: crate::app_facts::version().to_string(),
                os: Some(Os::Mac),
            })
        } else {
            Err(LinkError::Down("the scene starts no link".into()))
        }
    }
}

/// A worker's answer for the page.
#[derive(Debug)]
pub enum MachinesMsg {
    /// The machines and the ssh hosts, read.
    Loaded {
        machines: Vec<Machine>,
        hosts: Vec<String>,
    },
    /// One machine's check: its Flare version, or the line that says why not.
    Checked {
        id: String,
        result: Result<String, String>,
    },
    /// The add form's check and save: the new list, or the form's red line.
    Added(Result<(Vec<Machine>, String), String>),
    /// A machine was removed: the new list.
    Removed(Vec<Machine>),
}

/// The page's state on the UI thread.
pub struct MachinesPage {
    io: Arc<dyn MachinesIo>,
    post: Post,
    machines: Vec<Machine>,
    /// Each machine's last check in this run: its version and status.
    checks: HashMap<String, (String, LinkStatus)>,
    hosts: Vec<String>,
    add: AddForm,
    /// A read is on its way (the page shows what it had until then).
    loading: bool,
}

/// Runs `f` on a named worker; a failure to start is logged.
fn worker(name: &str, f: impl FnOnce() + Send + 'static) {
    if let Err(e) = std::thread::Builder::new().name(name.into()).spawn(f) {
        log::warn!("could not start {name}: {e}");
    }
}

fn os_of(kind: Option<Os>) -> Option<MachineOs> {
    kind.map(|k| match k {
        Os::Mac => MachineOs::Mac,
        Os::Pc => MachineOs::Pc,
    })
}

impl MachinesPage {
    pub fn new(io: Arc<dyn MachinesIo>, post: Post) -> Self {
        Self {
            io,
            post,
            machines: Vec::new(),
            checks: HashMap::new(),
            hosts: Vec::new(),
            add: AddForm::default(),
            loading: false,
        }
    }

    fn say(post: &Post, m: MachinesMsg) {
        post(UiMsg::Settings(SettingsMsg::Machines(m)));
    }

    /// What the page shows now.
    pub fn info(&self) -> MachinesInfo {
        MachinesInfo {
            machines: self
                .machines
                .iter()
                .map(|m| {
                    let (version, status) = self.checks.get(&m.id).cloned().unwrap_or_default();
                    MachineRow {
                        id: m.id.clone(),
                        name: m.name.clone(),
                        os: os_of(m.kind),
                        ssh: m.ssh.clone(),
                        version,
                        status,
                    }
                })
                .collect(),
            ssh_hosts: self.hosts.clone(),
            add: self.add.clone(),
        }
    }

    /// The page came to the front: the machines and the hosts are read again on a worker; the machines not checked in
    /// this run are checked once it answers.
    pub fn page_shown(&mut self) {
        if self.loading {
            return;
        }
        self.loading = true;
        let (io, post) = (Arc::clone(&self.io), Arc::clone(&self.post));
        worker("flare-machines-read", move || {
            let machines = io.list();
            let hosts = io.ssh_hosts();
            Self::say(&post, MachinesMsg::Loaded { machines, hosts });
        });
    }

    /// Checks these machines one after the other on one worker.
    fn check(&mut self, ids: Vec<String>) {
        let list: Vec<Machine> = self
            .machines
            .iter()
            .filter(|m| ids.contains(&m.id))
            .cloned()
            .collect();
        if list.is_empty() {
            return;
        }
        for m in &list {
            let version = self
                .checks
                .get(&m.id)
                .map(|(v, _)| v.clone())
                .unwrap_or_default();
            self.checks
                .insert(m.id.clone(), (version, LinkStatus::Checking));
        }
        let (io, post) = (Arc::clone(&self.io), Arc::clone(&self.post));
        let here = flare_ctl::REMOTE_PROTOCOL;
        worker("flare-machines-check", move || {
            for m in list {
                let result = io
                    .probe(&m)
                    .map(|f| f.version)
                    .map_err(|e| failure_line(&m.ssh, &e, here));
                Self::say(&post, MachinesMsg::Checked { id: m.id, result });
            }
        });
    }

    /// The ⋯ menu's Test.
    pub fn test(&mut self, id: &str) {
        self.check(vec![id.to_string()]);
    }

    /// The ⋯ menu's Remove: the list without it, saved on a worker.
    pub fn remove(&mut self, id: &str) {
        let (io, post) = (Arc::clone(&self.io), Arc::clone(&self.post));
        let id = id.to_string();
        worker("flare-machines-save", move || {
            let list: Vec<Machine> = io.list().into_iter().filter(|m| m.id != id).collect();
            match io.save(&list) {
                Ok(()) => {
                    post(UiMsg::Machines(list.clone()));
                    Self::say(&post, MachinesMsg::Removed(list));
                }
                Err(e) => log::warn!("{e}"),
            }
        });
    }

    /// "+ Add a machine": the form opens in the row's place, the first host picked.
    pub fn open_form(&mut self) {
        self.add = AddForm {
            open: true,
            host: self.hosts.first().cloned().unwrap_or_default(),
            ..AddForm::default()
        };
    }

    pub fn cancel_form(&mut self) {
        self.add = AddForm::default();
    }

    pub fn set_host(&mut self, host: &str) {
        self.add.host = host.to_string();
        self.add.error = None;
    }

    /// What is typed in the Name field (no rebuild: the field keeps its caret).
    pub fn set_name(&mut self, name: &str) {
        self.add.name = name.to_string();
    }

    /// Add: the host is checked (Flare looked for where it may be) and saved once it answered.
    pub fn submit(&mut self) {
        if self.add.busy {
            return;
        }
        let host = self.add.host.trim().to_string();
        if !crate::machines::argv_word(&host) {
            self.add.error = Some("Pick an SSH host.".into());
            return;
        }
        if let Some(m) = self.machines.iter().find(|m| m.ssh == host) {
            self.add.error = Some(format!("{host} is linked already, as {}.", m.name));
            return;
        }
        let name = match self.add.name.trim() {
            "" => host.clone(),
            n => n.chars().take(60).collect(),
        };
        self.add.busy = true;
        self.add.error = None;
        let (io, post) = (Arc::clone(&self.io), Arc::clone(&self.post));
        worker("flare-machines-add", move || {
            let answer = Self::add_on_worker(&*io, &host, &name);
            if let Ok((list, _)) = &answer {
                post(UiMsg::Machines(list.clone()));
            }
            Self::say(&post, MachinesMsg::Added(answer));
        });
    }

    /// BLOCKING: finds Flare on `host`, then saves the machine; the new list and its id, or the form's line.
    fn add_on_worker(
        io: &dyn MachinesIo,
        host: &str,
        name: &str,
    ) -> Result<(Vec<Machine>, String), String> {
        let here = flare_ctl::REMOTE_PROTOCOL;
        let mut last = None;
        // Each path quoted the way its own shape says (rule 1207): the host's system is not known until it answers.
        let tries = FLARE_CANDIDATES
            .iter()
            .flat_map(|flare| probe_kinds(flare).into_iter().map(move |k| (*flare, k)));
        for (flare, kind) in tries {
            let probe = Machine {
                id: "new".into(),
                name: name.to_string(),
                kind,
                ssh: host.to_string(),
                user: None,
                flare: flare.to_string(),
            };
            match io.probe(&probe) {
                Ok(facts) => {
                    let mut list = io.list();
                    let taken: Vec<String> = list.iter().map(|m| m.id.clone()).collect();
                    let id = machine_id(name, &taken);
                    list.push(Machine {
                        id: id.clone(),
                        // The host's own word; a host that names no known system keeps the quoting that answered.
                        kind: facts.os.or(kind),
                        ..probe
                    });
                    io.save(&list)?;
                    return Ok((list, facts.version));
                }
                // A host that answered and refused is the answer: no other path is tried.
                Err(e @ LinkError::Refused(_)) => return Err(failure_line(host, &e, here)),
                Err(e) => last = Some(e),
            }
        }
        Err(failure_line(
            host,
            &last.unwrap_or_else(|| LinkError::Down(String::new())),
            here,
        ))
    }

    /// A worker's answer. True when the page should be drawn again.
    pub fn on_msg(&mut self, m: MachinesMsg) -> bool {
        match m {
            MachinesMsg::Loaded { machines, hosts } => {
                self.loading = false;
                self.machines = machines;
                self.hosts = hosts;
                // A form opened before the hosts were read picks the first one now.
                if self.add.open && self.add.host.is_empty() {
                    self.add.host = self.hosts.first().cloned().unwrap_or_default();
                }
                self.checks
                    .retain(|id, _| self.machines.iter().any(|m| &m.id == id));
                let unchecked: Vec<String> = self
                    .machines
                    .iter()
                    .filter(|m| !self.checks.contains_key(&m.id))
                    .map(|m| m.id.clone())
                    .collect();
                self.check(unchecked);
            }
            MachinesMsg::Checked { id, result } => {
                let entry = match result {
                    Ok(v) => (v, LinkStatus::Linked),
                    Err(line) => (String::new(), LinkStatus::Failed(line)),
                };
                self.checks.insert(id, entry);
            }
            MachinesMsg::Added(Ok((list, version))) => {
                let new: Vec<String> = list
                    .iter()
                    .filter(|m| !self.machines.iter().any(|o| o.id == m.id))
                    .map(|m| m.id.clone())
                    .collect();
                for id in new {
                    self.checks
                        .insert(id, (version.clone(), LinkStatus::Linked));
                }
                self.machines = list;
                self.add = AddForm::default();
            }
            MachinesMsg::Added(Err(line)) => {
                self.add.busy = false;
                self.add.error = Some(line);
            }
            MachinesMsg::Removed(list) => {
                self.checks.retain(|id, _| list.iter().any(|m| &m.id == id));
                self.machines = list;
            }
        }
        true
    }

    /// The machines as last read or saved (the tests).
    #[cfg(test)]
    pub fn machines(&self) -> &[Machine] {
        &self.machines
    }
}

/// A disk and a set of hosts in memory, for the tests here and the ui tests.
#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use flare_ctl::CtlError;
    use std::sync::Mutex;

    /// `answers` says what each `(host, flare)` answers: the facts, or `(refused, message)`; a pair not listed is a
    /// link that never came up.
    #[derive(Default)]
    pub(crate) struct FakeIo {
        pub list: Mutex<Vec<Machine>>,
        pub hosts: Vec<String>,
        pub answers: HashMap<(String, String), Result<HelloFacts, (bool, String)>>,
        pub probes: Mutex<Vec<(String, String, Option<Os>)>>,
    }

    impl MachinesIo for FakeIo {
        fn list(&self) -> Vec<Machine> {
            self.list.lock().unwrap().clone()
        }
        fn save(&self, list: &[Machine]) -> Result<(), String> {
            *self.list.lock().unwrap() = list.to_vec();
            Ok(())
        }
        fn ssh_hosts(&self) -> Vec<String> {
            self.hosts.clone()
        }
        fn probe(&self, m: &Machine) -> Result<HelloFacts, LinkError> {
            self.probes
                .lock()
                .unwrap()
                .push((m.ssh.clone(), m.flare.clone(), m.kind));
            match self.answers.get(&(m.ssh.clone(), m.flare.clone())) {
                Some(Ok(f)) => Ok(f.clone()),
                Some(Err((true, msg))) => Err(LinkError::Refused(CtlError::new(
                    "unsupported_protocol",
                    msg.clone(),
                ))),
                _ => Err(LinkError::Down("ssh ended".into())),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeIo;
    use super::*;
    use std::sync::Mutex;

    /// The page and what its workers posted, run to the end.
    struct Rig {
        page: MachinesPage,
        io: Arc<FakeIo>,
        got: Arc<Mutex<Vec<UiMsg>>>,
    }

    impl Rig {
        fn new(io: FakeIo) -> Self {
            let io = Arc::new(io);
            let got = Arc::new(Mutex::new(Vec::new()));
            let g = Arc::clone(&got);
            let post: Post = Arc::new(move |m| g.lock().unwrap().push(m));
            Rig {
                page: MachinesPage::new(io.clone(), post),
                io,
                got,
            }
        }

        /// Waits for the workers' answers and hands them to the page; the window's lists come back.
        fn settle(&mut self) -> Vec<Vec<Machine>> {
            let mut lists = Vec::new();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                let msgs: Vec<UiMsg> = self.got.lock().unwrap().drain(..).collect();
                for m in msgs {
                    match m {
                        UiMsg::Settings(SettingsMsg::Machines(m)) => {
                            self.page.on_msg(m);
                        }
                        UiMsg::Machines(l) => lists.push(l),
                        _ => {}
                    }
                }
                let busy = self.page.loading
                    || self.page.add.busy
                    || self
                        .page
                        .checks
                        .values()
                        .any(|(_, s)| *s == LinkStatus::Checking);
                if !busy || std::time::Instant::now() > deadline {
                    return lists;
                }
                std::thread::sleep(std::time::Duration::from_millis(5)); // sync-ok: a test waits for its workers
            }
        }
    }

    fn mac() -> Machine {
        Machine {
            id: "mac-mini".into(),
            name: "Mac Mini".into(),
            kind: Some(Os::Mac),
            ssh: "mini".into(),
            user: None,
            flare: "flare".into(),
        }
    }

    fn facts(v: &str, os: Option<Os>) -> Result<HelloFacts, (bool, String)> {
        Ok(HelloFacts {
            version: v.into(),
            os,
        })
    }

    #[test]
    fn a_shown_page_reads_the_machines_and_hosts_and_checks_each_once() {
        let mut io = FakeIo {
            hosts: vec!["mini".into(), "studio".into()],
            ..FakeIo::default()
        };
        *io.list.get_mut().unwrap() = vec![mac()];
        io.answers.insert(
            ("mini".into(), "flare".into()),
            facts("0.2.0", Some(Os::Mac)),
        );
        let mut r = Rig::new(io);
        r.page.page_shown();
        r.settle();
        let info = r.page.info();
        assert_eq!(info.ssh_hosts, ["mini", "studio"]);
        assert_eq!(info.machines.len(), 1);
        assert_eq!(info.machines[0].status, LinkStatus::Linked);
        assert_eq!(info.machines[0].version, "0.2.0");
        assert_eq!(info.machines[0].os, Some(MachineOs::Mac));
        // Shown again: read again, but a machine checked in this run is not checked twice.
        r.page.page_shown();
        r.settle();
        assert_eq!(r.io.probes.lock().unwrap().len(), 1);
        // Test checks it again.
        r.page.test("mac-mini");
        r.settle();
        assert_eq!(r.io.probes.lock().unwrap().len(), 2);
    }

    #[test]
    fn adding_looks_for_flare_saves_only_what_answered_with_the_hosts_system() {
        let mut io = FakeIo {
            hosts: vec!["studio".into()],
            ..FakeIo::default()
        };
        // Not on the PATH; the Mac app answers.
        io.answers.insert(
            ("studio".into(), FLARE_CANDIDATES[3].into()),
            facts("0.2.0", Some(Os::Mac)),
        );
        let mut r = Rig::new(io);
        r.page.page_shown();
        r.settle();
        r.page.open_form();
        assert_eq!(r.page.info().add.host, "studio");
        r.page.set_name("Studio Mac");
        r.page.submit();
        assert!(r.page.info().add.busy);
        let lists = r.settle();
        let saved = r.io.list();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].id, "studio-mac");
        assert_eq!(saved[0].name, "Studio Mac");
        assert_eq!(saved[0].ssh, "studio");
        assert_eq!(saved[0].kind, Some(Os::Mac));
        assert_eq!(saved[0].flare, FLARE_CANDIDATES[3]);
        // The window heard the new list (the + menu and the logos follow).
        assert_eq!(lists, vec![saved.clone()]);
        let info = r.page.info();
        assert!(!info.add.open, "the form closes");
        assert_eq!(info.machines[0].status, LinkStatus::Linked);
        // Every path was tried in order, each quoted as its shape says (rule 1207): the bare name in sh's form, then
        // the PC's; the PC installs in the PC's; the Mac app in sh's.
        let probes = r.io.probes.lock().unwrap().clone();
        let want: Vec<(String, String, Option<Os>)> = [
            ("flare", None),
            ("flare", Some(Os::Pc)),
            (FLARE_CANDIDATES[1], Some(Os::Pc)),
            (FLARE_CANDIDATES[2], Some(Os::Pc)),
            (FLARE_CANDIDATES[3], None),
        ]
        .iter()
        .map(|(f, k)| ("studio".to_string(), f.to_string(), *k))
        .collect();
        assert_eq!(probes, want);
    }

    /// Rule 1207: a PC whose Flare is not on the PATH is found at its Stable install, through cmd, and saved as a PC
    /// with the path that answered (cmd expands `%LOCALAPPDATA%` there on every link).
    #[test]
    fn adding_a_pc_finds_its_stable_install_and_saves_it_as_a_pc() {
        let mut io = FakeIo {
            hosts: vec!["desk".into()],
            ..FakeIo::default()
        };
        io.answers.insert(
            ("desk".into(), FLARE_CANDIDATES[1].into()),
            facts("0.3.0", Some(Os::Pc)),
        );
        let mut r = Rig::new(io);
        r.page.page_shown();
        r.settle();
        r.page.open_form();
        r.page.submit();
        r.settle();
        let saved = r.io.list();
        assert_eq!(saved.len(), 1);
        assert_eq!(
            saved[0].flare,
            r"%LOCALAPPDATA%\StableFlare\current\FlareStable.exe"
        );
        assert_eq!(saved[0].kind, Some(Os::Pc));
        assert_eq!(
            crate::remote_link::remote_command(saved[0].kind, &saved[0].flare).as_deref(),
            Some(r#"cmd /c "%LOCALAPPDATA%\StableFlare\current\FlareStable.exe" --serve-remote"#)
        );
        // The Dev install and the Mac app were never tried once Stable answered.
        let probes = r.io.probes.lock().unwrap().clone();
        assert_eq!(probes.len(), 3);
        assert_eq!(
            probes[2],
            ("desk".into(), FLARE_CANDIDATES[1].into(), Some(Os::Pc))
        );
    }

    #[test]
    fn each_candidate_is_probed_with_the_quoting_its_path_implies() {
        assert_eq!(
            FLARE_CANDIDATES,
            [
                "flare",
                r"%LOCALAPPDATA%\StableFlare\current\FlareStable.exe",
                r"%LOCALAPPDATA%\Flare\current\Flare.exe",
                "/Applications/Flare.app/Contents/MacOS/flare",
            ]
        );
        assert_eq!(probe_kinds("flare"), [None, Some(Os::Pc)]);
        assert_eq!(probe_kinds(FLARE_CANDIDATES[1]), [Some(Os::Pc)]);
        assert_eq!(probe_kinds(r"D:\Tools\flare.exe"), [Some(Os::Pc)]);
        assert_eq!(probe_kinds("/opt/flare/bin/flare"), [None]);
        // Every candidate has a command in each quoting it is probed with.
        for flare in FLARE_CANDIDATES {
            for kind in probe_kinds(flare) {
                assert!(
                    crate::remote_link::remote_command(kind, flare).is_some(),
                    "{flare} {kind:?}"
                );
            }
        }
    }

    #[test]
    fn a_form_opened_before_the_hosts_are_read_picks_the_first_when_they_come() {
        let io = FakeIo {
            hosts: vec!["studio".into(), "nas".into()],
            ..FakeIo::default()
        };
        let mut r = Rig::new(io);
        r.page.page_shown();
        r.page.open_form();
        assert_eq!(r.page.info().add.host, "", "nothing read yet");
        r.settle();
        assert_eq!(r.page.info().add.host, "studio");
        // A host already picked is kept when the hosts are read again.
        r.page.set_host("nas");
        r.page.page_shown();
        r.settle();
        assert_eq!(r.page.info().add.host, "nas");
    }

    #[test]
    fn a_failed_add_saves_nothing_and_shows_one_line_under_the_form() {
        let mut io = FakeIo {
            hosts: vec!["studio".into(), "nas".into()],
            ..FakeIo::default()
        };
        io.answers.insert(
            ("studio".into(), "flare".into()),
            Err((true, "remote protocol 1 here, 0 there".into())),
        );
        let mut r = Rig::new(io);
        r.page.page_shown();
        r.settle();
        r.page.open_form();
        r.page.submit();
        let lists = r.settle();
        assert!(lists.is_empty() && r.io.list().is_empty());
        let add = r.page.info().add;
        assert!(add.open && !add.busy);
        assert_eq!(
            add.error.as_deref(),
            Some("Flare there speaks remote protocol 0, here 1. Update it there.")
        );
        // A refusal is the answer: no other path or quoting was tried.
        assert_eq!(r.io.probes.lock().unwrap().len(), 1);
        // No Flare anywhere on nas: every path tried, one line.
        r.page.set_host("nas");
        assert_eq!(r.page.info().add.error, None, "a new pick clears the line");
        r.page.submit();
        r.settle();
        assert_eq!(
            r.page.info().add.error.as_deref(),
            Some("No Flare answered over SSH on nas.")
        );
        // No host at all, and a host linked already, never reach a worker.
        r.page.set_host("");
        r.page.submit();
        assert_eq!(
            r.page.info().add.error.as_deref(),
            Some("Pick an SSH host.")
        );
        r.page.cancel_form();
        assert_eq!(r.page.info().add, AddForm::default());
    }

    #[test]
    fn remove_saves_the_list_without_it_and_tells_the_window() {
        let io = FakeIo::default();
        *io.list.lock().unwrap() = vec![mac()];
        let mut r = Rig::new(io);
        r.page.page_shown();
        r.settle();
        r.page.remove("mac-mini");
        // The remove's worker answers; the page reads the list it saved.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut lists = Vec::new();
        while (lists.is_empty() || !r.page.machines().is_empty())
            && std::time::Instant::now() < deadline
        {
            lists.extend(r.settle());
            std::thread::sleep(std::time::Duration::from_millis(5)); // sync-ok: a test waits for its worker
        }
        assert_eq!(lists, vec![Vec::<Machine>::new()]);
        assert!(r.io.list().is_empty());
        assert!(r.page.machines().is_empty());
        assert!(r.page.info().machines.is_empty());
    }
}

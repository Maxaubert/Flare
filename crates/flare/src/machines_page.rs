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

use flare_settings::kv::PrefStore;
use flare_settings::page::machines::{
    AddForm, INCOMING_PREFIX, LinkStatus, MachineOs, MachineRow, MachinesInfo, NEARBY_PREFIX,
    NearbyChoice,
};
use flare_tabs::tabs::Os;

use crate::pair_keys::PairFiles;
use crate::pairing::{HostEvent, Me, NoPairNet, PairMsg, PairNet, Pairing, ViewerEvent};
use crate::sharing::{NoSharingOs, Sharing, SharingOs};

use crate::appctl::{Post, UiMsg};
use crate::machines::{
    FLARE_CANDIDATES, FileMachines, Incoming, Machine, MachinesPort, failure_line, incoming_file,
    incoming_json, machine_id, os_of_hello, parse_incoming, probe_kinds, saved_kind,
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
    /// The computers paired to reach this one (machine pairing PR 3, rule 1246).
    fn list_in(&self) -> Vec<Incoming> {
        Vec::new()
    }
    fn save_in(&self, _list: &[Incoming]) -> Result<(), String> {
        Ok(())
    }
}

/// The real disk and links: the profile's `machines.json` and `machines_in.json`, the home folder's ssh config, ssh
/// (or under `--e2e` the scenario's loopback command, `FLARE_E2E_REMOTE_COMMAND`).
pub struct RealMachinesIo {
    file: FileMachines,
    in_file: PathBuf,
    home: PathBuf,
    loopback: Option<Vec<String>>,
}

impl RealMachinesIo {
    pub fn new(profile: &std::path::Path, home: PathBuf, loopback: Option<Vec<String>>) -> Self {
        Self {
            file: FileMachines::new(profile),
            in_file: incoming_file(profile),
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

    fn list_in(&self) -> Vec<Incoming> {
        std::fs::read_to_string(&self.in_file) // sync-ok: a worker's call
            .map(|raw| parse_incoming(&raw))
            .unwrap_or_default()
    }

    fn save_in(&self, list: &[Incoming]) -> Result<(), String> {
        flare_settings::atomic::write_atomic(&self.in_file, &incoming_json(list))
            .map_err(|e| format!("machines_in.json could not be saved: {e}"))
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
                ..Default::default()
            },
            Machine {
                id: "desk".into(),
                name: "Desk".into(),
                kind: Some(Os::Pc),
                ssh: "desk".into(),
                user: None,
                flare: "flare".into(),
                ..Default::default()
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
    /// The machines, the computers that reach this one, and the ssh hosts, read.
    Loaded {
        machines: Vec<Machine>,
        incoming: Vec<Incoming>,
        hosts: Vec<String>,
    },
    /// A computer that reaches this one was removed (its key with it): the new list, or the line that says what stays.
    RemovedIn(Result<Vec<Incoming>, String>),
    /// Pairing (machine pairing PR 3).
    Pair(PairMsg),
    /// One machine's check: its Flare version, or the line that says why not.
    Checked {
        id: String,
        result: Result<String, String>,
    },
    /// The add form's check and save: the new list, or the form's red line.
    Added(Result<(Vec<Machine>, String), String>),
    /// A machine was removed: the new list.
    Removed(Vec<Machine>),
    /// The Computer sharing switch's worker (machine pairing PR 2).
    Sharing(crate::sharing::SharingMsg),
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
    /// This computer's Computer sharing switch (machine pairing PR 2, rule 1238).
    sharing: Sharing,
    /// The computers paired to reach this one (machine pairing PR 3, rule 1246).
    incoming: Vec<Incoming>,
    /// Pairing: the nearby computers, the viewer's request, the host's listener and code dialog (PR 3).
    pairing: Pairing,
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

/// Pure: the machine the dialog names, before Flare is looked for on it (rule 1237): an SSH host as it is; a typed
/// address as its own host, with the User field's name (or the `user@` before the address) and a `:port` after it. Its
/// name is the host or the address. The one red line when it names none.
pub fn new_machine(form: &AddForm) -> Result<Machine, String> {
    use crate::machines::argv_word;
    use flare_settings::page::machines::TYPE_ADDRESS;
    let pick = form.pick.as_str();
    if pick == TYPE_ADDRESS {
        let mut address = form.address.trim();
        let mut user = form.user.trim().to_string();
        // A `user@` before the address is its user, unless the User field names one (the field wins); it never stays
        // in the address, where ssh would read it as the user again and "linked already" would miss it.
        if let Some((u, rest)) = address.split_once('@') {
            if user.is_empty() {
                user = u.to_string();
            }
            address = rest;
        }
        if address.contains('@') {
            return Err("Type the address with one @ at most.".into());
        }
        // One colon and digits after it is a port; an IPv6 address (several colons) is left whole, and one in brackets
        // may have a port after them (`[fe80::1]:2222`).
        let mut port = None;
        let parse_port = |p: &str| {
            p.parse::<u16>()
                .ok()
                .filter(|p| *p > 0)
                .ok_or_else(|| "Type a port from 1 to 65535 after the colon.".to_string())
        };
        if let Some(inner) = address.strip_prefix('[') {
            let Some((a, rest)) = inner.split_once(']') else {
                return Err("Type the address with no spaces.".into());
            };
            if let Some(p) = rest.strip_prefix(':') {
                port = Some(parse_port(p)?);
            } else if !rest.is_empty() {
                return Err("Type the address with no spaces.".into());
            }
            address = a;
        } else if address.matches(':').count() == 1
            && let Some((a, p)) = address.split_once(':')
        {
            port = Some(parse_port(p)?);
            address = a;
        }
        if address.is_empty() {
            return Err("Type an address.".into());
        }
        if !argv_word(address) {
            return Err("Type the address with no spaces.".into());
        }
        if !user.is_empty() && !argv_word(&user) {
            return Err("Type the user name with no spaces.".into());
        }
        return Ok(Machine {
            name: address.to_string(),
            ssh: address.to_string(),
            host: Some(address.to_string()),
            user: (!user.is_empty()).then_some(user),
            port,
            ..Machine::default()
        });
    }
    // A nearby computer (machine pairing PR 3) is paired with its code, not added here; its id is no plain word.
    if !argv_word(pick) {
        return Err("Pick a machine.".into());
    }
    Ok(Machine {
        name: pick.to_string(),
        ssh: pick.to_string(),
        ..Machine::default()
    })
}

impl MachinesPage {
    /// The page with a sharing switch that sets nothing up (`NoSharingOs`) and keeps its state in memory, until the
    /// window hands over the OS calls ([`Self::with_sharing_os`]) and Settings its store ([`Self::use_prefs`]).
    pub fn new(io: Arc<dyn MachinesIo>, post: Post) -> Self {
        let sharing = Sharing::new(
            Arc::new(NoSharingOs),
            PrefStore::memory(),
            Arc::clone(&post),
        );
        let pairing = Pairing::new(
            Arc::new(NoPairNet),
            Arc::new(NoSharingOs),
            None,
            Me::this_computer(),
            Arc::clone(&post),
        );
        Self {
            io,
            post,
            machines: Vec::new(),
            checks: HashMap::new(),
            hosts: Vec::new(),
            add: AddForm::default(),
            loading: false,
            sharing,
            incoming: Vec::new(),
            pairing,
        }
    }

    /// The OS calls the sharing switch makes (the real ones in the app, a fake in the tests).
    pub fn with_sharing_os(mut self, os: Arc<dyn SharingOs>) -> Self {
        self.pairing.set_os(Arc::clone(&os));
        self.sharing = Sharing::new(os, PrefStore::memory(), Arc::clone(&self.post));
        self.sync_host();
        self
    }

    /// Pairing (machine pairing PR 3): the network (`RealPairNet` in the app, a fake in the tests), Flare's key and
    /// known hosts in `files`, and this computer as it says itself.
    pub fn with_pairing(mut self, net: Arc<dyn PairNet>, files: PairFiles, me: Me) -> Self {
        let os = self.sharing.os();
        self.pairing.host(false, &self.io);
        self.pairing = Pairing::new(net, os, Some(files), me, Arc::clone(&self.post));
        self.sync_host();
        self
    }

    /// The settings store: the switch reads where it was left (on after a restart) and saves there from now on.
    pub fn use_prefs(&mut self, prefs: PrefStore) {
        self.sharing = Sharing::new(self.sharing.os(), prefs, Arc::clone(&self.post));
        self.sync_host();
    }

    /// The Computer sharing switch.
    pub fn sharing(&self) -> &Sharing {
        &self.sharing
    }

    /// Pairing's state (the host's code dialog).
    pub fn pairing(&self) -> &Pairing {
        &self.pairing
    }

    /// The host listens and is announced exactly while Computer sharing is on (rule 1242).
    fn sync_host(&mut self) {
        let on = self.sharing.is_on();
        self.pairing.host(on, &self.io);
    }

    /// The Computer sharing switch was flipped. Turning it off forgets the computers that reached this one: their keys
    /// go with the clean up (rule 1241).
    pub fn sharing_switch(&mut self, on: bool) {
        let was = self.sharing.is_on();
        self.sharing.switch(on);
        if was && !on && !self.incoming.is_empty() {
            self.incoming.clear();
            let io = Arc::clone(&self.io);
            worker("flare-machines-save", move || {
                if let Err(e) = io.save_in(&[]) {
                    log::warn!("{e}");
                }
            });
        }
        self.sync_host();
    }

    /// A button of the sharing dialog.
    pub fn sharing_button(&mut self, id: &str) {
        self.sharing.button(id);
        self.sync_host();
    }

    /// Escape, or the Settings tab closing: the sharing dialog's way out.
    pub fn sharing_dismiss(&mut self) {
        self.sharing.dismiss();
        self.sync_host();
    }

    /// Decline in the host's code dialog (or Escape there): the request ends (rule 1243).
    pub fn decline(&mut self) {
        self.pairing.decline();
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
                        address: m.target().to_string(),
                        version,
                        status,
                    }
                })
                .chain(self.incoming.iter().map(|m| MachineRow {
                    // A computer that reaches this one is linked while its key is here (rule 1246).
                    id: format!("{INCOMING_PREFIX}{}", m.id),
                    name: m.name.clone(),
                    os: os_of(m.kind),
                    address: m.address.clone(),
                    version: m.version.clone(),
                    status: LinkStatus::Linked,
                }))
                .collect(),
            ssh_hosts: self.hosts.clone(),
            nearby: self
                .pairing
                .nearby()
                .iter()
                .map(|n| NearbyChoice {
                    id: n.id.clone(),
                    name: n.name.clone(),
                    os: os_of(n.os),
                })
                .collect(),
            add: self.add.clone(),
            sharing: self.sharing.info(),
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
            let incoming = io.list_in();
            let hosts = io.ssh_hosts();
            Self::say(
                &post,
                MachinesMsg::Loaded {
                    machines,
                    incoming,
                    hosts,
                },
            );
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
                    .map_err(|e| failure_line(m.target(), &e, here));
                Self::say(&post, MachinesMsg::Checked { id: m.id, result });
            }
        });
    }

    /// The ⋯ menu's Test.
    pub fn test(&mut self, id: &str) {
        self.check(vec![id.to_string()]);
    }

    /// The ⋯ menu's Remove: the list without it, saved on a worker. A computer that reaches this one goes with its key
    /// (rule 1246).
    pub fn remove(&mut self, id: &str) {
        if let Some(id) = id.strip_prefix(INCOMING_PREFIX) {
            let Some(gone) = self.incoming.iter().find(|m| m.id == id).cloned() else {
                return;
            };
            let (io, post, os) = (
                Arc::clone(&self.io),
                Arc::clone(&self.post),
                self.sharing.os(),
            );
            let admin = self.sharing.admin_keys();
            worker("flare-machines-save", move || {
                let answer = crate::sharing::remove_key(&*os, &gone.key, admin).and_then(|()| {
                    let list: Vec<Incoming> = io
                        .list_in()
                        .into_iter()
                        .filter(|m| m.id != gone.id)
                        .collect();
                    io.save_in(&list).map(|()| list)
                });
                Self::say(&post, MachinesMsg::RemovedIn(answer));
            });
            return;
        }
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

    /// "+ Add a machine": the dialog opens with nothing picked ("Pick a machine"), Add waiting for a pick (rule 1237).
    /// The computers with Computer sharing on are looked for while it is open (rule 1242).
    pub fn open_form(&mut self) {
        self.add = AddForm {
            open: true,
            ..AddForm::default()
        };
        self.pairing.browse(true);
    }

    /// Cancel: the dialog closes, a pairing request ends (the other computer's code dialog closes with it), and no
    /// more computers are looked for. A link check that runs keeps the dialog until it answers.
    pub fn cancel_form(&mut self) {
        if !self.add.busy || self.add.pairing() {
            self.pairing.end_viewer();
            self.pairing.browse(false);
            self.add = AddForm::default();
        }
    }

    /// A pick in the Machine drop-down: a nearby computer (Flare connects and the other computer shows its code), an
    /// SSH host or "Type an address". A new pick clears the red line and ends any older pairing request.
    pub fn set_pick(&mut self, id: &str) {
        if id == self.add.pick && self.add.pairing() && self.pairing.viewer_to().is_some() {
            return;
        }
        self.add.pick = id.to_string();
        self.add.error = None;
        self.add.code.clear();
        self.add.ready = false;
        self.add.code_for.clear();
        self.pairing.end_viewer();
        if let Some(near) = id.strip_prefix(NEARBY_PREFIX) {
            match self.pairing.nearby_by_id(near) {
                Some(n) => {
                    self.add.code_for = n.name.clone();
                    self.pairing.start_viewer(near, Arc::clone(&self.io));
                }
                None => self.add.error = Some("Not answering".into()),
            }
        }
    }

    /// What is typed in the "Code shown on <name>" field (no rebuild: it keeps its caret). It clears the red line.
    pub fn set_code(&mut self, text: &str) {
        self.add.code = text.to_string();
        self.add.error = None;
    }

    /// What is typed in the Address field (no rebuild of the field: it keeps its caret). It clears the red line.
    pub fn set_address(&mut self, text: &str) {
        self.add.address = text.to_string();
        self.add.error = None;
    }

    /// What is typed in the optional User field.
    pub fn set_user(&mut self, text: &str) {
        self.add.user = text.to_string();
    }

    /// Add: the machine picked, or the address typed, is checked (Flare looked for where it may be) and saved once it
    /// answered.
    pub fn submit(&mut self) {
        if self.add.busy || !self.add.open {
            return;
        }
        // A nearby computer: the typed code goes to its request (rule 1243).
        if self.add.pairing() {
            if !self.add.can_add() {
                return;
            }
            let Some(digits) = crate::pair_wire::code_digits(&self.add.code) else {
                return;
            };
            if self.pairing.send_code(digits) {
                self.add.busy = true;
                self.add.error = None;
            }
            return;
        }
        let base = match new_machine(&self.add) {
            Ok(m) => m,
            Err(line) => {
                self.add.error = Some(line);
                return;
            }
        };
        if let Some(m) = self
            .machines
            .iter()
            .find(|m| m.target() == base.target() && m.port == base.port)
        {
            self.add.error = Some(format!(
                "{} is linked already, as {}.",
                base.target(),
                m.name
            ));
            return;
        }
        self.add.busy = true;
        self.add.error = None;
        let (io, post) = (Arc::clone(&self.io), Arc::clone(&self.post));
        worker("flare-machines-add", move || {
            let answer = Self::add_on_worker(&*io, &base);
            if let Ok((list, _)) = &answer {
                post(UiMsg::Machines(list.clone()));
            }
            Self::say(&post, MachinesMsg::Added(answer));
        });
    }

    /// BLOCKING: finds Flare on the machine `base` names (its address, port and user kept), then saves it; the new
    /// list and the version it said, or the dialog's line.
    fn add_on_worker(
        io: &dyn MachinesIo,
        base: &Machine,
    ) -> Result<(Vec<Machine>, String), String> {
        let here = flare_ctl::REMOTE_PROTOCOL;
        let host = base.target().to_string();
        let host = host.as_str();
        let name = base.name.as_str();
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
                flare: flare.to_string(),
                ..base.clone()
            };
            match io.probe(&probe) {
                Ok(facts) => {
                    let mut list = io.list();
                    let taken: Vec<String> = list.iter().map(|m| m.id.clone()).collect();
                    let id = machine_id(name, &taken);
                    list.push(Machine {
                        id: id.clone(),
                        // The host's own word when its quoting is the one that answered (rule 1208).
                        kind: saved_kind(kind, facts.os, flare),
                        ..probe
                    });
                    io.save(&list)?;
                    return Ok((list, facts.version));
                }
                // A host that answered and refused is the answer: no other path is tried.
                Err(e @ LinkError::Refused(_)) => return Err(failure_line(host, &e, here)),
                // ssh itself said why (not answering, not paired, ...): no other path can answer, so none is tried
                // and Add waits one connect timeout, not one per path (rule 1236).
                Err(LinkError::Down(m)) if crate::ssh_problem::sort_down(&m).is_some() => {
                    return Err(failure_line(host, &LinkError::Down(m), here));
                }
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
            MachinesMsg::Loaded {
                machines,
                incoming,
                hosts,
            } => {
                self.loading = false;
                self.machines = machines;
                self.incoming = incoming;
                self.hosts = hosts;
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
                self.pairing.browse(false);
            }
            MachinesMsg::Added(Err(line)) => {
                self.add.busy = false;
                self.add.error = Some(line);
            }
            MachinesMsg::Removed(list) => {
                self.checks.retain(|id, _| list.iter().any(|m| &m.id == id));
                self.machines = list;
            }
            MachinesMsg::RemovedIn(Ok(list)) => self.incoming = list,
            MachinesMsg::RemovedIn(Err(line)) => {
                self.sharing.notice(line);
            }
            MachinesMsg::Sharing(m) => {
                let redraw = self.sharing.on_msg(m);
                self.sync_host();
                return redraw;
            }
            MachinesMsg::Pair(m) => return self.on_pair(m),
        }
        true
    }

    /// Pairing's answers (rules 1242 to 1246). True when the page should be drawn again.
    fn on_pair(&mut self, m: PairMsg) -> bool {
        match m {
            PairMsg::Nearby(list) => {
                self.pairing.set_nearby(list);
                // A nearby computer that went away while picked keeps its pick (its request may still run).
            }
            PairMsg::Viewer { run, event } => {
                if self.pairing.viewer_to().map(|(r, _)| r) != Some(run) {
                    return false;
                }
                match event {
                    ViewerEvent::Shown => self.add.ready = true,
                    ViewerEvent::Wrong => {
                        self.add.busy = false;
                        self.add.error = Some("Wrong code".into());
                    }
                    ViewerEvent::Failed(line) => {
                        self.pairing.viewer_ended(run);
                        self.add.busy = false;
                        self.add.ready = false;
                        self.add.error = Some(line);
                    }
                    ViewerEvent::Linked { list, id } => {
                        self.pairing.viewer_ended(run);
                        self.pairing.browse(false);
                        (self.post)(UiMsg::Machines(list.clone()));
                        self.machines = list;
                        self.add = AddForm::default();
                        // The new machine is checked over ssh as any other (rule 1245).
                        self.checks.remove(&id);
                        self.check(vec![id]);
                    }
                }
            }
            PairMsg::Host(e) => {
                self.pairing.on_host(&e);
                if let HostEvent::Done {
                    linked: Some((list, admin)),
                    ..
                } = e
                {
                    if admin {
                        self.sharing.note_admin_keys();
                    }
                    self.incoming = list;
                }
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
        /// The computers that reach this one (machine pairing PR 3).
        pub incoming: Mutex<Vec<Incoming>>,
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
                Some(Err((false, msg))) => Err(LinkError::Down(msg.clone())),
                _ => Err(LinkError::Down("ssh ended".into())),
            }
        }
        fn list_in(&self) -> Vec<Incoming> {
            self.incoming.lock().unwrap().clone()
        }
        fn save_in(&self, list: &[Incoming]) -> Result<(), String> {
            *self.incoming.lock().unwrap() = list.to_vec();
            Ok(())
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
            ..Default::default()
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
            ("studio".into(), FLARE_CANDIDATES[5].into()),
            facts("0.2.0", Some(Os::Mac)),
        );
        let mut r = Rig::new(io);
        r.page.page_shown();
        r.settle();
        r.page.open_form();
        assert_eq!(r.page.info().add.pick, "", "nothing picked yet");
        r.page.set_pick("studio");
        r.page.submit();
        assert!(r.page.info().add.busy);
        let lists = r.settle();
        let saved = r.io.list();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].id, "studio");
        assert_eq!(saved[0].name, "studio");
        assert_eq!(saved[0].ssh, "studio");
        assert_eq!(saved[0].host, None, "an SSH host keeps its config");
        assert_eq!(saved[0].kind, Some(Os::Mac));
        assert_eq!(saved[0].flare, FLARE_CANDIDATES[5]);
        // The window heard the new list (the + menu and the logos follow).
        assert_eq!(lists, vec![saved.clone()]);
        let info = r.page.info();
        assert!(!info.add.open, "the form closes");
        assert_eq!(info.machines[0].status, LinkStatus::Linked);
        // Every path was tried in order, each quoted as its shape says (rule 1207): the bare name in sh's form, then
        // the PC's; the PC installs in the PC's (cmd's variable, then PowerShell's, rule 1208); the Mac app in sh's.
        let probes = r.io.probes.lock().unwrap().clone();
        let want: Vec<(String, String, Option<Os>)> = [
            ("flare", None),
            ("flare", Some(Os::Pc)),
            (FLARE_CANDIDATES[1], Some(Os::Pc)),
            (FLARE_CANDIDATES[2], Some(Os::Pc)),
            (FLARE_CANDIDATES[3], Some(Os::Pc)),
            (FLARE_CANDIDATES[4], Some(Os::Pc)),
            (FLARE_CANDIDATES[5], None),
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
        r.page.set_pick("desk");
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

    /// Rule 1208: a PC whose login shell is PowerShell and whose profile folder holds a space (`C:\Users\John Doe`)
    /// does not answer at cmd's `%LOCALAPPDATA%` (PowerShell drops the quotes, cmd splits the expanded path); it is
    /// found at PowerShell's `$env:LOCALAPPDATA`, saved as a PC with that path, and linked with the same command.
    #[test]
    fn a_pc_whose_profile_folder_has_a_space_is_found_through_powershells_variable() {
        let mut io = FakeIo {
            hosts: vec!["desk".into()],
            ..FakeIo::default()
        };
        io.answers.insert(
            (
                "desk".into(),
                r"$env:LOCALAPPDATA\StableFlare\current\FlareStable.exe".into(),
            ),
            facts("0.3.0", Some(Os::Pc)),
        );
        let mut r = Rig::new(io);
        r.page.page_shown();
        r.settle();
        r.page.open_form();
        r.page.set_pick("desk");
        r.page.submit();
        r.settle();
        let saved = r.io.list();
        assert_eq!(saved.len(), 1, "{:?}", r.page.info().add.error);
        assert_eq!(saved[0].kind, Some(Os::Pc));
        assert_eq!(
            crate::remote_link::remote_command(saved[0].kind, &saved[0].flare).as_deref(),
            Some(
                r#"cmd /c "$env:LOCALAPPDATA\StableFlare\current\FlareStable.exe" --serve-remote"#
            )
        );
        // cmd's variable was tried first for both installs, then PowerShell's Stable answered.
        let probes: Vec<String> =
            r.io.probes
                .lock()
                .unwrap()
                .iter()
                .map(|p| p.1.clone())
                .collect();
        assert_eq!(probes[2..], FLARE_CANDIDATES[1..4]);
    }

    /// Rule 1208: a Windows host whose ssh login shell is a POSIX shell (Git Bash) answers the bare name in sh's
    /// quoting and names itself a PC. It keeps the sh quoting that answered: the PC's `cmd /c` would reach bash, which
    /// rewrites `/c` to `C:/`, and every later link would hang.
    #[test]
    fn a_pc_that_answered_in_sh_quoting_keeps_that_quoting() {
        let mut io = FakeIo {
            hosts: vec!["bashpc".into()],
            ..FakeIo::default()
        };
        io.answers.insert(
            ("bashpc".into(), "flare".into()),
            facts("0.3.0", Some(Os::Pc)),
        );
        let mut r = Rig::new(io);
        r.page.page_shown();
        r.settle();
        r.page.open_form();
        r.page.set_pick("bashpc");
        r.page.submit();
        r.settle();
        let saved = r.io.list();
        assert_eq!(saved.len(), 1);
        assert_eq!(r.io.probes.lock().unwrap()[0].2, None, "sh's form answered");
        assert_eq!(saved[0].kind, None);
        assert_eq!(
            crate::remote_link::remote_command(saved[0].kind, &saved[0].flare).as_deref(),
            Some("'flare' --serve-remote")
        );
    }

    /// Rule 1208: the system a host names is saved only when its quoting is the one that answered; otherwise the
    /// quoting that answered is kept.
    #[test]
    fn the_saved_kind_keeps_the_quoting_that_answered() {
        let (pc, mac) = (Some(Os::Pc), Some(Os::Mac));
        // A Mac's quoting is sh's: the logo is kept.
        assert_eq!(saved_kind(None, mac, "flare"), mac);
        assert_eq!(saved_kind(pc, pc, "flare"), pc);
        // A host that names no system keeps the quoting that answered.
        assert_eq!(saved_kind(pc, None, "flare"), pc);
        assert_eq!(saved_kind(None, None, "flare"), None);
        // A PC that answered in sh's quoting (Git Bash as its login shell) keeps sh's.
        assert_eq!(saved_kind(None, pc, "flare"), None);
    }

    #[test]
    fn each_candidate_is_probed_with_the_quoting_its_path_implies() {
        assert_eq!(
            FLARE_CANDIDATES,
            [
                "flare",
                r"%LOCALAPPDATA%\StableFlare\current\FlareStable.exe",
                r"%LOCALAPPDATA%\Flare\current\Flare.exe",
                r"$env:LOCALAPPDATA\StableFlare\current\FlareStable.exe",
                r"$env:LOCALAPPDATA\Flare\current\Flare.exe",
                "/Applications/Flare.app/Contents/MacOS/flare",
            ]
        );
        assert_eq!(probe_kinds("flare"), [None, Some(Os::Pc)]);
        assert_eq!(probe_kinds(FLARE_CANDIDATES[1]), [Some(Os::Pc)]);
        assert_eq!(probe_kinds(FLARE_CANDIDATES[3]), [Some(Os::Pc)]);
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

    /// Rule 1237: the dialog opens with nothing picked; "Type an address" saves the address as the machine's own host,
    /// with the User field's name and a port after a colon, and runs ssh at that address.
    #[test]
    fn a_typed_address_is_checked_and_saved_with_its_user_and_port() {
        use flare_settings::page::machines::TYPE_ADDRESS;
        let mut io = FakeIo::default();
        io.answers.insert(
            ("192.168.0.139".into(), "flare".into()),
            facts("0.4.2", Some(Os::Mac)),
        );
        let mut r = Rig::new(io);
        r.page.page_shown();
        r.settle();
        r.page.open_form();
        assert_eq!(r.page.info().add.pick, "", "nothing picked: Pick a machine");
        assert!(!r.page.info().add.can_add());
        r.page.set_pick(TYPE_ADDRESS);
        assert!(r.page.info().add.typing() && !r.page.info().add.can_add());
        r.page.set_address("192.168.0.139:2222");
        r.page.set_user("max");
        assert!(r.page.info().add.can_add());
        r.page.submit();
        r.settle();
        let saved = r.io.list();
        assert_eq!(saved.len(), 1, "{:?}", r.page.info().add.error);
        assert_eq!(saved[0].host.as_deref(), Some("192.168.0.139"));
        assert_eq!(saved[0].ssh, "192.168.0.139");
        assert_eq!(saved[0].user.as_deref(), Some("max"));
        assert_eq!(saved[0].port, Some(2222));
        assert_eq!(saved[0].name, "192.168.0.139");
        assert_eq!(r.page.info().machines[0].address, "192.168.0.139");
        assert!(!r.page.info().add.open, "the dialog closes");
        // The same address again is refused before any check.
        let probes = r.io.probes.lock().unwrap().len();
        r.page.open_form();
        r.page.set_pick(TYPE_ADDRESS);
        r.page.set_address("max@192.168.0.139:2222");
        r.page.submit();
        assert_eq!(
            r.page.info().add.error.as_deref(),
            Some("192.168.0.139 is linked already, as 192.168.0.139.")
        );
        assert_eq!(r.io.probes.lock().unwrap().len(), probes);
    }

    /// Rule 1236: when ssh itself said why the first try failed (here: refused), Add stops there with the plain line;
    /// no other Flare path is tried, so a host that is off costs one connect timeout, not one per path.
    #[test]
    fn a_reason_from_ssh_ends_the_add_after_one_try() {
        let mut io = FakeIo::default();
        for flare in FLARE_CANDIDATES {
            io.answers.insert(
                ("studio".into(), flare.to_string()),
                Err((
                    false,
                    "the link ended; ssh said: ssh: connect to host studio port 22: Connection refused"
                        .into(),
                )),
            );
        }
        let mut r = Rig::new(io);
        r.page.page_shown();
        r.settle();
        r.page.open_form();
        r.page.set_pick("studio");
        r.page.submit();
        r.settle();
        assert_eq!(r.page.info().add.error.as_deref(), Some("Not answering"));
        assert_eq!(r.io.probes.lock().unwrap().len(), 1, "one try only");
        assert!(r.io.list().is_empty());
    }

    /// Rule 1236: what a shell on the host printed (a Flare path that is not runnable) is no reason of ssh's: Add
    /// goes on to the next Flare path, which answers.
    #[test]
    fn a_shells_permission_denied_does_not_end_the_add() {
        let mut io = FakeIo::default();
        io.answers.insert(
            ("studio".into(), FLARE_CANDIDATES[0].to_string()),
            Err((
                false,
                format!(
                    "the link ended; ssh said: bash: {}: Permission denied",
                    FLARE_CANDIDATES[0]
                ),
            )),
        );
        io.answers.insert(
            ("studio".into(), FLARE_CANDIDATES[1].to_string()),
            facts("0.4.2", Some(Os::Mac)),
        );
        let mut r = Rig::new(io);
        r.page.page_shown();
        r.settle();
        r.page.open_form();
        r.page.set_pick("studio");
        r.page.submit();
        r.settle();
        let saved = r.io.list();
        assert_eq!(saved.len(), 1, "{:?}", r.page.info().add.error);
        assert_eq!(saved[0].flare, FLARE_CANDIDATES[1]);
    }

    /// Rule 1237: what the dialog names, before any check: `user@` and `:port` are read from a typed address, and an
    /// address, a user or a port that cannot be one is one red line.
    #[test]
    fn the_dialog_names_a_machine_or_says_why_not_in_one_line() {
        use flare_settings::page::machines::{NEARBY_PREFIX, TYPE_ADDRESS};
        let form = |pick: &str, address: &str, user: &str| AddForm {
            open: true,
            pick: pick.into(),
            address: address.into(),
            user: user.into(),
            ..AddForm::default()
        };
        let m = new_machine(&form(TYPE_ADDRESS, " max@studio.local ", "")).unwrap();
        assert_eq!(
            (m.target(), m.user.as_deref(), m.port),
            ("studio.local", Some("max"), None)
        );
        // The User field and a port after the colon.
        let m = new_machine(&form(TYPE_ADDRESS, "10.0.0.5:22", "admin")).unwrap();
        assert_eq!((m.user.as_deref(), m.port), (Some("admin"), Some(22)));
        let m = new_machine(&form(TYPE_ADDRESS, "fe80::1", "")).unwrap();
        assert_eq!((m.target(), m.port), ("fe80::1", None), "IPv6 stays whole");
        let m = new_machine(&form(TYPE_ADDRESS, "[fe80::1]:2222", "")).unwrap();
        assert_eq!(
            (m.target(), m.port),
            ("fe80::1", Some(2222)),
            "IPv6 in brackets"
        );
        let m = new_machine(&form(TYPE_ADDRESS, "[fe80::1]", "")).unwrap();
        assert_eq!((m.target(), m.port), ("fe80::1", None));
        // The User field wins over a `user@`, which never stays in the address.
        let m = new_machine(&form(TYPE_ADDRESS, "max@studio.local", "admin")).unwrap();
        assert_eq!(
            (m.target(), m.user.as_deref()),
            ("studio.local", Some("admin"))
        );
        let line = |f: AddForm| new_machine(&f).unwrap_err();
        assert_eq!(
            line(form(TYPE_ADDRESS, "a@b@studio", "")),
            "Type the address with one @ at most."
        );
        assert_eq!(
            line(form(TYPE_ADDRESS, "[fe80::1]x", "")),
            "Type the address with no spaces."
        );
        assert_eq!(line(form(TYPE_ADDRESS, "  ", "")), "Type an address.");
        assert_eq!(
            line(form(TYPE_ADDRESS, "my mac", "")),
            "Type the address with no spaces."
        );
        assert_eq!(
            line(form(TYPE_ADDRESS, "-oProxyCommand=x", "")),
            "Type the address with no spaces."
        );
        assert_eq!(
            line(form(TYPE_ADDRESS, "mac:99999", "")),
            "Type a port from 1 to 65535 after the colon."
        );
        assert_eq!(
            line(form(TYPE_ADDRESS, "mac", "a b")),
            "Type the user name with no spaces."
        );
        assert_eq!(line(form("", "", "")), "Pick a machine.");
        assert_eq!(
            line(form(&format!("{NEARBY_PREFIX}x"), "", "")),
            "Pick a machine."
        );
        let m = new_machine(&form("studio", "ignored", "ignored")).unwrap();
        assert_eq!(
            (m.ssh.as_str(), m.host.clone(), m.user.clone()),
            ("studio", None, None)
        );
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
        r.page.set_pick("studio");
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
        r.page.set_pick("nas");
        assert_eq!(r.page.info().add.error, None, "a new pick clears the line");
        r.page.submit();
        r.settle();
        assert_eq!(
            r.page.info().add.error.as_deref(),
            Some("No Flare answered over SSH on nas.")
        );
        // No host at all, and a host linked already, never reach a worker.
        r.page.set_pick("");
        r.page.submit();
        assert_eq!(r.page.info().add.error.as_deref(), Some("Pick a machine."));
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

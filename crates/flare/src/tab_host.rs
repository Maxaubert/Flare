//! TermHandles per tab, spawn, prewarm, kill, shutdown (Task 27).
//!
//! One `TermHandle` per live tab, all over one `SessionManager`. The host owns no UI state: the
//! window asks it to spawn, kill, resize the front tab and drain what the shells reported.

#![allow(dead_code)] // wired by AppCtl (a later task); remove this line then

use flare_agent::resume::valid_resume;
use flare_settings::agent_prefs::agent_hooks_on;
use flare_settings::kv::PrefStore;
use flare_settings::recent_roots::remember_root;
use flare_settings::term_prefs::saved_shell_id;
use flare_shells::env::PluginEnv;
use flare_term::emulator::TermEvent;
use flare_term::handle::{StartOptions, TermHandle, Typist};
use flare_term::pty::PtySpawner;
use flare_term::session::{SessionManager, SessionOptions, ShellSource};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

/// The shell a new tab starts: the saved one. macOS under `--e2e` only: with none saved, the shell the
/// run names in `FLARE_E2E_SHELL` (the Mac E2E runs its PowerShell scenarios in pwsh, rule 1106); the
/// variable reads as absent without the flag, and Windows never looks.
#[cfg(not(target_os = "macos"))]
fn shell_id_of(prefs: &PrefStore) -> Option<String> {
    saved_shell_id(prefs)
}

#[cfg(target_os = "macos")]
fn shell_id_of(prefs: &PrefStore) -> Option<String> {
    saved_shell_id(prefs).or_else(|| {
        crate::e2e::global()
            .env(&["FLARE_E2E", "_SHELL"].concat())
            .filter(|v| !v.is_empty())
    })
}

/// How long the quit waits for the shells (rule 28).
const QUIT_WAIT: Duration = Duration::from_secs(3);

/// What a tab's shell reported, tagged with the tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostEvent {
    Cwd {
        id: String,
        path: String,
    },
    Exited {
        id: String,
        code: Option<u32>,
    },
    Started {
        id: String,
    },
    StartFailed {
        id: String,
    },
    Title {
        id: String,
        text: String,
    },
    Bell {
        id: String,
    },
    /// A program wrote the clipboard (OSC 52).
    ClipboardWrite {
        id: String,
        text: String,
    },
    /// An OSC 777 payload (everything after `777;`); the agent code parses it.
    Osc777 {
        id: String,
        data: String,
    },
    /// An OSC 7501 program-status payload (everything after `7501;`); the agent code parses it.
    Osc7501 {
        id: String,
        data: String,
    },
    /// The pty printed (one per drained batch).
    Output {
        id: String,
    },
    /// A person typed in the tab.
    Typed {
        id: String,
    },
    /// Enter, Esc, Ctrl+C or a digit was sent by a user key.
    AnswerKey {
        id: String,
        key: String,
    },
    /// A refit ran.
    Resized {
        id: String,
    },
    /// A resuming tab's terminal is shown now.
    Revealed {
        id: String,
    },
}

/// Who sets a tab's size (remote sessions PR 2, rule 282): this window, or a remote viewer that sent `remote.resize`.
/// The live terminal has one size, the last typist's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SizeOwner {
    Local,
    Remote,
}

pub struct TabHost {
    mgr: Arc<SessionManager>,
    handles: HashMap<String, Arc<TermHandle>>,
    /// Spawn order, so `live_ids` and `drain_events` are stable.
    order: Vec<String>,
    shell_ids: HashMap<String, Option<String>>,
    dims: (usize, usize),
    waker: Arc<dyn Fn() + Send + Sync>,
    prefs: PrefStore,
    /// Where the Claude plugin lives, found once at startup. None: no shell's environment is
    /// touched.
    plugin_dir: Option<String>,
    /// Tabs whose size a remote viewer set: the window's refits pass them by until a key is typed here (rule 282).
    /// A cell, so the event drain (which takes `&self`) can hand a tab back when a key was typed here.
    remote_sized: RefCell<HashMap<String, (usize, usize)>>,
}

impl TabHost {
    pub fn new(
        spawner: Arc<dyn PtySpawner>,
        shells: Arc<dyn ShellSource>,
        prefs: PrefStore,
        waker: Arc<dyn Fn() + Send + Sync>,
        plugin_dir: Option<String>,
    ) -> Self {
        Self::with_options(
            spawner,
            shells,
            prefs,
            waker,
            plugin_dir,
            SessionOptions::default(),
        )
    }

    /// `new` with the session manager's options (tests give a base environment).
    pub fn with_options(
        spawner: Arc<dyn PtySpawner>,
        shells: Arc<dyn ShellSource>,
        prefs: PrefStore,
        waker: Arc<dyn Fn() + Send + Sync>,
        plugin_dir: Option<String>,
        options: SessionOptions,
    ) -> Self {
        Self {
            mgr: SessionManager::new(spawner, shells, options),
            handles: HashMap::new(),
            order: Vec::new(),
            shell_ids: HashMap::new(),
            dims: (80, 24),
            waker,
            prefs,
            plugin_dir,
            remote_sized: RefCell::new(HashMap::new()),
        }
    }

    /// The plugin handed to a NEW shell: the folder found at startup, switched by the setting
    /// read now (only a new shell sees a change).
    pub fn plugin_env(&self) -> Option<PluginEnv> {
        self.plugin_dir.as_ref().map(|dir| PluginEnv {
            dir: dir.clone(),
            on: agent_hooks_on(&self.prefs),
        })
    }

    /// Remembers the folder, makes a handle at the CURRENT grid size and starts its shell on a
    /// worker thread. `resume` only ever reaches the shell through `StartOptions` (rule 2).
    pub fn spawn(&mut self, id: &str, cwd: &str, resume: Option<&str>) {
        if self.handles.contains_key(id) {
            return;
        }
        remember_root(&self.prefs, cwd);
        let (cols, rows) = self.dims;
        let handle = TermHandle::new(id, Arc::clone(&self.mgr), cols, rows);
        handle.set_waker(Arc::clone(&self.waker));
        let shell_id = shell_id_of(&self.prefs);
        // The second of the three validations of a resume id (rule 2): only a session id or the
        // Codex marker ever reaches a command line.
        let valid = valid_resume(resume);
        if resume.is_some() && valid.is_none() {
            log::warn!(
                "a resume id of {} bytes is not a session id: no resume",
                resume.map_or(0, str::len)
            );
        }
        handle.start(StartOptions {
            root: cwd.to_string(),
            shell_id: shell_id.clone(),
            resume: valid.map(str::to_string),
            plugin: self.plugin_env(),
            remote: None,
        });
        self.handles.insert(id.to_string(), handle);
        self.order.push(id.to_string());
        self.shell_ids.insert(id.to_string(), shell_id);
    }

    /// A remote tab (remote sessions PR 3, rule 287): a handle at the current grid size whose session is the host's,
    /// shown through a `RemotePty`. The folder is the host's: it is not remembered among this machine's folders, and
    /// no shell, resume or plugin of this machine is named (rules 290, 291).
    pub fn spawn_remote(&mut self, id: &str, spec: flare_term::remote_pty::RemoteSpec) {
        if self.handles.contains_key(id) {
            return;
        }
        let (cols, rows) = self.dims;
        let handle = TermHandle::new(id, Arc::clone(&self.mgr), cols, rows);
        handle.set_waker(Arc::clone(&self.waker));
        handle.start(StartOptions {
            root: spec.folder.clone(),
            remote: Some(spec),
            ..StartOptions::default()
        });
        self.handles.insert(id.to_string(), handle);
        self.order.push(id.to_string());
        self.shell_ids.insert(id.to_string(), None);
    }

    pub fn handle(&self, id: &str) -> Option<Arc<TermHandle>> {
        self.handles.get(id).cloned()
    }

    /// The shell kind the tab was spawned with; `None` for a tab this host never spawned.
    pub fn shell_id_of(&self, id: &str) -> Option<Option<String>> {
        self.shell_ids.get(id).cloned()
    }

    /// Settings and placeholders own no handle: a no-op.
    pub fn kill(&mut self, id: &str) {
        let Some(h) = self.handles.remove(id) else {
            return;
        };
        h.kill();
        self.mgr.kill(id);
        self.order.retain(|x| x != id);
        self.shell_ids.remove(id);
        self.remote_sized.borrow_mut().remove(id);
    }

    /// A remote tab goes from this window and its host session goes on (remote sessions PR 7, "Hide here only", rule
    /// 1192): the pty lets go (`remote.detach`), never `remote.close`. A no-op for a tab with no handle.
    pub fn leave(&mut self, id: &str) {
        if self.handles.remove(id).is_none() {
            return;
        }
        self.mgr.leave(id);
        self.order.retain(|x| x != id);
        self.shell_ids.remove(id);
        self.remote_sized.borrow_mut().remove(id);
    }

    /// Who sets the tab's size now.
    pub fn size_owner(&self, id: &str) -> SizeOwner {
        if self.remote_sized.borrow().contains_key(id) {
            SizeOwner::Remote
        } else {
            SizeOwner::Local
        }
    }

    /// A remote viewer's `remote.resize` (rule 282): the tab takes that size and its owner is the remote viewer, so the
    /// window's refits leave it alone until someone types here. False for a tab this host has no handle for.
    pub fn remote_resize(&mut self, id: &str, cols: usize, rows: usize) -> bool {
        let Some(h) = self.handles.get(id) else {
            return false;
        };
        if cols < 2 || rows < 1 {
            return false;
        }
        self.remote_sized
            .borrow_mut()
            .insert(id.to_string(), (cols, rows));
        if h.size() != (cols, rows) {
            h.resize(cols, rows);
        }
        true
    }

    /// The size goes back to this window (a key typed here, or the last remote viewer gone): the tab fits the grid
    /// area again. Never ends anything (rule 280).
    pub fn take_size_back(&self, id: &str) {
        if self.remote_sized.borrow_mut().remove(id).is_none() {
            return;
        }
        if let Some(h) = self.handles.get(id) {
            h.resize(self.dims.0, self.dims.1);
        }
    }

    fn local_owned(&self, id: &str) -> bool {
        !self.remote_sized.borrow().contains_key(id)
    }

    /// A size or family change, or the shared zoom (rule 1154): every open tab, visible or not,
    /// takes the new grid size through the same resize path as a window resize, so each shell
    /// reflows.
    pub fn refit_all(&mut self, cols: usize, rows: usize) {
        if cols < 2 || rows < 1 {
            return;
        }
        self.dims = (cols, rows);
        self.mgr.resize("", cols, rows);
        for id in &self.order {
            // A tab a remote viewer sized keeps that size until a key is typed here (rule 282).
            if !self.local_owned(id) {
                continue;
            }
            if let Some(h) = self.handles.get(id) {
                h.resize(cols, rows);
            }
        }
    }

    /// BLOCKING inside `SessionManager::prewarm`, so it runs on a worker thread; best effort.
    pub fn prewarm(&self, cwd: &str) {
        let mgr = Arc::clone(&self.mgr);
        let shell_id = shell_id_of(&self.prefs);
        let plugin = self.plugin_env();
        let cwd = cwd.to_string();
        let r = std::thread::Builder::new()
            .name("flare-prewarm".into())
            .spawn(move || mgr.prewarm(&cwd, shell_id.as_deref(), plugin.as_ref()));
        if let Err(e) = r {
            log::warn!("could not start the prewarm worker: {e}");
        }
    }

    /// The grid area changed: tabs born from now on start at this size, and the front tab's
    /// handle (if any) resizes. Hidden tabs keep their size until `activate`.
    pub fn set_dims(&mut self, cols: usize, rows: usize, front: Option<&str>) {
        if cols < 2 || rows < 1 {
            return;
        }
        self.dims = (cols, rows);
        // The default size a spawn or a prewarm uses (the manager's "" entry).
        self.mgr.resize("", cols, rows);
        if let Some(f) = front.filter(|f| self.local_owned(f))
            && let Some(h) = self.handles.get(f)
        {
            h.resize(cols, rows);
        }
    }

    pub fn dims(&self) -> (usize, usize) {
        self.dims
    }

    /// A tab shown after being hidden fits the area it now has.
    pub fn activate(&self, id: &str) {
        if !self.local_owned(id) {
            return;
        }
        if let Some(h) = self.handles.get(id) {
            h.resize(self.dims.0, self.dims.1);
        }
    }

    /// Every handle's events, mapped. Bell is already gated by the handle. Events the host has
    /// no reader for (raw OSC) are dropped here.
    pub fn drain_events(&self) -> Vec<HostEvent> {
        let mut out = Vec::new();
        let mut typed_here = Vec::new();
        for id in &self.order {
            let Some(h) = self.handles.get(id) else {
                continue;
            };
            let evs = h.drain_events();
            // The last typist wins (rule 282): a key typed here takes a remote-sized tab back to this window's size.
            if self.remote_sized.borrow().contains_key(id)
                && evs.contains(&TermEvent::Typed)
                && h.last_typist() == Some(Typist::Local)
            {
                typed_here.push(id.clone());
            }
            for ev in evs {
                let id = id.clone();
                match ev {
                    TermEvent::Cwd(path) => out.push(HostEvent::Cwd { id, path }),
                    TermEvent::Exited(code) => out.push(HostEvent::Exited { id, code }),
                    TermEvent::Started => out.push(HostEvent::Started { id }),
                    TermEvent::StartFailed => out.push(HostEvent::StartFailed { id }),
                    TermEvent::Title(text) => out.push(HostEvent::Title { id, text }),
                    TermEvent::Bell => out.push(HostEvent::Bell { id }),
                    TermEvent::ClipboardWrite(text) => {
                        out.push(HostEvent::ClipboardWrite { id, text });
                    }
                    TermEvent::Osc { ident: 777, data } => {
                        out.push(HostEvent::Osc777 { id, data });
                    }
                    TermEvent::Osc { ident: 7501, data } => {
                        out.push(HostEvent::Osc7501 { id, data });
                    }
                    TermEvent::Output => out.push(HostEvent::Output { id }),
                    TermEvent::Typed => out.push(HostEvent::Typed { id }),
                    TermEvent::AnswerKey(key) => out.push(HostEvent::AnswerKey { id, key }),
                    TermEvent::Resized => out.push(HostEvent::Resized { id }),
                    TermEvent::Revealed => out.push(HostEvent::Revealed { id }),
                    _ => {}
                }
            }
        }
        for id in typed_here {
            self.take_size_back(&id);
        }
        out
    }

    /// The session manager, for the agent poll (live pids, output ticks) and the tab tokens.
    pub fn manager(&self) -> Arc<SessionManager> {
        Arc::clone(&self.mgr)
    }

    pub fn live_ids(&self) -> Vec<String> {
        self.order.clone()
    }

    /// Each live tab's scrollback bytes, in spawn order (the memory breakdown).
    pub fn scrollbacks(&self) -> Vec<(String, usize)> {
        self.order
            .iter()
            .filter_map(|id| Some((id.clone(), self.handles.get(id)?.scrollback_bytes())))
            .collect()
    }

    /// Kills every local shell and every spare, lets go of every remote tab (its host session goes on, rule 288), then
    /// waits (3 s cap) for the shells to be gone. `kill_all` does both, so no tab is killed one by one here.
    pub fn shutdown(&mut self) -> bool {
        self.handles.clear();
        self.order.clear();
        self.shell_ids.clear();
        self.remote_sized.borrow_mut().clear();
        self.mgr.kill_all();
        let ok = self.mgr.shells_gone(QUIT_WAIT);
        if !ok {
            log::warn!("the quit gave up waiting for the shells after 3 s");
        }
        ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flare_settings::recent_roots::recent_roots;
    use flare_shells::list::ShellDef;
    use flare_term::fake::{FakeShells, FakeSpawner};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(3) {
            if f() {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("timed out waiting for {what}");
    }

    fn host_with(shells: Arc<dyn ShellSource>) -> (TabHost, Arc<FakeSpawner>, PrefStore) {
        let sp = FakeSpawner::new();
        let prefs = PrefStore::memory();
        let n = Arc::new(AtomicUsize::new(0));
        let waker: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            n.fetch_add(1, Ordering::SeqCst);
        });
        let h = TabHost::new(sp.clone(), shells, prefs.clone(), waker, None);
        (h, sp, prefs)
    }

    fn host() -> (TabHost, Arc<FakeSpawner>, PrefStore) {
        host_with(FakeShells::new())
    }

    fn made(sp: &FakeSpawner, n: usize) {
        wait_until("the spawns", || sp.made().len() >= n);
    }

    #[test]
    fn spawn_starts_a_shell_in_the_folder_and_remembers_it() {
        let (mut h, sp, prefs) = host();
        h.spawn("t1", "C:\\a", None);
        made(&sp, 1);
        assert_eq!(sp.made().len(), 1);
        assert_eq!(sp.made()[0].spec().cwd, "C:\\a");
        assert_eq!(recent_roots(&prefs), vec!["C:\\a".to_string()]);
        assert!(h.handle("t1").is_some());
    }

    /// Rule 282: a remote resize makes the remote viewer the size owner; the window's refits, a new front size and
    /// an activation pass the tab by; a key typed here (the last typist) takes it back to the window's size.
    #[test]
    fn a_remote_resize_owns_the_size_until_a_key_is_typed_here() {
        let (mut h, sp, _p) = host();
        h.spawn("t1", "C:\\a", None);
        h.spawn("t2", "C:\\b", None);
        made(&sp, 2);
        assert_eq!(h.size_owner("t1"), SizeOwner::Local);
        assert!(h.remote_resize("t1", 50, 20));
        assert_eq!(h.size_owner("t1"), SizeOwner::Remote);
        assert_eq!(h.handle("t1").unwrap().size(), (50, 20));
        h.refit_all(100, 30);
        h.set_dims(110, 32, Some("t1"));
        h.activate("t1");
        assert_eq!(
            h.handle("t1").unwrap().size(),
            (50, 20),
            "the window's refits pass it by"
        );
        assert_eq!(
            h.handle("t2").unwrap().size(),
            (100, 30),
            "a local tab still follows the window"
        );
        // Remote typing keeps it remote; a local key takes it back.
        let t1 = h.handle("t1").unwrap();
        t1.remote_input(&[flare_term::handle::RemoteIntent::Text("x".into())]);
        let _ = h.drain_events();
        assert_eq!(h.size_owner("t1"), SizeOwner::Remote);
        t1.send_text("y");
        let _ = h.drain_events();
        assert_eq!(h.size_owner("t1"), SizeOwner::Local);
        assert_eq!(h.handle("t1").unwrap().size(), (110, 32));
        assert!(!h.remote_resize("nobody", 50, 20));
    }

    /// Rule 280: the size going back (a detach, a link gone) never ends the tab's shell.
    #[test]
    fn taking_the_size_back_never_kills_the_shell() {
        let (mut h, sp, _p) = host();
        h.spawn("t1", "C:\\a", None);
        made(&sp, 1);
        h.refit_all(90, 25);
        assert!(h.remote_resize("t1", 40, 10));
        h.take_size_back("t1");
        assert_eq!(h.size_owner("t1"), SizeOwner::Local);
        assert_eq!(h.handle("t1").unwrap().size(), (90, 25));
        assert!(!sp.made()[0].killed());
        assert!(h.handle("t1").is_some());
    }

    /// Rules 287, 288, 291: a remote tab runs over the spawner's remote side, never a local shell or a spare; its host
    /// folder is not remembered among this machine's folders; the quit lets go of its session and never closes it.
    #[test]
    fn a_remote_tab_never_starts_a_local_shell_and_the_quit_leaves_its_session() {
        let sp = FakeSpawner::new();
        let remote = flare_term::fake::FakeRemote::new();
        let prefs = PrefStore::memory();
        let mut h = TabHost::new(
            Arc::new(flare_term::pty::RoutingSpawner {
                local: sp.clone(),
                remote: remote.clone(),
            }),
            FakeShells::new(),
            prefs.clone(),
            Arc::new(|| {}),
            None,
        );
        h.spawn_remote(
            "r1",
            flare_term::remote_pty::RemoteSpec {
                machine: "mac".into(),
                folder: "/Users/max/proj".into(),
                resume: None,
                live: flare_term::remote_pty::RemoteLive::default(),
            },
        );
        wait_until("the remote pty", || remote.made().len() == 1);
        assert!(sp.made().is_empty());
        assert!(recent_roots(&prefs).is_empty());
        assert!(h.handle("r1").is_some());
        assert!(h.shutdown());
        assert_eq!(
            remote.made()[0].1.leaving(),
            Some(flare_term::remote_pty::Leaving::Detach)
        );
    }

    #[test]
    fn the_saved_shell_id_is_passed_on() {
        let defs = vec![
            ShellDef {
                id: "pwsh".into(),
                name: "PowerShell 7".into(),
                exe: "pwsh.exe".into(),
                args: vec![],
            },
            ShellDef {
                id: "cmd".into(),
                name: "Command Prompt".into(),
                exe: "cmd.exe".into(),
                args: vec![],
            },
        ];
        let (mut h, sp, prefs) = host_with(FakeShells::with(defs.clone()));
        prefs.set("flare.term.shell", "cmd");
        h.spawn("t1", "C:\\a", None);
        made(&sp, 1);
        assert_eq!(sp.made()[0].spec().exe, "cmd.exe");
        assert_eq!(h.shell_id_of("t1"), Some(Some("cmd".to_string())));

        let (mut h2, sp2, _p) = host_with(FakeShells::with(defs));
        h2.spawn("t1", "C:\\a", None);
        made(&sp2, 1);
        assert_eq!(sp2.made()[0].spec().exe, "pwsh.exe");
        assert_eq!(h2.shell_id_of("t1"), Some(None));
    }

    #[test]
    fn a_resume_goes_through_start_options_only() {
        // The id travels in the environment of a shell that has arguments to extend.
        let pwsh = ShellDef {
            id: "pwsh".into(),
            name: "PowerShell 7".into(),
            exe: "pwsh.exe".into(),
            args: vec!["-NoLogo".into()],
        };
        let (mut h, sp, _p) = host_with(FakeShells::with(vec![pwsh]));
        let id = "5a1d0c2e-1111-4222-8333-94445555a666";
        h.spawn("t1", "C:\\a", Some(id));
        made(&sp, 1);
        let spec = sp.made()[0].spec();
        assert!(
            spec.env
                .iter()
                .any(|(k, v)| k == "FLARE_RESUME_ID" && v == id)
        );
        assert!(sp.made()[0].written().is_empty());
    }

    fn row0(h: &TermHandle) -> String {
        let g = h.snapshot();
        (0..g.cols())
            .map(|c| g.cell(c, 0).ch)
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    #[test]
    fn two_tabs_two_handles_and_no_cross_talk() {
        let (mut h, sp, _p) = host();
        h.spawn("a", "C:\\a", None);
        made(&sp, 1);
        h.spawn("b", "C:\\b", None);
        made(&sp, 2);
        let a = sp
            .made()
            .into_iter()
            .find(|m| m.spec().cwd == "C:\\a")
            .unwrap();
        a.feed(b"x");
        let (ha, hb) = (h.handle("a").unwrap(), h.handle("b").unwrap());
        wait_until("a shows x", || row0(&ha) == "x");
        assert_eq!(row0(&hb), "");
    }

    #[test]
    fn a_size_change_refits_every_open_tab() {
        let (mut h, sp, _p) = host();
        for (i, id) in ["a", "b", "c"].iter().enumerate() {
            h.spawn(id, "C:\\a", None);
            made(&sp, i + 1);
        }
        h.refit_all(100, 30);
        wait_until("all three resized", || {
            sp.made().iter().all(|m| m.size() == (100, 30))
        });
        assert_eq!(h.dims(), (100, 30));
    }

    #[test]
    fn a_zero_size_or_hidden_panel_does_not_refit_the_grid() {
        let (mut h, sp, _p) = host();
        for (i, id) in ["a", "b"].iter().enumerate() {
            h.spawn(id, "C:\\a", None);
            made(&sp, i + 1);
        }
        h.refit_all(100, 30);
        wait_until("both resized", || {
            sp.made().iter().all(|m| m.size() == (100, 30))
        });
        // A hidden or collapsed panel measures 0 x 0 (or a sliver): nothing refits, in either path.
        for (c, r) in [(0, 0), (0, 30), (100, 0), (1, 30)] {
            h.refit_all(c, r);
            h.set_dims(c, r, Some("a"));
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
        assert_eq!(h.dims(), (100, 30));
        assert!(sp.made().iter().all(|m| m.size() == (100, 30)));
    }

    #[test]
    fn kill_removes_the_handle_and_kills_the_shell() {
        let (mut h, sp, _p) = host();
        h.spawn("t1", "C:\\a", None);
        made(&sp, 1);
        wait_until("started", || h.mgr.live_pids().len() == 1);
        h.kill("t1");
        assert!(h.handle("t1").is_none());
        wait_until("killed", || sp.made()[0].killed());
        h.kill("t1");
        assert!(h.live_ids().is_empty());
    }

    #[test]
    fn kill_of_an_unknown_id_is_a_no_op() {
        let (mut h, sp, _p) = host();
        h.kill("settings");
        h.kill("placeholder-1");
        assert_eq!(sp.made().len(), 0);
    }

    fn events_until(h: &TabHost, want: impl Fn(&[HostEvent]) -> bool) -> Vec<HostEvent> {
        let mut all = Vec::new();
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(3) {
            all.extend(h.drain_events());
            if want(&all) {
                return all;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("timed out; events: {all:?}");
    }

    #[test]
    fn a_cwd_report_becomes_an_event() {
        let (mut h, sp, _p) = host();
        h.spawn("t1", "C:\\a", None);
        made(&sp, 1);
        sp.made()[0].feed(b"\x1b]9;9;C:\\w\x07");
        let evs = events_until(&h, |e| e.iter().any(|x| matches!(x, HostEvent::Cwd { .. })));
        let want = HostEvent::Cwd {
            id: "t1".into(),
            path: "C:\\w".into(),
        };
        assert_eq!(evs.iter().filter(|e| **e == want).count(), 1);
    }

    #[test]
    fn osc_777_and_the_activity_events_reach_the_host_and_other_oscs_do_not() {
        let (mut h, sp, _p) = host();
        h.spawn("t1", "C:\\a", None);
        made(&sp, 1);
        sp.made()[0].feed(b"\x1b]777;flare-agent;state=done\x07\x1b]1337;SetMark\x07x");
        let evs = events_until(&h, |e| {
            e.iter().any(|x| matches!(x, HostEvent::Output { .. }))
        });
        assert!(evs.contains(&HostEvent::Osc777 {
            id: "t1".into(),
            data: "flare-agent;state=done".into()
        }));
        assert_eq!(
            evs.iter()
                .filter(|e| matches!(e, HostEvent::Osc777 { .. }))
                .count(),
            1
        );
    }

    #[test]
    fn a_shell_that_exits_becomes_an_exited_event() {
        let (mut h, sp, _p) = host();
        h.spawn("t1", "C:\\a", None);
        made(&sp, 1);
        sp.made()[0].feed(b"bye\r\n");
        sp.made()[0].exit(0);
        let evs = events_until(&h, |e| {
            e.iter().any(|x| matches!(x, HostEvent::Exited { .. }))
        });
        assert!(evs.contains(&HostEvent::Exited {
            id: "t1".into(),
            code: Some(0)
        }));
    }

    #[test]
    fn new_tabs_are_born_at_the_current_grid_size_and_only_the_front_resizes() {
        let (mut h, sp, _p) = host();
        h.set_dims(100, 30, None);
        h.spawn("a", "C:\\a", None);
        made(&sp, 1);
        assert_eq!(sp.made()[0].spec().cols, 100);
        wait_until("a started", || h.mgr.live_pids().len() == 1);
        h.set_dims(120, 40, Some("a"));
        wait_until("a resized", || sp.made()[0].size() == (120, 40));
        h.spawn("b", "C:\\b", None);
        made(&sp, 2);
        let b = sp.made()[1].clone();
        assert_eq!((b.spec().cols, b.spec().rows), (120, 40));
        wait_until("b started", || h.mgr.live_pids().len() == 2);
        h.set_dims(90, 20, Some("a"));
        wait_until("a follows", || sp.made()[0].size() == (90, 20));
        assert_eq!(b.size(), (120, 40));
        h.activate("b");
        wait_until("b follows", || b.size() == (90, 20));
    }

    #[test]
    fn prewarm_starts_a_warm_shell_off_the_ui_thread() {
        let (shells, opener) = FakeShells::gated();
        let (mut h, sp, _p) = host_with(shells);
        let t = Instant::now();
        h.prewarm("C:\\home");
        assert!(t.elapsed() < Duration::from_millis(50));
        opener.open();
        wait_until("the spare", || sp.made().len() == 1);
        h.spawn("t1", "C:\\home", None);
        let mut started = false;
        wait_until("started", || {
            started |= h
                .drain_events()
                .iter()
                .any(|e| matches!(e, HostEvent::Started { .. }));
            started
        });
        assert_eq!(sp.made().len(), 1);
    }

    #[test]
    fn shutdown_kills_every_shell_and_waits() {
        let (mut h, sp, _p) = host();
        for (i, n) in ["a", "b", "c"].iter().enumerate() {
            h.spawn(n, "C:\\a", None);
            made(&sp, i + 1);
        }
        wait_until("all started", || h.mgr.live_pids().len() == 3);
        assert!(h.shutdown());
        assert!(sp.made().iter().all(|m| m.killed()));
        assert!(h.live_ids().is_empty());
        assert!(h.shutdown());
    }

    #[test]
    fn shutdown_gives_up_after_three_seconds_and_says_so() {
        let (mut h, sp, _p) = host();
        sp.set_late_exit(true);
        h.spawn("a", "C:\\a", None);
        h.spawn("b", "C:\\b", None);
        made(&sp, 2);
        wait_until("started", || h.mgr.live_pids().len() == 2);
        let t = Instant::now();
        assert!(!h.shutdown());
        assert!(t.elapsed() < Duration::from_secs(4));
        for m in sp.made() {
            m.exit(0);
        }
    }

    #[test]
    fn start_results_are_events() {
        let (mut h, sp, _p) = host();
        h.spawn("good", "C:\\a", None);
        made(&sp, 1);
        let evs = events_until(&h, |e| {
            e.iter().any(|x| matches!(x, HostEvent::Started { .. }))
        });
        assert!(evs.contains(&HostEvent::Started { id: "good".into() }));
        sp.fail_next_spawn();
        h.spawn("bad", "C:\\b", None);
        let evs = events_until(&h, |e| {
            e.iter().any(|x| matches!(x, HostEvent::StartFailed { .. }))
        });
        assert!(evs.contains(&HostEvent::StartFailed { id: "bad".into() }));
    }

    // ---- resume and the plugin (M6, Task 30) ---------------------------------------------------

    const SESSION: &str = "3f2a9c1e-77aa-4c0d-9b1e-0a1b2c3d4e5f";
    const SESSION_B: &str = "0f0e0d0c-2222-4333-8444-955566667777";

    fn shell_def(id: &str, exe: &str, args: &[&str]) -> ShellDef {
        ShellDef {
            id: id.to_string(),
            name: id.to_string(),
            exe: exe.to_string(),
            args: args.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    fn pwsh_def() -> ShellDef {
        shell_def(
            "pwsh",
            "pwsh.exe",
            &["-NoLogo", "-NoExit", "-Command", "boot"],
        )
    }

    fn powershell_def() -> ShellDef {
        shell_def(
            "powershell",
            "powershell.exe",
            &["-NoLogo", "-NoExit", "-Command", "boot"],
        )
    }

    fn cmd_def() -> ShellDef {
        shell_def("cmd", "cmd.exe", &[])
    }

    /// A host over one shell kind, with a plugin folder and a base environment.
    fn host_over(
        def: ShellDef,
        plugin_dir: Option<&str>,
        base_env: Option<Vec<(String, String)>>,
    ) -> (TabHost, Arc<FakeSpawner>, PrefStore) {
        let sp = FakeSpawner::new();
        let prefs = PrefStore::memory();
        prefs.set("flare.term.shell", &def.id);
        let waker: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {});
        let opts = SessionOptions {
            base_env,
            ..SessionOptions::default()
        };
        let h = TabHost::with_options(
            sp.clone(),
            FakeShells::with(vec![def]),
            prefs.clone(),
            waker,
            plugin_dir.map(str::to_string),
            opts,
        );
        (h, sp, prefs)
    }

    fn env_of(m: &flare_term::fake::FakeState, key: &str) -> Option<String> {
        m.spec()
            .env
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.clone())
    }

    #[test]
    fn a_bad_resume_id_never_reaches_a_command_line() {
        let (mut h, sp, _p) = host_over(pwsh_def(), None, Some(Vec::new()));
        h.spawn("a", "C:\\a", Some("x; rm -rf"));
        made(&sp, 1);
        let m = &sp.made()[0];
        assert!(env_of(m, "FLARE_RESUME_ID").is_none());
        assert_eq!(m.spec().args, pwsh_def().args, "plain args");
        assert!(m.written().is_empty());
    }

    #[test]
    fn a_cmd_resume_carries_only_hex_into_the_environment() {
        let ids = [
            "a&calc",
            "a|b",
            "%PATH%",
            "x\"y",
            SESSION,
            "codex:last",
            "",
            "12345678&calc",
        ];
        for (n, id) in ids.iter().enumerate() {
            let (mut h, sp, _p) = host_over(cmd_def(), None, Some(Vec::new()));
            h.spawn("a", "C:\\a", Some(id));
            made(&sp, 1);
            let m = &sp.made()[0];
            let spec = m.spec();
            match env_of(m, "FLARE_RESUME_ID") {
                None => {}
                Some(v) => {
                    assert!(
                        !v.is_empty() && v.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-'),
                        "case {n}: {v:?}"
                    );
                }
            }
            for a in &spec.args {
                assert!(
                    id.is_empty() || !a.contains(id),
                    "case {n}: the id is in an argument"
                );
            }
        }
    }

    #[test]
    fn the_resume_is_the_only_write_and_it_is_not_a_write() {
        let (mut h, sp, _p) = host_over(pwsh_def(), None, Some(Vec::new()));
        h.spawn("a", "C:\\a", Some(SESSION));
        made(&sp, 1);
        let m = &sp.made()[0];
        assert!(m.written().is_empty(), "nothing is typed into the shell");
        assert_eq!(env_of(m, "FLARE_RESUME_ID").as_deref(), Some(SESSION));
        let last = m.spec().args.last().cloned().unwrap_or_default();
        assert!(
            last.ends_with("Remove-Item Env:FLARE_RESUME_ID; claude --resume $r }"),
            "{last}"
        );
        assert!(!last.contains(SESSION));
    }

    #[test]
    fn no_resume_command_line_uses_an_encoded_command() {
        for def in [pwsh_def(), powershell_def(), cmd_def()] {
            for resume in [SESSION, "codex:last"] {
                let (mut h, sp, _p) = host_over(def.clone(), None, Some(Vec::new()));
                h.spawn("a", "C:\\a", Some(resume));
                made(&sp, 1);
                let spec = sp.made()[0].spec();
                for a in &spec.args {
                    let low = a.to_lowercase();
                    assert!(
                        !low.starts_with("-enc") && !low.contains("-encodedcommand"),
                        "{a}"
                    );
                    assert!(!a.contains(SESSION), "the id is in an argument: {a}");
                }
                let want = flare_shells::resume::with_resume(&def, resume).args;
                assert_eq!(spec.args, want, "{} {resume}", def.id);
                if def.id == "cmd" {
                    assert_eq!(spec.args.first().map(String::as_str), Some("/K"));
                }
            }
        }
    }

    #[test]
    fn every_restored_shell_of_a_kind_has_the_same_command_line() {
        for def in [pwsh_def(), cmd_def()] {
            let (mut h, sp, _p) = host_over(def.clone(), None, Some(Vec::new()));
            h.spawn("a", "C:\\a", Some(SESSION));
            h.spawn("b", "C:\\b", Some(SESSION_B));
            made(&sp, 2);
            let made = sp.made();
            assert_eq!(made[0].spec().args, made[1].spec().args, "{}", def.id);
            assert_eq!(made[0].spec().exe, made[1].spec().exe);
        }
    }

    #[test]
    fn a_new_shell_gets_the_plugin_while_the_setting_is_on() {
        let dir = "C:\\Flare\\claude-plugin";
        let (mut h, sp, prefs) = host_over(pwsh_def(), Some(dir), Some(Vec::new()));
        h.spawn("a", "C:\\a", None);
        made(&sp, 1);
        let first = env_of(&sp.made()[0], "CLAUDE_CODE_PLUGIN_DIRS").unwrap_or_default();
        assert!(first.ends_with(dir), "{first}");
        flare_settings::agent_prefs::set_agent_hooks_on(&prefs, false);
        h.spawn("b", "C:\\b", None);
        made(&sp, 2);
        let second = sp.made()[1].clone();
        assert!(env_of(&second, "CLAUDE_CODE_PLUGIN_DIRS").is_none());
        // The shell that was running keeps what it started with.
        assert!(
            env_of(&sp.made()[0], "CLAUDE_CODE_PLUGIN_DIRS")
                .unwrap_or_default()
                .ends_with(dir)
        );
    }

    #[test]
    fn no_plugin_folder_means_the_environment_is_untouched() {
        let base = vec![(
            "CLAUDE_CODE_PLUGIN_DIRS".to_string(),
            "D:\\mine".to_string(),
        )];
        let (mut h, sp, _p) = host_over(pwsh_def(), None, Some(base));
        h.spawn("a", "C:\\a", None);
        made(&sp, 1);
        assert_eq!(
            env_of(&sp.made()[0], "CLAUDE_CODE_PLUGIN_DIRS").as_deref(),
            Some("D:\\mine")
        );
    }
}

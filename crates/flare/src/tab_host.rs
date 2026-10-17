//! TermHandles per tab, spawn, prewarm, kill, shutdown (Task 27).
//!
//! One `TermHandle` per live tab, all over one `SessionManager`. The host owns no UI state: the
//! window asks it to spawn, kill, resize the front tab and drain what the shells reported.

#![allow(dead_code)] // wired by AppCtl (a later task); remove this line then

use flare_settings::kv::PrefStore;
use flare_settings::recent_roots::remember_root;
use flare_settings::term_prefs::saved_shell_id;
use flare_term::emulator::TermEvent;
use flare_term::handle::{StartOptions, TermHandle};
use flare_term::pty::PtySpawner;
use flare_term::session::{SessionManager, SessionOptions, ShellSource};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

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

pub struct TabHost {
    mgr: Arc<SessionManager>,
    handles: HashMap<String, Arc<TermHandle>>,
    /// Spawn order, so `live_ids` and `drain_events` are stable.
    order: Vec<String>,
    shell_ids: HashMap<String, Option<String>>,
    dims: (usize, usize),
    waker: Arc<dyn Fn() + Send + Sync>,
    prefs: PrefStore,
    /// Ctrl+wheel zoom per tab, in pixels. Never saved (`tabs.json` is unchanged).
    font_override: HashMap<String, i32>,
}

impl TabHost {
    pub fn new(
        spawner: Arc<dyn PtySpawner>,
        shells: Arc<dyn ShellSource>,
        prefs: PrefStore,
        waker: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        Self {
            mgr: SessionManager::new(spawner, shells, SessionOptions::default()),
            handles: HashMap::new(),
            order: Vec::new(),
            shell_ids: HashMap::new(),
            dims: (80, 24),
            waker,
            prefs,
            font_override: HashMap::new(),
        }
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
        let shell_id = saved_shell_id(&self.prefs);
        handle.start(StartOptions {
            root: cwd.to_string(),
            shell_id: shell_id.clone(),
            resume: resume.map(str::to_string),
            plugin: None,
        });
        self.handles.insert(id.to_string(), handle);
        self.order.push(id.to_string());
        self.shell_ids.insert(id.to_string(), shell_id);
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
        self.font_override.remove(id);
    }

    /// The tab's own zoom in pixels, if it has one.
    pub fn font_override(&self, id: &str) -> Option<i32> {
        self.font_override.get(id).copied()
    }

    /// Sets or clears the tab's own zoom. Held in memory only.
    pub fn set_font_override(&mut self, id: &str, px: Option<i32>) {
        match px {
            Some(px) => {
                self.font_override.insert(id.to_string(), px);
            }
            None => {
                self.font_override.remove(id);
            }
        }
    }

    /// A size or family change: every open tab, visible or not, takes the new grid size through
    /// the same resize path as a window resize.
    pub fn refit_all(&mut self, cols: usize, rows: usize) {
        if cols < 2 || rows < 1 {
            return;
        }
        self.dims = (cols, rows);
        self.mgr.resize("", cols, rows);
        for id in &self.order {
            if let Some(h) = self.handles.get(id) {
                h.resize(cols, rows);
            }
        }
    }

    /// BLOCKING inside `SessionManager::prewarm`, so it runs on a worker thread; best effort.
    pub fn prewarm(&self, cwd: &str) {
        let mgr = Arc::clone(&self.mgr);
        let shell_id = saved_shell_id(&self.prefs);
        let cwd = cwd.to_string();
        let r = std::thread::Builder::new()
            .name("flare-prewarm".into())
            .spawn(move || mgr.prewarm(&cwd, shell_id.as_deref(), None));
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
        if let Some(h) = front.and_then(|f| self.handles.get(f)) {
            h.resize(cols, rows);
        }
    }

    pub fn dims(&self) -> (usize, usize) {
        self.dims
    }

    /// A tab shown after being hidden fits the area it now has.
    pub fn activate(&self, id: &str) {
        if let Some(h) = self.handles.get(id) {
            h.resize(self.dims.0, self.dims.1);
        }
    }

    /// Every handle's events, mapped. Bell is already gated by the handle. Events the host has
    /// no reader for (raw OSC) are dropped here.
    pub fn drain_events(&self) -> Vec<HostEvent> {
        let mut out = Vec::new();
        for id in &self.order {
            let Some(h) = self.handles.get(id) else {
                continue;
            };
            for ev in h.drain_events() {
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
                    TermEvent::Output => out.push(HostEvent::Output { id }),
                    TermEvent::Typed => out.push(HostEvent::Typed { id }),
                    TermEvent::AnswerKey(key) => out.push(HostEvent::AnswerKey { id, key }),
                    TermEvent::Resized => out.push(HostEvent::Resized { id }),
                    TermEvent::Revealed => out.push(HostEvent::Revealed { id }),
                    _ => {}
                }
            }
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

    /// Kills every handle and every spare, then waits (3 s cap) for the shells to be gone.
    pub fn shutdown(&mut self) -> bool {
        for h in self.handles.values() {
            h.kill();
        }
        self.handles.clear();
        self.order.clear();
        self.shell_ids.clear();
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
        let h = TabHost::new(sp.clone(), shells, prefs.clone(), waker);
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
    fn the_font_override_is_per_tab_and_dies_with_the_tab() {
        let (mut h, sp, _p) = host();
        h.spawn("a", "C:\\a", None);
        made(&sp, 1);
        h.set_font_override("a", Some(20));
        assert_eq!(h.font_override("a"), Some(20));
        assert_eq!(h.font_override("b"), None);
        h.kill("a");
        assert_eq!(h.font_override("a"), None);
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
}

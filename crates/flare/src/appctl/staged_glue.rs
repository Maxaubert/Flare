//! Build ready (2026-10-29, one click since 2026-11-10), in the window: the "Build ready" chip, the toast and the
//! restart. A click on the chip installs; nothing installs on its own.
//!
//! The rules are `flare_update::{staged, staged_flow}` (pure or temp-dir tested). This file feeds the flow what the
//! window knows (the agents, the close question), runs the disk work on workers, and ends in the normal end of life
//! followed by the new exe, started on the same profile (`handoff::restart_args_keeping`).
//!
//! - Only an installed copy (an exe in a folder named `current`) has this; a build run from `target\release` never
//!   looks for a stage.
//! - The root folder is watched (`FindFirstChangeNotificationW`, on a thread of its own); every change is one check
//!   of `staged\` on a worker. The chip shows only once every hash matches.
//! - The quit is the one the update installer uses: `finish` (tabs.json with the agents, so the sessions resume),
//!   then the new exe, then the event loop ends. Nothing here forces a close.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use flare_update::copy;
use flare_update::staged::{self, Layout, Start};
use flare_update::staged_flow::{Do, Phase, Ready, StagedFlow, Times};
use slint::TimerMode;

use super::{AppCtl, Post, UiMsg};
use crate::startup::AskTarget;
use crate::update_ctl::Spawn;

/// How often the flow is ticked while it counts down or shows a notice.
const TICK_MS: u64 = 250;

/// Starts the new exe: (exe, args, working folder). False when it could not be started.
pub type Restart = Rc<dyn Fn(&Path, &[String], &str) -> bool>;

/// What a worker or the watch tells the window.
#[derive(Debug)]
pub enum StagedMsg {
    /// The folder changed: look at `staged\` again.
    Changed,
    /// A look at `staged\` answered: the ready build, or none.
    Found(Option<Ready>),
    /// The swap ended.
    Swapped(Result<(), String>),
    /// A click on the chip may count down: `agreed` when the close question was answered yes (an agent was at work), false
    /// when none was at work and no question was needed.
    GoAhead { agreed: bool },
    /// A swap this run took back (an agent got busy while it ran) is undone on disk.
    TakenBack,
    /// The 20 s since the start of a new build are up.
    Confirm,
}

/// What the window glue hands over.
pub struct StagedParts {
    pub layout: Layout,
    /// What the start-up guard found (`staged::on_start`, run in `main`).
    pub start: Start,
    pub times: Times,
    /// Runs disk work on a worker (tests record it).
    pub spawn: Spawn,
    /// Starts the new exe after the end of life.
    pub restart: Restart,
    /// Watch the folder (production); tests post `Changed` by hand.
    pub watch: bool,
    pub user_data_dir: Option<PathBuf>,
    /// This run's arguments without the exe (the restart keeps `--e2e` and the like).
    pub argv: Vec<String>,
    /// Under `--e2e`: where `staged-state.json` is written.
    pub e2e_dir: Option<PathBuf>,
}

pub(super) struct StagedState {
    parts: StagedParts,
    flow: StagedFlow,
    timer: slint::Timer,
    timer_on: bool,
    confirm_timer: slint::Timer,
    /// A look at `staged\` is running; `again` asks for one more after it.
    looking: bool,
    again: bool,
    /// This run is a new build that has not been confirmed yet.
    unconfirmed: bool,
    /// This run swapped a build in (its own record must never be confirmed by this run).
    swapped_here: bool,
    /// The running countdown (or swap) was agreed to in the close question: it goes on while agents work.
    agreed: bool,
}

impl AppCtl {
    /// Makes the staged-build glue over this window. Called once by the window glue for an installed copy.
    pub fn attach_staged(rc: &Rc<std::cell::RefCell<AppCtl>>, parts: StagedParts) {
        let post = Arc::clone(&rc.borrow().deps.post);
        if parts.watch {
            watch_root(parts.layout.root().to_path_buf(), Arc::clone(&post));
        }
        let mut flow = StagedFlow::new(parts.times);
        let now = rc.borrow().now();
        let unconfirmed = matches!(parts.start, Start::Installed { .. } | Start::Trying { .. });
        match &parts.start {
            Start::Installed { commit } => {
                flow.say(&copy::staged_installed(&staged::short(commit)), now)
            }
            Start::RolledBack { commit } => {
                flow.say(&copy::staged_rolled_back(&staged::short(commit)), now)
            }
            _ => {}
        }
        let mut c = rc.borrow_mut();
        c.staged = Some(StagedState {
            parts,
            flow,
            timer: slint::Timer::default(),
            timer_on: false,
            confirm_timer: slint::Timer::default(),
            looking: false,
            again: false,
            unconfirmed,
            swapped_here: false,
            agreed: false,
        });
        if unconfirmed {
            let post = Arc::clone(&post);
            if let Some(s) = c.staged.as_mut() {
                s.confirm_timer.start(
                    TimerMode::SingleShot,
                    Duration::from_millis(staged::START_GRACE_MS),
                    move || post(UiMsg::Staged(StagedMsg::Confirm)),
                );
            }
        }
        c.staged_look();
        c.staged_push();
    }

    pub(super) fn on_staged_msg(&mut self, m: StagedMsg) {
        if self.staged.is_none() {
            return;
        }
        match m {
            StagedMsg::Changed => self.staged_look(),
            StagedMsg::Found(r) => {
                if let Some(s) = self.staged.as_mut() {
                    s.looking = false;
                    s.flow.found(r);
                    if std::mem::take(&mut s.again) {
                        self.staged_look();
                    }
                }
            }
            StagedMsg::GoAhead { agreed } => {
                let now = self.now();
                if let Some(s) = self.staged.as_mut()
                    && s.flow.phase() == Phase::Waiting
                {
                    s.agreed = agreed;
                    s.flow.start_countdown(now);
                }
            }
            StagedMsg::TakenBack => {
                if let Some(s) = self.staged.as_mut() {
                    s.swapped_here = false;
                    s.flow.taken_back();
                }
                // The click still stands: it asks now that an agent works.
                self.staged_ask();
            }
            StagedMsg::Swapped(r) => self.staged_swapped(r),
            StagedMsg::Confirm => self.staged_confirm(),
        }
        self.staged_push();
    }

    /// One look at `staged\` on a worker (one at a time; a change during it asks for another).
    fn staged_look(&mut self) {
        let post = Arc::clone(&self.deps.post);
        let Some(s) = self.staged.as_mut() else {
            return;
        };
        if s.looking {
            s.again = true;
            return;
        }
        s.looking = true;
        let layout = s.parts.layout.clone();
        (s.parts.spawn)(Box::new(move || {
            let ready = match staged::check(&layout) {
                Ok(m) => Some(Ready {
                    commit: m.short_commit(),
                    sig: m.signature(),
                }),
                Err(staged::NotReady::Missing) => None,
                Err(e) => {
                    log::warn!("staged build not ready: {e}");
                    None
                }
            };
            post(UiMsg::Staged(StagedMsg::Found(ready)));
        }));
    }

    /// An agent in some tab is working.
    fn staged_working(&self) -> bool {
        !self.busy_agents().is_empty() || self.hub.working_count() > 0
    }

    /// The chip was clicked: install now. No menu (rule 258).
    pub(super) fn staged_chip_clicked(&mut self) {
        let Some(s) = self.staged.as_mut() else {
            return;
        };
        if s.flow.click() == Do::Ask {
            crate::diag_ctl::crumb("staged-click", &[]);
            self.staged_ask();
        }
        self.staged_push();
    }

    /// A click on the chip: with an agent at work the close question asks first (in its install words); else at once.
    fn staged_ask(&mut self) {
        let post = Arc::clone(&self.deps.post);
        let go = move |agreed: bool| post(UiMsg::Staged(StagedMsg::GoAhead { agreed }));
        let busy = self.busy_agents();
        let Some(mut ask) = super::window_ask(&busy) else {
            go(false);
            return;
        };
        ask.install = true;
        ask.target = AskTarget::Window;
        self.drop_install_ask();
        self.install_cb = Some(Box::new(move |yes| {
            if yes {
                go(true);
            }
        }));
        (self.deps.raise)();
        self.ask = Some(ask);
        self.show_ask();
    }

    /// Cancel on the toast.
    pub(super) fn staged_toast_clicked(&mut self) {
        if let Some(s) = self.staged.as_mut() {
            s.flow.cancel();
            s.agreed = false;
        }
        self.staged_push();
    }

    /// The flow's clock: the countdown, the notice.
    fn staged_tick(&mut self) {
        let now = self.now();
        let working = self.staged_working();
        let Some(s) = self.staged.as_mut() else {
            return;
        };
        let counting = matches!(s.flow.phase(), Phase::Countdown { .. });
        if counting && !s.agreed && working {
            // The chip was clicked with no agent at work, and one started: the close question asks first.
            s.flow.pause();
            self.staged_ask();
            self.staged_push();
            return;
        }
        if s.flow.tick(now) == Do::Swap {
            s.swapped_here = true;
            // A new build that has not been confirmed yet is the one being replaced: it has run well enough to be
            // picked over, and the swap only goes ahead over a confirmed build (its `.old` files are the last good one).
            let confirm_first = std::mem::take(&mut s.unconfirmed);
            let layout = s.parts.layout.clone();
            let post = Arc::clone(&self.deps.post);
            crate::diag_ctl::crumb("staged-install", &[]);
            (s.parts.spawn)(Box::new(move || {
                if confirm_first {
                    staged::confirm(&layout);
                }
                let r = staged::swap(&layout).map(|_| ()).map_err(|e| e.to_string());
                post(UiMsg::Staged(StagedMsg::Swapped(r)));
            }));
        }
        self.staged_push();
    }

    /// After a good swap: whether the restart must wait after all. The swap takes a moment; an agent that started
    /// in it holds an install that was not agreed to in the close question.
    fn staged_held_after_swap(&self) -> bool {
        let Some(s) = self.staged.as_ref() else {
            return false;
        };
        !s.agreed && self.staged_working()
    }

    fn staged_swapped(&mut self, r: Result<(), String>) {
        let now = self.now();
        match r {
            Ok(()) if self.staged_held_after_swap() => {
                crate::diag_ctl::crumb("staged-taken-back", &[]);
                let post = Arc::clone(&self.deps.post);
                if let Some(s) = self.staged.as_ref() {
                    let layout = s.parts.layout.clone();
                    (s.parts.spawn)(Box::new(move || {
                        staged::take_back(&layout);
                        post(UiMsg::Staged(StagedMsg::TakenBack));
                    }));
                }
            }
            Ok(()) => {
                if let Some(s) = self.staged.as_mut() {
                    s.flow.swapped(true, now);
                }
                self.staged_restart();
            }
            Err(e) => {
                log::warn!("the staged build could not be installed: {e}");
                crate::diag_ctl::crumb("staged-failed", &[("why", serde_json::Value::String(e))]);
                if let Some(s) = self.staged.as_mut() {
                    s.swapped_here = false;
                    s.agreed = false;
                    s.flow.swapped(false, now);
                }
            }
        }
    }

    /// The new build is in place: the end of life (tabs.json with the agents), the new exe, then the quit.
    fn staged_restart(&mut self) {
        let Some(s) = self.staged.as_ref() else {
            return;
        };
        let exe = s.parts.layout.exe();
        let mut args = flare_update::handoff::restart_args_keeping(
            s.parts.user_data_dir.as_deref(),
            &s.parts.argv,
        );
        // The new exe waits for this one to end before it takes the lock (see `--after-pid`).
        args.push("--after-pid".to_string());
        args.push(std::process::id().to_string());
        let restart = Rc::clone(&s.parts.restart);
        let layout = s.parts.layout.clone();
        self.finish();
        let home = self.deps.home.clone();
        if !restart(&exe, &args, &home) {
            // The new exe would not start: the old files go back, and the old exe (this one's files) starts.
            log::warn!("the new build would not start; putting the old one back");
            staged::abandon(&layout); // sync-ok: the quit path, after the shells are gone; a few renames
            let _ = restart(&exe, &args, &home);
        }
        (self.deps.quit)();
    }

    /// The new build stayed up 20 s: it is good, and the leftovers go (on a worker).
    fn staged_confirm(&mut self) {
        let Some(s) = self.staged.as_mut() else {
            return;
        };
        if !s.unconfirmed || s.swapped_here {
            return;
        }
        s.unconfirmed = false;
        let layout = s.parts.layout.clone();
        (s.parts.spawn)(Box::new(move || staged::confirm(&layout)));
    }

    /// The end of life of a new build that quit cleanly before 20 s: that counts as a good start.
    pub(super) fn staged_on_finish(&mut self) {
        let Some(s) = self.staged.as_mut() else {
            return;
        };
        if s.unconfirmed && !s.swapped_here {
            s.unconfirmed = false;
            staged::confirm(&s.parts.layout); // sync-ok: the quit path, one small file and a few deletes
        }
    }

    /// Puts the flow's view on the window, keeps the clock running only while it is needed, writes the e2e file.
    pub(super) fn staged_push(&mut self) {
        let me = self.me.clone();
        let app = self.app();
        let Some(s) = self.staged.as_mut() else {
            return;
        };
        let v = s.flow.view();
        if let Some(app) = app {
            app.set_staged_chip_shown(v.chip_shown);
            app.set_staged_chip_label(v.chip_label.as_str().into());
            app.set_staged_chip_aria(v.chip_aria.as_str().into());
            app.set_staged_chip_tip(v.chip_tip.as_str().into());
            app.set_staged_toast(v.toast.as_str().into());
            app.set_staged_toast_button(v.toast_button.as_str().into());
        }
        let need = s.flow.needs_tick();
        if need && !s.timer_on {
            s.timer.start(
                TimerMode::Repeated,
                Duration::from_millis(TICK_MS),
                move || {
                    if let Some(rc) = me.upgrade()
                        && let Ok(mut c) = rc.try_borrow_mut()
                    {
                        crate::diag_ctl::timed("staged_tick", || c.staged_tick());
                    }
                },
            );
            s.timer_on = true;
        } else if !need && s.timer_on {
            s.timer.stop();
            s.timer_on = false;
        }
        if let Some(dir) = s.parts.e2e_dir.clone() {
            let state = serde_json::json!({
                "chip": v.chip_shown,
                "label": v.chip_label,
                "tip": v.chip_tip,
                "toast": v.toast,
                "button": v.toast_button,
                "commit": s.flow.ready().map(|r| r.commit.clone()),
                "phase": match s.flow.phase() {
                    Phase::Waiting => "waiting",
                    Phase::Countdown { .. } => "countdown",
                    Phase::Installing => "installing",
                },
                "start": format!("{:?}", s.parts.start),
                "pid": std::process::id(),
            })
            .to_string();
            (s.parts.spawn)(Box::new(move || {
                let path = dir.join("staged-state.json");
                let tmp = dir.join("staged-state.json.tmp");
                // sync-ok: a worker, one small e2e file
                if std::fs::write(&tmp, state).is_ok() {
                    let _ = std::fs::rename(&tmp, &path); // sync-ok: a worker, one small e2e file
                }
            }));
        }
    }

    /// The flow, for the tests.
    #[cfg(test)]
    pub(crate) fn staged_flow(&self) -> Option<&StagedFlow> {
        self.staged.as_ref().map(|s| &s.flow)
    }

    /// The window's post, for the tests (what the watch thread posts with).
    #[cfg(test)]
    pub(crate) fn post_for_test(&self) -> Post {
        Arc::clone(&self.deps.post)
    }
}

/// Watches the install root (and below) on a thread of its own; every change posts one `Changed` (a burst of
/// writes, such as a stage being copied, settles for 300 ms first).
#[cfg(windows)]
fn watch_root(root: PathBuf, post: Post) {
    use windows_sys::Win32::Foundation::{INVALID_HANDLE_VALUE, WAIT_OBJECT_0};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE,
        FindFirstChangeNotificationW, FindNextChangeNotification,
    };
    use windows_sys::Win32::System::Threading::{INFINITE, WaitForSingleObject};
    let spawned = std::thread::Builder::new()
        .name("flare-staged-watch".into())
        .spawn(move || {
            let wide: Vec<u16> = root
                .as_os_str()
                .to_string_lossy()
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            // SAFETY: `wide` is a NUL-terminated UTF-16 path that outlives the call; the handle is checked below.
            let h = unsafe {
                FindFirstChangeNotificationW(
                    wide.as_ptr(),
                    1,
                    FILE_NOTIFY_CHANGE_FILE_NAME
                        | FILE_NOTIFY_CHANGE_DIR_NAME
                        | FILE_NOTIFY_CHANGE_LAST_WRITE,
                )
            };
            if h == INVALID_HANDLE_VALUE || h.is_null() {
                log::warn!("cannot watch {} for a staged build", root.display());
                return;
            }
            loop {
                // SAFETY: `h` is a valid change-notification handle for the life of this thread.
                let r = unsafe { WaitForSingleObject(h, INFINITE) };
                if r != WAIT_OBJECT_0 {
                    return;
                }
                std::thread::sleep(Duration::from_millis(300)); // sync-ok: the watch thread, a settle
                // SAFETY: as above; re-arms the same handle.
                if unsafe { FindNextChangeNotification(h) } == 0 {
                    return;
                }
                post(UiMsg::Staged(StagedMsg::Changed));
            }
        });
    if let Err(e) = spawned {
        log::warn!("could not start the staged-build watch: {e}");
    }
}

#[cfg(not(windows))]
fn watch_root(_root: PathBuf, _post: Post) {}

/// What the start-up guard found, for the window glue.
static STARTED: std::sync::OnceLock<(Layout, Start)> = std::sync::OnceLock::new();

/// The last-good guard, once per start of an installed copy (`main`, after the single-instance check, so a launch
/// that only hands its folders over does not count). When the new build died early too often, its files were just
/// put back: the old exe starts on the same profile and this process ends.
pub fn guard_at_start(argv: &[String], user_data_dir: Option<&Path>) {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let Some(layout) = Layout::from_exe(&exe) else {
        return;
    };
    let start = staged::on_start(&layout); // sync-ok: startup, before the window; one small file
    let back = match &start {
        Start::RollBack { commit } => {
            log::warn!("build {commit} died early twice; starting the last good build");
            true
        }
        Start::Interrupted {
            commit,
            restart: true,
        } => {
            log::warn!(
                "the install of build {commit} was cut off; its files are back, starting the old exe"
            );
            true
        }
        _ => false,
    };
    if back {
        let args = flare_update::handoff::restart_args_keeping(user_data_dir, argv);
        let home = std::env::var("USERPROFILE").unwrap_or_default();
        spawn_exe()(&layout.exe(), &args, &home);
        std::process::exit(0);
    }
    let _ = STARTED.set((layout, start));
}

/// The installed copy's layout and what its start found; None for a run that is no installed copy.
pub fn started() -> Option<(Layout, Start)> {
    STARTED.get().cloned()
}

/// The flow's waits: the real ones, or under `--e2e` the scenario's (`FLARE_E2E_STAGED_COUNTDOWN_MS`).
pub fn times() -> Times {
    let ms =
        |suffix: &str| crate::ship_ctl::e2e_var(suffix).and_then(|v| v.trim().parse::<u64>().ok());
    let mut t = Times::default();
    if let Some(v) = ms("STAGED_COUNTDOWN_MS") {
        t.countdown_ms = v;
    }
    t
}

/// The production restart: the new exe, detached from nothing (a test run's job keeps it, so the reaper finds it).
pub fn spawn_exe() -> Restart {
    Rc::new(|exe, args, dir| {
        let mut cmd = std::process::Command::new(exe); // sync-ok: the quit path, one process start
        cmd.args(args);
        if Path::new(dir).is_dir() {
            cmd.current_dir(dir);
        }
        match cmd.spawn() {
            Ok(_) => true,
            Err(e) => {
                log::warn!("could not start {}: {e}", exe.display());
                false
            }
        }
    })
}

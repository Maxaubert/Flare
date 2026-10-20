//! M9 Task 30: UpdateCtl, the update flow into the window.
//!
//! Everything with a rule lives in `flare_update` (the reducer, the copy, the notes parser, the
//! engine); this file only moves data between that model and the `AppWindow` the chip, the notice
//! and the window draw from. The chip and the window are Props only, like Prism Terminal's: the
//! controller pushes one [`UpdateView`] after every change of the flow and binds the five callbacks
//! the window reports. The release notes are plain strings all the way (`Text`, never markup).
//!
//! Source: `PT/core/renderer/lib/useUpdateFlow.ts`, `PT/src/renderer/src/App.tsx` (the install
//! guard and the hide/return around the close question).
#![allow(dead_code)] // the engine glue (Task 32) is the caller of `PostEvents` and `std_spawn`

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use flare_update::copy::{
    can_abort, cancel_enabled, cancel_label, cancel_tip, chip_aria, chip_label, chip_tip, pct_text,
    status_line, window_sub, window_title,
};
use flare_update::engine::{Engine, Events, Fld};
use flare_update::flow::{Action, Flow, reduce};
use flare_update::geom::{NOTICE_MS, notes_max_h};
use flare_update::groups::group_release_notes;
use flare_update::notes::{more_line, parse_release_notes};
use flare_update::types::{Offer, Phase};
use slint::{ComponentHandle, ModelRc, SharedString, TimerMode, VecModel};

use crate::appctl::{Post, UiMsg};
use crate::{AppWindow, NoteSection};

/// A job for a worker thread.
pub type Job = Box<dyn FnOnce() + Send>;
/// Runs a job on a worker thread.
pub type Spawn = Rc<dyn Fn(Job)>;
/// The answer to the host's close question: go ahead, or not.
pub type Answer = Box<dyn FnOnce(bool)>;
/// The host's close question for an install.
pub type AskInstall = Rc<dyn Fn(Answer)>;

/// Everything the Slint side shows, pure, built from the flow and the running version.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct UpdateView {
    pub chip_shown: bool,
    pub chip_label: String,
    pub chip_aria: String,
    pub chip_tip: String,
    pub chip_busy: bool,
    /// The flow's notice, ONLY while the window is not up (else empty).
    pub notice: String,
    pub open: bool,
    pub title: String,
    pub sub: String,
    pub sections: Vec<(String, Vec<String>)>,
    pub empty_note: String,
    pub more: String,
    pub status: String,
    pub pct: u8,
    pub pct_text: String,
    pub running: bool,
    pub active: bool,
    pub cancel_label: String,
    pub cancel_enabled: bool,
    pub cancel_tip: String,
    pub install_enabled: bool,
}

/// The view of a flow. `active` is the progress block's visibility: a run, or an ending said in the window.
pub fn view_of(flow: &Flow, current: &str) -> UpdateView {
    let Some(info) = &flow.info else {
        return UpdateView::default();
    };
    let v = info.version.as_str();
    let running = flow.phase != Phase::Idle;
    let notes = parse_release_notes(Some(&info.notes));
    let sections = group_release_notes(&notes)
        .into_iter()
        .map(|s| (s.heading.to_string(), s.entries))
        .collect();
    let notice = flow.notice.as_deref();
    UpdateView {
        chip_shown: true,
        chip_label: chip_label(v),
        chip_aria: chip_aria(v),
        chip_tip: chip_tip(v, info.mock, flow.phase),
        chip_busy: running,
        notice: if flow.open {
            String::new()
        } else {
            notice.unwrap_or("").to_string()
        },
        open: flow.open,
        title: window_title(v, running),
        sub: window_sub(current),
        sections,
        empty_note: if notes.empty {
            notes.entries.first().cloned().unwrap_or_default()
        } else {
            String::new()
        },
        more: if notes.more > 0 {
            more_line(notes.more)
        } else {
            String::new()
        },
        status: status_line(flow.phase, info.mock, flow.aborting, notice),
        pct: flow.pct,
        pct_text: pct_text(flow.pct),
        running,
        active: running || notice.is_some(),
        cancel_label: cancel_label(running, notice).to_string(),
        cancel_enabled: cancel_enabled(running, can_abort(flow.phase, flow.aborting)),
        cancel_tip: cancel_tip(running, flow.phase, flow.aborting).to_string(),
        install_enabled: !running,
    }
}

/// What a worker or a timer tells the controller.
#[derive(Clone, Debug)]
pub enum UpdateMsg {
    Offer(Offer),
    Progress(u8),
    Settled(bool),
    PreviewRequested,
    QuitIn(u64),
    NoticeTimeout,
}

/// What the controller needs from the outside.
pub struct UpdateDeps {
    pub engine: Arc<Engine>,
    pub post: Post,
    pub current_version: String,
    /// The host's close question, asked before an install starts. Answers `true` (go ahead) at
    /// once when no agent is at work, else later through the dialog.
    pub ask_install: AskInstall,
    /// Pre-answers the close question; called by the engine's worker once the installer is started.
    pub agree: Arc<dyn Fn() + Send + Sync>,
    /// Runs a closure on a worker thread (tests record it).
    pub spawn: Spawn,
    /// Under `--e2e`: where `update-state.json`, `update-calls.json` and `update-command` live.
    pub e2e_dir: Option<PathBuf>,
    /// The end of life: flush, kill the shells, wait, exit.
    pub quit: Rc<dyn Fn()>,
}

/// The engine's events, posted to the UI thread as [`UpdateMsg`]s.
pub struct PostEvents {
    pub post: Post,
}

impl Events for PostEvents {
    fn offer(&self, o: Offer) {
        (self.post)(UiMsg::Update(UpdateMsg::Offer(o)));
    }

    fn progress(&self, pct: u8) {
        (self.post)(UiMsg::Update(UpdateMsg::Progress(pct)));
    }

    fn crumb(&self, name: &str, fields: Vec<(String, Fld)>) {
        let fields: Vec<(&str, serde_json::Value)> = fields
            .iter()
            .map(|(k, v)| {
                let v = match v {
                    Fld::Bool(b) => serde_json::Value::Bool(*b),
                    Fld::Num(n) => serde_json::Value::from(*n),
                    Fld::Str(s) => serde_json::Value::String(s.clone()),
                };
                (k.as_str(), v)
            })
            .collect();
        crate::diag_ctl::crumb(name, &fields);
    }

    fn quit_in(&self, ms: u64) {
        (self.post)(UiMsg::Update(UpdateMsg::QuitIn(ms)));
    }

    fn rss_mb(&self) -> Option<u64> {
        crate::memory::process_memory().map(|m| (m.working_set as u64) >> 20)
    }
}

/// A worker thread per closure, named.
pub fn std_spawn() -> Spawn {
    Rc::new(|f| {
        if let Err(e) = std::thread::Builder::new()
            .name("flare-update-job".into())
            .spawn(f)
        {
            log::warn!("could not start an update worker: {e}");
        }
    })
}

pub struct UpdateCtl {
    weak: slint::Weak<AppWindow>,
    me: Weak<RefCell<UpdateCtl>>,
    deps: UpdateDeps,
    flow: Flow,
    view: UpdateView,
    sections: Rc<VecModel<NoteSection>>,
    notice_timer: slint::Timer,
    notice_on: bool,
    quit_timer: slint::Timer,
    cmd_timer: slint::Timer,
    /// Counts the e2e state writes; a worker skips its write when a later one exists.
    write_seq: Arc<AtomicU64>,
    write_lock: Arc<Mutex<()>>,
}

impl UpdateCtl {
    /// Binds the five callbacks, hands the window its notes model and pushes the empty view.
    pub fn new(app: &AppWindow, deps: UpdateDeps) -> Rc<RefCell<UpdateCtl>> {
        let sections: Rc<VecModel<NoteSection>> = Rc::new(VecModel::default());
        app.set_update_sections(ModelRc::from(sections.clone()));
        let e2e = deps.e2e_dir.is_some();
        let rc = Rc::new(RefCell::new(UpdateCtl {
            weak: app.as_weak(),
            me: Weak::new(),
            deps,
            flow: Flow::default(),
            view: UpdateView::default(),
            sections,
            notice_timer: slint::Timer::default(),
            notice_on: false,
            quit_timer: slint::Timer::default(),
            cmd_timer: slint::Timer::default(),
            write_seq: Arc::new(AtomicU64::new(0)),
            write_lock: Arc::new(Mutex::new(())),
        }));
        rc.borrow_mut().me = Rc::downgrade(&rc);
        macro_rules! bind {
            ($on:ident, |$c:ident| $body:expr) => {{
                let me = Rc::downgrade(&rc);
                app.$on(move || {
                    if let Some(rc) = me.upgrade()
                        && let Ok(mut $c) = rc.try_borrow_mut()
                    {
                        crate::diag_ctl::timed(stringify!($on), || $body)
                    }
                });
            }};
        }
        bind!(on_update_chip_clicked, |c| c.chip_clicked());
        bind!(on_update_install, |c| c.install_pressed());
        bind!(on_update_cancel, |c| c.cancel_pressed());
        bind!(on_update_abort, |c| c.cancel_pressed());
        bind!(on_update_notice_dismissed, |c| c
            .on_msg(UpdateMsg::NoticeTimeout));
        {
            let mut c = rc.borrow_mut();
            if e2e {
                c.start_command_poll();
            }
            c.push();
        }
        rc
    }

    /// The one entry for everything the engine and the timers say.
    pub fn on_msg(&mut self, m: UpdateMsg) {
        match m {
            UpdateMsg::Offer(o) => self.apply(Action::Available(o)),
            UpdateMsg::Progress(p) => self.apply(Action::Progress(f64::from(p))),
            UpdateMsg::Settled(ok) => self.apply(Action::Settled { ok }),
            UpdateMsg::PreviewRequested => self.deps.engine.preview_if_none(),
            UpdateMsg::QuitIn(ms) => {
                let quit = Rc::clone(&self.deps.quit);
                self.quit_timer.start(
                    TimerMode::SingleShot,
                    Duration::from_millis(ms),
                    move || quit(),
                );
            }
            UpdateMsg::NoticeTimeout => self.apply(Action::Dismiss),
        }
    }

    /// A click on the chip opens the window. It never installs.
    pub fn chip_clicked(&mut self) {
        self.update_crumb("update-open");
        self.apply(Action::Open);
    }

    /// Install was pressed. The host's close question comes first: an install is the one thing
    /// that quits the app under a working agent.
    pub fn install_pressed(&mut self) {
        if self.flow.info.is_none() || self.flow.phase != Phase::Idle {
            return;
        }
        let inline = Rc::new(Cell::new(true));
        let answer: Rc<Cell<Option<bool>>> = Rc::new(Cell::new(None));
        let me = self.me.clone();
        let ask = Rc::clone(&self.deps.ask_install);
        {
            let inline = Rc::clone(&inline);
            let answer = Rc::clone(&answer);
            ask(Box::new(move |go| {
                if inline.get() {
                    // Answered at once, while `install_pressed` still holds the controller.
                    answer.set(Some(go));
                    return;
                }
                if let Some(rc) = me.upgrade()
                    && let Ok(mut c) = rc.try_borrow_mut()
                {
                    if go {
                        c.begin_install();
                    } else {
                        c.show_again();
                    }
                }
            }));
        }
        inline.set(false);
        match answer.get() {
            Some(true) => self.begin_install(),
            Some(false) => {}
            // The question is up: the update window steps aside until it is answered.
            None => self.hide(),
        }
    }

    /// Cancel: backs out while idle; while a download runs it stops the download.
    pub fn cancel_pressed(&mut self) {
        if self.flow.phase == Phase::Idle {
            self.apply(Action::Cancel);
            return;
        }
        let was = self.flow.aborting;
        self.apply(Action::Abort);
        if self.flow.aborting && !was {
            self.deps.engine.cancel();
        }
    }

    /// The host puts the window away (the close question is up); the install carries on.
    pub fn hide(&mut self) {
        self.apply(Action::Hide);
    }

    /// `update-open` / `update-install`: the offered version and whether it is a preview.
    fn update_crumb(&self, name: &str) {
        let Some(info) = &self.flow.info else { return };
        crate::diag_ctl::crumb(
            name,
            &[
                ("version", crate::diag_ctl::txt(&info.version)),
                ("preview", serde_json::Value::Bool(info.mock)),
            ],
        );
    }

    /// The window comes back (the close question was answered no).
    pub fn show_again(&mut self) {
        self.apply(Action::Open);
    }

    /// The window is up: the keys and the chords belong to it.
    pub fn window_up(&self) -> bool {
        self.flow.open
    }

    /// The window changed size or scale: the notes list is capped by the window's height.
    pub fn on_scale_or_size(&mut self, _win_w: f32, win_h: f32, _scale: f32) {
        if let Some(app) = self.weak.upgrade() {
            app.set_update_notes_max_h(notes_max_h(win_h));
        }
    }

    fn begin_install(&mut self) {
        let Some(info) = self.flow.info.clone() else {
            return;
        };
        if self.flow.phase != Phase::Idle {
            return;
        }
        self.update_crumb("update-install");
        self.apply(Action::Install);
        let engine = Arc::clone(&self.deps.engine);
        let post = Arc::clone(&self.deps.post);
        let agree = Arc::clone(&self.deps.agree);
        let token = info.token;
        (self.deps.spawn)(Box::new(move || {
            // The install waits on a download and the installer: it is never a slow work item.
            let ok = crate::diag_ctl::timed_waits("update_install", || {
                engine.install(Some(token), &*agree)
            });
            post(UiMsg::Update(UpdateMsg::Settled(ok)));
        }));
    }

    fn apply(&mut self, a: Action) {
        self.flow = reduce(&self.flow, &a);
        self.push();
    }

    /// Puts the whole view on the window, runs the notice timer and writes the e2e files.
    fn push(&mut self) {
        let v = view_of(&self.flow, &self.deps.current_version);
        if let Some(app) = self.weak.upgrade() {
            if v.sections != self.view.sections {
                self.sections.set_vec(
                    v.sections
                        .iter()
                        .map(|(heading, entries)| NoteSection {
                            heading: heading.into(),
                            entries: ModelRc::new(VecModel::from(
                                entries.iter().map(SharedString::from).collect::<Vec<_>>(),
                            )),
                        })
                        .collect::<Vec<_>>(),
                );
            }
            app.set_update_chip_shown(v.chip_shown);
            app.set_update_chip_label((&v.chip_label).into());
            app.set_update_chip_aria((&v.chip_aria).into());
            app.set_update_chip_tip((&v.chip_tip).into());
            app.set_update_chip_busy(v.chip_busy);
            app.set_update_notice((&v.notice).into());
            app.set_update_open(v.open);
            app.set_update_title((&v.title).into());
            app.set_update_sub((&v.sub).into());
            app.set_update_empty_note((&v.empty_note).into());
            app.set_update_more((&v.more).into());
            app.set_update_status((&v.status).into());
            app.set_update_pct(i32::from(v.pct));
            app.set_update_pct_text((&v.pct_text).into());
            app.set_update_running(v.running);
            app.set_update_active(v.active);
            app.set_update_cancel_label((&v.cancel_label).into());
            app.set_update_cancel_enabled(v.cancel_enabled);
            app.set_update_cancel_tip((&v.cancel_tip).into());
            app.set_update_install_enabled(v.install_enabled);
        }
        self.sync_notice_timer(!v.notice.is_empty());
        self.view = v;
        self.write_e2e_files();
    }

    /// The notice under the chip goes after 8000 ms (or on a click, which dismisses at once).
    fn sync_notice_timer(&mut self, has_notice: bool) {
        if has_notice && !self.notice_on {
            let me = self.me.clone();
            self.notice_timer.start(
                TimerMode::SingleShot,
                Duration::from_millis(NOTICE_MS),
                move || {
                    if let Some(rc) = me.upgrade()
                        && let Ok(mut c) = rc.try_borrow_mut()
                    {
                        c.on_msg(UpdateMsg::NoticeTimeout);
                    }
                },
            );
            self.notice_on = true;
        } else if !has_notice && self.notice_on {
            self.notice_timer.stop();
            self.notice_on = false;
        }
    }

    // ---- the e2e seams (Appendix F): only under `--e2e` -----------------------------------------

    fn start_command_poll(&mut self) {
        let Some(dir) = self.deps.e2e_dir.clone() else {
            return;
        };
        let me = self.me.clone();
        self.cmd_timer
            .start(TimerMode::Repeated, Duration::from_millis(250), move || {
                let Some(word) = claim_command(&dir) else {
                    return;
                };
                if let Some(rc) = me.upgrade()
                    && let Ok(mut c) = rc.try_borrow_mut()
                {
                    match word.trim() {
                        "open" => c.chip_clicked(),
                        "install" => c.install_pressed(),
                        "cancel" => c.cancel_pressed(),
                        _ => {}
                    }
                }
            });
    }

    fn write_e2e_files(&self) {
        let Some(dir) = self.deps.e2e_dir.clone() else {
            return;
        };
        let f = &self.flow;
        let state = serde_json::json!({
            "chip": f.info.is_some(),
            "version": f.info.as_ref().map(|o| o.version.clone()).unwrap_or_default(),
            "mock": f.info.as_ref().is_some_and(|o| o.mock),
            "open": f.open,
            "phase": match f.phase {
                Phase::Idle => "idle",
                Phase::Downloading => "downloading",
                Phase::Installing => "installing",
            },
            "pct": f.pct,
            "notice": f.notice,
        })
        .to_string();
        let calls = self.deps.engine.calls();
        let calls =
            serde_json::json!({"checks": calls.checks, "installs": calls.installs}).to_string();
        let seq = self.write_seq.fetch_add(1, Ordering::SeqCst) + 1;
        let latest = Arc::clone(&self.write_seq);
        let lock = Arc::clone(&self.write_lock);
        (self.deps.spawn)(Box::new(move || {
            let _held = lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if latest.load(Ordering::SeqCst) != seq {
                return;
            }
            write_atomic(&dir.join("update-state.json"), &state);
            write_atomic(&dir.join("update-calls.json"), &calls);
        }));
    }
}

/// Takes the e2e command: renames `update-command` to a claimed name first (so a command written
/// meanwhile is a new file, never deleted unread), then reads and removes the claimed copy.
/// Writers put the word in `update-command.tmp` and rename it, so a half-written word is never read.
fn claim_command(dir: &std::path::Path) -> Option<String> {
    let path = dir.join("update-command");
    let claimed = dir.join("update-command.claimed");
    // sync-ok: the e2e command file, polled only under --e2e
    std::fs::rename(&path, &claimed).ok()?;
    let word = std::fs::read_to_string(&claimed).ok(); // sync-ok: the e2e command file
    let _ = std::fs::remove_file(&claimed); // sync-ok: the e2e command file
    word
}

/// Writes next to the target, then renames over it, so a reader never sees half a file.
fn write_atomic(path: &std::path::Path, text: &str) {
    let tmp = path.with_extension("json.tmp");
    // sync-ok: runs on a worker thread
    if std::fs::write(&tmp, text).is_ok() && std::fs::rename(&tmp, path).is_err() {
        let _ = std::fs::remove_file(&tmp); // sync-ok: runs on a worker thread
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use flare_update::engine::{CondvarSleeper, Mode};
    use flare_update::feed::{FakeFeed, Feed, RefusingFeed};
    use flare_update::flow::{INSTALL_FAILED, PREVIEW_DONE};
    use flare_update::preview::{SAMPLE_NOTES, Waiter};

    use super::*;
    use crate::ui_harness::Harness;

    struct NoWait;
    impl Waiter for NoWait {
        fn wait(&self, _ms: u64) {}
    }

    type Queue = Arc<Mutex<VecDeque<UiMsg>>>;
    type Jobs = Rc<RefCell<Vec<Box<dyn FnOnce() + Send>>>>;
    type Asks = Rc<RefCell<Vec<Box<dyn FnOnce(bool)>>>>;

    struct Fx {
        h: Harness<AppWindow>,
        ctl: Rc<RefCell<UpdateCtl>>,
        q: Queue,
        engine: Arc<Engine>,
        jobs: Jobs,
        asks: Asks,
        quits: Rc<Cell<u32>>,
        agreed: Arc<Mutex<Vec<usize>>>,
        feed: FakeFeed,
    }

    #[derive(Clone, Copy, PartialEq)]
    enum Ask {
        Yes,
        Hold,
    }

    fn notes() -> &'static str {
        "## What's Changed\n* feat: a thing by @x in https://github.com/x/y/pull/1\n* fix: another by @x in https://github.com/x/y/pull/2"
    }

    fn build(
        mode: Mode,
        feed: FakeFeed,
        dyn_feed: Option<Arc<dyn Feed>>,
        ask: Ask,
        e2e_dir: Option<PathBuf>,
    ) -> Fx {
        let q: Queue = Arc::default();
        let post: Post = {
            let q = q.clone();
            Arc::new(move |m| q.lock().unwrap().push_back(m))
        };
        let feed_arc: Arc<dyn Feed> = dyn_feed.unwrap_or_else(|| Arc::new(feed.clone()));
        let engine = Engine::new(
            mode,
            feed_arc,
            "0.1.0",
            None,
            Arc::new(PostEvents { post: post.clone() }),
            Arc::new(CondvarSleeper::new()),
            Arc::new(NoWait),
        );
        let jobs: Jobs = Rc::default();
        let asks: Asks = Rc::default();
        let quits = Rc::new(Cell::new(0));
        let agreed: Arc<Mutex<Vec<usize>>> = Arc::default();
        let h = Harness::new(1.0, 1000, 640, || AppWindow::new().unwrap());
        let deps = UpdateDeps {
            engine: engine.clone(),
            post,
            current_version: "0.1.0".to_string(),
            ask_install: {
                let asks = asks.clone();
                Rc::new(move |cb| match ask {
                    Ask::Yes => cb(true),
                    Ask::Hold => asks.borrow_mut().push(cb),
                })
            },
            agree: {
                let agreed = agreed.clone();
                let feed = feed.clone();
                Arc::new(move || agreed.lock().unwrap().push(feed.applies()))
            },
            spawn: {
                let jobs = jobs.clone();
                Rc::new(move |f| jobs.borrow_mut().push(f))
            },
            e2e_dir,
            quit: {
                let quits = quits.clone();
                Rc::new(move || quits.set(quits.get() + 1))
            },
        };
        let ctl = UpdateCtl::new(&h.comp, deps);
        Fx {
            h,
            ctl,
            q,
            engine,
            jobs,
            asks,
            quits,
            agreed,
            feed,
        }
    }

    fn live(feed: FakeFeed, ask: Ask) -> Fx {
        build(Mode::Live, feed, None, ask, None)
    }

    impl Fx {
        fn pump(&self) {
            loop {
                let m = self.q.lock().unwrap().pop_front();
                let Some(m) = m else { break };
                if let UiMsg::Update(m) = m {
                    self.ctl.borrow_mut().on_msg(m);
                }
            }
        }

        fn run_jobs(&self) {
            let jobs: Vec<_> = self.jobs.borrow_mut().drain(..).collect();
            for j in jobs {
                j();
            }
        }

        fn offered(&self) {
            self.engine.tick();
            self.pump();
        }

        fn app(&self) -> &AppWindow {
            &self.h.comp
        }

        fn view(&self) -> UpdateView {
            self.ctl.borrow().view.clone()
        }

        fn click_chip(&self) {
            self.ctl.borrow_mut().chip_clicked();
        }

        fn install(&self) {
            self.ctl.borrow_mut().install_pressed();
        }
    }

    fn offering() -> FakeFeed {
        FakeFeed::new().offering("99.0.0", notes())
    }

    #[test]
    fn no_offer_no_chip() {
        let f = live(FakeFeed::new().nothing(), Ask::Yes);
        f.offered();
        assert!(!f.view().chip_shown);
        assert!(!f.app().get_update_chip_shown());
        assert!(!f.app().get_update_open());
    }

    #[test]
    fn an_offer_shows_the_chip_and_does_not_open_the_window() {
        let f = live(offering(), Ask::Yes);
        f.offered();
        assert!(f.app().get_update_chip_shown());
        assert_eq!(f.app().get_update_chip_label(), "Update 99.0.0");
        assert_eq!(f.app().get_update_chip_aria(), "Update to 99.0.0");
        assert_eq!(f.app().get_update_chip_tip(), "See what is new in 99.0.0");
        assert!(!f.app().get_update_open());
    }

    #[test]
    fn a_click_opens_the_window_and_never_installs() {
        let f = live(offering(), Ask::Yes);
        f.offered();
        f.app().invoke_update_chip_clicked();
        assert!(f.app().get_update_open());
        assert_eq!(f.app().get_update_title(), "Update to 99.0.0");
        assert_eq!(f.app().get_update_sub(), "You have 0.1.0");
        assert_eq!(f.engine.calls().installs, 0);
        assert!(f.jobs.borrow().is_empty());
        f.app().invoke_update_cancel();
        assert!(!f.app().get_update_open());
    }

    #[test]
    fn install_with_no_agent_starts_at_once() {
        let f = live(offering(), Ask::Yes);
        f.offered();
        f.click_chip();
        f.install();
        assert_eq!(f.ctl.borrow().flow.phase, Phase::Downloading);
        assert_eq!(f.jobs.borrow().len(), 1);
        assert!(f.app().get_update_running());
        assert!(f.app().get_update_open());
    }

    #[test]
    fn install_with_a_working_agent_asks_first_and_cancel_starts_nothing() {
        let f = live(offering(), Ask::Hold);
        f.offered();
        f.click_chip();
        f.install();
        assert_eq!(f.asks.borrow().len(), 1, "the host was asked");
        assert_eq!(f.ctl.borrow().flow.phase, Phase::Idle);
        assert!(!f.app().get_update_open(), "the window steps aside");
        let cb = f.asks.borrow_mut().pop().unwrap();
        cb(false);
        assert_eq!(f.ctl.borrow().flow.phase, Phase::Idle);
        assert!(f.app().get_update_open(), "and comes back");
        assert!(f.jobs.borrow().is_empty());
        assert_eq!(f.engine.calls().installs, 0);
    }

    #[test]
    fn a_go_ahead_after_the_question_starts_the_install() {
        let f = live(offering(), Ask::Hold);
        f.offered();
        f.click_chip();
        f.install();
        let cb = f.asks.borrow_mut().pop().unwrap();
        cb(true);
        assert_eq!(f.ctl.borrow().flow.phase, Phase::Downloading);
        assert!(f.app().get_update_open());
        assert_eq!(f.jobs.borrow().len(), 1);
    }

    #[test]
    fn the_window_is_hidden_during_the_question_and_returns() {
        let f = live(offering(), Ask::Yes);
        f.offered();
        f.click_chip();
        f.install();
        f.ctl.borrow_mut().hide();
        assert!(!f.app().get_update_open());
        assert_eq!(f.ctl.borrow().flow.phase, Phase::Downloading);
        f.ctl.borrow_mut().show_again();
        assert!(f.app().get_update_open());
        assert!(f.ctl.borrow().window_up());
    }

    #[test]
    fn the_close_question_is_pre_answered_at_the_quit_not_at_the_start() {
        let f = live(offering(), Ask::Yes);
        f.offered();
        f.install();
        assert!(f.agreed.lock().unwrap().is_empty(), "not at the start");
        f.run_jobs();
        assert_eq!(
            *f.agreed.lock().unwrap(),
            [1],
            "after the apply, never before the download ends"
        );
        f.pump();
        assert_eq!(f.quits.get(), 0);
        f.h.advance(399);
        assert_eq!(f.quits.get(), 0);
        f.h.advance(1);
        assert_eq!(f.quits.get(), 1, "the app quits 400 ms after the apply");
    }

    #[test]
    fn progress_fills_the_view_and_100_reads_installing() {
        let f = live(offering(), Ask::Yes);
        f.offered();
        f.install();
        f.ctl.borrow_mut().on_msg(UpdateMsg::Progress(42));
        assert_eq!(f.app().get_update_pct(), 42);
        assert_eq!(f.app().get_update_pct_text(), "42%");
        assert_eq!(f.app().get_update_status(), "Downloading the update");
        assert!(f.app().get_update_active());
        f.ctl.borrow_mut().on_msg(UpdateMsg::Progress(100));
        assert_eq!(
            f.app().get_update_status(),
            "Installing. The app will close and reopen."
        );
        assert_eq!(f.app().get_update_title(), "Updating to 99.0.0");
    }

    #[test]
    fn a_failed_download_says_it_once_in_the_window_and_the_chip_offers_again() {
        let refusing: Arc<dyn Feed> = Arc::new(RefusingFeed::new(notes()));
        let f = build(Mode::Off, FakeFeed::new(), Some(refusing), Ask::Yes, None);
        f.engine.set_offer(Offer {
            version: "99.0.0".into(),
            notes: notes().into(),
            mock: false,
            token: 0,
        });
        f.pump();
        f.click_chip();
        f.install();
        f.run_jobs();
        f.pump();
        assert_eq!(f.app().get_update_status(), INSTALL_FAILED);
        assert_eq!(f.ctl.borrow().flow.phase, Phase::Idle);
        assert!(f.view().chip_shown);
        assert!(f.app().get_update_active());
        assert_eq!(f.app().get_update_cancel_label(), "Close");
        assert_eq!(f.app().get_update_notice(), "", "not said twice");
        assert!(f.app().get_update_install_enabled());
    }

    #[test]
    fn a_failed_download_with_the_window_hidden_is_said_under_the_chip() {
        let refusing: Arc<dyn Feed> = Arc::new(RefusingFeed::new(notes()));
        let f = build(Mode::Off, FakeFeed::new(), Some(refusing), Ask::Yes, None);
        f.engine.set_offer(Offer {
            version: "99.0.0".into(),
            notes: notes().into(),
            mock: false,
            token: 0,
        });
        f.pump();
        f.install();
        f.ctl.borrow_mut().hide();
        f.run_jobs();
        f.pump();
        assert!(!f.view().open);
        assert_eq!(f.view().notice, INSTALL_FAILED);
        assert_eq!(f.app().get_update_notice(), INSTALL_FAILED);
    }

    #[test]
    fn the_notice_goes_after_8000_ms_or_on_a_click() {
        let make = || {
            let refusing: Arc<dyn Feed> = Arc::new(RefusingFeed::new(notes()));
            let f = build(Mode::Off, FakeFeed::new(), Some(refusing), Ask::Yes, None);
            f.engine.set_offer(Offer {
                version: "99.0.0".into(),
                notes: notes().into(),
                mock: false,
                token: 0,
            });
            f.pump();
            f.install();
            f.ctl.borrow_mut().hide();
            f.run_jobs();
            f.pump();
            f
        };
        let f = make();
        assert_eq!(f.app().get_update_notice(), INSTALL_FAILED);
        f.h.advance(7999);
        assert_eq!(f.app().get_update_notice(), INSTALL_FAILED);
        f.h.advance(1);
        assert_eq!(f.app().get_update_notice(), "");
        let g = make();
        g.app().invoke_update_notice_dismissed();
        assert_eq!(g.app().get_update_notice(), "");
    }

    #[test]
    fn a_preview_install_runs_the_fake_and_says_preview_only() {
        let f = build(Mode::Preview, FakeFeed::new(), None, Ask::Yes, None);
        f.engine.start();
        f.pump();
        assert!(f.view().chip_shown);
        assert!(
            f.app()
                .get_update_chip_tip()
                .starts_with("Preview: see what")
        );
        f.click_chip();
        f.install();
        f.run_jobs();
        f.pump();
        assert_eq!(f.app().get_update_status(), PREVIEW_DONE);
        assert_eq!(f.engine.calls().installs, 0);
        assert_eq!(f.engine.calls().checks, 0);
        assert_eq!(f.feed.downloads(), 0);
        assert_eq!(f.feed.applies(), 0);
    }

    #[test]
    fn cancel_while_downloading_aborts_and_the_window_closes_silently() {
        let f = live(offering(), Ask::Yes);
        f.offered();
        f.click_chip();
        f.install();
        f.ctl.borrow_mut().on_msg(UpdateMsg::Progress(30));
        f.app().invoke_update_abort();
        assert_eq!(f.app().get_update_status(), "Cancelling\u{2026}");
        assert!(!f.app().get_update_cancel_enabled());
        f.ctl.borrow_mut().on_msg(UpdateMsg::Settled(false));
        assert!(!f.app().get_update_open(), "a cancel closes the window");
        assert_eq!(f.app().get_update_notice(), "", "and says nothing");
        assert_eq!(f.app().get_update_status(), "");
        assert!(f.view().chip_shown, "the offer stands");
    }

    #[test]
    fn cancel_is_disabled_once_the_installer_has_the_file() {
        let f = live(offering(), Ask::Yes);
        f.offered();
        f.install();
        f.ctl.borrow_mut().on_msg(UpdateMsg::Progress(50));
        assert!(f.app().get_update_cancel_enabled());
        f.ctl.borrow_mut().on_msg(UpdateMsg::Progress(100));
        assert!(!f.app().get_update_cancel_enabled());
        assert_eq!(f.app().get_update_cancel_tip(), "The installer has started");
    }

    #[test]
    fn notes_are_plain_strings() {
        let body = "## What's Changed\n* <script>alert(1)</script> by @x in https://github.com/x/y/pull/1\n* fix: <b>bold</b> & more";
        let f = live(FakeFeed::new().offering("99.0.0", body), Ask::Yes);
        f.offered();
        let parsed = parse_release_notes(Some(body));
        let want: Vec<(String, Vec<String>)> = group_release_notes(&parsed)
            .into_iter()
            .map(|s| (s.heading.to_string(), s.entries))
            .collect();
        assert!(!want.is_empty());
        assert_eq!(f.view().sections, want);
        let joined = want
            .iter()
            .flat_map(|(_, e)| e.iter())
            .cloned()
            .collect::<Vec<_>>()
            .join("|");
        assert!(!joined.contains("</"), "no markup survives: {joined}");
        assert_eq!(f.sections_len(), want.len());
    }

    impl Fx {
        fn sections_len(&self) -> usize {
            use slint::Model;
            self.app().get_update_sections().row_count()
        }
    }

    #[test]
    fn the_view_pushes_notes_max_h_for_the_window_height() {
        let f = live(offering(), Ask::Yes);
        f.ctl.borrow_mut().on_scale_or_size(1000.0, 420.0, 1.0);
        assert!((f.app().get_update_notes_max_h() - 184.8).abs() < 0.01);
        f.ctl.borrow_mut().on_scale_or_size(1000.0, 2000.0, 1.0);
        assert!((f.app().get_update_notes_max_h() - 300.0).abs() < 0.01);
    }

    #[test]
    fn the_e2e_state_file_is_written_only_under_e2e() {
        let none = live(offering(), Ask::Yes);
        none.offered();
        none.run_jobs();
        assert!(none.jobs.borrow().is_empty());

        let dir = tempfile::tempdir().unwrap();
        let f = build(
            Mode::Live,
            offering(),
            None,
            Ask::Yes,
            Some(dir.path().to_path_buf()),
        );
        f.offered();
        f.run_jobs();
        let state: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("update-state.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(state["chip"], true);
        assert_eq!(state["version"], "99.0.0");
        assert_eq!(state["mock"], false);
        assert_eq!(state["open"], false);
        assert_eq!(state["phase"], "idle");
        assert_eq!(state["pct"], 0);
        assert!(state["notice"].is_null());
        let calls: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("update-calls.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(calls["checks"], 1);
        assert_eq!(calls["installs"], 0);
    }

    #[test]
    fn the_e2e_command_file_drives_open_install_cancel_and_is_consumed() {
        let dir = tempfile::tempdir().unwrap();
        let f = build(
            Mode::Live,
            offering(),
            None,
            Ask::Yes,
            Some(dir.path().to_path_buf()),
        );
        f.offered();
        let cmd = dir.path().join("update-command");
        std::fs::write(&cmd, "open").unwrap();
        f.h.advance(250);
        assert!(f.app().get_update_open());
        assert!(!cmd.exists(), "the file is consumed");
        std::fs::write(&cmd, "install").unwrap();
        f.h.advance(250);
        assert_eq!(f.ctl.borrow().flow.phase, Phase::Downloading);
        std::fs::write(&cmd, "cancel").unwrap();
        f.h.advance(250);
        assert!(f.ctl.borrow().flow.aborting);
        std::fs::write(&cmd, "nonsense").unwrap();
        f.h.advance(250);
        assert!(!cmd.exists(), "an unknown word is consumed and ignored");
    }

    #[test]
    fn claiming_a_command_takes_it_whole_and_never_eats_the_next_or_a_half_written_one() {
        let dir = tempfile::tempdir().unwrap();
        let cmd = dir.path().join("update-command");
        assert_eq!(claim_command(dir.path()), None);
        // A writer still filling its temp file is invisible.
        std::fs::write(dir.path().join("update-command.tmp"), "inst").unwrap();
        assert_eq!(claim_command(dir.path()), None);
        std::fs::write(&cmd, "open").unwrap();
        assert_eq!(claim_command(dir.path()).as_deref(), Some("open"));
        assert!(!cmd.exists());
        assert!(!dir.path().join("update-command.claimed").exists());
        // The next command lands after a claim: it is a new file and is read in its turn.
        std::fs::write(&cmd, "install").unwrap();
        assert_eq!(claim_command(dir.path()).as_deref(), Some("install"));
        // A claimed copy left by a crash does not block the next claim.
        std::fs::write(dir.path().join("update-command.claimed"), "old").unwrap();
        std::fs::write(&cmd, "cancel").unwrap();
        assert_eq!(claim_command(dir.path()).as_deref(), Some("cancel"));
    }

    #[test]
    fn view_of_nothing_is_empty_and_sample_notes_group_under_headings() {
        assert_eq!(view_of(&Flow::default(), "1.0.0"), UpdateView::default());
        let flow = reduce(
            &Flow::default(),
            &Action::Available(Offer {
                version: "1.1.0".into(),
                notes: SAMPLE_NOTES.into(),
                mock: true,
                token: 0,
            }),
        );
        let v = view_of(&flow, "1.0.0");
        assert!(!v.sections.is_empty());
        assert_eq!(v.empty_note, "");
    }
}

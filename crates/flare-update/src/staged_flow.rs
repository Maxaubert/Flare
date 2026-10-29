//! Install when idle (2026-10-29): the chip, its menu and the toast, as one pure state machine.
//!
//! The controller feeds it what it sees (a staged build found or gone, a tick with the busy reading, a pick from the
//! chip's menu, a click on Cancel, the end of a swap) and draws [`StagedFlow::view`]. What happens on disk is the
//! controller's: this answers with a [`Do`].
//!
//! - A ready build shows the chip. By default it installs when idle: after [`crate::idle::IDLE_HOLD_MS`] of quiet a
//!   10 s toast says so, with Cancel; then the swap runs.
//! - "Install now" asks first (the controller raises the close question when an agent is at work), then runs the same
//!   toast. "Not today" hides the chip until the next start (a new build shows it again). "Discard" removes it.
//! - A countdown the idle rule started stops when anything gets busy again; one started by hand does not.
//! - Cancel stops the countdown and leaves the build waiting for a click (no new idle install this run until
//!   "Install when idle" is picked again).

use crate::copy;
use crate::idle::{IDLE_HOLD_MS, IdleClock};

/// How long the toast counts down before the install.
pub const COUNTDOWN_MS: u64 = 10_000;
/// How long a one-line notice ("Installed abc1234") stays.
pub const NOTICE_MS: u64 = 8_000;

/// The flow's waits; the e2e shortens them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Times {
    pub hold_ms: u64,
    pub countdown_ms: u64,
    pub notice_ms: u64,
}

impl Default for Times {
    fn default() -> Self {
        Self {
            hold_ms: IDLE_HOLD_MS,
            countdown_ms: COUNTDOWN_MS,
            notice_ms: NOTICE_MS,
        }
    }
}

/// A staged build that passed its hash check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ready {
    /// The short commit.
    pub commit: String,
    /// One string per stage ([`crate::staged::Manifest::signature`]).
    pub sig: String,
}

/// How the build is to be installed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// On its own, once idle (the default).
    WhenIdle,
    /// Only from the menu.
    Manual,
}

/// Where an install stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Waiting,
    /// The toast is up; the install starts at `ends`.
    Countdown {
        ends: u64,
        by_hand: bool,
    },
    /// The swap runs; a good one ends in the restart.
    Installing,
}

/// A row of the chip's menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    InstallNow,
    WhenIdle,
    NotToday,
    Discard,
}

impl Pick {
    pub const ALL: [Pick; 4] = [
        Pick::InstallNow,
        Pick::WhenIdle,
        Pick::NotToday,
        Pick::Discard,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Pick::InstallNow => copy::STAGED_INSTALL_NOW,
            Pick::WhenIdle => copy::STAGED_WHEN_IDLE,
            Pick::NotToday => copy::STAGED_NOT_TODAY,
            Pick::Discard => copy::STAGED_DISCARD,
        }
    }
}

/// What the controller is to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Do {
    Nothing,
    /// "Install now": ask the close question if an agent is at work, then call [`StagedFlow::start_countdown`].
    Ask,
    /// Run the swap on a worker, then report with [`StagedFlow::swapped`].
    Swap,
    /// Remove `staged\`.
    Discard,
}

/// What the window shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StagedView {
    pub chip_shown: bool,
    pub chip_label: String,
    pub chip_aria: String,
    pub chip_tip: String,
    /// The toast's line; empty hides it.
    pub toast: String,
    /// The toast's button; empty for none.
    pub toast_button: String,
    /// The menu's rows: label and whether it can be picked.
    pub menu: Vec<(Pick, bool)>,
}

#[derive(Clone, Debug)]
pub struct StagedFlow {
    ready: Option<Ready>,
    mode: Mode,
    phase: Phase,
    /// The stage "Not today" hid.
    hidden: Option<String>,
    idle: IdleClock,
    notice: Option<(String, u64)>,
    times: Times,
}

impl StagedFlow {
    pub fn new(times: Times) -> Self {
        Self {
            ready: None,
            mode: Mode::WhenIdle,
            phase: Phase::Waiting,
            hidden: None,
            idle: IdleClock::default(),
            notice: None,
            times,
        }
    }

    pub fn ready(&self) -> Option<&Ready> {
        self.ready.as_ref()
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// The chip shows: a ready build that "Not today" did not hide.
    pub fn shown(&self) -> bool {
        self.ready
            .as_ref()
            .is_some_and(|r| self.hidden.as_deref() != Some(r.sig.as_str()))
    }

    /// A check of `staged\` answered. The same stage again changes nothing; a new one starts over (installs when
    /// idle). Nothing changes while the swap runs.
    pub fn found(&mut self, r: Option<Ready>) {
        if self.phase == Phase::Installing || self.ready == r {
            return;
        }
        self.ready = r;
        self.mode = Mode::WhenIdle;
        self.phase = Phase::Waiting;
        self.idle.reset();
    }

    /// A row of the chip's menu was picked.
    pub fn pick(&mut self, p: Pick) -> Do {
        if self.ready.is_none() || self.phase == Phase::Installing {
            return Do::Nothing;
        }
        match p {
            Pick::InstallNow => Do::Ask,
            Pick::WhenIdle => {
                self.mode = Mode::WhenIdle;
                self.idle.reset();
                Do::Nothing
            }
            Pick::NotToday => {
                self.hidden = self.ready.as_ref().map(|r| r.sig.clone());
                self.phase = Phase::Waiting;
                Do::Nothing
            }
            Pick::Discard => {
                self.ready = None;
                self.phase = Phase::Waiting;
                Do::Discard
            }
        }
    }

    /// The toast starts counting down (the idle rule held, or "Install now" was answered yes).
    pub fn start_countdown(&mut self, now: u64, by_hand: bool) {
        if !self.shown() || self.phase != Phase::Waiting {
            return;
        }
        self.notice = None;
        self.phase = Phase::Countdown {
            ends: now + self.times.countdown_ms,
            by_hand,
        };
    }

    /// Cancel on the toast: back to waiting, and only a click installs it now.
    pub fn cancel(&mut self) {
        if matches!(self.phase, Phase::Countdown { .. }) {
            self.phase = Phase::Waiting;
            self.mode = Mode::Manual;
            self.idle.reset();
        }
    }

    /// One reading of the clock and of the busy state.
    pub fn tick(&mut self, now: u64, busy: bool) -> Do {
        if self.notice.as_ref().is_some_and(|(_, until)| now >= *until) {
            self.notice = None;
        }
        match self.phase {
            Phase::Waiting => {
                let idle = self.idle.observe(busy, now, self.times.hold_ms);
                if idle && self.shown() && self.mode == Mode::WhenIdle {
                    self.start_countdown(now, false);
                }
                Do::Nothing
            }
            Phase::Countdown { by_hand: false, .. } if busy => {
                self.phase = Phase::Waiting;
                self.idle.observe(true, now, self.times.hold_ms);
                Do::Nothing
            }
            Phase::Countdown { ends, .. } if now >= ends => {
                self.phase = Phase::Installing;
                Do::Swap
            }
            _ => Do::Nothing,
        }
    }

    /// The swap ended. A good one is followed by the restart; a failed one put everything back.
    pub fn swapped(&mut self, ok: bool, now: u64) {
        if self.phase != Phase::Installing {
            return;
        }
        if ok {
            return;
        }
        self.phase = Phase::Waiting;
        self.mode = Mode::Manual;
        self.idle.reset();
        self.say(copy::STAGED_FAILED, now);
    }

    /// A one-line notice in the toast for [`NOTICE_MS`] ("Installed abc1234").
    pub fn say(&mut self, text: &str, now: u64) {
        self.notice = Some((text.to_string(), now + self.times.notice_ms));
    }

    /// Whether the controller's clock must run: a build that may install on its own, a countdown or a notice.
    pub fn needs_tick(&self) -> bool {
        let waits = self.shown() && self.mode == Mode::WhenIdle && self.phase == Phase::Waiting;
        waits || matches!(self.phase, Phase::Countdown { .. }) || self.notice.is_some()
    }

    pub fn view(&self) -> StagedView {
        let mut v = StagedView::default();
        if let Some(r) = self.ready.as_ref().filter(|_| self.shown()) {
            let idle = self.mode == Mode::WhenIdle;
            v.chip_shown = true;
            v.chip_label = copy::STAGED_CHIP.to_string();
            v.chip_aria = copy::staged_chip_aria(idle);
            v.chip_tip = copy::staged_chip_tip(&r.commit, idle);
            v.menu = Pick::ALL
                .iter()
                .map(|p| (*p, !(idle && *p == Pick::WhenIdle)))
                .collect();
        }
        match self.phase {
            Phase::Countdown { .. } => {
                v.toast = copy::STAGED_COUNTDOWN.to_string();
                v.toast_button = copy::STAGED_CANCEL.to_string();
            }
            Phase::Installing => v.toast = copy::STAGED_COUNTDOWN.to_string(),
            Phase::Waiting => {
                if let Some((text, _)) = &self.notice {
                    v.toast = text.clone();
                }
            }
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready(sig: &str) -> Option<Ready> {
        Some(Ready {
            commit: "abc1234".into(),
            sig: sig.into(),
        })
    }

    fn flow() -> StagedFlow {
        let mut f = StagedFlow::new(Times::default());
        f.found(ready("s1"));
        f
    }

    #[test]
    fn nothing_staged_shows_nothing_and_needs_no_clock() {
        let f = StagedFlow::new(Times::default());
        assert_eq!(f.view(), StagedView::default());
        assert!(!f.needs_tick());
    }

    #[test]
    fn a_ready_build_shows_the_chip_with_its_menu() {
        let f = flow();
        let v = f.view();
        assert!(v.chip_shown);
        assert_eq!(v.chip_label, "Build ready");
        assert_eq!(v.chip_aria, "Build ready, installs when idle");
        assert_eq!(
            v.chip_tip,
            "Build abc1234 installs once no agent has worked for a minute"
        );
        assert_eq!(
            v.menu,
            vec![
                (Pick::InstallNow, true),
                (Pick::WhenIdle, false),
                (Pick::NotToday, true),
                (Pick::Discard, true)
            ]
        );
        assert!(v.toast.is_empty());
        assert!(f.needs_tick());
    }

    #[test]
    fn a_quiet_minute_starts_the_toast_and_ten_seconds_later_the_swap() {
        let mut f = flow();
        assert_eq!(f.tick(0, false), Do::Nothing);
        assert_eq!(f.tick(59_999, false), Do::Nothing);
        assert_eq!(f.phase(), Phase::Waiting);
        assert_eq!(f.tick(60_000, false), Do::Nothing);
        assert_eq!(
            f.phase(),
            Phase::Countdown {
                ends: 70_000,
                by_hand: false
            }
        );
        let v = f.view();
        assert_eq!(v.toast, "Installing the new build and restarting");
        assert_eq!(v.toast_button, "Cancel");
        assert_eq!(f.tick(69_999, false), Do::Nothing);
        assert_eq!(f.tick(70_000, false), Do::Swap);
        assert_eq!(f.phase(), Phase::Installing);
        assert_eq!(f.view().toast_button, "", "no Cancel once the swap runs");
    }

    #[test]
    fn a_working_agent_holds_it_and_starts_the_minute_again() {
        let mut f = flow();
        f.tick(0, false);
        f.tick(50_000, true);
        f.tick(50_001, false);
        f.tick(110_000, false);
        assert_eq!(f.phase(), Phase::Waiting, "only 59.999 s of quiet");
        f.tick(110_001, false);
        assert!(matches!(f.phase(), Phase::Countdown { .. }));
    }

    #[test]
    fn work_during_an_idle_countdown_stops_it_but_not_one_started_by_hand() {
        let mut f = flow();
        f.tick(0, false);
        f.tick(60_000, false);
        assert_eq!(f.tick(65_000, true), Do::Nothing);
        assert_eq!(f.phase(), Phase::Waiting);
        assert!(f.view().toast.is_empty());

        let mut g = flow();
        assert_eq!(g.pick(Pick::InstallNow), Do::Ask);
        g.start_countdown(1_000, true);
        assert_eq!(g.tick(5_000, true), Do::Nothing);
        assert_eq!(g.tick(11_000, true), Do::Swap);
    }

    #[test]
    fn cancel_stops_the_countdown_and_waits_for_a_click() {
        let mut f = flow();
        f.tick(0, false);
        f.tick(60_000, false);
        f.cancel();
        assert_eq!(f.phase(), Phase::Waiting);
        assert_eq!(f.mode(), Mode::Manual);
        assert_eq!(f.tick(500_000, false), Do::Nothing);
        assert_eq!(f.phase(), Phase::Waiting, "no idle install after a cancel");
        let v = f.view();
        assert_eq!(v.chip_aria, "Build ready");
        assert_eq!(v.chip_tip, "Build abc1234 is ready to install");
        assert!(v.menu.contains(&(Pick::WhenIdle, true)));
        assert!(!f.needs_tick());
        f.pick(Pick::WhenIdle);
        f.tick(500_001, false);
        f.tick(560_001, false);
        assert!(matches!(f.phase(), Phase::Countdown { .. }));
    }

    #[test]
    fn not_today_hides_this_build_and_a_new_stage_shows_again() {
        let mut f = flow();
        f.pick(Pick::NotToday);
        assert!(!f.view().chip_shown);
        f.tick(0, false);
        f.tick(100_000, false);
        assert_eq!(
            f.phase(),
            Phase::Waiting,
            "a hidden build never installs on its own"
        );
        f.found(ready("s1"));
        assert!(!f.view().chip_shown);
        f.found(ready("s2"));
        assert!(f.view().chip_shown);
    }

    #[test]
    fn discard_asks_the_controller_to_remove_it() {
        let mut f = flow();
        assert_eq!(f.pick(Pick::Discard), Do::Discard);
        assert!(!f.view().chip_shown);
        assert_eq!(f.pick(Pick::InstallNow), Do::Nothing);
    }

    #[test]
    fn a_failed_swap_says_so_and_leaves_the_build_for_a_click() {
        let mut f = flow();
        f.pick(Pick::InstallNow);
        f.start_countdown(0, true);
        assert_eq!(f.tick(10_000, false), Do::Swap);
        f.found(None);
        assert!(f.ready().is_some(), "nothing changes while the swap runs");
        f.swapped(false, 10_500);
        assert_eq!(f.phase(), Phase::Waiting);
        assert_eq!(f.mode(), Mode::Manual);
        assert_eq!(
            f.view().toast,
            "The build could not be installed; nothing changed"
        );
        f.tick(18_499, false);
        assert!(!f.view().toast.is_empty());
        f.tick(18_500, false);
        assert!(f.view().toast.is_empty());
    }

    #[test]
    fn a_notice_shows_alone_and_goes() {
        let mut f = StagedFlow::new(Times::default());
        f.say("Installed abc1234", 0);
        assert_eq!(f.view().toast, "Installed abc1234");
        assert_eq!(f.view().toast_button, "");
        assert!(f.needs_tick());
        f.tick(8_000, false);
        assert!(f.view().toast.is_empty());
        assert!(!f.needs_tick());
    }

    #[test]
    fn the_same_stage_found_again_keeps_the_mode() {
        let mut f = flow();
        f.tick(0, false);
        f.tick(60_000, false);
        f.cancel();
        f.found(ready("s1"));
        assert_eq!(f.mode(), Mode::Manual);
        f.found(ready("s2"));
        assert_eq!(f.mode(), Mode::WhenIdle);
    }

    #[test]
    fn the_menu_words() {
        let labels: Vec<&str> = Pick::ALL.iter().map(|p| p.label()).collect();
        assert_eq!(
            labels,
            ["Install now", "Install when idle", "Not today", "Discard"]
        );
    }
}

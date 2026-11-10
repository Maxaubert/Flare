//! Build ready (2026-10-29, one click since 2026-11-10): the chip and the toast, as one pure state machine.
//!
//! The controller feeds it what it sees (a staged build found or gone, a tick, a click on the chip, a click on
//! Cancel, the end of a swap) and draws [`StagedFlow::view`]. What happens on disk is the controller's: this answers
//! with a [`Do`].
//!
//! - A ready build shows the chip. It has no menu: a click is "install now" (rule 258). The controller asks the close question
//!   first when an agent is at work, then the 10 s toast with Cancel runs, then the swap.
//! - Nothing installs on its own. A build stays on offer until it is installed or a new stage replaces it.
//! - Cancel stops the countdown and leaves the build waiting for the next click.

use crate::copy;

/// How long the toast counts down before the install.
pub const COUNTDOWN_MS: u64 = 10_000;
/// How long a one-line notice ("Installed abc1234") stays.
pub const NOTICE_MS: u64 = 8_000;

/// The flow's waits; the e2e shortens them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Times {
    pub countdown_ms: u64,
    pub notice_ms: u64,
}

impl Default for Times {
    fn default() -> Self {
        Self {
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

/// Where an install stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Waiting,
    /// The toast is up; the install starts at `ends`.
    Countdown {
        ends: u64,
    },
    /// The swap runs; a good one ends in the restart.
    Installing,
}

/// What the controller is to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Do {
    Nothing,
    /// The chip was clicked: ask the close question if an agent is at work, then call
    /// [`StagedFlow::start_countdown`].
    Ask,
    /// Run the swap on a worker, then report with [`StagedFlow::swapped`].
    Swap,
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
}

#[derive(Clone, Debug)]
pub struct StagedFlow {
    ready: Option<Ready>,
    phase: Phase,
    notice: Option<(String, u64)>,
    times: Times,
}

impl StagedFlow {
    pub fn new(times: Times) -> Self {
        Self {
            ready: None,
            phase: Phase::Waiting,
            notice: None,
            times,
        }
    }

    pub fn ready(&self) -> Option<&Ready> {
        self.ready.as_ref()
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// The chip shows: a build is ready.
    pub fn shown(&self) -> bool {
        self.ready.is_some()
    }

    /// A check of `staged\` answered. The same stage again changes nothing; a new one (or none) starts over.
    /// Nothing changes while the swap runs.
    pub fn found(&mut self, r: Option<Ready>) {
        if self.phase == Phase::Installing || self.ready == r {
            return;
        }
        self.ready = r;
        self.phase = Phase::Waiting;
    }

    /// The chip was clicked: install now (once the controller has asked, if it must). A click while the toast is up
    /// or the swap runs does nothing.
    pub fn click(&mut self) -> Do {
        if self.ready.is_none() || self.phase != Phase::Waiting {
            return Do::Nothing;
        }
        Do::Ask
    }

    /// The toast starts counting down (the click was answered yes, or needed no question).
    pub fn start_countdown(&mut self, now: u64) {
        if !self.shown() || self.phase != Phase::Waiting {
            return;
        }
        self.notice = None;
        self.phase = Phase::Countdown {
            ends: now + self.times.countdown_ms,
        };
    }

    /// Cancel on the toast: back to waiting for a click.
    pub fn cancel(&mut self) {
        if matches!(self.phase, Phase::Countdown { .. }) {
            self.phase = Phase::Waiting;
        }
    }

    /// The controller stops a countdown to ask first (an agent started working after a click with none at work):
    /// back to waiting.
    pub fn pause(&mut self) {
        self.cancel();
    }

    /// The controller took a good swap back before the restart (an agent got busy while it ran): back to waiting.
    pub fn taken_back(&mut self) {
        if self.phase == Phase::Installing {
            self.phase = Phase::Waiting;
        }
    }

    /// One reading of the clock.
    pub fn tick(&mut self, now: u64) -> Do {
        if self.notice.as_ref().is_some_and(|(_, until)| now >= *until) {
            self.notice = None;
        }
        match self.phase {
            Phase::Countdown { ends } if now >= ends => {
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
        self.say(copy::STAGED_FAILED, now);
    }

    /// A one-line notice in the toast for [`NOTICE_MS`] ("Installed abc1234").
    pub fn say(&mut self, text: &str, now: u64) {
        self.notice = Some((text.to_string(), now + self.times.notice_ms));
    }

    /// Whether the controller's clock must run: a countdown or a notice. A build on offer needs none.
    pub fn needs_tick(&self) -> bool {
        matches!(self.phase, Phase::Countdown { .. }) || self.notice.is_some()
    }

    pub fn view(&self) -> StagedView {
        let mut v = StagedView::default();
        if let Some(r) = self.ready.as_ref() {
            v.chip_shown = true;
            v.chip_label = copy::STAGED_CHIP.to_string();
            v.chip_aria = copy::STAGED_CHIP_ARIA.to_string();
            v.chip_tip = copy::staged_chip_tip(&r.commit);
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
        let mut f = StagedFlow::new(Times::default());
        assert_eq!(f.view(), StagedView::default());
        assert!(!f.needs_tick());
        assert_eq!(f.click(), Do::Nothing);
    }

    #[test]
    fn a_ready_build_shows_the_chip_and_needs_no_clock() {
        let f = flow();
        let v = f.view();
        assert!(v.chip_shown);
        assert_eq!(v.chip_label, "Build ready");
        assert_eq!(v.chip_aria, "Build ready, click to install and restart");
        assert_eq!(v.chip_tip, "Build abc1234: click to install and restart");
        assert!(v.toast.is_empty());
        assert!(!f.needs_tick(), "nothing waits for idle");
    }

    #[test]
    fn nothing_installs_on_its_own() {
        let mut f = flow();
        for t in (0..=600_000).step_by(1_000) {
            assert_eq!(f.tick(t), Do::Nothing);
        }
        assert_eq!(f.phase(), Phase::Waiting);
        assert!(f.view().chip_shown, "the build stays on offer");
    }

    #[test]
    fn a_click_asks_then_the_toast_counts_ten_seconds_then_the_swap() {
        let mut f = flow();
        assert_eq!(f.click(), Do::Ask);
        assert_eq!(
            f.phase(),
            Phase::Waiting,
            "nothing until the controller says go"
        );
        f.start_countdown(1_000);
        assert_eq!(f.phase(), Phase::Countdown { ends: 11_000 });
        let v = f.view();
        assert_eq!(v.toast, "Installing the new build and restarting");
        assert_eq!(v.toast_button, "Cancel");
        assert!(f.needs_tick());
        assert_eq!(
            f.click(),
            Do::Nothing,
            "a second click while the toast is up"
        );
        assert_eq!(f.tick(10_999), Do::Nothing);
        assert_eq!(f.tick(11_000), Do::Swap);
        assert_eq!(f.phase(), Phase::Installing);
        assert_eq!(f.view().toast_button, "", "no Cancel once the swap runs");
        assert_eq!(f.click(), Do::Nothing);
    }

    #[test]
    fn cancel_stops_the_countdown_and_the_build_waits_for_the_next_click() {
        let mut f = flow();
        f.click();
        f.start_countdown(0);
        f.cancel();
        assert_eq!(f.phase(), Phase::Waiting);
        assert!(f.view().toast.is_empty());
        assert!(f.view().chip_shown);
        assert_eq!(f.tick(500_000), Do::Nothing);
        assert!(!f.needs_tick());
        assert_eq!(f.click(), Do::Ask);
    }

    #[test]
    fn a_new_stage_replaces_the_offer_and_the_same_one_changes_nothing() {
        let mut f = flow();
        f.start_countdown(0);
        f.found(ready("s1"));
        assert!(matches!(f.phase(), Phase::Countdown { .. }));
        f.found(ready("s2"));
        assert_eq!(f.phase(), Phase::Waiting);
        assert_eq!(f.ready().unwrap().sig, "s2");
        f.found(None);
        assert!(!f.view().chip_shown);
    }

    #[test]
    fn a_failed_swap_says_so_and_leaves_the_build_for_a_click() {
        let mut f = flow();
        f.click();
        f.start_countdown(0);
        assert_eq!(f.tick(10_000), Do::Swap);
        f.found(None);
        assert!(f.ready().is_some(), "nothing changes while the swap runs");
        f.swapped(false, 10_500);
        assert_eq!(f.phase(), Phase::Waiting);
        assert_eq!(
            f.view().toast,
            "The build could not be installed; nothing changed"
        );
        f.tick(18_499);
        assert!(!f.view().toast.is_empty());
        f.tick(18_500);
        assert!(f.view().toast.is_empty());
        assert_eq!(f.click(), Do::Ask);
    }

    #[test]
    fn a_notice_shows_alone_and_goes() {
        let mut f = StagedFlow::new(Times::default());
        f.say("Installed abc1234", 0);
        assert_eq!(f.view().toast, "Installed abc1234");
        assert_eq!(f.view().toast_button, "");
        assert!(f.needs_tick());
        f.tick(8_000);
        assert!(f.view().toast.is_empty());
        assert!(!f.needs_tick());
    }

    #[test]
    fn a_paused_countdown_or_a_swap_taken_back_waits_for_a_click_again() {
        let mut f = flow();
        f.start_countdown(0);
        f.pause();
        assert_eq!(f.phase(), Phase::Waiting);
        f.start_countdown(0);
        assert_eq!(f.tick(10_000), Do::Swap);
        f.taken_back();
        assert_eq!(f.phase(), Phase::Waiting);
        assert!(f.view().toast.is_empty());
        assert_eq!(f.tick(500_000), Do::Nothing, "never on its own");
        assert_eq!(f.click(), Do::Ask);
    }
}

//! M9 Task 7: the update flow reducer, the port of Prism Terminal's `updateFlow.ts`.
//!
//! What the update chip and its window are doing, as one pure reducer. `reduce` takes the state and one
//! action and returns the next state; nothing else. The rules are Prism Terminal's, and
//! `tests/fixtures/flow.json` (its own answers over 200 sequences) is the golden check.

use crate::types::{Offer, Phase};

/// Said when a preview's fake install finishes.
pub const PREVIEW_DONE: &str = "Preview only: nothing was installed";
/// Said when a real download failed.
pub const INSTALL_FAILED: &str = "The update could not be downloaded. Nothing was installed.";

/// What the chip and window show. `info: None` means no chip at all. `pct` is 0 to 100, whole, and only
/// ever rising within one install.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Flow {
    pub info: Option<Offer>,
    pub phase: Phase,
    pub pct: u8,
    pub open: bool,
    pub notice: Option<String>,
    pub aborting: bool,
}

/// One thing that happened: an offer, a click, the user backing out, the host hiding the window, an
/// install starting, progress, a cancel of the download, the bridge's answer, or a dismissed notice.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Available(Offer),
    Open,
    Cancel,
    Hide,
    Install,
    Progress(f64),
    Abort,
    Settled { ok: bool },
    Dismiss,
}

/// JS `Math.round`: halves go up (toward +infinity). Exact for every finite double.
fn js_round(n: f64) -> f64 {
    let floor = n.floor();
    if n - floor >= 0.5 { floor + 1.0 } else { floor }
}

/// `Number.isFinite(n) ? clamp(round(n), 0, 100) : 0`: a NaN or an infinity reads as 0, as in the spec.
fn whole(n: f64) -> u8 {
    if !n.is_finite() {
        return 0;
    }
    // Clamped to 0..=100 first, so the cast cannot truncate.
    js_round(n).clamp(0.0, 100.0) as u8
}

/// The next state. Pure: the same input always gives the same output and `s` is not changed.
pub fn reduce(s: &Flow, a: &Action) -> Flow {
    let mut n = s.clone();
    match a {
        Action::Available(offer) => {
            // An offer that arrives mid-install is not swapped in under the bar.
            if s.phase == Phase::Idle {
                n.info = Some(offer.clone());
            }
        }
        Action::Open => {
            if s.info.is_some() && !s.open {
                n.open = true;
                // Opened fresh, the last ending is history. Over a running install, only the window moves.
                if s.phase == Phase::Idle {
                    n.notice = None;
                }
            }
        }
        Action::Cancel => {
            // Not dismissable while it runs: a window closed over a download would hide the install.
            if s.open && s.phase == Phase::Idle {
                n.open = false;
                n.notice = None;
            }
        }
        Action::Hide => n.open = false,
        Action::Install => {
            if s.info.is_some() && s.phase == Phase::Idle {
                n.open = true;
                n.phase = Phase::Downloading;
                n.pct = 0;
                n.notice = None;
                n.aborting = false;
            }
        }
        Action::Progress(p) => {
            if s.phase != Phase::Idle {
                n.pct = s.pct.max(whole(*p));
                if n.pct >= 100 {
                    n.phase = Phase::Installing;
                }
            }
        }
        Action::Abort => {
            // Only the download can be cancelled. At 100 the installer has the file.
            if s.phase == Phase::Downloading && !s.aborting {
                n.aborting = true;
            }
        }
        Action::Settled { ok } => {
            if s.phase != Phase::Idle && !*ok {
                if s.aborting {
                    // A cancel is not a failure: the window goes and nothing is said.
                    n.phase = Phase::Idle;
                    n.pct = 0;
                    n.open = false;
                    n.notice = None;
                    n.aborting = false;
                } else {
                    // `open` is left as it is: said in the window if it is up, under the chip if not.
                    let mock = s.info.as_ref().is_some_and(|o| o.mock);
                    n.phase = Phase::Idle;
                    n.pct = 0;
                    n.aborting = false;
                    n.notice = Some(if mock { PREVIEW_DONE } else { INSTALL_FAILED }.to_string());
                }
            }
        }
        Action::Dismiss => n.notice = None,
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn real() -> Offer {
        Offer {
            version: "0.5.0".into(),
            notes: "* x".into(),
            mock: false,
            token: 1,
        }
    }

    fn mock() -> Offer {
        Offer {
            version: "0.5.0".into(),
            notes: "* x".into(),
            mock: true,
            token: 0,
        }
    }

    fn run(from: &Flow, actions: &[Action]) -> Flow {
        actions.iter().fold(from.clone(), |s, a| reduce(&s, a))
    }

    fn offered_with(o: Offer) -> Flow {
        run(&Flow::default(), &[Action::Available(o)])
    }

    fn offered() -> Flow {
        offered_with(real())
    }

    fn progress(pct: f64) -> Action {
        Action::Progress(pct)
    }

    #[test]
    fn starts_with_nothing_no_chip_no_window() {
        assert_eq!(
            Flow::default(),
            Flow {
                info: None,
                phase: Phase::Idle,
                pct: 0,
                open: false,
                notice: None,
                aborting: false
            }
        );
    }

    #[test]
    fn an_offer_shows_the_chip_and_does_not_open_the_window_by_itself() {
        let s = offered();
        assert_eq!(s.info, Some(real()));
        assert!(!s.open);
    }

    #[test]
    fn a_click_opens_the_window_cancel_closes_it_and_changes_nothing_else() {
        let open = run(&offered(), &[Action::Open]);
        assert!(open.open);
        assert_eq!(run(&open, &[Action::Cancel]), offered());
    }

    #[test]
    fn cannot_open_a_window_about_nothing() {
        assert!(!run(&Flow::default(), &[Action::Open]).open);
    }

    #[test]
    fn takes_a_newer_offer_while_idle_the_window_staying_as_it_was() {
        let newer = Offer {
            version: "0.6.0".into(),
            ..real()
        };
        let s = run(
            &offered(),
            &[Action::Open, Action::Available(newer.clone())],
        );
        assert_eq!(s.info, Some(newer));
        assert!(s.open);
    }

    #[test]
    fn install_keeps_the_window_up_and_the_bar_starts_from_zero() {
        let s = run(&offered(), &[Action::Open, Action::Install]);
        assert!(s.open);
        assert_eq!(s.phase, Phase::Downloading);
        assert_eq!(s.pct, 0);
        assert_eq!(s.notice, None);
    }

    #[test]
    fn brings_the_window_up_for_an_install_started_with_it_hidden() {
        let s = run(&offered(), &[Action::Open, Action::Hide, Action::Install]);
        assert!(s.open);
        assert_eq!(s.phase, Phase::Downloading);
    }

    #[test]
    fn cannot_be_put_away_by_the_user_while_it_runs() {
        let s = run(&offered(), &[Action::Open, Action::Install, progress(10.0)]);
        assert_eq!(run(&s, &[Action::Cancel]), s);
        let installing = run(&s, &[progress(100.0)]);
        assert_eq!(run(&installing, &[Action::Cancel]), installing);
    }

    #[test]
    fn the_host_can_hide_it_while_it_runs_the_install_carries_on_and_it_comes_back() {
        let s = run(&offered(), &[Action::Install, progress(10.0), Action::Hide]);
        assert!(!s.open);
        assert_eq!(s.phase, Phase::Downloading);
        assert_eq!(s.pct, 10);
        let back = run(&s, &[progress(40.0), Action::Open]);
        assert!(back.open);
        assert_eq!(back.phase, Phase::Downloading);
        assert_eq!(back.pct, 40);
    }

    #[test]
    fn progress_fills_the_bar_and_100_is_installing() {
        let s = run(&offered(), &[Action::Install, progress(42.0)]);
        assert_eq!(s.phase, Phase::Downloading);
        assert_eq!(s.pct, 42);
        let done = run(&s, &[progress(100.0)]);
        assert_eq!(done.phase, Phase::Installing);
        assert_eq!(done.pct, 100);
    }

    #[test]
    fn reads_a_percentage_defensively() {
        let s = run(&offered(), &[Action::Install]);
        assert_eq!(run(&s, &[progress(250.0)]).pct, 100);
        assert_eq!(run(&s, &[progress(-5.0)]).pct, 0);
        assert_eq!(run(&s, &[progress(f64::NAN)]).pct, 0);
        assert_eq!(run(&s, &[progress(f64::INFINITY)]).pct, 0);
        assert_eq!(run(&s, &[progress(41.6)]).pct, 42);
    }

    #[test]
    fn never_runs_the_bar_backwards() {
        let s = run(
            &offered(),
            &[Action::Install, progress(60.0), progress(30.0)],
        );
        assert_eq!(s.pct, 60);
    }

    #[test]
    fn ignores_progress_nobody_asked_for() {
        assert_eq!(run(&offered(), &[progress(50.0)]), offered());
    }

    #[test]
    fn cannot_be_started_twice_and_opening_a_window_that_is_up_changes_nothing() {
        let s = run(&offered(), &[Action::Install, progress(10.0)]);
        assert_eq!(run(&s, &[Action::Open]), s);
        assert_eq!(run(&s, &[Action::Install]), s);
    }

    #[test]
    fn holds_the_offer_it_is_installing_a_newer_one_waits_for_the_next_launch() {
        let newer = Offer {
            version: "9.9.9".into(),
            ..real()
        };
        let s = run(&offered(), &[Action::Install, Action::Available(newer)]);
        assert_eq!(s.info, Some(real()));
    }

    #[test]
    fn cannot_install_nothing() {
        assert_eq!(run(&Flow::default(), &[Action::Install]), Flow::default());
    }

    #[test]
    fn a_success_changes_nothing_the_app_is_quitting_under_the_installer() {
        let s = run(&offered(), &[Action::Install, progress(100.0)]);
        assert_eq!(run(&s, &[Action::Settled { ok: true }]), s);
    }

    #[test]
    fn a_failure_goes_back_to_idle_and_says_so_in_the_window() {
        let s = run(
            &offered(),
            &[
                Action::Install,
                progress(30.0),
                Action::Settled { ok: false },
            ],
        );
        assert_eq!(s.phase, Phase::Idle);
        assert_eq!(s.pct, 0);
        assert!(s.open);
        assert_eq!(s.notice.as_deref(), Some(INSTALL_FAILED));
        assert_eq!(s.info, Some(real()));
    }

    #[test]
    fn a_failure_with_the_window_hidden_is_said_under_the_chip_instead() {
        let s = run(
            &offered(),
            &[Action::Install, Action::Hide, Action::Settled { ok: false }],
        );
        assert_eq!(s.phase, Phase::Idle);
        assert!(!s.open);
        assert_eq!(s.notice.as_deref(), Some(INSTALL_FAILED));
    }

    #[test]
    fn closing_the_window_that_said_how_it_ended_clears_the_line() {
        let s = run(
            &offered(),
            &[
                Action::Install,
                Action::Settled { ok: false },
                Action::Cancel,
            ],
        );
        assert_eq!(s, offered());
    }

    #[test]
    fn a_preview_goes_back_to_idle_and_says_it_was_one() {
        let s = run(
            &offered_with(mock()),
            &[
                Action::Install,
                progress(100.0),
                Action::Settled { ok: false },
            ],
        );
        assert_eq!(s.phase, Phase::Idle);
        assert_eq!(s.pct, 0);
        assert_eq!(s.notice.as_deref(), Some(PREVIEW_DONE));
        assert_eq!(PREVIEW_DONE, "Preview only: nothing was installed");
    }

    #[test]
    fn the_line_goes_when_dismissed_and_when_the_window_is_opened_again() {
        let s = run(
            &offered_with(mock()),
            &[Action::Install, Action::Hide, Action::Settled { ok: false }],
        );
        assert_eq!(run(&s, &[Action::Dismiss]).notice, None);
        let opened = run(&s, &[Action::Open]);
        assert_eq!(opened.notice, None);
        assert!(opened.open);
    }

    #[test]
    fn can_be_run_again_straight_from_the_window_that_said_it_ended() {
        let s = run(
            &offered_with(mock()),
            &[
                Action::Install,
                Action::Settled { ok: false },
                Action::Install,
            ],
        );
        assert_eq!(s.phase, Phase::Downloading);
        assert_eq!(s.pct, 0);
        assert_eq!(s.notice, None);
        assert!(s.open);
    }

    #[test]
    fn ignores_an_ending_with_nothing_running() {
        assert_eq!(run(&offered(), &[Action::Settled { ok: false }]), offered());
    }

    #[test]
    fn a_cancel_is_not_a_failure_the_window_goes_nothing_is_said_the_offer_stands() {
        let s = run(
            &offered(),
            &[Action::Open, Action::Install, progress(30.0), Action::Abort],
        );
        assert!(s.aborting);
        assert_eq!(s.phase, Phase::Downloading);
        assert!(s.open);
        assert_eq!(run(&s, &[Action::Settled { ok: false }]), offered());
    }

    #[test]
    fn only_a_download_can_be_cancelled_not_idle_not_once_the_installer_has_the_file() {
        assert_eq!(run(&offered(), &[Action::Abort]), offered());
        let installing = run(&offered(), &[Action::Install, progress(100.0)]);
        assert_eq!(run(&installing, &[Action::Abort]), installing);
    }

    #[test]
    fn a_cancel_that_lost_the_race_changes_nothing_the_app_is_quitting() {
        let s = run(&offered(), &[Action::Install, Action::Abort]);
        assert_eq!(run(&s, &[Action::Settled { ok: true }]), s);
    }

    #[test]
    fn the_next_install_does_not_inherit_the_cancel() {
        let s = run(
            &offered(),
            &[
                Action::Install,
                Action::Abort,
                Action::Settled { ok: false },
                Action::Install,
                Action::Settled { ok: false },
            ],
        );
        assert!(!s.aborting);
        assert_eq!(s.notice.as_deref(), Some(INSTALL_FAILED));
    }

    #[test]
    fn reduce_is_pure() {
        let s = run(&offered(), &[Action::Install, progress(33.0)]);
        let before = s.clone();
        let a = progress(50.0);
        let first = reduce(&s, &a);
        let second = reduce(&s, &a);
        assert_eq!(first, second);
        assert_eq!(s, before);
    }

    /// mulberry32, the seeded generator the JS tests use; the same numbers for the same seed.
    fn mulberry32(seed: u32) -> impl FnMut() -> u32 {
        let mut a = seed;
        move || {
            a = a.wrapping_add(0x6D2B_79F5);
            let mut t = a;
            t = (t ^ (t >> 15)).wrapping_mul(t | 1);
            t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
            t ^ (t >> 14)
        }
    }

    fn random_action(r: &mut impl FnMut() -> u32) -> Action {
        let kind = r() % 9;
        let x = r();
        match kind {
            0 => Action::Available(if x.is_multiple_of(2) { real() } else { mock() }),
            1 => Action::Open,
            2 => Action::Cancel,
            3 => Action::Hide,
            4 => Action::Install,
            5 => {
                if x.is_multiple_of(50) {
                    Action::Progress(f64::NAN)
                } else {
                    // Negative, above 100 and fractional values all occur.
                    Action::Progress(f64::from(x % 400) - 100.0 + f64::from(r() % 4) / 4.0)
                }
            }
            6 => Action::Abort,
            7 => Action::Settled {
                ok: x.is_multiple_of(2),
            },
            _ => Action::Dismiss,
        }
    }

    #[test]
    fn pct_never_exceeds_100_nor_goes_back_within_an_install() {
        for seed in 0..1000u32 {
            let mut r = mulberry32(seed);
            let mut s = Flow::default();
            for step in 0..200 {
                let a = random_action(&mut r);
                let n = reduce(&s, &a);
                assert!(n.pct <= 100, "seed {seed} step {step}: pct {}", n.pct);
                if s.phase != Phase::Idle && n.phase != Phase::Idle {
                    assert!(
                        n.pct >= s.pct,
                        "seed {seed} step {step}: pct went back from {} to {}",
                        s.pct,
                        n.pct
                    );
                }
                s = n;
            }
        }
    }
}

//! The rules for a hooked session and its measured timelines (Task 5).
//!
//! A hook is the agent's own word, above its title. Two things stay the title's: an Esc (no hook
//! fires on an interrupt, so an idle title after work is a stop) and the work a Yes approved (a
//! spinner after a question is Working). A Stop with the spinner up waits for the next title, and
//! work still on the screen after a Stop keeps the tab Working. The box on screen holds a question.

use crate::signal::{AgentSignal, HookState};

/// How often a session in `background` re-reads its screen (`bg-check`).
pub const BACKGROUND_CHECK_MS: u64 = 3000;

/// How long a Stop with the spinner still up waits for the next title before it is a plain
/// finish: past one spinner frame (about 960 ms, measured).
pub const STOP_SETTLE_MS: u64 = 1500;

/// The phase a hooked session is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookPhase {
    Working,
    Question,
    Done,
    Failed,
    Stopped,
    Ending,
    Background,
}

impl HookPhase {
    /// The phase's name, for logs and tests.
    pub fn as_str(self) -> &'static str {
        match self {
            HookPhase::Working => "working",
            HookPhase::Question => "question",
            HookPhase::Done => "done",
            HookPhase::Failed => "failed",
            HookPhase::Stopped => "stopped",
            HookPhase::Ending => "ending",
            HookPhase::Background => "background",
        }
    }
}

/// A mark a tab can wear for the agent's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AttentionMark {
    Finished,
    Question,
    Failed,
}

impl AttentionMark {
    /// The mark's name.
    pub fn as_str(self) -> &'static str {
        match self {
            AttentionMark::Finished => "finished",
            AttentionMark::Question => "question",
            AttentionMark::Failed => "failed",
        }
    }
}

/// A session's phase, and the failure's kind while it is `Failed`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookSession {
    pub phase: HookPhase,
    pub kind: Option<String>,
}

/// What one step does to a session: the new phase, whether the agent works, and the marks to put
/// up (only on a tab nobody is looking at) or take down.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookOutcome {
    pub phase: HookPhase,
    pub kind: Option<String>,
    pub working: bool,
    pub raise: Vec<AttentionMark>,
    pub clear: Vec<AttentionMark>,
}

/// One input to the rules: a hook signal, or a title or timer the glue reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HookEvent {
    Signal(AgentSignal),
    IdleTitle,
    WorkingTitle,
    Settle,
    BgCheck,
}

/// What the screen showed when the step was taken. All false is "not read".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HookSeen {
    /// Claude's question or permission box is on screen.
    pub question_on_screen: bool,
    /// The session's title last said working (a spinner) when a Stop arrives.
    pub title_working: bool,
    /// Claude's screen shows work still running after its turn (a background shell or workflow).
    pub background_on_screen: bool,
}

fn finish() -> HookOutcome {
    HookOutcome {
        phase: HookPhase::Done,
        kind: None,
        working: false,
        raise: vec![AttentionMark::Finished],
        clear: vec![AttentionMark::Question, AttentionMark::Failed],
    }
}

fn background() -> HookOutcome {
    HookOutcome {
        phase: HookPhase::Background,
        kind: None,
        working: true,
        raise: vec![],
        clear: vec![AttentionMark::Question, AttentionMark::Failed],
    }
}

fn is_done(ev: &HookEvent) -> bool {
    matches!(
        ev,
        HookEvent::Signal(AgentSignal {
            state: HookState::Done,
            ..
        })
    )
}

/// What one event does to a session, or `None` when it changes nothing.
pub fn hook_step(
    prev: Option<&HookSession>,
    ev: &HookEvent,
    seen: &HookSeen,
) -> Option<HookOutcome> {
    let prev_phase = prev.map(|p| p.phase);
    // WHILE THE BOX IS ON SCREEN, THE QUESTION STANDS (#148). Subagents' hooks write the same
    // signal, so a sibling's tool call or the main turn's Stop must not take a pending question
    // down. A Stop still raises Finished under it.
    if seen.question_on_screen && screen_decides(prev, ev) {
        if is_done(ev) {
            return Some(HookOutcome {
                phase: HookPhase::Question,
                kind: None,
                working: false,
                raise: vec![AttentionMark::Finished],
                clear: vec![],
            });
        }
        return None;
    }
    match ev {
        HookEvent::WorkingTitle => {
            // After a Stop with the spinner up, another frame is work still going on (#149).
            if prev_phase == Some(HookPhase::Ending) {
                return Some(HookOutcome {
                    phase: HookPhase::Background,
                    kind: None,
                    working: true,
                    raise: vec![],
                    clear: vec![],
                });
            }
            // Not after a finish or a failure: a new turn always says UserPromptSubmit first.
            if prev_phase != Some(HookPhase::Question) && prev_phase != Some(HookPhase::Stopped) {
                return None;
            }
            Some(HookOutcome {
                phase: HookPhase::Working,
                kind: None,
                working: true,
                raise: vec![],
                clear: vec![AttentionMark::Question],
            })
        }
        HookEvent::IdleTitle => {
            // The star after a Stop, or after the background work it left: the turn is over.
            if prev_phase == Some(HookPhase::Ending) || prev_phase == Some(HookPhase::Background) {
                if seen.background_on_screen {
                    return if prev_phase == Some(HookPhase::Background) {
                        None
                    } else {
                        Some(background())
                    };
                }
                return Some(finish());
            }
            // Otherwise only an interrupt is the title's to say.
            if prev_phase != Some(HookPhase::Working) {
                return None;
            }
            Some(HookOutcome {
                phase: HookPhase::Stopped,
                kind: None,
                working: false,
                raise: vec![],
                clear: vec![],
            })
        }
        HookEvent::Settle => {
            // No title came after the Stop: a plain finish, unless the screen shows work going on.
            if prev_phase != Some(HookPhase::Ending) {
                return None;
            }
            Some(if seen.background_on_screen {
                background()
            } else {
                finish()
            })
        }
        HookEvent::BgCheck => {
            // Background work's re-read: over once neither the screen nor the spinner says so.
            if prev_phase != Some(HookPhase::Background) {
                return None;
            }
            if seen.background_on_screen || seen.title_working {
                None
            } else {
                Some(finish())
            }
        }
        HookEvent::Signal(sig) => match sig.state {
            HookState::Working => Some(HookOutcome {
                phase: HookPhase::Working,
                kind: None,
                working: true,
                raise: vec![],
                clear: vec![
                    AttentionMark::Question,
                    AttentionMark::Failed,
                    AttentionMark::Finished,
                ],
            }),
            HookState::Question => Some(HookOutcome {
                phase: HookPhase::Question,
                kind: None,
                working: false,
                raise: vec![AttentionMark::Question],
                clear: vec![AttentionMark::Failed],
            }),
            HookState::Done => {
                // The spinner still up: the next title decides (#149).
                if seen.title_working {
                    return Some(HookOutcome {
                        phase: HookPhase::Ending,
                        kind: None,
                        working: true,
                        raise: vec![],
                        clear: vec![AttentionMark::Question, AttentionMark::Failed],
                    });
                }
                if seen.background_on_screen {
                    return Some(background());
                }
                Some(finish())
            }
            HookState::Failed => {
                // Two hooks answer one failure (the kind's own and the catch-all), in either
                // order: a kind once said is kept.
                let kind = sig.kind.clone().or_else(|| {
                    prev.filter(|p| p.phase == HookPhase::Failed)
                        .and_then(|p| p.kind.clone())
                });
                Some(HookOutcome {
                    phase: HookPhase::Failed,
                    kind,
                    working: false,
                    raise: vec![AttentionMark::Failed, AttentionMark::Finished],
                    clear: vec![AttentionMark::Question],
                })
            }
        },
    }
}

/// Whether `hook_step` needs to know if the box is on screen: only work or a Stop while a
/// question is pending.
pub fn screen_decides(prev: Option<&HookSession>, ev: &HookEvent) -> bool {
    let pending = prev.is_some_and(|p| p.phase == HookPhase::Question);
    pending
        && match ev {
            HookEvent::WorkingTitle => true,
            HookEvent::Signal(sig) => matches!(sig.state, HookState::Working | HookState::Done),
            _ => false,
        }
}

/// Whether a mark goes up on the tab in front. Finished and Failed are news, so they go up only
/// where nobody is looking. A question is state, so it goes up whoever is looking.
pub fn raised_while_seen(mark: AttentionMark) -> bool {
    mark == AttentionMark::Question
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sig(state: HookState, kind: Option<&str>) -> HookEvent {
        HookEvent::Signal(AgentSignal {
            state,
            kind: kind.map(str::to_owned),
            session: None,
        })
    }
    fn working() -> HookEvent {
        sig(HookState::Working, None)
    }
    fn question() -> HookEvent {
        sig(HookState::Question, None)
    }
    fn done() -> HookEvent {
        sig(HookState::Done, None)
    }
    fn failed(kind: Option<&str>) -> HookEvent {
        sig(HookState::Failed, kind)
    }
    fn session(phase: HookPhase) -> HookSession {
        HookSession { phase, kind: None }
    }
    fn none() -> HookSeen {
        HookSeen::default()
    }
    fn words(marks: &[AttentionMark]) -> Vec<&'static str> {
        let mut w: Vec<&'static str> = marks.iter().map(|m| m.as_str()).collect();
        w.sort_unstable();
        w
    }
    /// Folds events through `hook_step` the way the glue does: a `None` changes nothing.
    fn fold(events: &[HookEvent]) -> Option<HookSession> {
        let mut prev: Option<HookSession> = None;
        for ev in events {
            if let Some(o) = hook_step(prev.as_ref(), ev, &HookSeen::default()) {
                prev = Some(HookSession {
                    phase: o.phase,
                    kind: o.kind,
                });
            }
        }
        prev
    }

    #[test]
    fn phase_words_are_the_seven_names() {
        let names: Vec<&str> = [
            HookPhase::Working,
            HookPhase::Question,
            HookPhase::Done,
            HookPhase::Failed,
            HookPhase::Stopped,
            HookPhase::Ending,
            HookPhase::Background,
        ]
        .iter()
        .map(|p| p.as_str())
        .collect();
        assert_eq!(
            names,
            [
                "working",
                "question",
                "done",
                "failed",
                "stopped",
                "ending",
                "background"
            ]
        );
    }

    #[test]
    fn a_prompt_is_work_and_clears_every_mark() {
        let o = hook_step(None, &working(), &none()).expect("a prompt is a step");
        assert_eq!(o.phase, HookPhase::Working);
        assert!(o.working);
        assert!(o.raise.is_empty());
        assert_eq!(words(&o.clear), ["failed", "finished", "question"]);
    }

    #[test]
    fn a_stop_finishes_with_the_finished_line() {
        let s = fold(&[working(), done()]).expect("a stop is a step");
        assert_eq!(s.phase, HookPhase::Done);
        let o = hook_step(Some(&session(HookPhase::Working)), &done(), &none()).expect("a stop");
        assert_eq!(o.phase, HookPhase::Done);
        assert!(!o.working);
        assert_eq!(o.raise, [AttentionMark::Finished]);
        assert_eq!(words(&o.clear), ["failed", "question"]);
    }

    #[test]
    fn a_question_is_the_question_line_not_a_finish() {
        let o = hook_step(Some(&session(HookPhase::Working)), &question(), &none())
            .expect("a question");
        assert_eq!(o.phase, HookPhase::Question);
        assert_eq!(o.raise, [AttentionMark::Question]);
        assert!(!o.raise.contains(&AttentionMark::Finished));
    }

    #[test]
    fn answering_is_work_again_and_takes_the_question_down() {
        let o = hook_step(Some(&session(HookPhase::Question)), &working(), &none()).expect("work");
        assert!(o.clear.contains(&AttentionMark::Question));
    }

    #[test]
    fn a_failure_is_failed_with_its_kind_and_finished_under_it() {
        let o = hook_step(None, &failed(Some("rate_limit")), &none()).expect("a failure");
        assert_eq!(o.phase, HookPhase::Failed);
        assert_eq!(o.kind.as_deref(), Some("rate_limit"));
        assert!(!o.working);
        assert_eq!(words(&o.raise), ["failed", "finished"]);
    }

    #[test]
    fn the_kind_is_kept_whichever_failure_hook_lands_first() {
        let a = fold(&[failed(Some("overloaded")), failed(None)]).expect("two failures");
        assert_eq!(a.kind.as_deref(), Some("overloaded"));
        let b = fold(&[failed(None), failed(Some("overloaded"))]).expect("two failures");
        assert_eq!(b.kind.as_deref(), Some("overloaded"));
        let c = fold(&[failed(None)]).expect("one failure");
        assert_eq!(c.kind, None);
    }

    #[test]
    fn a_new_prompt_clears_a_failure_and_the_next_has_no_old_kind() {
        let o = hook_step(
            Some(&HookSession {
                phase: HookPhase::Failed,
                kind: Some("rate_limit".into()),
            }),
            &working(),
            &none(),
        )
        .expect("work");
        assert!(o.clear.contains(&AttentionMark::Failed));
        let s = fold(&[failed(Some("rate_limit")), working(), failed(None)]).expect("steps");
        assert_eq!(s.kind, None);
    }

    #[test]
    fn an_idle_title_after_work_is_an_esc() {
        let o = hook_step(
            Some(&session(HookPhase::Working)),
            &HookEvent::IdleTitle,
            &none(),
        )
        .expect("an esc");
        assert_eq!(
            o,
            HookOutcome {
                phase: HookPhase::Stopped,
                kind: None,
                working: false,
                raise: vec![],
                clear: vec![],
            }
        );
    }

    #[test]
    fn an_idle_title_anywhere_else_changes_nothing() {
        for p in [HookPhase::Question, HookPhase::Done, HookPhase::Failed] {
            assert_eq!(
                hook_step(Some(&session(p)), &HookEvent::IdleTitle, &none()),
                None,
                "after {p:?}"
            );
        }
        assert_eq!(hook_step(None, &HookEvent::IdleTitle, &none()), None);
    }

    #[test]
    fn a_stop_after_the_idle_title_still_finishes() {
        let s = fold(&[working(), HookEvent::IdleTitle, done()]).expect("steps");
        assert_eq!(s.phase, HookPhase::Done);
        let o = hook_step(Some(&session(HookPhase::Stopped)), &done(), &none()).expect("a stop");
        assert_eq!(o.raise, [AttentionMark::Finished]);
    }

    #[test]
    fn a_question_goes_up_on_the_tab_in_front_finished_and_failed_do_not() {
        assert!(raised_while_seen(AttentionMark::Question));
        assert!(!raised_while_seen(AttentionMark::Finished));
        assert!(!raised_while_seen(AttentionMark::Failed));
    }

    #[test]
    fn a_working_title_after_a_question_or_an_esc_is_work() {
        let want = HookOutcome {
            phase: HookPhase::Working,
            kind: None,
            working: true,
            raise: vec![],
            clear: vec![AttentionMark::Question],
        };
        let after_q = hook_step(
            Some(&session(HookPhase::Question)),
            &HookEvent::WorkingTitle,
            &none(),
        );
        assert_eq!(after_q, Some(want.clone()));
        let after_esc = hook_step(
            Some(&session(HookPhase::Stopped)),
            &HookEvent::WorkingTitle,
            &none(),
        );
        assert_eq!(after_esc, Some(want));
    }

    #[test]
    fn anywhere_else_a_working_title_changes_nothing() {
        for p in [
            Some(HookPhase::Working),
            Some(HookPhase::Done),
            Some(HookPhase::Failed),
            None,
        ] {
            let prev = p.map(session);
            assert_eq!(
                hook_step(prev.as_ref(), &HookEvent::WorkingTitle, &none()),
                None,
                "after {p:?}"
            );
        }
    }

    #[test]
    fn a_no_or_an_esc_to_the_box_leaves_nothing_working() {
        let s = fold(&[
            working(),
            HookEvent::IdleTitle,
            question(),
            HookEvent::IdleTitle,
        ])
        .expect("steps");
        assert_eq!(s.phase, HookPhase::Question);
    }

    #[test]
    fn the_screen_is_read_only_for_work_or_a_stop_while_a_question_is_pending() {
        let phases = [
            None,
            Some(HookPhase::Working),
            Some(HookPhase::Question),
            Some(HookPhase::Done),
            Some(HookPhase::Failed),
            Some(HookPhase::Stopped),
        ];
        let events: [(&str, HookEvent); 6] = [
            ("working", working()),
            ("question", question()),
            ("done", done()),
            ("failed", failed(None)),
            ("working-title", HookEvent::WorkingTitle),
            ("idle-title", HookEvent::IdleTitle),
        ];
        for p in phases {
            let prev = p.map(session);
            for (name, ev) in events.iter() {
                let want = p == Some(HookPhase::Question)
                    && ["working", "working-title", "done"].contains(name);
                assert_eq!(screen_decides(prev.as_ref(), ev), want, "{p:?} x {name}");
            }
        }
    }

    #[test]
    fn work_from_any_agent_leaves_the_question_standing() {
        let seen = HookSeen {
            question_on_screen: true,
            ..none()
        };
        let q = Some(session(HookPhase::Question));
        assert_eq!(hook_step(q.as_ref(), &working(), &seen), None);
        assert_eq!(hook_step(q.as_ref(), &HookEvent::WorkingTitle, &seen), None);
    }

    #[test]
    fn a_stop_raises_finished_under_it_and_keeps_the_question() {
        let seen = HookSeen {
            question_on_screen: true,
            ..none()
        };
        let q = Some(session(HookPhase::Question));
        assert_eq!(
            hook_step(q.as_ref(), &done(), &seen),
            Some(HookOutcome {
                phase: HookPhase::Question,
                kind: None,
                working: false,
                raise: vec![AttentionMark::Finished],
                clear: vec![],
            })
        );
    }

    #[test]
    fn a_question_or_a_failure_is_as_without_it() {
        let with = HookSeen {
            question_on_screen: true,
            ..none()
        };
        for prev in [None, Some(HookPhase::Working)] {
            let p = prev.map(session);
            assert_eq!(
                hook_step(p.as_ref(), &question(), &with),
                hook_step(p.as_ref(), &question(), &none())
            );
            assert_eq!(
                hook_step(p.as_ref(), &failed(Some("rate_limit")), &with),
                hook_step(p.as_ref(), &failed(Some("rate_limit")), &none())
            );
        }
    }

    #[test]
    fn with_the_box_gone_every_step_is_as_before() {
        let off = HookSeen {
            question_on_screen: false,
            ..none()
        };
        let q = Some(session(HookPhase::Question));
        for ev in [working(), done(), question(), failed(None)] {
            assert_eq!(
                hook_step(q.as_ref(), &ev, &off),
                hook_step(q.as_ref(), &ev, &none())
            );
        }
        let w = hook_step(q.as_ref(), &working(), &off).expect("work");
        assert!(w.clear.contains(&AttentionMark::Question));
        let d = hook_step(q.as_ref(), &done(), &off).expect("stop");
        assert!(d.clear.contains(&AttentionMark::Question));
    }

    #[test]
    fn a_stop_with_the_spinner_up_is_not_a_finish_yet() {
        let seen = HookSeen {
            title_working: true,
            ..none()
        };
        let o = hook_step(Some(&session(HookPhase::Working)), &done(), &seen).expect("a stop");
        assert_eq!(o.phase, HookPhase::Ending);
        assert!(o.working);
        assert!(o.raise.is_empty());
        assert_eq!(words(&o.clear), ["failed", "question"]);
    }

    #[test]
    fn the_star_next_is_the_finish() {
        let o = hook_step(
            Some(&session(HookPhase::Ending)),
            &HookEvent::IdleTitle,
            &none(),
        )
        .expect("the star");
        assert_eq!(o.phase, HookPhase::Done);
        assert_eq!(o.raise, [AttentionMark::Finished]);
    }

    #[test]
    fn another_spinner_frame_is_background_work() {
        let o = hook_step(
            Some(&session(HookPhase::Ending)),
            &HookEvent::WorkingTitle,
            &none(),
        )
        .expect("a frame");
        assert_eq!(
            o,
            HookOutcome {
                phase: HookPhase::Background,
                kind: None,
                working: true,
                raise: vec![],
                clear: vec![],
            }
        );
        assert_eq!(
            hook_step(
                Some(&session(HookPhase::Background)),
                &HookEvent::WorkingTitle,
                &none()
            ),
            None
        );
        let idle = hook_step(
            Some(&session(HookPhase::Background)),
            &HookEvent::IdleTitle,
            &none(),
        )
        .expect("the star");
        assert_eq!(idle.raise, [AttentionMark::Finished]);
    }

    #[test]
    fn no_title_in_time_settles_as_a_plain_finish() {
        let o = hook_step(
            Some(&session(HookPhase::Ending)),
            &HookEvent::Settle,
            &none(),
        )
        .expect("settle");
        assert_eq!(o.phase, HookPhase::Done);
        assert_eq!(o.raise, [AttentionMark::Finished]);
        for p in [
            HookPhase::Working,
            HookPhase::Question,
            HookPhase::Done,
            HookPhase::Background,
        ] {
            assert_eq!(
                hook_step(Some(&session(p)), &HookEvent::Settle, &none()),
                None,
                "settle on {p:?}"
            );
        }
    }

    #[test]
    fn with_the_star_already_up_a_stop_finishes_at_once() {
        let o = hook_step(Some(&session(HookPhase::Working)), &done(), &none()).expect("a stop");
        assert_eq!(o.phase, HookPhase::Done);
        assert_eq!(o.raise, [AttentionMark::Finished]);
    }

    #[test]
    fn a_stop_with_background_work_on_screen_stays_working() {
        let seen = HookSeen {
            background_on_screen: true,
            ..none()
        };
        let o = hook_step(Some(&session(HookPhase::Working)), &done(), &seen).expect("a stop");
        assert_eq!(o.phase, HookPhase::Background);
        assert!(o.working);
        assert!(o.raise.is_empty());
    }

    #[test]
    fn the_settle_holds_it_too() {
        let bg = HookSeen {
            background_on_screen: true,
            ..none()
        };
        let held =
            hook_step(Some(&session(HookPhase::Ending)), &HookEvent::Settle, &bg).expect("settle");
        assert_eq!(held.phase, HookPhase::Background);
        let plain = hook_step(
            Some(&session(HookPhase::Ending)),
            &HookEvent::Settle,
            &none(),
        )
        .expect("settle");
        assert_eq!(plain.phase, HookPhase::Done);
    }

    #[test]
    fn an_idle_star_does_not_end_it_while_the_screen_says_work_goes_on() {
        let bg = HookSeen {
            background_on_screen: true,
            ..none()
        };
        let prev = Some(session(HookPhase::Background));
        assert_eq!(hook_step(prev.as_ref(), &HookEvent::IdleTitle, &bg), None);
        let plain = hook_step(prev.as_ref(), &HookEvent::IdleTitle, &none()).expect("the star");
        assert_eq!(plain.phase, HookPhase::Done);
    }

    #[test]
    fn the_re_read_finishes_it_once_the_screen_lets_it_go() {
        let prev = Some(session(HookPhase::Background));
        let bg = HookSeen {
            background_on_screen: true,
            ..none()
        };
        assert_eq!(hook_step(prev.as_ref(), &HookEvent::BgCheck, &bg), None);
        let spinner = HookSeen {
            title_working: true,
            ..none()
        };
        assert_eq!(
            hook_step(prev.as_ref(), &HookEvent::BgCheck, &spinner),
            None
        );
        let gone = hook_step(prev.as_ref(), &HookEvent::BgCheck, &none()).expect("finished");
        assert_eq!(gone.phase, HookPhase::Done);
        assert_eq!(gone.raise, [AttentionMark::Finished]);
        assert_eq!(
            hook_step(
                Some(&session(HookPhase::Done)),
                &HookEvent::BgCheck,
                &none()
            ),
            None
        );
    }

    #[test]
    fn a_new_prompt_during_it_is_plain_work() {
        let o =
            hook_step(Some(&session(HookPhase::Background)), &working(), &none()).expect("work");
        assert_eq!(o.phase, HookPhase::Working);
        assert!(o.working);
    }
}

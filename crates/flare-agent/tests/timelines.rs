//! The measured timelines (#148) through the hook rules: a faithful port of Prism Terminal's
//! `replay` in `agentHookState.timelines.test.ts`. The steps are in `fixtures/hook_timelines.json`,
//! converted once from `agentHookTimelines.ts` (not edited by hand).

use std::collections::HashSet;

use flare_agent::hook_state::{
    AttentionMark, BACKGROUND_CHECK_MS, HookEvent, HookPhase, HookSeen, HookSession,
    STOP_SETTLE_MS, hook_step, screen_decides,
};
use flare_agent::signal::{AgentSignal, HookState};

const FIXTURE: &str = include_str!("fixtures/hook_timelines.json");

/// One measured step: `[ms from spawn, event, box, box 250 ms later]`.
#[derive(Clone, Debug)]
struct Step {
    t: u64,
    event: String,
    box_on: bool,
    later: bool,
}

#[derive(Clone, Debug)]
enum Action {
    Step(Step),
    /// A key that settles a box, with the box gone after it answers the question (#144).
    RemoveQuestion,
    /// A re-read queued by a held spinner or a settle.
    Apply(HookEvent, bool),
}

fn timeline(name: &str) -> Vec<Step> {
    let all: serde_json::Value = serde_json::from_str(FIXTURE).expect("fixture parses");
    all[name]
        .as_array()
        .expect("a timeline")
        .iter()
        .map(|row| {
            let row = row.as_array().expect("a step");
            Step {
                t: row[0].as_u64().expect("ms"),
                event: row[1].as_str().expect("event").to_owned(),
                box_on: row[2].as_u64() == Some(1),
                later: row[3].as_u64() == Some(1),
            }
        })
        .collect()
}

fn event_of(name: &str) -> HookEvent {
    let sig = |state| HookEvent::Signal(AgentSignal { state, kind: None });
    match name {
        "idle-title" => HookEvent::IdleTitle,
        "working-title" => HookEvent::WorkingTitle,
        "settle" => HookEvent::Settle,
        "bg-check" => HookEvent::BgCheck,
        "working" => sig(HookState::Working),
        "question" => sig(HookState::Question),
        "done" => sig(HookState::Done),
        "failed" => sig(HookState::Failed),
        other => panic!("unknown timeline event {other}"),
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

/// The state of one replay, as the glue keeps it for a tab nobody is looking at.
struct Replay {
    s: Option<HookSession>,
    working: bool,
    title_working: bool,
    marks: HashSet<AttentionMark>,
    queue: Vec<(u64, Action)>,
    shown: Vec<(u64, Option<&'static str>)>,
}

impl Replay {
    fn show(&self) -> Option<&'static str> {
        if self.working {
            return Some("working");
        }
        [
            AttentionMark::Question,
            AttentionMark::Failed,
            AttentionMark::Finished,
        ]
        .into_iter()
        .find(|m| self.marks.contains(m))
        .map(|m| m.as_str())
    }

    fn apply(&mut self, ev: &HookEvent, box_on: bool) -> bool {
        let seen = HookSeen {
            question_on_screen: screen_decides(self.s.as_ref(), ev) && box_on,
            title_working: is_done(ev) && self.title_working,
            background_on_screen: false,
        };
        let outcome = hook_step(self.s.as_ref(), ev, &seen);
        match ev {
            HookEvent::WorkingTitle => self.title_working = true,
            HookEvent::IdleTitle => self.title_working = false,
            _ => {}
        }
        let Some(o) = outcome else {
            return false;
        };
        self.s = Some(HookSession {
            phase: o.phase,
            kind: o.kind.clone(),
        });
        self.working = o.working;
        for m in &o.raise {
            self.marks.insert(*m);
        }
        for m in &o.clear {
            if !o.raise.contains(m) {
                self.marks.remove(m);
            }
        }
        true
    }

    fn run(&mut self, action: Action) {
        match action {
            Action::RemoveQuestion => {
                self.marks.remove(&AttentionMark::Question);
            }
            Action::Apply(ev, box_on) => {
                self.apply(&ev, box_on);
            }
            Action::Step(step) => {
                if step.event.starts_with("key:") {
                    if step.event != "key:submit"
                        && self.marks.contains(&AttentionMark::Question)
                        && !step.later
                    {
                        self.queue.push((step.t + 250, Action::RemoveQuestion));
                    }
                    return;
                }
                let ev = event_of(&step.event);
                let applied = self.apply(&ev, step.box_on);
                let held = !applied
                    && step.event == "working-title"
                    && screen_decides(self.s.as_ref(), &ev)
                    && step.box_on;
                if held {
                    self.queue
                        .push((step.t + 250, Action::Apply(ev.clone(), step.later)));
                }
                let ending = self.s.as_ref().map(|s| s.phase) == Some(HookPhase::Ending);
                if step.event == "done" && ending {
                    self.queue.push((
                        step.t + STOP_SETTLE_MS,
                        Action::Apply(HookEvent::Settle, false),
                    ));
                }
            }
        }
    }
}

/// Runs the steps the way the TS `replay` does: the queue is sorted (stably) before each entry runs.
fn replay(steps: &[Step]) -> Vec<(u64, Option<&'static str>)> {
    let mut r = Replay {
        s: None,
        working: false,
        title_working: false,
        marks: HashSet::new(),
        queue: steps
            .iter()
            .map(|s| (s.t, Action::Step(s.clone())))
            .collect(),
        shown: Vec::new(),
    };
    let mut i = 0;
    while i < r.queue.len() {
        r.queue.sort_by_key(|(t, _)| *t);
        let (t, action) = r.queue[i].clone();
        r.run(action);
        let now = r.show();
        r.shown.push((t, now));
        i += 1;
    }
    r.shown
}

/// The last value recorded at or before `t`.
fn at(shown: &[(u64, Option<&'static str>)], t: u64) -> Option<&'static str> {
    let mut at = None;
    for (when, v) in shown {
        if *when <= t {
            at = *v;
        }
    }
    at
}

/// Every 100 ms from `from` up to (not including) `to`, the tab shows `want`.
fn holds(shown: &[(u64, Option<&'static str>)], from: u64, to: u64, want: Option<&'static str>) {
    for t in (from..to).step_by(100) {
        assert_eq!(at(shown, t), want, "at {t} ms");
    }
}

#[test]
fn timeline_a_a_yes() {
    let shown = replay(&timeline("A"));
    holds(&shown, 13805, 22861, Some("question"));
    // The first spinner (22861) came 6 ms before the repaint: one re-read later.
    holds(&shown, 23111, 49815, Some("working"));
    // The Stop came with the spinner up; the star 42 ms later finishes it (#149).
    holds(&shown, 49815, 49857, Some("working"));
    holds(&shown, 49857, 52000, Some("finished"));
}

#[test]
fn timeline_c_auto_mode_with_an_ask_rule() {
    let shown = replay(&timeline("C"));
    holds(&shown, 13120, 22174, Some("question"));
    holds(&shown, 22424, 49244, Some("working"));
    holds(&shown, 49244, 52000, Some("finished"));
}

#[test]
fn timeline_e_auto_mode_with_a_hook_that_asks() {
    let shown = replay(&timeline("E"));
    holds(&shown, 10913, 22664, Some("question"));
    holds(&shown, 22914, 49715, Some("working"));
    holds(&shown, 49715, 52000, Some("finished"));
}

#[test]
fn timeline_d_two_prompts_in_a_row() {
    let shown = replay(&timeline("D"));
    holds(&shown, 12212, 15344, Some("question"));
    holds(&shown, 15594, 30212, Some("working"));
    holds(&shown, 30227, 33348, Some("question"));
    holds(&shown, 33598, 45465, Some("working"));
    holds(&shown, 45465, 48000, Some("finished"));
}

#[test]
fn timeline_b_an_esc() {
    let shown = replay(&timeline("B"));
    holds(&shown, 10848, 19939, Some("question"));
    holds(&shown, 20189, 43000, None);
}

#[test]
fn timeline_g2_a_subagent_asks_while_its_sibling_works() {
    let shown = replay(&timeline("G2"));
    holds(&shown, 22267, 36409, Some("question"));
    // NOT FINISHED WHILE A BACKGROUND SUBAGENT STILL WORKS (#149).
    holds(&shown, 36659, 63980, Some("working"));
    holds(&shown, 63980, 66000, Some("finished"));
}

#[test]
fn timeline_c3_the_classifier_denies() {
    let shown = replay(&timeline("C3"));
    holds(&shown, 10640, 19133, Some("working"));
    holds(&shown, 19133, 20000, Some("finished"));
    for t in (0..54000).step_by(100) {
        assert_ne!(at(&shown, t), Some("question"), "question at {t} ms");
    }
}

#[test]
fn the_fixture_has_the_measured_step_counts() {
    for (name, count) in [
        ("A", 43),
        ("B", 11),
        ("C", 43),
        ("D", 45),
        ("E", 43),
        ("G2", 68),
        ("C3", 16),
    ] {
        assert_eq!(timeline(name).len(), count, "timeline {name}");
    }
    // The background check is the same cadence the glue uses.
    assert_eq!(BACKGROUND_CHECK_MS, 3000);
}

//! The indicators probe (Task 25): `marks.state`, what each tab's mark is doing. Read-only: it asks
//! the tab rows the window shows and sets nothing. `app.state` already reports each tab's mark
//! (`state`, `motion`, `colour`); this probe adds what a scenario needs to tell the motions apart:
//! whether the mark is still (reduced motion) and whether a finished line wears the rainbow.

use serde_json::{Value, json};
use slint::Model;

use super::{ProbeCtx, ProbeRegistry};

/// Adds this file's probes to `r`.
pub fn register(r: &mut ProbeRegistry) {
    r.add("marks.state", marks_state);
}

/// One tab's mark as the row holds it.
#[derive(Clone, Debug, PartialEq)]
pub struct MarkFacts {
    pub id: String,
    /// The row's mark code: 0 none, 1 working, 2 question, 3 finished, 4 failed.
    pub state: i32,
    /// The row's mark kind: 0 none, 1 ring, 2 line (Bar's question and finished line), 3 run (Bar's
    /// working run).
    pub kind: i32,
    /// A finished Bar line wears the flowing rainbow.
    pub rainbow: bool,
    /// The mark's colour as `#rrggbb`.
    pub colour: String,
}

/// The state word for a mark code (the words `app.state` uses).
pub fn state_word(state: i32) -> &'static str {
    match state {
        1 => "working",
        2 => "question",
        3 => "finished",
        4 => "failed",
        _ => "none",
    }
}

/// The motion a mark makes. `still` is the reduced-motion setting: every moving mark stands still.
/// A ring spins while its agent works and its question dot breathes; a Bar line breathes for a
/// question, flows for a rainbow finish and stays for a plain one; the Bar run moves while the agent
/// works.
pub fn motion_word(state: i32, kind: i32, rainbow: bool, still: bool) -> &'static str {
    let moving = match (kind, state) {
        (1, 1) => "spin",
        (1, 2) | (2, 2) => "breathe",
        (2, 3) if rainbow => "flow",
        (3, 1) => "run",
        _ => return "none",
    };
    if still { "still" } else { moving }
}

/// The probe's answer: `{ tabs: [ { id, state, kind, motion, colour, rainbow, paused } ] }`. `paused`\n/// is a mark that would move held by a minimised window (the mark clock stops until a restore).
pub fn answer(marks: &[MarkFacts], still: bool, minimised: bool) -> Value {
    let tabs: Vec<Value> = marks
        .iter()
        .map(|m| {
            let motion = motion_word(m.state, m.kind, m.rainbow, still);
            json!({
                "id": m.id,
                "state": state_word(m.state),
                "kind": match m.kind { 1 => "ring", 2 => "line", 3 => "run", _ => "none" },
                "motion": motion,
                "paused": minimised && !still && !matches!(motion, "none" | "still"),
                "colour": m.colour,
                "rainbow": m.rainbow,
            })
        })
        .collect();
    json!({ "tabs": tabs })
}

/// `marks.state`: see `answer`. A line's pixel thickness is a snapshot fact, not reported.
fn marks_state(ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    let marks: Vec<MarkFacts> = ctx
        .app
        .get_tab_rows()
        .iter()
        .filter(|row| !row.settings)
        .map(|row| {
            let c = row.mark_color;
            MarkFacts {
                id: row.id.to_string(),
                state: row.mark_state,
                kind: row.mark_kind,
                rainbow: row.mark_rainbow,
                colour: format!("#{:02x}{:02x}{:02x}", c.red(), c.green(), c.blue()),
            }
        })
        .collect();
    let minimised = ctx
        .ctl
        .try_borrow()
        .map_err(|_| "the controller is busy")?
        .marks_paused();
    Ok(answer(&marks, !crate::e2e::animations_enabled(), minimised))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ring_spins_only_while_its_agent_works() {
        assert_eq!(motion_word(1, 1, false, false), "spin");
        assert_eq!(motion_word(0, 1, false, false), "none");
        assert_eq!(motion_word(4, 1, false, false), "none");
    }

    #[test]
    fn a_question_breathes_in_both_styles_and_a_finished_one_flows_only_with_the_rainbow() {
        assert_eq!(motion_word(2, 1, false, false), "breathe");
        assert_eq!(motion_word(2, 2, false, false), "breathe");
        assert_eq!(motion_word(3, 2, true, false), "flow");
        assert_eq!(motion_word(3, 2, false, false), "none");
    }

    #[test]
    fn a_working_run_moves_and_a_failed_line_does_not() {
        assert_eq!(motion_word(1, 3, false, false), "run");
        assert_eq!(motion_word(4, 2, false, false), "none");
    }

    #[test]
    fn reduced_motion_stills_every_moving_mark() {
        assert_eq!(motion_word(2, 2, false, true), "still");
        assert_eq!(motion_word(3, 2, true, true), "still");
        assert_eq!(motion_word(1, 3, false, true), "still");
        // A mark that never moved keeps "none", still or not.
        assert_eq!(motion_word(0, 0, false, true), "none");
    }

    #[test]
    fn the_answer_names_each_tab_its_state_kind_and_motion() {
        let marks = [
            MarkFacts {
                id: "a".into(),
                state: 2,
                kind: 2,
                rainbow: false,
                colour: "#ff0000".into(),
            },
            MarkFacts {
                id: "b".into(),
                state: 3,
                kind: 2,
                rainbow: true,
                colour: "#00ff00".into(),
            },
        ];
        let v = answer(&marks, false, false);
        assert_eq!(v["tabs"][0]["state"], "question");
        assert_eq!(v["tabs"][0]["kind"], "line");
        assert_eq!(v["tabs"][0]["motion"], "breathe");
        assert_eq!(v["tabs"][1]["motion"], "flow");
        assert_eq!(v["tabs"][1]["rainbow"], true);
        assert_eq!(v["tabs"][1]["colour"], "#00ff00");
        let still = answer(&marks, true, false);
        assert_eq!(still["tabs"][0]["motion"], "still");
    }

    #[test]
    fn a_minimised_window_pauses_only_the_marks_that_move() {
        let marks = [
            MarkFacts {
                id: "a".into(),
                state: 1,
                kind: 1,
                rainbow: false,
                colour: "#fff".into(),
            },
            MarkFacts {
                id: "b".into(),
                state: 0,
                kind: 0,
                rainbow: false,
                colour: "#fff".into(),
            },
            MarkFacts {
                id: "c".into(),
                state: 3,
                kind: 2,
                rainbow: false,
                colour: "#fff".into(),
            },
        ];
        let up = answer(&marks, false, false);
        assert!(
            up["tabs"]
                .as_array()
                .unwrap()
                .iter()
                .all(|t| t["paused"] == false)
        );
        let down = answer(&marks, false, true);
        assert_eq!(down["tabs"][0]["paused"], true, "a spinning ring is held");
        assert_eq!(down["tabs"][1]["paused"], false, "no mark, nothing to hold");
        assert_eq!(
            down["tabs"][2]["paused"], false,
            "a plain finished line never moved"
        );
    }
}

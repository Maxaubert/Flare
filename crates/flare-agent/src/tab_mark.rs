//! Which mark a tab wears, from its state (Task 13). Ring and Bar only (decision D6): Flare has
//! no Off, no Full, no Powerline edge and no overlay. Ported from Prism Terminal's `tabMark.ts`.

/// The indicator style the user picked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Indicator {
    /// One mono mark by the tab name: a spinner, a breathing dot, a check, a cross.
    Ring,
    /// A 3 px line under the tab, and the running bar while working.
    Bar,
}

/// The stored word `minimal` is Bar; `ring`, `off`, `full`, junk and nothing are Ring.
pub fn read_indicator(raw: Option<&str>) -> Indicator {
    match raw {
        Some("minimal") => Indicator::Bar,
        _ => Indicator::Ring,
    }
}

/// A tab's agent state, in the order the strip reads them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MarkState {
    Working,
    Done,
    Question,
    Failed,
}

/// Where a mark is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkPlace {
    None,
    /// The running bar (Bar, working).
    Run,
    /// The 3 px line (Bar, every other state).
    Line,
    /// The mono glyph beside the name (Ring).
    Ring,
}

/// A colour role; the theme's palette turns it into a colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkColour {
    Working,
    Done,
    Rainbow,
    Question,
    Failed,
}

/// How a mark moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkMotionKind {
    Run,
    Breathe,
    Flow,
    Spin,
    Still,
}

/// What a tab's mark is: where, in which colour role, moving how.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TabMark {
    pub place: MarkPlace,
    pub colour: Option<MarkColour>,
    pub motion: Option<MarkMotionKind>,
}

/// A tab with no state wears nothing.
pub const NO_MARK: TabMark = TabMark {
    place: MarkPlace::None,
    colour: None,
    motion: None,
};

/// Which mark a tab wears. Ring: one mono mark for every state (a spinner while working, a
/// breathing dot for a question, still for a finish or a failure). Bar: the running bar while
/// working; otherwise a 3 px line in the state's colour, a question breathing, a finished line
/// flowing the rainbow when the rainbow is on (still in the finished colour when it is off).
pub fn resolve_tab_mark(indicator: Indicator, state: Option<MarkState>, rainbow: bool) -> TabMark {
    let Some(state) = state else {
        return NO_MARK;
    };
    match indicator {
        Indicator::Ring => TabMark {
            place: MarkPlace::Ring,
            colour: None,
            motion: Some(match state {
                MarkState::Working => MarkMotionKind::Spin,
                MarkState::Question => MarkMotionKind::Breathe,
                MarkState::Done | MarkState::Failed => MarkMotionKind::Still,
            }),
        },
        Indicator::Bar => {
            if state == MarkState::Working {
                return TabMark {
                    place: MarkPlace::Run,
                    colour: Some(MarkColour::Working),
                    motion: Some(MarkMotionKind::Run),
                };
            }
            let colour = match state {
                MarkState::Working => MarkColour::Working,
                MarkState::Done if rainbow => MarkColour::Rainbow,
                MarkState::Done => MarkColour::Done,
                MarkState::Question => MarkColour::Question,
                MarkState::Failed => MarkColour::Failed,
            };
            let motion = match state {
                MarkState::Question => MarkMotionKind::Breathe,
                MarkState::Done if rainbow => MarkMotionKind::Flow,
                _ => MarkMotionKind::Still,
            };
            TabMark {
                place: MarkPlace::Line,
                colour: Some(colour),
                motion: Some(motion),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `place/colour/motion`, `-` for none, lowercase words.
    fn m(indicator: Indicator, state: Option<MarkState>, rainbow: bool) -> String {
        let t = resolve_tab_mark(indicator, state, rainbow);
        let place = match t.place {
            MarkPlace::None => "none",
            MarkPlace::Run => "run",
            MarkPlace::Line => "line",
            MarkPlace::Ring => "ring",
        };
        let colour = match t.colour {
            None => "-",
            Some(MarkColour::Working) => "working",
            Some(MarkColour::Done) => "done",
            Some(MarkColour::Rainbow) => "rainbow",
            Some(MarkColour::Question) => "question",
            Some(MarkColour::Failed) => "failed",
        };
        let motion = match t.motion {
            None => "-",
            Some(MarkMotionKind::Run) => "run",
            Some(MarkMotionKind::Breathe) => "breathe",
            Some(MarkMotionKind::Flow) => "flow",
            Some(MarkMotionKind::Spin) => "spin",
            Some(MarkMotionKind::Still) => "still",
        };
        format!("{place}/{colour}/{motion}")
    }

    #[test]
    fn bar_working_is_the_running_line_and_the_rest_are_3px_lines() {
        for rainbow in [true, false] {
            assert_eq!(
                m(Indicator::Bar, Some(MarkState::Working), rainbow),
                "run/working/run"
            );
        }
        assert_eq!(
            m(Indicator::Bar, Some(MarkState::Done), true),
            "line/rainbow/flow"
        );
        assert_eq!(
            m(Indicator::Bar, Some(MarkState::Question), true),
            "line/question/breathe"
        );
        assert_eq!(
            m(Indicator::Bar, Some(MarkState::Failed), true),
            "line/failed/still"
        );
    }

    #[test]
    fn ring_is_one_mono_mark_for_every_state() {
        for rainbow in [true, false] {
            assert_eq!(
                m(Indicator::Ring, Some(MarkState::Working), rainbow),
                "ring/-/spin"
            );
            assert_eq!(
                m(Indicator::Ring, Some(MarkState::Question), rainbow),
                "ring/-/breathe"
            );
            assert_eq!(
                m(Indicator::Ring, Some(MarkState::Done), rainbow),
                "ring/-/still"
            );
            assert_eq!(
                m(Indicator::Ring, Some(MarkState::Failed), rainbow),
                "ring/-/still"
            );
        }
    }

    #[test]
    fn with_the_rainbow_off_finished_is_the_finished_colour_still() {
        assert_eq!(
            m(Indicator::Bar, Some(MarkState::Done), false),
            "line/done/still"
        );
    }

    #[test]
    fn a_tab_with_no_state_wears_nothing() {
        for indicator in [Indicator::Ring, Indicator::Bar] {
            for rainbow in [true, false] {
                assert_eq!(m(indicator, None, rainbow), "none/-/-");
            }
        }
    }

    #[test]
    fn the_render_model_of_each_mark() {
        // The Bar question is a line that breathes; the Bar finished rainbow flows and says so.
        let q = resolve_tab_mark(Indicator::Bar, Some(MarkState::Question), true);
        assert_eq!(q.place, MarkPlace::Line);
        assert_eq!(q.motion, Some(MarkMotionKind::Breathe));
        let d = resolve_tab_mark(Indicator::Bar, Some(MarkState::Done), true);
        assert_eq!(d.motion, Some(MarkMotionKind::Flow));
        assert_eq!(d.colour, Some(MarkColour::Rainbow));
        // Ring is mono: no colour role at all, whatever the state.
        for s in [
            MarkState::Working,
            MarkState::Done,
            MarkState::Question,
            MarkState::Failed,
        ] {
            let r = resolve_tab_mark(Indicator::Ring, Some(s), true);
            assert_eq!(r.colour, None);
            assert_eq!(r.place, MarkPlace::Ring);
        }
        assert_eq!(
            resolve_tab_mark(Indicator::Ring, Some(MarkState::Working), true).motion,
            Some(MarkMotionKind::Spin)
        );
        assert_eq!(
            resolve_tab_mark(Indicator::Ring, Some(MarkState::Done), true).motion,
            Some(MarkMotionKind::Still)
        );
        assert_eq!(
            resolve_tab_mark(Indicator::Ring, Some(MarkState::Failed), true).motion,
            Some(MarkMotionKind::Still)
        );
    }

    #[test]
    fn the_stored_word_minimal_is_bar_everything_else_ring() {
        assert_eq!(read_indicator(Some("minimal")), Indicator::Bar);
        for raw in [
            Some("ring"),
            Some("off"),
            Some("full"),
            Some(""),
            Some("x"),
            None,
        ] {
            assert_eq!(read_indicator(raw), Indicator::Ring, "{raw:?}");
        }
    }
}

//! The mark colours from the look and the agent picks (Task 27). Source: PT `tabLooks.ts` (the palette
//! block), `agentColors.ts` (both copies) and M4's `marks` and `agent_colours`. The picks come from
//! `LookStore` (decision D13); failed is always the theme's red.
#![allow(dead_code, reason = "Task 28 wires the tab marks to these; remove this allow there")]

use flare_agent::tab_mark::{Indicator, MarkState};
use flare_settings::look::{AgentColourState, AgentIndicator, LookStore};
use flare_theme::agent_colours::AgentColours;
use flare_theme::look::AppliedLook;
use flare_theme::marks::{MarkColours, MarkPalette, mark_palette};

/// The three agent picks; `None` follows the theme.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Picks {
    pub working: Option<String>,
    pub finished: Option<String>,
    pub question: Option<String>,
}

/// The stored picks. `""` (follow the theme) becomes `None`.
pub fn picks_from(look: &LookStore) -> Picks {
    let pick = |s: AgentColourState| {
        let v = look.agent_colour_choice(s);
        if v.is_empty() { None } else { Some(v) }
    };
    Picks {
        working: pick(AgentColourState::Working),
        finished: pick(AgentColourState::Finished),
        question: pick(AgentColourState::Question),
    }
}

/// `Minimal` is Bar; Ring, Off and Full are Ring.
pub fn indicator_of(look: &LookStore) -> Indicator {
    match look.agent_indicator() {
        AgentIndicator::Minimal => Indicator::Bar,
        _ => Indicator::Ring,
    }
}

/// What the marks wear now: a pick where there is one, else the theme's. Failed is always the theme's.
pub fn current_palette(
    look: &AppliedLook,
    themed: &AgentColours,
    picks: &Picks,
    rainbow: bool,
) -> MarkPalette {
    let colours = MarkColours {
        working: picks
            .working
            .clone()
            .unwrap_or_else(|| themed.working.clone()),
        finished: picks
            .finished
            .clone()
            .unwrap_or_else(|| themed.finished.clone()),
        question: picks
            .question
            .clone()
            .unwrap_or_else(|| themed.question.clone()),
        failed: themed.failed.clone(),
    };
    let text = look
        .chrome
        .vars
        .get("--p-text")
        .cloned()
        .unwrap_or_default();
    mark_palette(
        &colours,
        &[look.solid_ground.as_str()],
        &look.solid_ground,
        &text,
        rainbow,
    )
}

/// The marks as Slint colours.
#[derive(Clone, Debug, PartialEq)]
pub struct UiMarks {
    pub working: slint::Color,
    pub finished: slint::Color,
    pub question: slint::Color,
    pub failed: slint::Color,
    /// The seven rainbow stops when the rainbow is on, else empty.
    pub rainbow: Vec<slint::Color>,
    /// The name's ink (the look's `--p-text`).
    pub ink: slint::Color,
}

/// Every colour the marks wear, as Slint colours.
pub fn ui_marks(p: &MarkPalette, rainbow: bool, text: &str) -> UiMarks {
    UiMarks {
        working: parse_hex(&p.line.working),
        finished: parse_hex(&p.line.done),
        question: parse_hex(&p.line.question),
        failed: parse_hex(&p.line.failed),
        rainbow: if rainbow {
            p.rainbow_stops.iter().map(|c| parse_hex(c)).collect()
        } else {
            Vec::new()
        },
        ink: parse_hex(text),
    }
}

/// The line colour of a mark state.
pub fn colour_of(m: &UiMarks, state: MarkState) -> slint::Color {
    match state {
        MarkState::Working => m.working,
        MarkState::Done => m.finished,
        MarkState::Question => m.question,
        MarkState::Failed => m.failed,
    }
}

/// `#rrggbb` or `#rrggbbaa`; an alpha-less colour is opaque. Anything else is black.
fn parse_hex(s: &str) -> slint::Color {
    let digits = s.strip_prefix('#').unwrap_or(s);
    let byte = |i: usize| u8::from_str_radix(digits.get(i..i + 2).unwrap_or("00"), 16).unwrap_or(0);
    match digits.len() {
        6 => slint::Color::from_rgb_u8(byte(0), byte(2), byte(4)),
        8 => slint::Color::from_argb_u8(byte(6), byte(0), byte(2), byte(4)),
        _ => slint::Color::from_rgb_u8(0, 0, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flare_settings::kv::PrefStore;
    use flare_theme::agent_colours::theme_agent_colours;
    use flare_theme::ansi::contrast_ratio;
    use flare_theme::edges::WindowEdges;
    use flare_theme::look::{LookInput, apply_look};
    use flare_theme::resolve::preset_accent;

    fn look_of(theme_id: &str) -> AppliedLook {
        apply_look(&LookInput {
            theme_id,
            custom: None,
            draft: None,
            background: None,
            accent: None,
            secondary: None,
            edges: WindowEdges::Hairline,
            see_through: 0,
            glass_possible: false,
        })
    }

    fn themed_of(look: &AppliedLook) -> AgentColours {
        let accent = preset_accent(&look.theme_id, &look.theme_id, None, None);
        theme_agent_colours(&look.term, accent.as_deref(), None, None)
    }

    fn palette(look: &AppliedLook, picks: &Picks, rainbow: bool) -> MarkPalette {
        current_palette(look, &themed_of(look), picks, rainbow)
    }

    fn ratio(c: slint::Color, ground: &str) -> f64 {
        let hex = format!("#{:02x}{:02x}{:02x}", c.red(), c.green(), c.blue());
        contrast_ratio(&hex, ground)
    }

    #[test]
    fn working_is_the_chrome_accent_until_picked() {
        let look = look_of("pt-default");
        let themed = themed_of(&look);
        let m = ui_marks(&palette(&look, &Picks::default(), false), false, "#ffffff");
        let accent = look.chrome.vars.get("--p-accent-solid").cloned().unwrap();
        assert_eq!(themed.working, accent);
        assert_eq!(m.working, parse_hex(&accent));
    }

    #[test]
    fn a_pick_replaces_the_theme_and_the_line_is_floored_on_the_ground() {
        let look = look_of("paper");
        let picks = Picks {
            working: Some("#d8ff26".into()),
            ..Picks::default()
        };
        let m = ui_marks(&palette(&look, &picks, false), false, "#000000");
        assert!(ratio(m.working, &look.solid_ground) >= 3.0);
        assert_ne!(
            m.working,
            ui_marks(&palette(&look, &Picks::default(), false), false, "#000000").working
        );
    }

    #[test]
    fn finished_and_question_follow_picks_failed_never_does() {
        let look = look_of("pt-default");
        let picks = Picks {
            finished: Some("#00aa55".into()),
            question: Some("#ff00ff".into()),
            ..Picks::default()
        };
        let with = ui_marks(&palette(&look, &picks, false), false, "#ffffff");
        let without = ui_marks(&palette(&look, &Picks::default(), false), false, "#ffffff");
        let expect_finished = ui_marks(
            &palette(
                &look,
                &Picks {
                    finished: Some("#00aa55".into()),
                    ..Picks::default()
                },
                false,
            ),
            false,
            "#ffffff",
        );
        assert_eq!(with.finished, expect_finished.finished);
        assert_ne!(with.finished, without.finished);
        assert_ne!(with.question, without.question);
        assert_eq!(with.failed, without.failed);
    }

    #[test]
    fn the_rainbow_is_the_seven_when_on_and_empty_when_off() {
        let look = look_of("pt-default");
        let on = ui_marks(&palette(&look, &Picks::default(), true), true, "#ffffff");
        let off = ui_marks(&palette(&look, &Picks::default(), false), false, "#ffffff");
        assert_eq!(on.rainbow.len(), 7);
        assert_eq!(on.rainbow[0], parse_hex("#12cee5"));
        assert!(off.rainbow.is_empty());
    }

    #[test]
    fn a_see_through_pick_is_laid_on_the_ground() {
        let look = look_of("pt-default");
        let picks = Picks {
            working: Some("#e07a2f80".into()),
            ..Picks::default()
        };
        let m = ui_marks(&palette(&look, &picks, false), false, "#ffffff");
        assert_eq!(m.working.alpha(), 255);
    }

    #[test]
    fn ink_is_the_names_ink_of_the_look() {
        let look = look_of("pt-default");
        let text = look.chrome.vars.get("--p-text").cloned().unwrap();
        let m = ui_marks(&palette(&look, &Picks::default(), false), false, &text);
        assert_eq!(m.ink, parse_hex(&text));
    }

    #[test]
    fn picks_and_the_indicator_come_from_the_look_store() {
        let look = LookStore::new(PrefStore::memory());
        let fresh = picks_from(&look);
        assert_eq!(fresh, Picks::default());
        assert_eq!(indicator_of(&look), Indicator::Ring);
        look.set_agent_colour(AgentColourState::Working, "#E07A2F");
        assert_eq!(picks_from(&look).working.as_deref(), Some("#e07a2f"));
        look.set_agent_indicator(AgentIndicator::Minimal);
        assert_eq!(indicator_of(&look), Indicator::Bar);
    }

    #[test]
    fn a_look_change_changes_the_palette() {
        let a = look_of("pt-default");
        let b = look_of("paper");
        let ma = ui_marks(&palette(&a, &Picks::default(), false), false, "#ffffff");
        let mb = ui_marks(&palette(&b, &Picks::default(), false), false, "#000000");
        assert_ne!(ma.working, mb.working);
        assert!(ratio(mb.finished, &b.solid_ground) >= 3.0);
        assert!(ratio(ma.finished, &a.solid_ground) >= 3.0);
    }

    #[test]
    fn colour_of_maps_each_state_to_its_line() {
        let look = look_of("pt-default");
        let m = ui_marks(&palette(&look, &Picks::default(), false), false, "#ffffff");
        assert_eq!(colour_of(&m, MarkState::Working), m.working);
        assert_eq!(colour_of(&m, MarkState::Done), m.finished);
        assert_eq!(colour_of(&m, MarkState::Question), m.question);
        assert_eq!(colour_of(&m, MarkState::Failed), m.failed);
    }
}

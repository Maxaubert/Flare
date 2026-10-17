//! The page models of the Settings tab (M7 Task 19).
//!
//! Every page is DATA: a list of sections, each a list of blocks, each row a `RowModel`. The
//! Slint side draws what these models hold; the pages are tested here without a window.

pub mod agents;
pub mod appearance;
pub mod misc;
pub mod terminal;

use crate::kv::PrefStore;
use crate::look::LookStore;
use crate::rows::RowDef;
use crate::sections::section_title;
use crate::verb::VerbState;
use flare_theme::colour_contrast::AlphaRange;

/// One option of a segmented or select control. `face` is "" for the UI font.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Opt {
    pub id: String,
    pub name: String,
    pub face: String,
}

/// The control a row draws.
#[derive(Clone, Debug, PartialEq)]
pub enum Control {
    None,
    Pill(String),
    Switch {
        on: bool,
    },
    Segmented {
        value: String,
        options: Vec<Opt>,
    },
    Select {
        value: String,
        options: Vec<Opt>,
    },
    Slider {
        value: u32,
        max: u32,
        text: String,
        disabled: bool,
    },
    /// `value` is the stored form in force (the picked one, else the theme's).
    Colour {
        value: String,
        range: AlphaRange,
        alpha_disabled: bool,
    },
    Save {
        dirty: bool,
        title: String,
    },
}

/// A small text button beside a row's control (ROW_BUTTON, 32 high).
#[derive(Clone, Debug, PartialEq)]
pub struct Button {
    pub id: &'static str,
    pub label: String,
    pub alt: Option<String>,
    pub off: bool,
}

/// One row of a page, ready to draw.
#[derive(Clone, Debug, PartialEq)]
pub struct RowModel {
    pub id: &'static str,
    pub icon: &'static str,
    pub label: String,
    pub sub: String,
    pub warn: bool,
    pub off: bool,
    /// A press anywhere on the row flips it (switch rows).
    pub tap: bool,
    pub row_tip: String,
    pub control: Control,
    pub reset: bool,
    pub buttons: Vec<Button>,
}

/// One block of a section's panel.
#[derive(Clone, Debug, PartialEq)]
pub enum BlockModel {
    Row(RowModel),
    Wall,
    AboutCard {
        name: String,
        line_one: String,
        line_two: String,
    },
    SlintBadge,
    Note(String),
}

/// The Save changes action in a section heading (Mark colours).
#[derive(Clone, Debug, PartialEq)]
pub struct HeadingAction {
    pub dirty: bool,
    pub title: String,
}

/// One section: heading, optional tag, optional heading action, its blocks.
#[derive(Clone, Debug, PartialEq)]
pub struct SectionModel {
    pub id: &'static str,
    pub title: String,
    pub tag: Option<String>,
    pub action: Option<HeadingAction>,
    pub blocks: Vec<BlockModel>,
}

/// One page of the Settings tab.
#[derive(Clone, Debug, PartialEq)]
pub struct PageModel {
    pub id: &'static str,
    pub label: &'static str,
    pub sections: Vec<SectionModel>,
}

/// What the running Windows and renderer can do for the glass rows (M4's checks).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlassSupport {
    Ok,
    NeedsWindows11,
    NeedsWindows11Update,
    NeedsGraphicsRenderer,
}

/// The diagnostics switch and its folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagInfo {
    pub verbose: bool,
    pub dir: String,
}

/// What the theme in force gives the window (the swatch while nothing is picked).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThemeColours {
    pub accent: String,
    pub background: String,
    pub secondary: String,
}

/// Everything a page is built from. Pages read it and change nothing.
pub struct PageEnv<'a> {
    pub prefs: &'a PrefStore,
    pub look: &'a LookStore,
    /// `None` until detection answers: the Shell section is not drawn.
    pub shells: Option<Vec<(String, String)>>,
    pub verb: VerbState,
    pub home: String,
    pub version: String,
    pub copyright: String,
    pub glass: GlassSupport,
    pub diag: DiagInfo,
    pub diag_marked: bool,
    pub nvidia: bool,
    pub dictation_wired: bool,
    pub theme_colours: ThemeColours,
    /// Working, finished and question: the agent colours in force.
    pub agent_colours: [String; 3],
    /// The agent chords' labels from the key table, joined by ", " (the row's tooltip).
    pub agent_key_labels: String,
    /// Font id to the first installed family of its stack.
    pub face_of: &'a dyn Fn(&str) -> String,
}

/// The Dictation page's content. M7 ships only the placeholder (decision D7); M8 replaces it.
pub trait DictationPage {
    fn sections(&self, env: &PageEnv<'_>) -> Vec<SectionModel>;
}

/// The M7 Dictation page: one note, no controls.
pub struct PlaceholderDictation;

impl DictationPage for PlaceholderDictation {
    fn sections(&self, _env: &PageEnv<'_>) -> Vec<SectionModel> {
        vec![section(
            "dictation",
            vec![BlockModel::Note(
                "Dictation arrives in a later update.".to_string(),
            )],
        )]
    }
}

/// The row with this id, in any section of the page.
pub fn row_of<'p>(page: &'p PageModel, id: &str) -> Option<&'p RowModel> {
    page.sections
        .iter()
        .flat_map(|s| s.blocks.iter())
        .find_map(|b| match b {
            BlockModel::Row(r) if r.id == id => Some(r),
            _ => None,
        })
}

/// Every row id of the page, in drawn order.
pub fn all_row_ids(page: &PageModel) -> Vec<&'static str> {
    page.sections
        .iter()
        .flat_map(|s| s.blocks.iter())
        .filter_map(|b| match b {
            BlockModel::Row(r) => Some(r.id),
            _ => None,
        })
        .collect()
}

// The page builders of Tasks 20 to 23 are the first callers; until then only the tests use these.
/// A row from its table entry, with the given control. Not a tap row.
#[allow(dead_code)]
pub(crate) fn plain_row(def: &RowDef, control: Control) -> RowModel {
    RowModel {
        id: def.id,
        icon: def.icon,
        label: def.label.to_string(),
        sub: def.sub.to_string(),
        warn: false,
        off: false,
        tap: false,
        row_tip: String::new(),
        control,
        reset: false,
        buttons: Vec::new(),
    }
}

/// A switch row. A press anywhere on the row flips it.
#[allow(dead_code)]
pub(crate) fn switch_row(def: &RowDef, on: bool) -> RowModel {
    RowModel {
        tap: true,
        ..plain_row(def, Control::Switch { on })
    }
}

/// Options from (id, name) pairs, in the UI font.
#[allow(dead_code)]
pub(crate) fn opts(pairs: &[(&str, &str)]) -> Vec<Opt> {
    pairs
        .iter()
        .map(|(id, name)| Opt {
            id: (*id).to_string(),
            name: (*name).to_string(),
            face: String::new(),
        })
        .collect()
}

/// A section of a page. Its heading comes from the section table; an unknown id has none.
pub(crate) fn section(id: &'static str, blocks: Vec<BlockModel>) -> SectionModel {
    SectionModel {
        id,
        title: section_title(id).unwrap_or("").to_string(),
        tag: None,
        action: None,
        blocks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(_: &str) -> String {
        String::new()
    }

    fn with_env<R>(f: impl FnOnce(&PageEnv<'_>) -> R) -> R {
        let prefs = PrefStore::memory();
        let look = LookStore::new(PrefStore::memory());
        let face_of: &dyn Fn(&str) -> String = &face;
        let env = PageEnv {
            prefs: &prefs,
            look: &look,
            shells: None,
            verb: VerbState::Busy,
            home: String::new(),
            version: "0.0.0".to_string(),
            copyright: String::new(),
            glass: GlassSupport::Ok,
            diag: DiagInfo {
                verbose: false,
                dir: String::new(),
            },
            diag_marked: false,
            nvidia: false,
            dictation_wired: false,
            theme_colours: ThemeColours {
                accent: String::new(),
                background: String::new(),
                secondary: String::new(),
            },
            agent_colours: [String::new(), String::new(), String::new()],
            agent_key_labels: String::new(),
            face_of,
        };
        f(&env)
    }

    #[test]
    fn the_placeholder_dictation_page_is_one_note() {
        with_env(|env| {
            let sections = PlaceholderDictation.sections(env);
            assert_eq!(sections.len(), 1);
            assert_eq!(sections[0].id, "dictation");
            assert_eq!(sections[0].blocks.len(), 1);
            assert!(matches!(
                &sections[0].blocks[0],
                BlockModel::Note(text) if text == "Dictation arrives in a later update."
            ));
        });
    }

    #[test]
    fn plain_row_takes_label_sub_and_icon_from_the_table() {
        let def = crate::rows::row("tab-layout").expect("tab-layout is a row");
        let r = plain_row(def, Control::None);
        assert_eq!(r.id, "tab-layout");
        assert_eq!(r.label, def.label);
        assert_eq!(r.sub, def.sub);
        assert_eq!(r.icon, def.icon);
        assert!(!r.tap);
        assert!(!r.off);
        assert!(!r.warn);
    }

    #[test]
    fn a_switch_row_is_tap_and_a_plain_row_is_not() {
        let def = crate::rows::row("tab-switch").expect("tab-switch is a row");
        let on = switch_row(def, true);
        assert!(on.tap);
        assert_eq!(on.control, Control::Switch { on: true });
        let off = switch_row(def, false);
        assert_eq!(off.control, Control::Switch { on: false });
        assert!(!plain_row(def, Control::None).tap);
    }
}

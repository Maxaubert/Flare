//! The page models of the Settings tab (M7 Task 19).
//!
//! Every page is DATA: a list of sections, each a list of blocks, each row a `RowModel`. The
//! Slint side draws what these models hold; the pages are tested here without a window.

pub mod agents;
pub mod appearance;
pub mod dictation;
pub mod misc;
pub mod mods;
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
    /// The dictation key: the binding's text, or the prompt while the key is being captured.
    Key {
        text: String,
        capturing: bool,
    },
    /// A one-line text field (plugins phase 7: the address to install a plugin from): what is typed, and the faint
    /// words shown while it is empty.
    Text {
        value: String,
        placeholder: String,
    },
    /// The microphone picker (`""` is the system default) and its test meter.
    Mic {
        value: String,
        options: Vec<Opt>,
        testing: bool,
        level: f32,
        problem: Option<String>,
    },
}

/// A small text button beside a row's control (ROW_BUTTON, 32 high). With an `icon` (a name in `icons`) and an empty
/// `label` it is a square icon button; `tip` is said on hover (an icon button's only words, so it must be set).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Button {
    pub id: &'static str,
    pub label: String,
    pub alt: Option<String>,
    pub off: bool,
    pub icon: &'static str,
    pub tip: String,
    /// The icon is drawn in the warning colour (a plugin's info button while it has warnings).
    pub warn: bool,
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
    /// A logo drawn in place of the icon tile (a plugin's brand, on its card); None draws `icon`.
    pub logo: Option<Logo>,
}

/// A plugin's logo: SVG path data on the 24-unit box, filled in `color` (`#rrggbb`) on a white rounded tile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Logo {
    pub path: String,
    pub color: String,
}

/// One block of a section's panel.
#[derive(Clone, Debug, PartialEq)]
pub enum BlockModel {
    Row(RowModel),
    /// One downloadable speech model in the Speech models card (M8 Task 24).
    Model(dictation::ModelRow),
    /// The NVIDIA GPU pack card (drawn only with an NVIDIA adapter).
    Gpu(dictation::GpuRow),
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
    /// What the Mods page shows (Surfaces stage 8); `None` builds it empty.
    pub mods: Option<&'a mods::ModsInfo>,
}

/// The Dictation page's content. The app's provider (`flare`'s `dictation_page`, M8 Task 24) fills it.
pub trait DictationPage {
    fn sections(&self, env: &PageEnv<'_>) -> Vec<SectionModel>;
}

/// A Dictation page with no sections, for this crate's own tests (the page's rows are the app's).
#[cfg(test)]
pub(crate) struct NoDictation;

#[cfg(test)]
impl DictationPage for NoDictation {
    fn sections(&self, _env: &PageEnv<'_>) -> Vec<SectionModel> {
        Vec::new()
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
        logo: None,
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

/// The page for a rail id (M7 Task 23). An unknown id builds Appearance.
pub fn build_page(id: &str, env: &PageEnv<'_>, dictation: &dyn DictationPage) -> PageModel {
    match id {
        "terminal" => terminal::build_terminal(env),
        "agents" => agents::build(env),
        "dictation" => misc::build_dictation(env, dictation),
        "mods" => mods::build(env),
        "diagnostics" => misc::build_diagnostics(env),
        "about" => misc::build_about(env),
        _ => appearance::build(env),
    }
}

/// The fixture the page tests share: a page environment over fresh stores.
#[cfg(test)]
pub(crate) mod test_env {
    use super::*;

    fn face(_: &str) -> String {
        String::new()
    }

    /// A page environment with the given version, copyright, diagnostics state and mark, and
    /// the shells detection found (`None` until it answers).
    pub(crate) fn with_env_full<R>(
        version: &str,
        copyright: &str,
        diag: DiagInfo,
        diag_marked: bool,
        shells: Option<Vec<(String, String)>>,
        f: impl FnOnce(&PageEnv<'_>) -> R,
    ) -> R {
        with_env_look(
            version,
            copyright,
            diag,
            diag_marked,
            shells,
            &LookStore::new(PrefStore::memory()),
            f,
        )
    }

    /// As `with_env_full`, over this look.
    fn with_env_look<R>(
        version: &str,
        copyright: &str,
        diag: DiagInfo,
        diag_marked: bool,
        shells: Option<Vec<(String, String)>>,
        look: &LookStore,
        f: impl FnOnce(&PageEnv<'_>) -> R,
    ) -> R {
        let prefs = PrefStore::memory();
        let face_of: &dyn Fn(&str) -> String = &face;
        let env = PageEnv {
            prefs: &prefs,
            look,
            shells,
            verb: VerbState::Busy,
            home: String::new(),
            version: version.to_string(),
            copyright: copyright.to_string(),
            glass: GlassSupport::Ok,
            diag,
            diag_marked,
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
            mods: None,
        };
        f(&env)
    }

    /// A page environment that holds this Mods page content.
    pub(crate) fn with_env_mods<R>(info: mods::ModsInfo, f: impl FnOnce(&PageEnv<'_>) -> R) -> R {
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
            mods: Some(&info),
        };
        f(&env)
    }

    /// A page environment with the given version, copyright, diagnostics state and mark; no
    /// shells yet.
    pub(crate) fn with_env_fields<R>(
        version: &str,
        copyright: &str,
        diag: DiagInfo,
        diag_marked: bool,
        f: impl FnOnce(&PageEnv<'_>) -> R,
    ) -> R {
        with_env_full(version, copyright, diag, diag_marked, None, f)
    }

    /// A page environment with default values and one shell found, so the Shell section draws.
    pub(crate) fn with_env<R>(f: impl FnOnce(&PageEnv<'_>) -> R) -> R {
        with_env_full(
            "0.0.0",
            "",
            DiagInfo {
                verbose: false,
                dir: String::new(),
            },
            false,
            Some(vec![("pwsh".to_string(), "PowerShell".to_string())]),
            f,
        )
    }

    /// As `with_env`, with the tab marks on Bar, so the Agents page draws its Mark colours.
    pub(crate) fn with_env_bar<R>(f: impl FnOnce(&PageEnv<'_>) -> R) -> R {
        let look = LookStore::new(PrefStore::memory());
        look.set_agent_indicator(crate::look::AgentIndicator::Minimal);
        with_env_look(
            "0.0.0",
            "",
            DiagInfo {
                verbose: false,
                dir: String::new(),
            },
            false,
            Some(vec![("pwsh".to_string(), "PowerShell".to_string())]),
            &look,
            f,
        )
    }
}

#[cfg(test)]
mod cross_page_tests {
    use super::test_env::{with_env, with_env_bar};
    use super::*;
    use crate::rows::{DICTATION_OPTIONS, ROW_ORDER, row};
    use std::collections::BTreeSet;

    const SIX: [&str; 6] = [
        "appearance",
        "terminal",
        "agents",
        "dictation",
        "diagnostics",
        "about",
    ];

    #[test]
    fn build_page_for_an_unknown_id_builds_appearance() {
        with_env(|env| {
            assert_eq!(build_page("nope", env, &NoDictation).id, "appearance");
            for id in SIX {
                assert_eq!(build_page(id, env, &NoDictation).id, id);
            }
        });
    }

    #[test]
    fn every_row_of_every_page_is_in_the_row_order_exactly_once() {
        // With Bar, so the Agents page draws its Mark colours (Ring hides them).
        with_env_bar(|env| {
            let drawn: Vec<&str> = SIX
                .iter()
                .flat_map(|id| all_row_ids(&build_page(id, env, &NoDictation)))
                .collect();
            let unique: BTreeSet<&str> = drawn.iter().copied().collect();
            assert_eq!(unique.len(), drawn.len(), "a row is drawn twice: {drawn:?}");
            let expected: BTreeSet<&str> = ROW_ORDER
                .iter()
                .copied()
                .filter(|id| !DICTATION_OPTIONS.iter().any(|d| d.id == *id))
                .collect();
            assert_eq!(unique, expected);
        });
    }

    #[test]
    fn the_drawn_order_inside_each_section_follows_the_tables() {
        with_env(check_drawn_order);
        with_env_bar(check_drawn_order);
    }

    fn check_drawn_order(env: &PageEnv<'_>) {
        for id in SIX {
            let page = build_page(id, env, &NoDictation);
            for section in &page.sections {
                let drawn: Vec<&str> = section
                    .blocks
                    .iter()
                    .filter_map(|b| match b {
                        BlockModel::Row(r) => Some(r.id),
                        _ => None,
                    })
                    .collect();
                // The placeholder Dictation page draws none of the nine dictation rows.
                let expected: Vec<&str> = ROW_ORDER
                    .iter()
                    .copied()
                    .filter(|rid| !DICTATION_OPTIONS.iter().any(|d| d.id == *rid))
                    .filter(|rid| row(rid).is_some_and(|d| d.section == section.id))
                    .collect();
                assert_eq!(drawn, expected, "section {} on page {id}", section.id);
            }
        }
    }
}

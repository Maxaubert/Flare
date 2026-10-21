//! The Appearance page model (M7 Task 20).
//! Source: PT `AppearancePage.tsx` (the window section, the theme section and its window colours),
//! `TerminalThemeSection.tsx` (the wall, the Save changes row), `opts.ts` and `fields.tsx` (`SaveButton`).

use flare_theme::colour_contrast::AlphaRange;

use super::{BlockModel, Control, GlassSupport, Opt, PageEnv, PageModel, RowModel, ThemeColours};
use crate::kv::PrefStore;
use crate::look::LookStore;

/// The Save changes title while something differs from what Custom keeps.
pub const SAVE_TITLE: &str = "Saves the theme, agent colours and see-through window as Custom";
/// The Save changes title while nothing differs.
pub const SAVE_CLEAN: &str = "Nothing to save yet";

/// The most see-through the slider offers (PT's slider runs 0 to 70).
const SEE_THROUGH_MAX: u32 = 70;

/// The colours the theme in force gives the window, computed the way the window computes them (PT
/// `themeColours`): the accent is measured against the picked Background when there is one. The
/// caller stores the result in `PageEnv::theme_colours`.
pub fn theme_colours(look: &LookStore, prefs: &PrefStore, _glass_possible: bool) -> ThemeColours {
    use crate::window_colours::{BACKGROUND, window_edges};
    use flare_theme::chrome::chrome_tokens;
    use flare_theme::resolve::{preset_accent, resolve_saved_theme};

    let id = look.theme_id();
    let custom = look.custom().map(|c| c.palette);
    let theme = resolve_saved_theme(&id, custom.as_ref(), None);
    let wanted = preset_accent(&id, &id, custom.as_ref(), None);
    let edges = window_edges(prefs);

    let mut picked = theme.clone();
    if let Some(bg) = BACKGROUND.get(prefs).filter(|b| !b.is_empty()) {
        picked.background = bg;
    }
    let with_pick = chrome_tokens(&picked, 1.0, wanted.as_deref(), edges, None, None).vars;
    let own = chrome_tokens(&theme, 1.0, wanted.as_deref(), edges, None, None).vars;
    let take = |vars: &std::collections::BTreeMap<&'static str, String>, key: &str| {
        vars.get(key).cloned().unwrap_or_default()
    };
    ThemeColours {
        accent: take(&with_pick, "--p-accent"),
        background: take(&own, "--p-bg-solid"),
        secondary: take(&own, "--p-secondary"),
    }
}

/// Whether the three window colours in force differ from what Custom keeps (PT `coloursDiffer`).
/// Outside Custom, nothing is kept, so any picked colour counts.
pub fn extra_dirty(prefs: &PrefStore, look: &LookStore) -> bool {
    use crate::window_colours::{
        ACCENT, BACKGROUND, NO_COLOURS, SECONDARY, WindowColours, colours_differ,
        custom_window_colours,
    };

    let now = WindowColours {
        background: BACKGROUND.get(prefs),
        accent: ACCENT.get(prefs),
        secondary: SECONDARY.get(prefs),
    };
    let kept = if look.theme_id() == "custom" {
        custom_window_colours(prefs)
    } else {
        NO_COLOURS
    };
    colours_differ(&now, &kept)
}

/// Save changes lights when the terminal setup or the window colours deviate from what is kept.
pub fn save_dirty(prefs: &PrefStore, look: &LookStore) -> bool {
    look.setup_state().dirty || extra_dirty(prefs, look)
}

/// The row table entry of an id that is in the table (the tests and this page both rely on it).
fn def(id: &str) -> &'static crate::rows::RowDef {
    crate::rows::row(id).expect("the Appearance page draws only rows of the table")
}

/// A row with its table label and subtext, with a different subtext where the choice in force says more.
fn row_with(id: &str, control: Control) -> RowModel {
    super::plain_row(def(id), control)
}

fn segmented(value: &str, options: Vec<Opt>) -> Control {
    Control::Segmented {
        value: value.to_string(),
        options,
    }
}

fn edge_name(word: &str) -> &'static str {
    match word {
        "none" => "None",
        "faint" => "Faint",
        "hairline" => "Hairline",
        _ => "Solid",
    }
}

/// The see-through row: a 0 to 70 slider that reads Off at 0. Without glass the row is off and says why.
fn see_through_row(env: &PageEnv) -> RowModel {
    let table = def("term-acrylic");
    let (value, sub) = match env.glass {
        GlassSupport::Ok => (u32::from(env.look.see_through()), table.sub.to_string()),
        GlassSupport::NeedsWindows11 => (0, "Needs Windows 11.".to_string()),
        GlassSupport::NeedsWindows11Update => (0, "Needs Windows 11 22H2.".to_string()),
        GlassSupport::NeedsGraphicsRenderer => (0, "Needs the graphics renderer.".to_string()),
    };
    let off = env.glass != GlassSupport::Ok;
    let text = if value == 0 {
        "Off".to_string()
    } else {
        format!("{value}%")
    };
    RowModel {
        off,
        sub,
        control: Control::Slider {
            value,
            max: SEE_THROUGH_MAX,
            text,
            disabled: off,
        },
        ..row_with("term-acrylic", Control::None)
    }
}

/// A window colour row: the picked colour, or the theme's while nothing is picked. Reset shows only when picked.
fn colour_row(id: &str, picked: Option<String>, from_theme: &str, range: AlphaRange) -> RowModel {
    let reset = picked.is_some();
    let value = picked.unwrap_or_else(|| from_theme.to_string());
    RowModel {
        reset,
        control: Control::Colour {
            value,
            range,
            alpha_disabled: false,
        },
        ..row_with(id, Control::None)
    }
}

/// The Appearance page: the window section, then the theme section (the wall and the see-through row
/// under the theme row, the window's own colours under the wall).
pub fn build(env: &PageEnv) -> PageModel {
    use crate::tab_layout::{TabLayout, tab_layout};
    use crate::tab_switch::{TabSwitch, tab_switch};
    use crate::window_colours::{ACCENT, BACKGROUND, SECONDARY, window_edges};
    use flare_theme::edges::WINDOW_EDGES;

    let layout_word = match tab_layout(env.prefs) {
        TabLayout::Horizontal => "horizontal",
        TabLayout::Vertical => "vertical",
    };
    let layout = row_with(
        "tab-layout",
        segmented(
            layout_word,
            super::opts(&[("horizontal", "Horizontal"), ("vertical", "Vertical")]),
        ),
    );

    let switching = tab_switch(env.prefs);
    let (switch_word, switch_sub) = match switching {
        TabSwitch::Order => ("order", "The next tab along the strip."),
        TabSwitch::Recent => ("recent", "Back to the tab you used last."),
    };
    let switch = RowModel {
        sub: switch_sub.to_string(),
        ..row_with(
            "tab-switch",
            segmented(
                switch_word,
                super::opts(&[("order", "In order"), ("recent", "Most recent")]),
            ),
        )
    };

    let edges_word = window_edges(env.prefs).word();
    let edge_options: Vec<Opt> = WINDOW_EDGES
        .iter()
        .map(|e| Opt {
            id: e.word().to_string(),
            name: edge_name(e.word()).to_string(),
            face: String::new(),
        })
        .collect();
    let edges = row_with("window-edges", segmented(edges_word, edge_options));

    let window = super::section(
        "window",
        vec![
            BlockModel::Row(layout),
            BlockModel::Row(switch),
            BlockModel::Row(edges),
        ],
    );

    let dirty = save_dirty(env.prefs, env.look);
    let title = if dirty { SAVE_TITLE } else { SAVE_CLEAN };
    let theme_row = row_with(
        "term-theme",
        Control::Save {
            dirty,
            title: title.to_string(),
        },
    );

    let wide = AlphaRange {
        alpha: false,
        ..AlphaRange::default()
    };
    let accent_range = AlphaRange {
        alpha_min: 0.1,
        ..AlphaRange::default()
    };
    let theme = super::section(
        "theme",
        vec![
            BlockModel::Row(theme_row),
            BlockModel::Wall,
            BlockModel::Row(see_through_row(env)),
            BlockModel::Row(colour_row(
                "window-background",
                BACKGROUND.get(env.prefs),
                &env.theme_colours.background,
                wide,
            )),
            BlockModel::Row(colour_row(
                "window-secondary",
                SECONDARY.get(env.prefs),
                &env.theme_colours.secondary,
                wide,
            )),
            BlockModel::Row(colour_row(
                "window-accent",
                ACCENT.get(env.prefs),
                &env.theme_colours.accent,
                accent_range,
            )),
        ],
    );

    PageModel {
        id: "appearance",
        label: "Appearance",
        sections: vec![window, theme],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::custom_theme::parse_custom;
    use crate::kv::PrefStore;
    use crate::look::LookStore;
    use crate::page::{BlockModel, Control, DiagInfo, GlassSupport, PageEnv, ThemeColours, row_of};
    use crate::tab_switch::{TabSwitch, set_tab_switch};
    use crate::verb::VerbState;
    use crate::window_colours::{ACCENT, BACKGROUND, WindowColours, set_custom_window_colours};

    const CUSTOM_JSON: &str = r##"{"bg":"#111111","fg":"#eeeeee","cursor":"#ffffff","ansi":{}}"##;

    fn no_face(_: &str) -> String {
        String::new()
    }

    fn colours() -> ThemeColours {
        ThemeColours {
            accent: "#fe8f34".to_string(),
            background: "#171717".to_string(),
            secondary: "#0d0d0d".to_string(),
        }
    }

    /// Runs `f` on an environment over the given stores and glass answer.
    fn with_env<R>(
        prefs: &PrefStore,
        look: &LookStore,
        glass: GlassSupport,
        f: impl FnOnce(&PageEnv<'_>) -> R,
    ) -> R {
        let face_of: &dyn Fn(&str) -> String = &no_face;
        let env = PageEnv {
            prefs,
            look,
            shells: None,
            verb: VerbState::Busy,
            home: String::new(),
            version: "0.0.0".to_string(),
            copyright: String::new(),
            glass,
            diag: DiagInfo {
                verbose: false,
                dir: String::new(),
            },
            diag_marked: false,
            nvidia: false,
            dictation_wired: false,
            theme_colours: colours(),
            agent_colours: [String::new(), String::new(), String::new()],
            agent_key_labels: String::new(),
            face_of,
        };
        f(&env)
    }

    fn fresh() -> (PrefStore, LookStore) {
        let p = PrefStore::memory();
        (p.clone(), LookStore::new(p))
    }

    fn page(prefs: &PrefStore, look: &LookStore, glass: GlassSupport) -> PageModel {
        with_env(prefs, look, glass, build)
    }

    fn theme_blocks(p: &PageModel) -> &[BlockModel] {
        &p.sections
            .iter()
            .find(|s| s.id == "theme")
            .expect("a theme section")
            .blocks
    }

    fn row<'a>(p: &'a PageModel, id: &str) -> &'a RowModel {
        row_of(p, id).unwrap_or_else(|| panic!("row {id} is drawn"))
    }

    #[test]
    fn the_window_section_comes_first_then_the_theme_section() {
        let (prefs, look) = fresh();
        let p = page(&prefs, &look, GlassSupport::Ok);
        let ids: Vec<&str> = p.sections.iter().map(|s| s.id).collect();
        assert_eq!(ids, vec!["window", "theme"]);
        let window: Vec<&str> = p.sections[0]
            .blocks
            .iter()
            .filter_map(|b| match b {
                BlockModel::Row(r) => Some(r.id),
                _ => None,
            })
            .collect();
        assert_eq!(window, vec!["tab-layout", "tab-switch", "window-edges"]);
    }

    #[test]
    fn tab_switching_says_what_the_choice_in_force_does() {
        let (prefs, look) = fresh();
        set_tab_switch(&prefs, TabSwitch::Order);
        let p = page(&prefs, &look, GlassSupport::Ok);
        assert_eq!(row(&p, "tab-switch").sub, "The next tab along the strip.");
        set_tab_switch(&prefs, TabSwitch::Recent);
        let p = page(&prefs, &look, GlassSupport::Ok);
        assert_eq!(row(&p, "tab-switch").sub, "Back to the tab you used last.");
    }

    #[test]
    fn edges_are_offered_weakest_to_strongest() {
        let (prefs, look) = fresh();
        let p = page(&prefs, &look, GlassSupport::Ok);
        let Control::Segmented { options, .. } = &row(&p, "window-edges").control else {
            panic!("the edges row is a segmented control");
        };
        let pairs: Vec<(&str, &str)> = options
            .iter()
            .map(|o| (o.id.as_str(), o.name.as_str()))
            .collect();
        assert_eq!(
            pairs,
            vec![
                ("none", "None"),
                ("faint", "Faint"),
                ("hairline", "Hairline"),
                ("solid", "Solid"),
            ]
        );
    }

    #[test]
    fn the_wall_sits_under_the_theme_row_and_the_see_through_row_under_the_wall() {
        let (prefs, look) = fresh();
        let p = page(&prefs, &look, GlassSupport::Ok);
        let kinds: Vec<&str> = theme_blocks(&p)
            .iter()
            .map(|b| match b {
                BlockModel::Row(_) => "Row",
                BlockModel::Wall => "Wall",
                _ => "other",
            })
            .collect();
        assert_eq!(kinds, vec!["Row", "Wall", "Row", "Row", "Row", "Row"]);
        let ids: Vec<&str> = theme_blocks(&p)
            .iter()
            .filter_map(|b| match b {
                BlockModel::Row(r) => Some(r.id),
                _ => None,
            })
            .collect();
        assert_eq!(
            ids,
            vec![
                "term-theme",
                "term-acrylic",
                "window-background",
                "window-secondary",
                "window-accent",
            ]
        );
    }

    #[test]
    fn the_see_through_row_is_a_slider_0_to_70_reading_off_at_zero() {
        let (prefs, look) = fresh();
        look.set_see_through(0);
        let p = page(&prefs, &look, GlassSupport::Ok);
        let r = row(&p, "term-acrylic");
        let def = crate::rows::row("term-acrylic").expect("the row exists");
        assert_eq!(r.label, def.label);
        assert_eq!(r.sub, "How much of the desktop shows behind every surface.");
        assert!(!r.off);
        assert_eq!(
            r.control,
            Control::Slider {
                value: 0,
                max: 70,
                text: "Off".to_string(),
                disabled: false,
            }
        );
        look.set_see_through(35);
        let p = page(&prefs, &look, GlassSupport::Ok);
        assert_eq!(
            row(&p, "term-acrylic").control,
            Control::Slider {
                value: 35,
                max: 70,
                text: "35%".to_string(),
                disabled: false,
            }
        );
    }

    #[test]
    fn see_through_is_disabled_and_says_why_when_glass_is_not_ok() {
        let (prefs, look) = fresh();
        look.set_see_through(35);
        let cases = [
            (GlassSupport::NeedsWindows11, "Needs Windows 11."),
            (GlassSupport::NeedsWindows11Update, "Needs Windows 11 22H2."),
            (
                GlassSupport::NeedsGraphicsRenderer,
                "Needs the graphics renderer.",
            ),
        ];
        for (glass, text) in cases {
            let p = page(&prefs, &look, glass);
            let r = row(&p, "term-acrylic");
            assert_eq!(r.sub, text);
            assert!(r.off);
            assert_eq!(
                r.control,
                Control::Slider {
                    value: 0,
                    max: 70,
                    text: "Off".to_string(),
                    disabled: true,
                }
            );
        }
    }

    #[test]
    fn reset_shows_only_while_a_colour_is_picked_and_the_swatch_shows_the_themes_until_then() {
        let (prefs, look) = fresh();
        let p = page(&prefs, &look, GlassSupport::Ok);
        let r = row(&p, "window-accent");
        assert!(!r.reset);
        assert_eq!(
            r.control,
            Control::Colour {
                value: "#fe8f34".to_string(),
                range: AlphaRange {
                    alpha_min: 0.1,
                    ..AlphaRange::default()
                },
                alpha_disabled: false,
            }
        );
        ACCENT.set(&prefs, Some("#112233"));
        let p = page(&prefs, &look, GlassSupport::Ok);
        let r = row(&p, "window-accent");
        assert!(r.reset);
        assert!(matches!(
            &r.control,
            Control::Colour { value, .. } if value == "#112233"
        ));
    }

    #[test]
    fn ranges_are_background_and_secondary_opaque_accent_from_ten_percent() {
        let (prefs, look) = fresh();
        let p = page(&prefs, &look, GlassSupport::Ok);
        let range_of = |id: &str| match &row(&p, id).control {
            Control::Colour { range, .. } => *range,
            other => panic!("{id} is a colour row, got {other:?}"),
        };
        assert!(!range_of("window-background").alpha);
        assert!(!range_of("window-secondary").alpha);
        let accent = range_of("window-accent");
        assert!(accent.alpha);
        assert_eq!(accent.alpha_min, 0.1);
    }

    #[test]
    fn save_lights_only_when_the_setup_deviates_from_the_themes_stock() {
        let (prefs, look) = fresh();
        assert!(!save_dirty(&prefs, &look));
        look.set_see_through(35);
        assert!(save_dirty(&prefs, &look));
        look.set_see_through(0);
        assert!(!save_dirty(&prefs, &look));
        // A picked Background is a window colour, not part of the terminal setup.
        BACKGROUND.set(&prefs, Some("#222222"));
        assert!(!look.setup_state().dirty);
    }

    #[test]
    fn a_picked_window_colour_lights_save_and_custom_keeps_it() {
        let (prefs, look) = fresh();
        let custom = parse_custom(CUSTOM_JSON).expect("the fixture parses");
        look.save_custom(&custom);
        look.set_theme_id("custom");
        BACKGROUND.set(&prefs, Some("#222222"));
        set_custom_window_colours(
            &prefs,
            Some(&WindowColours {
                background: Some("#222222".to_string()),
                accent: None,
                secondary: None,
            }),
        );
        assert!(!extra_dirty(&prefs, &look));
        ACCENT.set(&prefs, Some("#445566"));
        assert!(extra_dirty(&prefs, &look));
        assert!(save_dirty(&prefs, &look));
    }

    #[test]
    fn save_title_is_the_sentence_when_dirty_and_nothing_to_save_when_clean() {
        let (prefs, look) = fresh();
        let clean = page(&prefs, &look, GlassSupport::Ok);
        assert_eq!(
            row(&clean, "term-theme").control,
            Control::Save {
                dirty: false,
                title: "Nothing to save yet".to_string(),
            }
        );
        look.set_see_through(35);
        let dirty = page(&prefs, &look, GlassSupport::Ok);
        assert_eq!(
            row(&dirty, "term-theme").control,
            Control::Save {
                dirty: true,
                title: "Saves the theme, agent colours and see-through window as Custom"
                    .to_string(),
            }
        );
    }

    #[test]
    fn the_swatch_colour_of_the_theme_is_computed_not_read_back() {
        let (prefs, look) = fresh();
        let c = theme_colours(&look, &prefs, true);
        assert_eq!(c.accent, "#fe8f34");
        assert_eq!(c.background, "#171717");
        assert_eq!(c.secondary, "#0d0d0d");
        // A picked Background changes the accent only through the chrome rule.
        BACKGROUND.set(&prefs, Some("#ffffff"));
        let picked = theme_colours(&look, &prefs, true);
        assert_eq!(picked.background, "#171717");
        assert_ne!(picked.accent, c.accent);
    }
}

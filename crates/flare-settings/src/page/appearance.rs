//! The Appearance page model (M7 Task 20).
//! Source: PT `AppearancePage.tsx` (the window section, the theme section and its window colours),
//! `TerminalThemeSection.tsx` (the wall, the Save changes row), `opts.ts` and `fields.tsx` (`SaveButton`).

use flare_theme::colour_contrast::AlphaRange;

use super::{BlockModel, Button, Control, Opt, PageEnv, PageModel, RowModel, ThemeColours};
use crate::kv::PrefStore;
use crate::look::LookStore;

/// The Save changes title while something differs from what Custom keeps.
pub const SAVE_TITLE: &str = "Saves the theme and agent colours as Custom";
/// The Save changes title while nothing differs.
pub const SAVE_CLEAN: &str = "Nothing to save yet";

/// The colours the theme in force gives the window, computed the way the window computes them (PT
/// `themeColours`): the accent is measured against the picked Background when there is one. The
/// caller stores the result in `PageEnv::theme_colours`.
pub fn theme_colours(look: &LookStore, prefs: &PrefStore) -> ThemeColours {
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
    let with_pick = chrome_tokens(&picked, wanted.as_deref(), edges, None, None).vars;
    let own = chrome_tokens(&theme, wanted.as_deref(), edges, None, None).vars;
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

/// What the app icon row says under its label: where the pick shows. On a Mac, Finder keeps the original (rule 1231:
/// the signed bundle is never written to); on Windows the Start menu shortcut wears the pick too (rule 1272).
pub const APP_ICON_SUB: &str = if cfg!(target_os = "macos") {
    "The Dock and the app switcher. Finder keeps the original."
} else {
    "The taskbar, the window and the Start menu."
};

/// The App icon row's buttons on `host`: on Windows one labelled Restart Explorer button (rule 1263), on a Mac none
/// (rule 1264), so the row's right is empty there and the tiles keep their place.
pub fn app_icon_buttons(host: crate::app_icon::ShellHost) -> Vec<Button> {
    use crate::app_icon::{RESTART_BUTTON, restart_label};
    restart_label(host)
        .map(|label| Button {
            id: RESTART_BUTTON,
            label: label.to_string(),
            // The mockup's circular arrow, left of the words. Labelled, so no tooltip (rule 1263).
            icon: "redo",
            ..Button::default()
        })
        .into_iter()
        .collect()
}

/// The app icon picker (rule 1230): the icon in force picked, the original first and named as the default. The row's
/// right holds the Restart Explorer button on Windows, not the picked icon's name (rule 1263), and nothing on a Mac
/// (rule 1264).
fn app_icon_row(env: &PageEnv) -> RowModel {
    use crate::app_icon::{APP_ICONS, ShellHost, app_icon, tile_name};
    let options = APP_ICONS
        .iter()
        .map(|i| Opt {
            id: i.id.to_string(),
            name: tile_name(i),
            face: String::new(),
            vendor: String::new(),
        })
        .collect();
    RowModel {
        sub: APP_ICON_SUB.to_string(),
        buttons: app_icon_buttons(ShellHost::current()),
        ..row_with(
            "app-icon",
            Control::Icons {
                value: app_icon(env.prefs).id.to_string(),
                options,
            },
        )
    }
}

/// The Appearance page: the window section, then the theme section (the wall under the theme row, the
/// window's own colours under the wall).
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
            vendor: String::new(),
        })
        .collect();
    let edges = row_with("window-edges", segmented(edges_word, edge_options));

    let window = super::section(
        "window",
        vec![
            BlockModel::Row(layout),
            BlockModel::Row(switch),
            BlockModel::Row(edges),
            BlockModel::Row(app_icon_row(env)),
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
    use crate::look::{AgentColourState, LookStore};
    use crate::page::{BlockModel, Control, DiagInfo, PageEnv, ThemeColours, row_of};
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

    /// Runs `f` on an environment over the given stores.
    fn with_env<R>(prefs: &PrefStore, look: &LookStore, f: impl FnOnce(&PageEnv<'_>) -> R) -> R {
        let face_of: &dyn Fn(&str) -> String = &no_face;
        let env = PageEnv {
            prefs,
            look,
            shells: None,
            verb: VerbState::Busy,
            home: String::new(),
            version: "0.0.0".to_string(),
            copyright: String::new(),
            diag: DiagInfo {
                verbose: false,
                dir: String::new(),
            },
            diag_marked: false,
            nvidia: false,
            dictation_wired: false,
            theme_colours: colours(),
            agent_colours: [String::new(), String::new(), String::new()],
            face_of,
            mods: None,
            machines: None,
        };
        f(&env)
    }

    fn fresh() -> (PrefStore, LookStore) {
        let p = PrefStore::memory();
        (p.clone(), LookStore::new(p))
    }

    fn page(prefs: &PrefStore, look: &LookStore) -> PageModel {
        with_env(prefs, look, build)
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
        let p = page(&prefs, &look);
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
        assert_eq!(
            window,
            vec!["tab-layout", "tab-switch", "window-edges", "app-icon"]
        );
    }

    /// Rule 1230: the app icon row offers every icon, the original first and named as the default, and shows the
    /// original picked on a fresh profile, a stored pick once there is one, and the original for an unknown word.
    #[test]
    fn the_app_icon_row_picks_the_original_until_another_is_chosen() {
        use crate::app_icon::{APP_ICONS, KEY};
        let (prefs, look) = fresh();
        let picked = |p: &PageModel| match &row(p, "app-icon").control {
            Control::Icons { value, options } => (value.clone(), options.clone()),
            other => panic!("app-icon is an icon picker, not {other:?}"),
        };
        let (value, options) = picked(&page(&prefs, &look));
        assert_eq!(value, "original");
        let ids: Vec<&str> = options.iter().map(|o| o.id.as_str()).collect();
        let want: Vec<&str> = APP_ICONS.iter().map(|i| i.id).collect();
        assert_eq!(ids, want);
        assert_eq!(options[0].name, "Original, the default");
        assert_eq!(options[1].name, "Liquid Glass");
        let r = row(&page(&prefs, &look), "app-icon").clone();
        assert_eq!(r.label, "App icon");
        assert_eq!(r.sub, APP_ICON_SUB);
        if cfg!(target_os = "macos") {
            assert!(r.sub.contains("Finder keeps the original"));
        } else {
            // Rule 1272: the Start menu shortcut wears the pick too.
            assert_eq!(r.sub, "The taskbar, the window and the Start menu.");
        }
        let diag = crate::verb::FakeDiag::new("C:/diag", false);
        let pending = std::cell::RefCell::new(None);
        let ctx = crate::actions::Ctx {
            prefs: &prefs,
            look: &look,
            diag: &diag,
            pending_pick: &pending,
        };
        crate::actions::apply(
            &ctx,
            crate::actions::Event::Choose {
                row: "app-icon".to_string(),
                option: "daybreak".to_string(),
            },
        );
        assert_eq!(picked(&page(&prefs, &look)).0, "daybreak");
        prefs.set(KEY, "neon");
        assert_eq!(picked(&page(&prefs, &look)).0, "original");
    }

    /// Rule 1263: on Windows the App icon row's right is one labelled button that restarts Explorer, with no tooltip.
    /// Rule 1264: a Mac's row has no restart button at all. Both arms are checked on any host.
    #[test]
    fn the_app_icon_row_offers_the_shell_restart_button() {
        use crate::app_icon::{RESTART_BUTTON, ShellHost};
        let win = app_icon_buttons(ShellHost::Windows);
        assert_eq!(win.len(), 1);
        let b = &win[0];
        assert_eq!(b.id, RESTART_BUTTON);
        assert_eq!(b.label, "Restart Explorer");
        assert!(b.tip.is_empty(), "a labelled button has no tooltip");
        assert_eq!(b.icon, "redo");
        assert!(crate::icons::button_icon_path(b.icon).is_some());
        assert!(!b.off);
        assert!(
            app_icon_buttons(ShellHost::Mac).is_empty(),
            "a Mac has no Restart Dock button"
        );
        // The page this host builds wears its own arm.
        let (prefs, look) = fresh();
        let r = row(&page(&prefs, &look), "app-icon").clone();
        assert_eq!(r.buttons, app_icon_buttons(ShellHost::current()));
    }

    #[test]
    fn tab_position_shows_vertical_on_a_fresh_profile_and_a_stored_horizontal() {
        use crate::tab_layout::{TabLayout, set_tab_layout};
        let (prefs, look) = fresh();
        let value = |p: &PageModel| match &row(p, "tab-layout").control {
            Control::Segmented { value, .. } => value.clone(),
            other => panic!("tab-layout is segmented, not {other:?}"),
        };
        assert_eq!(value(&page(&prefs, &look)), "vertical");
        set_tab_layout(&prefs, TabLayout::Horizontal);
        assert_eq!(value(&page(&prefs, &look)), "horizontal");
    }

    #[test]
    fn tab_switching_says_what_the_choice_in_force_does() {
        let (prefs, look) = fresh();
        set_tab_switch(&prefs, TabSwitch::Order);
        let p = page(&prefs, &look);
        assert_eq!(row(&p, "tab-switch").sub, "The next tab along the strip.");
        set_tab_switch(&prefs, TabSwitch::Recent);
        let p = page(&prefs, &look);
        assert_eq!(row(&p, "tab-switch").sub, "Back to the tab you used last.");
    }

    #[test]
    fn edges_are_offered_weakest_to_strongest() {
        let (prefs, look) = fresh();
        let p = page(&prefs, &look);
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
    fn the_wall_sits_under_the_theme_row_and_the_window_colours_under_the_wall() {
        let (prefs, look) = fresh();
        let p = page(&prefs, &look);
        let kinds: Vec<&str> = theme_blocks(&p)
            .iter()
            .map(|b| match b {
                BlockModel::Row(_) => "Row",
                BlockModel::Wall => "Wall",
                _ => "other",
            })
            .collect();
        assert_eq!(kinds, vec!["Row", "Wall", "Row", "Row", "Row"]);
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
                "window-background",
                "window-secondary",
                "window-accent",
            ]
        );
    }

    #[test]
    fn reset_shows_only_while_a_colour_is_picked_and_the_swatch_shows_the_themes_until_then() {
        let (prefs, look) = fresh();
        let p = page(&prefs, &look);
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
        let p = page(&prefs, &look);
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
        let p = page(&prefs, &look);
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
        look.set_agent_colour(AgentColourState::Working, "#123456");
        assert!(save_dirty(&prefs, &look));
        look.set_agent_colour(AgentColourState::Working, "");
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
        let clean = page(&prefs, &look);
        assert_eq!(
            row(&clean, "term-theme").control,
            Control::Save {
                dirty: false,
                title: "Nothing to save yet".to_string(),
            }
        );
        look.set_agent_colour(AgentColourState::Working, "#123456");
        let dirty = page(&prefs, &look);
        assert_eq!(
            row(&dirty, "term-theme").control,
            Control::Save {
                dirty: true,
                title: "Saves the theme and agent colours as Custom".to_string(),
            }
        );
    }

    #[test]
    fn the_swatch_colour_of_the_theme_is_computed_not_read_back() {
        let (prefs, look) = fresh();
        let c = theme_colours(&look, &prefs);
        assert_eq!(c.accent, "#fe8f34");
        assert_eq!(c.background, "#171717");
        assert_eq!(c.secondary, "#0d0d0d");
        // A picked Background changes the accent only through the chrome rule.
        BACKGROUND.set(&prefs, Some("#ffffff"));
        let picked = theme_colours(&look, &prefs);
        assert_eq!(picked.background, "#171717");
        assert_ne!(picked.accent, c.accent);
    }
}

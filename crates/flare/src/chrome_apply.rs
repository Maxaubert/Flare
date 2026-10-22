//! AppliedLook written into the Slint Theme global and the grid (Task 23).
//! Source: PT `src/renderer/src/lib/paintChrome.ts` (the setters) and `core/renderer/components/TerminalPanel.tsx`
//! (the grid's palette and ground). Colours come from `flare-theme` as strings; this file turns them into
//! Slint colours and the grid's palette once, at the edge.

// The look controller (Task 26) calls these; until it lands they are reached from the tests only.
#![allow(dead_code)]

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use flare_render::color::{Palette, Rgba};
use flare_render::scene::GroundPaint;
use flare_theme::look::AppliedLook;
use flare_theme::resolve::TermTheme;
use flare_theme::see_through::glyph_stroke_floor_device_px;
use slint::{Color, Global};

use crate::host::GridHost;
use crate::{AppWindow, ConvoInk, Motion, Theme};

/// (`--p-` token, Slint property, its setter, its getter), in `CHROME_COLOUR_TOKENS` order.
macro_rules! token_table {
    ($( ($tok:literal, $prop:literal, $set:ident, $get:ident) ),* $(,)?) => {
        /// Every chrome token and the Slint property that carries it (PT's `--p-` names, one to one).
        pub const TOKEN_TABLE: [(&str, &str); 31] = [ $( ($tok, $prop) ),* ];

        /// Writes every token: `value(token)` gives the colour string of each one.
        fn set_tokens(theme: &Theme<'_>, value: &dyn Fn(&str) -> Color) {
            $( theme.$set(value($tok)); )*
        }

        /// Reads every token back as the Slint property holds it (tests).
        #[cfg(test)]
        fn read_tokens(theme: &Theme<'_>) -> Vec<(&'static str, Color)> {
            vec![ $( ($tok, theme.$get()) ),* ]
        }
    };
}

token_table! {
    ("--p-bg", "ground", set_ground, get_ground),
    ("--p-bg-solid", "ground-solid", set_ground_solid, get_ground_solid),
    ("--p-side", "side", set_side, get_side),
    ("--p-side-flat", "side-flat", set_side_flat, get_side_flat),
    ("--p-secondary", "secondary", set_secondary, get_secondary),
    ("--p-title", "title", set_title, get_title),
    ("--p-tabs", "tabs", set_tabs, get_tabs),
    ("--p-tab-active", "tab-active", set_tab_active, get_tab_active),
    ("--p-text", "text", set_text, get_text),
    ("--p-text-soft", "text-soft", set_text_soft, get_text_soft),
    ("--p-dim", "dim", set_dim, get_dim),
    ("--p-dim2", "dim2", set_dim2, get_dim2),
    ("--p-accent", "accent", set_accent, get_accent),
    ("--p-accent-solid", "accent-solid", set_accent_solid, get_accent_solid),
    ("--p-accent-hi", "accent-hi", set_accent_hi, get_accent_hi),
    ("--p-on-accent", "on-accent", set_on_accent, get_on_accent),
    ("--p-icon", "icon", set_icon, get_icon),
    ("--p-hover", "hover", set_hover, get_hover),
    ("--p-hover-hi", "hover-hi", set_hover_hi, get_hover_hi),
    ("--p-divider", "divider", set_divider, get_divider),
    ("--p-line", "line", set_line, get_line),
    ("--p-track", "track", set_track, get_track),
    ("--p-control", "control", set_control, get_control),
    ("--p-preview", "preview", set_preview, get_preview),
    ("--p-sel-bg", "sel-bg", set_sel_bg, get_sel_bg),
    ("--p-tree-folder", "tree-folder", set_tree_folder, get_tree_folder),
    ("--p-switch-knob", "switch-knob", set_switch_knob, get_switch_knob),
    ("--p-pressed", "pressed", set_pressed, get_pressed),
    ("--p-pressed-ink", "pressed-ink", set_pressed_ink, get_pressed_ink),
    ("--p-side-text", "side-text", set_side_text, get_side_text),
    ("--p-side-dim", "side-dim", set_side_dim, get_side_dim),
}

static WARNED: AtomicBool = AtomicBool::new(false);

/// The text colour of the look last applied (`--p-text`): the close question's names wear it. Empty until
/// the first apply, which reads as the default ink.
static TEXT_INK: Mutex<String> = Mutex::new(String::new());

/// The theme's text colour (`--p-text`) of a look, or M3's default ink when the look names none.
pub fn ink_of(look: &AppliedLook) -> String {
    look.chrome
        .vars
        .get("--p-text")
        .cloned()
        .unwrap_or_else(|| flare_tabs::close_flow::NAME_INK.to_string())
}

/// The theme's text colour for the dialogs that name things (the close question), or M3's default ink
/// before any look has been applied.
pub fn text_ink() -> String {
    let ink = TEXT_INK.lock().map(|s| s.clone()).unwrap_or_default();
    if ink.is_empty() {
        flare_tabs::close_flow::NAME_INK.to_string()
    } else {
        ink
    }
}

/// `#rrggbb` or `#rrggbbaa` to a Slint colour. Anything else logs one error and is transparent.
pub fn colour(hex: &str) -> Color {
    match Rgba::from_hex(hex) {
        Some(c) => Color::from_argb_u8(c.a, c.r, c.g, c.b),
        None => {
            if !WARNED.swap(true, Ordering::Relaxed) {
                log::error!("not a colour: {hex:?}; drawing it transparent");
            }
            Color::from_argb_u8(0, 0, 0, 0)
        }
    }
}

/// Writes the look's 31 chrome tokens, light or dark, and the window's clear colour into the Slint Theme.
pub fn apply_chrome(app: &AppWindow, look: &AppliedLook) {
    let theme = Theme::get(app);
    set_tokens(&theme, &|tok| {
        colour(look.chrome.vars.get(tok).map(String::as_str).unwrap_or(""))
    });
    // PT's setBackgroundColor: solid ground, or transparent under glass.
    let clear = if look.glass {
        Color::from_argb_u8(0, 0, 0, 0)
    } else {
        colour(&look.solid_ground)
    };
    theme.set_window_clear(clear);
    theme.set_glyph_stroke_floor(glyph_stroke_floor_device_px(look.glass));
    theme.set_light(look.light);
    // The conversation panels' diff inks are the theme's terminal green and red (stage 6).
    let pal = palette_of(&look.term);
    let ink = |c: Rgba| Color::from_argb_u8(255, c.r, c.g, c.b);
    let diff = ConvoInk::get(app);
    diff.set_added(ink(pal.ansi[2]));
    diff.set_removed(ink(pal.ansi[1]));
    // Reduced motion, app-wide (parity line 37): every animate duration goes through Motion.d().
    Motion::get(app).set_reduced(!crate::e2e::animations_enabled());
    if let Ok(mut slot) = TEXT_INK.lock() {
        *slot = ink_of(look);
    }
}

/// The grid's palette for a resolved theme. Background, foreground and cursor are opaque; the selection keeps its
/// alpha; the cursor accent is the background. A missing slot takes pt-default's colour (decision D10).
pub fn palette_of(term: &TermTheme) -> Palette {
    let base = Palette::pt_default();
    let opaque = |c: Rgba| Rgba { a: 255, ..c };
    let read = |s: &str, fallback: Rgba| Rgba::from_hex(s).unwrap_or(fallback);
    let bg = opaque(read(&term.background, base.bg));
    let fg = opaque(read(&term.foreground, base.fg));
    let cursor = opaque(read(&term.cursor, base.cursor));
    let selection = read(&term.selection_background, base.selection);
    let ansi = std::array::from_fn(|i| {
        term.ansi_at(i)
            .and_then(Rgba::from_hex)
            .unwrap_or(base.ansi[i])
    });
    Palette::new(fg, bg, cursor, bg, selection, ansi)
}

/// Under glass the UI paints the ground (the grid leaves it clear); otherwise the grid paints its own.
pub fn ground_paint(look: &AppliedLook) -> GroundPaint {
    if look.glass {
        GroundPaint::ByUi
    } else {
        GroundPaint::InGrid
    }
}

/// Hands the grid its palette and who paints the ground (marks it dirty only on a change).
pub fn apply_to_host(host: &mut GridHost, look: &AppliedLook) {
    crate::term_ui::set_worn_link(&look.link_colour);
    host.set_look(palette_of(&look.term), ground_paint(look));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::{GridHost, GridPainter, GridSource};
    use crate::ui_harness::Frame;
    use crate::ui_harness::Harness;
    use flare_render::glyph::FakeGlyphs;
    use flare_render::grid::Grid;
    use flare_theme::chrome::CHROME_COLOUR_TOKENS;
    use flare_theme::edges::DEFAULT_WINDOW_EDGES;
    use flare_theme::look::{LookInput, apply_look};
    use flare_theme::preset::find_preset;
    use flare_theme::resolve::resolve_preset;

    /// A blank grid of whatever size it is given: every cell is the default one.
    #[derive(Default)]
    struct BlankSource {
        cols: usize,
        rows: usize,
    }

    impl GridSource for BlankSource {
        fn resize(&mut self, cols: usize, rows: usize) {
            self.cols = cols;
            self.rows = rows;
        }
        fn grid(&mut self) -> Grid {
            Grid::new(self.cols, self.rows)
        }
    }

    fn look_of(id: &str, see_through: u8, glass_possible: bool) -> AppliedLook {
        apply_look(&LookInput {
            theme_id: id,
            custom: None,
            draft: None,
            background: None,
            accent: None,
            secondary: None,
            edges: DEFAULT_WINDOW_EDGES,
            see_through,
            glass_possible,
        })
    }

    fn argb(c: Color) -> (u8, u8, u8, u8) {
        (c.alpha(), c.red(), c.green(), c.blue())
    }

    #[test]
    fn the_close_question_ink_is_the_applied_text_token() {
        let look = look_of("pt-default", 0, false);
        assert_eq!(ink_of(&look), look.chrome.vars["--p-text"]);
        let mut bare = look.clone();
        bare.chrome.vars.remove("--p-text");
        assert_eq!(ink_of(&bare), flare_tabs::close_flow::NAME_INK);
    }

    #[test]
    fn the_token_table_has_all_31_in_order() {
        assert_eq!(TOKEN_TABLE.len(), 31);
        for (i, (tok, prop)) in TOKEN_TABLE.iter().enumerate() {
            assert_eq!(*tok, CHROME_COLOUR_TOKENS[i]);
            let expected = match *tok {
                "--p-bg" => "ground",
                "--p-bg-solid" => "ground-solid",
                other => other.trim_start_matches("--p-"),
            };
            assert_eq!(*prop, expected, "token {tok}");
        }
    }

    #[test]
    fn colour_parses_hex6_and_hex8_and_survives_junk() {
        assert_eq!(argb(colour("#fe8f34")), (255, 254, 143, 52));
        assert_eq!(colour("#ffffff0f").alpha(), 15);
        assert_eq!(argb(colour("soup")), (0, 0, 0, 0));
    }

    #[test]
    fn applying_a_theme_sets_every_slint_token() {
        let look = look_of("paper", 0, false);
        let h = Harness::new(1.0, 1000, 640, || {
            AppWindow::new().expect("the window builds")
        });
        apply_chrome(&h.comp, &look);
        let theme = Theme::get(&h.comp);
        for ((tok, _), (_, got)) in TOKEN_TABLE.iter().zip(read_tokens(&theme)) {
            let want = colour(&look.chrome.vars[*tok]);
            assert_eq!(argb(got), argb(want), "token {tok}");
        }
        assert!(theme.get_light());
        assert_eq!(
            argb(theme.get_text()),
            argb(colour(&look.chrome.vars["--p-text"]))
        );
    }

    #[test]
    fn the_window_clear_is_solid_or_transparent() {
        let h = Harness::new(1.0, 1000, 640, || {
            AppWindow::new().expect("the window builds")
        });
        apply_chrome(&h.comp, &look_of("pt-default", 0, true));
        let solid = Theme::get(&h.comp).get_window_clear();
        assert_eq!(argb(solid), argb(Theme::get(&h.comp).get_ground_solid()));
        apply_chrome(&h.comp, &look_of("pt-default", 40, true));
        assert_eq!(Theme::get(&h.comp).get_window_clear().alpha(), 0);
    }

    #[test]
    fn the_glyph_stroke_floor_is_set_only_under_glass() {
        // Audit V7: the title buttons and the new-tab + read this, so their 1 px strokes stay whole under glass.
        let h = Harness::new(1.0, 1000, 640, || {
            AppWindow::new().expect("the window builds")
        });
        apply_chrome(&h.comp, &look_of("pt-default", 0, true));
        assert_eq!(Theme::get(&h.comp).get_glyph_stroke_floor(), 0.0);
        apply_chrome(&h.comp, &look_of("pt-default", 40, true));
        assert_eq!(Theme::get(&h.comp).get_glyph_stroke_floor(), 2.0);
    }

    #[test]
    fn palette_of_uses_the_themes_sixteen_and_names_the_cursor_accent() {
        let term = resolve_preset(find_preset("pt-default").expect("pt-default"));
        let p = palette_of(&term);
        assert_eq!(p.ansi[1], Rgba::rgb(0xff, 0x61, 0x5a));
        assert_eq!(p.cursor_accent, p.bg);
        assert_eq!(p.selection, Rgba::from_hex("#bbbbbb55").expect("hex"));
    }

    #[test]
    fn palette_of_fills_a_missing_slot_from_pt_default() {
        let mut term = resolve_preset(find_preset("pt-default").expect("pt-default"));
        term.ansi = Default::default();
        term.ansi[1] = Some("#ff615a".to_string());
        let p = palette_of(&term);
        assert_eq!(p.ansi[2], Palette::pt_default().ansi[2]);
        assert_eq!(p.ansi[2], Rgba::rgb(0xb1, 0xe9, 0x69));
        assert_eq!(p.ansi[1], Rgba::rgb(0xff, 0x61, 0x5a));
    }

    #[test]
    fn ground_paint_is_by_ui_only_under_glass() {
        assert_eq!(
            ground_paint(&look_of("paper", 0, true)),
            GroundPaint::InGrid
        );
        assert_eq!(ground_paint(&look_of("paper", 40, true)), GroundPaint::ByUi);
        assert_eq!(
            ground_paint(&look_of("paper", 40, false)),
            GroundPaint::InGrid
        );
    }

    #[test]
    fn a_rendered_window_wears_the_themes_ground() {
        for scale in [1.0_f32, 1.5, 2.25] {
            let look = look_of("paper", 0, false);
            let h = Harness::new(scale, 1000, 640, || {
                AppWindow::new().expect("the window builds")
            });
            apply_chrome(&h.comp, &look);
            let (gw, gh) = (
                h.comp.get_grid_phys_width() as u32,
                h.comp.get_grid_phys_height() as u32,
            );
            let mut host = GridHost::new(Box::new(FakeGlyphs::new(8, 16)), GridPainter::Cpu);
            host.set_source(Box::new(BlankSource::default()));
            apply_to_host(&mut host, &look);
            host.resize(gw, gh, scale);
            if let Some(img) = host.render() {
                h.comp.set_grid_image(img);
            }
            let frame = h.render();
            let want = [0xf6, 0xf4, 0xee, 255];
            let mid = frame.px(frame.w / 2, frame.h / 2);
            assert!(
                Frame::near(mid, want, 2),
                "scale {scale}: grid centre {mid:?}"
            );
            let tabs = colour(&look.chrome.vars["--p-tabs"]);
            let title = frame.px(2, 2);
            assert!(
                Frame::near(title, [tabs.red(), tabs.green(), tabs.blue(), 255], 2),
                "scale {scale}: title row {title:?}"
            );
        }
    }
}

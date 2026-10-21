//! UI tests of contrast and neutral colour on the Settings tab (M7 Task 46); headless only.
//!
//! The contrast test samples the rendered panel of `RowPreview` under the two reference looks
//! (`pt-default`, dark, and `paper`, light) and reads the ink colours Slint derives from them. The
//! neutral tests read the Slint sources of `ui/settings` and hold every accent use to a named place
//! (PT's `neutralControls.test.ts`, rule 78): only Save, an ON switch, the flash, and the keyboard
//! focus rings may wear the accent. Slider and Select, and the Reset link, are PT's own accent
//! text or fill (`fields.tsx`), kept as PT has them and listed below.

use flare_theme::ansi::{contrast_ratio, mix_hex};
use flare_theme::edges::DEFAULT_WINDOW_EDGES;
use flare_theme::look::{AppliedLook, LookInput, apply_look};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::chrome_apply::colour;
use crate::ui_harness::Harness;
use crate::{
    BlockKind, RowPreview, SetKind, SetRow, SetSection, SettingsBridge, SettingsPreview, Theme,
};

/// The warn hexes of `SettingsFixed` in `theme.slint` (`warn-dark`, `warn-light`).
const WARN_DARK: &str = "#e0a84b";
const WARN_LIGHT: &str = "#a46400";

const ICON: &str = "M4 6h16v12H4z";
const ROW_H: f32 = 58.0;
const PANEL_Y: f32 = 50.0;

fn look(id: &str) -> AppliedLook {
    apply_look(&LookInput {
        theme_id: id,
        custom: None,
        draft: None,
        background: None,
        accent: None,
        secondary: None,
        edges: DEFAULT_WINDOW_EDGES,
        see_through: 0,
        glass_possible: false,
    })
}

/// A look's token as a `#rrggbb` string.
fn token(look: &AppliedLook, name: &str) -> String {
    look.chrome.vars[name].clone()
}

fn hex_of(p: [u8; 4]) -> String {
    format!("#{:02x}{:02x}{:02x}", p[0], p[1], p[2])
}

/// Writes the look's ink into the row preview's Theme (the tokens the settings ink is derived from).
fn paint(h: &Harness<RowPreview>, look: &AppliedLook) {
    let t = h.comp.global::<Theme>();
    t.set_light(look.light);
    t.set_ground(colour(&token(look, "--p-bg")));
    t.set_text(colour(&token(look, "--p-text")));
    t.set_dim(colour(&token(look, "--p-dim")));
    t.set_text_soft(colour(&token(look, "--p-text-soft")));
    t.set_accent_hi(colour(&token(look, "--p-accent-hi")));
}

#[test]
fn label_and_subtext_hold_4_5_the_icon_3_and_the_warning_4_5_on_a_dark_and_a_light_theme() {
    for id in ["pt-default", "paper"] {
        let look = look(id);
        let h = Harness::new(1.0, 640, 400, || RowPreview::new().unwrap());
        h.comp.set_panel_w(560.0);
        paint(&h, &look);
        h.advance(0);
        // Inside row 1 (y 20..78) with no text under it: the panel's own composited fill.
        let panel = h.render().px(500, 25);
        let text = token(&look, "--p-text");
        let dim = token(&look, "--p-dim");
        let soft = token(&look, "--p-text-soft");
        // SettingsInk.sub = Theme.dim.mix(Theme.text, 0.72): 72 percent dim, 28 percent text.
        let sub = mix_hex(&dim, &text, 0.28);
        // SettingsInk.warn: warn-light mix text 0.75 on a light theme, warn-dark mix text 0.85 on a dark one.
        let warn = if look.light {
            mix_hex(WARN_LIGHT, &text, 0.25)
        } else {
            mix_hex(WARN_DARK, &text, 0.15)
        };
        let panel_hex = hex_of(panel);
        let label = contrast_ratio(&text, &panel_hex);
        let subtext = contrast_ratio(&sub, &panel_hex);
        let warning = contrast_ratio(&warn, &panel_hex);
        let icon = contrast_ratio(&soft, &panel_hex);
        assert!(
            label >= 4.5,
            "{id}: label {text} on {panel_hex} is {label:.2}"
        );
        assert!(
            subtext >= 4.5,
            "{id}: subtext {sub} on {panel_hex} is {subtext:.2}"
        );
        assert!(
            warning >= 4.5,
            "{id}: warning {warn} on {panel_hex} is {warning:.2}"
        );
        assert!(
            icon >= 3.0,
            "{id}: icon ink {soft} on {panel_hex} is {icon:.2}"
        );
    }
}

// ---- source scans -----------------------------------------------------------------------------

/// Every `.slint` file of `ui/settings`, with its name.
fn settings_sources() -> [(&'static str, &'static str); 11] {
    [
        ("about.slint", include_str!("../../ui/settings/about.slint")),
        (
            "colour.slint",
            include_str!("../../ui/settings/colour.slint"),
        ),
        (
            "controls.slint",
            include_str!("../../ui/settings/controls.slint"),
        ),
        ("frame.slint", include_str!("../../ui/settings/frame.slint")),
        (
            "help_panel.slint",
            include_str!("../../ui/settings/help_panel.slint"),
        ),
        ("row.slint", include_str!("../../ui/settings/row.slint")),
        (
            "select.slint",
            include_str!("../../ui/settings/select.slint"),
        ),
        (
            "slider.slint",
            include_str!("../../ui/settings/slider.slint"),
        ),
        ("types.slint", include_str!("../../ui/settings/types.slint")),
        ("view.slint", include_str!("../../ui/settings/view.slint")),
        ("wall.slint", include_str!("../../ui/settings/wall.slint")),
    ]
}

fn source_of(name: &str) -> &'static str {
    settings_sources()
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, s)| *s)
        .unwrap_or_else(|| panic!("no source for {name}"))
}

/// The Slint tokens that name the accent family (PT's `ACCENT` pattern, Flare's names).
const ACCENT_TOKENS: [&str; 4] = [
    "Theme.accent",
    "Theme.on-accent",
    "Theme.sel-bg",
    "Theme.switch-knob",
];

fn component_name(line: &str) -> Option<&str> {
    let t = line.trim_start();
    let t = t.strip_prefix("export ").unwrap_or(t);
    t.strip_prefix("component ")?.split_whitespace().next()
}

/// Lines of a source that name an accent token, outside comments, with the component they sit in.
fn accent_hits(src: &str) -> Vec<(String, usize, String)> {
    let mut comp = String::new();
    let mut out = Vec::new();
    for (i, line) in src.lines().enumerate() {
        if let Some(name) = component_name(line) {
            comp = name.to_string();
        }
        let code = line.trim_start();
        if code.starts_with("//") {
            continue;
        }
        if ACCENT_TOKENS.iter().any(|t| line.contains(t)) {
            out.push((comp.clone(), i + 1, line.to_string()));
        }
    }
    out
}

/// The text of one component, from its header line to the next top-level component.
fn body_of(src: &str, name: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for line in src.lines() {
        if let Some(found) = component_name(line) {
            if inside {
                break;
            }
            inside = found == name;
        }
        if inside {
            out.push_str(line);
            out.push('\n');
        }
    }
    assert!(!out.is_empty(), "component {name} is declared");
    out
}

#[test]
fn row_button_and_pressed_segment_use_no_accent_token() {
    let src = include_str!("../../ui/settings/controls.slint");
    for (comp, line_no, line) in accent_hits(src) {
        if comp == "SetRowButton" || comp == "SetSegmented" {
            // The keyboard focus line may name the accent: it marks the keyboard, not the button.
            assert!(
                line.contains("focus"),
                "controls.slint:{line_no} ({comp}) wears an accent: {line}"
            );
        }
    }
}

#[test]
fn an_on_switch_uses_sel_bg_and_switch_knob_and_off_is_track_and_white() {
    let body = body_of(
        include_str!("../../ui/settings/controls.slint"),
        "SetSwitch",
    );
    assert!(body.contains("Theme.sel-bg"), "the ON fill is Theme.sel-bg");
    assert!(
        body.contains("Theme.switch-knob"),
        "the ON knob is Theme.switch-knob"
    );
    assert!(body.contains("Theme.track"), "the OFF fill is Theme.track");
    assert!(
        body.contains("SettingsFixed.white"),
        "the OFF knob is white"
    );
}

#[test]
fn the_colour_picker_uses_the_accent_only_for_focus_outlines() {
    let src = include_str!("../../ui/settings/colour.slint");
    let lines: Vec<&str> = src.lines().collect();
    let hits = accent_hits(src);
    assert!(!hits.is_empty(), "the picker's focus outlines are accent");
    for (comp, line_no, line) in hits {
        let i = line_no - 1;
        // A focus outline names the focus on its own line or within the eight lines above it.
        let focus = line.contains("focus")
            || lines[i.saturating_sub(8)..i]
                .iter()
                .any(|l| l.contains("focus"));
        // The two defaults of ColourPreview are the test fixture's starting colours, not drawn ink.
        let fixture = comp == "ColourPreview" && line.trim_start().starts_with("in-out property");
        assert!(
            focus || fixture,
            "colour.slint:{line_no} ({comp}) is not a focus outline: {line}"
        );
    }
}

#[test]
fn the_save_button_uses_the_accent() {
    let body = body_of(
        include_str!("../../ui/settings/controls.slint"),
        "SetSaveButton",
    );
    let bg = body
        .lines()
        .find(|l| l.contains("background:"))
        .expect("SetSaveButton has a background");
    assert!(
        bg.contains("root.dirty"),
        "Save is accent only when dirty: {bg}"
    );
    assert!(
        bg.contains("Theme.accent"),
        "dirty Save is the accent: {bg}"
    );
}

#[test]
fn row_frame_and_frame_carry_no_accent_the_flash_aside() {
    // row.slint paints the flash through SettingsInk.flash; no other accent token is allowed there.
    for name in ["row.slint", "frame.slint"] {
        let hits = accent_hits(source_of(name));
        assert!(hits.is_empty(), "{name} names an accent token: {hits:?}");
    }
}

#[test]
fn no_other_settings_file_uses_the_accent_except_the_wall_and_help_marks() {
    // Allowed without a focus word: the wall and help panel (chosen card edge, pencil, Save as
    // Custom; chip on, caret, glow); controls' ON switch, Save and the Reset link (PT fields.tsx: Reset
    // is accent-hi text); Select's on-option text and check (PT fields.tsx Select); Slider's fill (see
    // the report: PT has no slider source in this scan, the owner decides).
    const ALL_OK: [&str; 4] = [
        "wall.slint",
        "help_panel.slint",
        "select.slint",
        "slider.slint",
    ];
    const CONTROLS_OK: [&str; 3] = ["SetSwitch", "SetSaveButton", "SetResetLink"];
    // Checked by their own tests: colour.slint (the picker) and row/frame (zero hits).
    const OWN_TEST: [&str; 3] = ["colour.slint", "row.slint", "frame.slint"];
    for (file, src) in settings_sources() {
        if OWN_TEST.contains(&file) {
            continue;
        }
        for (comp, line_no, line) in accent_hits(src) {
            if line.contains("focus") || ALL_OK.contains(&file) {
                continue;
            }
            assert!(
                file == "controls.slint" && CONTROLS_OK.contains(&comp.as_str()),
                "{file}:{line_no} ({comp}) wears the accent: {line}"
            );
        }
    }
}

#[test]
fn a_rendered_appearance_page_paints_the_accent_only_in_save_and_on_switches() {
    // The warm test: the accent family is orange, far redder than blue.
    let warm = |p: [u8; 4]| p[0] as i32 - p[2] as i32 > 60;
    let row = |id: &str, kind: SetKind, y: f32| SetRow {
        id: id.into(),
        icon: ICON.into(),
        label: id.into(),
        sub: "A line of subtext.".into(),
        block: BlockKind::Row,
        kind,
        y,
        h: ROW_H,
        ..Default::default()
    };
    let page = |switch_on: bool, save_dirty: bool| {
        let mut sw = row("sw", SetKind::Switch, 0.0);
        sw.on = switch_on;
        let mut sv = row("sv", SetKind::Save, ROW_H);
        sv.save_dirty = save_dirty;
        SetSection {
            id: "sec".into(),
            title: "Section".into(),
            tag: SharedString::new(),
            action_save: false,
            action_dirty: false,
            title_y: 20.0,
            panel_y: PANEL_Y,
            panel_h: 2.0 * ROW_H + 2.0,
            rows: ModelRc::new(VecModel::from(vec![sw, sv])),
            ..Default::default()
        }
    };
    let render = |scale: f32, switch_on: bool, save_dirty: bool| {
        let h = Harness::new(scale, 1000, 604, || {
            SettingsPreview::new().expect("the preview builds")
        });
        let b = h.comp.global::<SettingsBridge>();
        b.set_page_title("Appearance".into());
        b.set_content_h(900.0);
        b.set_sections(ModelRc::new(VecModel::from(vec![page(
            switch_on, save_dirty,
        )])));
        h.advance(0);
        h.render()
    };
    for scale in [1.0_f32, 2.0, 2.25] {
        // Nothing lit: no accent anywhere on the page.
        let quiet = render(scale, false, false);
        assert_eq!(
            quiet.count_where(0, 0, quiet.w, quiet.h, warm),
            0,
            "scale {scale}: the page paints the accent with nothing lit"
        );
        // The ON switch and a dirty Save: accent in exactly those two bands, and only on the right.
        let lit = render(scale, true, true);
        let band = |top: f32| {
            lit.count_where(
                0,
                ((PANEL_Y + top) * scale) as u32,
                lit.w,
                ((PANEL_Y + top + ROW_H) * scale) as u32,
                warm,
            )
        };
        assert!(
            band(0.0) > 0,
            "scale {scale}: the ON switch paints the accent"
        );
        assert!(
            band(ROW_H) > 0,
            "scale {scale}: a dirty Save paints the accent"
        );
        let leftmost = lit
            .bbox_where(0, 0, lit.w, lit.h, warm)
            .map(|b| b.0)
            .expect("accent pixels exist");
        assert!(
            leftmost as f32 > 500.0 * scale,
            "scale {scale}: accent at x {leftmost} is left of the control column"
        );
    }
}

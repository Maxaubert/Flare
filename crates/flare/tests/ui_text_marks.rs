//! Text checks of the mark glyphs (M6 Task 24). They read the .slint files as text.

use std::fs;
use std::path::PathBuf;

fn glyphs() -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/glyphs.slint");
    fs::read_to_string(p).expect("read glyphs.slint")
}

/// The string value of `out property <string> NAME: "VALUE";`.
fn glyph(text: &str, name: &str) -> String {
    let needle = format!("out property <string> {name}: \"");
    let at = text
        .find(&needle)
        .unwrap_or_else(|| panic!("glyph {name} is declared"));
    let rest = &text[at + needle.len()..];
    rest[..rest.find('"').expect("closing quote")].to_string()
}

#[test]
fn the_openai_glyph_is_the_agent_crates_path() {
    assert_eq!(glyph(&glyphs(), "openai"), flare_agent::icon::OPENAI_PATH);
}

#[test]
fn the_clawd_glyph_is_the_agent_crates_path() {
    assert_eq!(glyph(&glyphs(), "clawd"), flare_agent::icon::clawd_path());
}

#[test]
fn clawd_is_filled_from_the_theme_token_that_holds_the_agent_crates_orange() {
    let theme = ui_file("theme.slint");
    let line = theme
        .lines()
        .find(|l| l.contains("property <color> clawd:"))
        .expect("Theme.clawd declared");
    assert!(
        line.contains(flare_agent::icon::CLAWD_ORANGE),
        "Theme.clawd is {}: {line}",
        flare_agent::icon::CLAWD_ORANGE
    );
    assert!(ui_file("tabmark.slint").contains("fill: Theme.clawd;"));
}

/// The items of `out property <[KIND]> NAME: [A, B, ...];` as written (quotes kept off strings).
fn list_items(text: &str, kind: &str, name: &str) -> Vec<String> {
    let needle = format!("out property <[{kind}]> {name}: [");
    let at = text
        .find(&needle)
        .unwrap_or_else(|| panic!("list {name} is declared"));
    let rest = &text[at + needle.len()..];
    rest[..rest.find("];").expect("closing bracket")]
        .split(',')
        .map(|s| s.trim().trim_matches('"').to_string())
        .collect()
}

#[test]
fn the_agy_glyph_is_the_agent_crates_paths() {
    assert_eq!(
        list_items(&glyphs(), "string", "agy"),
        flare_agent::icon::agy_paths()
    );
}

#[test]
fn the_agy_arch_is_filled_from_the_theme_list_that_holds_the_agent_crates_colours() {
    let want: Vec<String> = flare_agent::icon::AGY_COLOURS
        .iter()
        .map(|c| c.to_string())
        .collect();
    assert_eq!(list_items(&ui_file("theme.slint"), "color", "agy"), want);
    let mark = ui_file("tabmark.slint");
    assert!(mark.contains("for cells[i] in Glyphs.agy : Path"));
    // Still, each path wears its Theme.agy colour; A1 swaps in the colour the mark clock pushed for it.
    assert!(mark.contains("fill: root.flows ? MarkMotion.agy-flow[i] : Theme.agy[i];"));
    let (x, y, w, h) = flare_agent::icon::AGY_VIEWBOX;
    for line in [
        format!("viewbox-x: {x};"),
        format!("viewbox-y: {y};"),
        format!("viewbox-width: {w};"),
        format!("viewbox-height: {h};"),
    ] {
        assert!(mark.contains(&line), "AgentIcon has {line}");
    }
}

#[test]
fn clawds_lids_are_the_agent_crates_path_in_clawds_colour() {
    assert_eq!(
        glyph(&glyphs(), "clawd-lids"),
        flare_agent::icon::clawd_lids_path()
    );
    let mark = ui_file("tabmark.slint");
    let at = mark
        .find("commands: Glyphs.clawd-lids;")
        .expect("AgentIcon draws the lids");
    assert!(
        mark[at..]
            .trim_start_matches("commands: Glyphs.clawd-lids;")
            .trim_start()
            .starts_with("fill: Theme.clawd;")
    );
}

/// Owner decision 2026-10-26: the agent icons move by the mark clock alone (MarkMotion), never by a Slint animation.
#[test]
fn the_agent_icon_moves_by_the_mark_clock_only() {
    let mark = ui_file("tabmark.slint");
    let start = mark.find("export component AgentIcon").expect("AgentIcon");
    let end = mark[start..]
        .find("\nexport component ")
        .map_or(mark.len(), |e| start + e);
    let icon = &mark[start..end];
    assert!(!icon.contains("animate"), "AgentIcon animates by itself");
    assert!(!icon.contains("animation-tick"));
    assert!(icon.contains("MarkMotion.still ? 0 : root.motion"));
    for name in [
        "hop-y",
        "hop-sx",
        "hop-sy",
        "blink",
        "spin-fade",
        "fade",
        "turn",
        "grow",
        "grow-alpha",
        "agy-flow",
    ] {
        assert!(
            icon.contains(&format!("MarkMotion.{name}")),
            "AgentIcon reads {name}"
        );
    }
    for name in ["tabs.slint", "sidebar.slint"] {
        assert!(
            ui_file(name).contains("motion: "),
            "{name} gives the icon its motion"
        );
    }
}

#[test]
fn the_check_and_the_cross_are_the_sixteen_unit_strokes() {
    let g = glyphs();
    assert_eq!(glyph(&g, "check"), "M3.2 8.6l3 3 6.6-7.2");
    assert_eq!(glyph(&g, "cross"), "M4 4l8 8M12 4l-8 8");
}

fn ui_file(name: &str) -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("ui")
        .join(name);
    fs::read_to_string(p).unwrap_or_else(|e| panic!("read {name}: {e}"))
}

#[test]
fn no_private_ring_clock_remains_in_the_tab_files() {
    for name in ["tabs.slint", "sidebar.slint", "menus.slint"] {
        let text = ui_file(name);
        assert!(
            !text.contains("animate turn"),
            "{name} still animates its own turn"
        );
        assert!(
            !text.contains("animation-tick"),
            "{name} still reads its own animation clock"
        );
    }
    assert!(
        ui_file("tabs.slint").contains("MarkMotion.spin"),
        "the loading ring reads the one clock"
    );
}

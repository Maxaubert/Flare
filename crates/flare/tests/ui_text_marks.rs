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

/// Owner 2026-11-13: the blink swaps in one shut shape for the body; no second path of lids is laid over it (a seam).
#[test]
fn the_shut_clawd_is_the_agent_crates_path_swapped_in_for_the_body() {
    assert_eq!(
        glyph(&glyphs(), "clawd-shut"),
        flare_agent::icon::clawd_shut_path()
    );
    assert!(!glyphs().contains("clawd-lids"), "no lids glyph remains");
    let mark = ui_file("tabmark.slint");
    let swap = "commands: root.shut ? Glyphs.clawd-shut : Glyphs.clawd;";
    let at = mark.find(swap).expect("AgentIcon swaps the shut Clawd in");
    assert!(
        mark[at..]
            .trim_start_matches(swap)
            .trim_start()
            .starts_with("fill: Theme.clawd;")
    );
    assert_eq!(
        mark.matches("Glyphs.clawd").count(),
        2,
        "one Clawd path, its two shapes"
    );
    assert!(
        mark.contains("property <bool> shut: root.kind == 1 && root.m == 1 && MarkMotion.blink;")
    );
}

/// Owner 2026-11-13 ("keep squash, snap to pixels"): the moving box follows flare_agent::icon_motion::icon_box,
/// every edge snapped to whole device pixels of the window, and a hop stands on its feet.
#[test]
fn the_agent_icons_moving_box_snaps_every_edge_to_whole_device_pixels() {
    let mark = ui_file("tabmark.slint");
    for line in [
        "property <length> ax: Px.snap(root.absolute-position.x);",
        "property <length> ay: Px.snap(root.absolute-position.y);",
        "property <length> rest: Px.snap(root.size);",
        "property <length> box-w: Px.snap(root.size * root.sx);",
        "property <length> box-h: Px.snap(root.size * root.sy);",
        "property <length> box-left: Px.snap(root.ax + (root.rest - root.box-w) / 2);",
        "property <float> feet: root.kind == 1 ? (10.04 + 2.5) / 17 : root.kind == 3 ? (8.02 + 1.5) / 11 : 1.0;",
        "? root.ay + Px.snap(root.feet * root.rest) - Px.snap(root.feet * root.box-h) + Px.snap(MarkMotion.hop-y * root.size)",
        ": Px.snap(root.ay + (root.rest - root.box-h) / 2);",
        "width: root.box-w;",
        "height: root.box-h;",
        "x: root.box-left - root.absolute-position.x;",
        "y: root.box-top - root.absolute-position.y;",
    ] {
        assert!(mark.contains(line), "AgentIcon has {line}");
    }
    // The feet in the slint are the agent crate's.
    let (cy, ch) = (
        flare_agent::icon::CLAWD_VIEWBOX.1,
        flare_agent::icon::CLAWD_VIEWBOX.3,
    );
    assert!((flare_agent::icon::CLAWD_FEET - (10.04 - cy) / ch).abs() < 1e-6);
    let (ay, ah) = (
        flare_agent::icon::AGY_VIEWBOX.1,
        flare_agent::icon::AGY_VIEWBOX.3,
    );
    assert!((flare_agent::icon::AGY_FEET - (8.02 - ay) / ah).abs() < 1e-6);
    assert_eq!((cy, ch, ay, ah), (-2.5, 17.0, -1.5, 11.0));
    assert_eq!(flare_agent::icon::OPENAI_FEET, 1.0);
    assert!(
        !mark.contains("width: root.size * root.sx;"),
        "no unsnapped box remains"
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

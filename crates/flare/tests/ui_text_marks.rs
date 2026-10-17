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
fn the_check_and_the_cross_are_the_sixteen_unit_strokes() {
    let g = glyphs();
    assert_eq!(glyph(&g, "check"), "M3.2 8.6l3 3 6.6-7.2");
    assert_eq!(glyph(&g, "cross"), "M4 4l8 8M12 4l-8 8");
}

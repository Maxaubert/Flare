//! The bundled UI face on macOS (rule 1232): Flare Selawik, built from Selawik, Microsoft's open (SIL OFL 1.1)
//! stand-in for Segoe UI with Segoe UI's widths. Its line is set to Segoe UI's 1.33 em (Selawik's own is
//! 1.2 em) and, as the OFL asks of a changed font, it is renamed (`tools/fonts/make-flare-selawik.py`), so the
//! Mac's rows, tabs and cards measure as the Windows ones do. Segoe UI itself may not ship on a Mac. Windows
//! keeps Segoe UI and never carries these files: the bytes and the call exist only in a macOS build. The faces
//! are in `assets/selawik/` (Regular 400, Semibold 600, Bold 700; 500 draws as 400 and 650 as 700, as with
//! Segoe UI's own weights). A character Selawik lacks (the ⌘ ⌃ ⌥ ⇧ of a shortcut label, CJK in a path)
//! falls back per character to the system's faces; its U+2026, which Slint's elide draws, is its own.

/// Registers the faces with the Slint platform just set on this thread. Each Slint context keeps its own
/// font collection, so every place that sets a platform (the app's backend, the headless screenshots, the
/// scene renders, the UI test harness) calls this right after.
#[cfg(target_os = "macos")]
pub fn register() {
    use slint::fontique_011::fontique::Blob;
    use std::sync::Arc;
    const FACES: [&[u8]; 3] = [
        include_bytes!("../../../assets/selawik/flare-selawik-regular.ttf"),
        include_bytes!("../../../assets/selawik/flare-selawik-semibold.ttf"),
        include_bytes!("../../../assets/selawik/flare-selawik-bold.ttf"),
    ];
    let mut collection = slint::fontique_011::shared_collection();
    for face in FACES {
        if collection
            .register_fonts(Blob::new(Arc::new(face)), None)
            .is_empty()
        {
            log::warn!(
                "a bundled Flare Selawik face did not register; the UI falls back to Helvetica"
            );
        }
    }
}

/// Windows keeps Segoe UI: nothing is bundled or registered.
#[cfg(not(target_os = "macos"))]
pub fn register() {}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use crate::ui_harness::{Frame, Harness};

    slint::slint! {
        export component GlyphLine inherits Window {
            width: 160px;
            height: 48px;
            background: black;
            in property <string> text;
            Text {
                x: 8px;
                y: 4px;
                text: root.text;
                font-family: "Flare Selawik";
                font-size: 30px;
                color: white;
            }
        }
    }

    /// The ink of `text` drawn alone at 100 percent: the frame's red channel, one byte a pixel.
    fn ink(text: &str) -> Vec<u8> {
        let h = Harness::new(1.0, 160, 48, || {
            GlyphLine::new().expect("the glyph line builds")
        });
        h.comp.set_text(text.into());
        h.advance(0);
        let f: Frame = h.render();
        f.rgba.chunks(4).map(|p| p[0]).collect()
    }

    /// The runs of columns holding ink, left to right.
    fn column_runs(ink: &[u8]) -> Vec<(usize, usize)> {
        let inked = |x: usize| (0..48).any(|y| ink[y * 160 + x] > 96);
        let mut runs: Vec<(usize, usize)> = Vec::new();
        for x in 0..160 {
            if inked(x) {
                match runs.last_mut() {
                    Some(r) if r.1 + 1 == x => r.1 = x,
                    _ => runs.push((x, x)),
                }
            }
        }
        runs
    }

    #[test]
    fn the_three_faces_register_as_one_flare_selawik_family() {
        crate::ui_harness::ensure_platform();
        let mut collection = slint::fontique_011::shared_collection();
        let family = collection
            .family_by_name("Flare Selawik")
            .expect("Flare Selawik is registered");
        let weights: Vec<f32> = family.fonts().iter().map(|f| f.weight().value()).collect();
        for w in [400.0, 600.0, 700.0] {
            assert!(weights.contains(&w), "weight {w} missing: {weights:?}");
        }
    }

    /// Slint's elide draws U+2026: Selawik has it (a composite of three periods), so a cut path ends in
    /// three dots, not one.
    #[test]
    fn the_ellipsis_is_three_dots() {
        let dots = column_runs(&ink("\u{2026}"));
        assert_eq!(dots.len(), 3, "the ellipsis' dots: {dots:?}");
        assert_eq!(column_runs(&ink(".")).len(), 1, "a period is one dot");
    }

    /// Every character outside ASCII that Flare's UI strings carry is drawn: the ones Selawik has, and the
    /// Mac's key symbols and CJK, which it lacks, from the system's faces by per-character fallback. A
    /// character no face has draws Selawik's empty missing glyph, so ink proves a real glyph.
    #[test]
    fn the_ui_characters_selawik_lacks_fall_back_to_a_real_glyph() {
        assert!(
            ink("\u{E000}").iter().all(|&v| v <= 96),
            "a character no face has draws no ink"
        );
        for c in [
            '\u{2318}', '\u{2303}', '\u{2325}',
            '\u{21E7}', // ⌘ ⌃ ⌥ ⇧, the Mac's shortcut labels
            '\u{00B7}', '\u{2013}', '\u{2014}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{00E9}',
            '\u{65E5}', // CJK in a path or a tab name
        ] {
            let drawn = ink(&c.to_string());
            assert!(
                drawn.iter().any(|&v| v > 96),
                "U+{:04X} draws nothing",
                c as u32
            );
        }
    }
}

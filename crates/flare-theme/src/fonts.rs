//! Terminal font faces, sizes and session zoom (Task 8).

/// One terminal face as Prism Terminal lists it (`stack` is the CSS font-family list).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TermFont {
    pub id: &'static str,
    pub name: &'static str,
    pub stack: &'static str,
}

pub use crate::fonts_data::TERM_FONTS;

/// The face a fresh install shows.
pub const DEFAULT_FONT_ID: &str = "cascadia";
/// The base font size in pixels at 100 percent (PT's `TERM_BASE_FONT_PX`).
pub const TERM_BASE_FONT_PX: i32 = 13;
/// Sizes, 50 to 200 percent in tens (PT's `FONT_PCTS`).
pub const FONT_PCTS: [u32; 16] = [
    50, 60, 70, 80, 90, 100, 110, 120, 130, 140, 150, 160, 170, 180, 190, 200,
];
/// The session zoom range in pixels (PT's `zoomSession`).
pub const ZOOM_MIN_PX: i32 = 7;
/// See [`ZOOM_MIN_PX`].
pub const ZOOM_MAX_PX: i32 = 64;

/// A listed face id, else `cascadia`.
pub fn font_id_or_default(v: Option<&str>) -> &'static str {
    v.and_then(|id| TERM_FONTS.iter().find(|f| f.id == id))
        .map_or(DEFAULT_FONT_ID, |f| f.id)
}

/// The CSS stack of that face (the default face's for an unknown id).
pub fn font_stack(id: &str) -> &'static str {
    TERM_FONTS
        .iter()
        .find(|f| f.id == id)
        .unwrap_or(&TERM_FONTS[0])
        .stack
}

/// The stack split on commas, trimmed, double quotes removed; `monospace` kept as the generic word.
pub fn font_families(id: &str) -> Vec<String> {
    font_stack(id)
        .split(',')
        .map(|part| part.trim().trim_matches('"').to_string())
        .collect()
}

/// `None`, empty, NaN, below 50 or above 200 gives 100; else the nearest ten.
pub fn font_pct_from(raw: Option<&str>) -> u32 {
    let v = match raw
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::parse::<f64>)
    {
        Some(Ok(v)) if v.is_finite() => v,
        _ => return 100,
    };
    if !(50.0..=200.0).contains(&v) {
        return 100;
    }
    (crate::num::js_round(v / 10.0) * 10.0) as u32
}

/// `Math.round(13 * pct / 100)`.
pub fn base_font_px(pct: u32) -> i32 {
    crate::num::js_round(f64::from(TERM_BASE_FONT_PX) * f64::from(pct) / 100.0) as i32
}

/// `clamp(current + delta, 7, 64)`.
pub fn zoom_step(current_px: i32, delta: i32) -> i32 {
    (current_px + delta).clamp(ZOOM_MIN_PX, ZOOM_MAX_PX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::fixture;

    #[test]
    fn defaults_are_cascadia_100_percent_of_13_px() {
        assert_eq!(font_id_or_default(None), "cascadia");
        assert_eq!(font_pct_from(None), 100);
        assert_eq!(base_font_px(100), 13);
    }

    #[test]
    fn a_size_maps_to_pixels() {
        assert_eq!(base_font_px(120), 16);
        assert_eq!(base_font_px(50), 7);
        assert_eq!(base_font_px(200), 26);
    }

    #[test]
    fn an_off_menu_percentage_falls_back_to_100() {
        assert_eq!(font_pct_from(Some("733")), 100);
        assert_eq!(font_pct_from(Some("20")), 100);
        assert_eq!(font_pct_from(Some("")), 100);
        assert_eq!(font_pct_from(Some("soup")), 100);
        assert_eq!(font_pct_from(Some("NaN")), 100);
    }

    #[test]
    fn sizes_run_50_to_200_in_tens_and_an_old_step_lands_on_the_nearest() {
        assert_eq!(FONT_PCTS[0], 50);
        assert_eq!(FONT_PCTS[FONT_PCTS.len() - 1], 200);
        assert!(FONT_PCTS.windows(2).all(|w| w[1] - w[0] == 10));
        assert_eq!(font_pct_from(Some("125")), 130);
        assert_eq!(font_pct_from(Some("175")), 180);
        assert_eq!(font_pct_from(Some("60")), 60);
    }

    #[test]
    fn fifteen_faces_cascadia_first() {
        assert_eq!(TERM_FONTS.len(), 15);
        let ids: Vec<&str> = TERM_FONTS.iter().map(|f| f.id).collect();
        assert_eq!(
            ids,
            [
                "cascadia",
                "cascadia-code",
                "consolas",
                "jetbrains",
                "jetbrains-nf",
                "fira",
                "source",
                "lucida",
                "courier",
                "ibm-plex",
                "roboto-mono",
                "ubuntu-mono",
                "hack",
                "iosevka",
                "dejavu",
            ]
        );
        let mut unique = ids.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), 15);
    }

    #[test]
    fn an_unknown_id_reads_cascadia() {
        assert_eq!(font_id_or_default(Some("comic")), "cascadia");
        assert_eq!(font_id_or_default(Some("")), "cascadia");
        assert_eq!(font_id_or_default(Some("courier")), "courier");
    }

    #[test]
    fn every_stack_names_its_face_first_and_falls_through() {
        assert_eq!(
            font_families("cascadia"),
            ["Cascadia Mono", "JetBrainsMono NF", "Consolas", "monospace"]
        );
        assert_eq!(
            font_families("courier"),
            ["Courier New", "Courier", "monospace"]
        );
        assert_eq!(font_families("hack")[0], "Hack");
        for face in TERM_FONTS.iter() {
            let families = font_families(face.id);
            let last = families.last().map(String::as_str).unwrap_or("");
            let ends_ok = last == "monospace" || TERM_FONTS.iter().any(|f| f.name == last);
            assert!(ends_ok, "{} ends with {last}", face.id);
        }
    }

    #[test]
    fn oracle_fonts_equal() {
        let want = fixture("fonts.json");
        let want = want.as_array().expect("fonts.json is an array");
        assert_eq!(want.len(), TERM_FONTS.len());
        for (got, want) in TERM_FONTS.iter().zip(want) {
            assert_eq!(got.id, want["id"].as_str().unwrap());
            assert_eq!(got.name, want["name"].as_str().unwrap());
            assert_eq!(got.stack, want["stack"].as_str().unwrap());
        }
    }

    #[test]
    fn zoom_clamps_7_to_64_one_per_notch() {
        assert_eq!(zoom_step(13, 1), 14);
        assert_eq!(zoom_step(13, -1), 12);
        assert_eq!(zoom_step(7, -1), 7);
        assert_eq!(zoom_step(64, 1), 64);
        assert_eq!(zoom_step(100, 1), 64);
    }
}

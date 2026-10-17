//! The wall order: oklch, order_term_themes, wall_ids (Task 7). Port of PT `core/renderer/lib/themeOrder.ts`
//! and the ordering in `ThemeWall.tsx`.

use crate::ansi::normalize_color;
use crate::oklab::to_lab;
use crate::preset::{presets, Preset};
use std::cmp::Ordering;

/// Below this OKLab chroma a ground reads as neutral (PT `NEUTRAL_CHROMA`).
pub const NEUTRAL_CHROMA: f64 = 0.0125;

/// (L, C) of a colour in OKLab: lightness and chroma.
pub fn oklch(colour: &str) -> (f64, f64) {
    let [l, a, b] = to_lab(&normalize_color(colour, "#000000"));
    (l, a.hypot(b))
}

/// Neutral grounds first, then coloured, each group black to white by OKLab L. Stable.
pub fn order_term_themes<T: Clone>(list: &[T], bg: impl Fn(&T) -> String) -> Vec<T> {
    let mut keyed: Vec<(u8, f64, T)> = list
        .iter()
        .map(|item| {
            let (l, c) = oklch(&bg(item));
            (u8::from(c >= NEUTRAL_CHROMA), l, item.clone())
        })
        .collect();
    keyed.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.1.partial_cmp(&b.1).unwrap_or(Ordering::Equal))
    });
    keyed.into_iter().map(|k| k.2).collect()
}

/// The wall: `custom` first when a Custom is saved, then the host default when it is a preset, then
/// every other preset in `order_term_themes` order.
pub fn wall_ids(custom_saved: bool, host_default: &str) -> Vec<String> {
    let mut out = Vec::new();
    if custom_saved {
        out.push("custom".to_string());
    }
    if let Some(default) = presets().iter().find(|p| p.id == host_default) {
        out.push(default.id.to_string());
    }
    let rest: Vec<Preset> = presets()
        .iter()
        .copied()
        .filter(|p| p.id != host_default)
        .collect();
    out.extend(
        order_term_themes(&rest, |p| p.bg.to_string())
            .into_iter()
            .map(|p| p.id.to_string()),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::fixture;
    use std::collections::BTreeSet;

    fn by_bg(p: &Preset) -> String {
        p.bg.to_string()
    }

    fn ids(list: &[Preset]) -> Vec<&'static str> {
        list.iter().map(|p| p.id).collect()
    }

    fn fixture_ids(name: &str) -> Vec<String> {
        fixture(name)
            .as_array()
            .expect("an array")
            .iter()
            .map(|v| v.as_str().expect("an id").to_string())
            .collect()
    }

    #[test]
    fn oklch_measures_black_white_and_grey_black_to_white() {
        let (black, _) = oklch("#000000");
        assert!(black.abs() < 5e-4, "black L {black}");
        let (white, _) = oklch("#ffffff");
        assert!((white - 1.0).abs() < 5e-4, "white L {white}");
        let (_, grey) = oklch("#808080");
        assert!(grey < 1e-4, "grey C {grey}");
        let (_, red) = oklch("#ff0000");
        assert!(red > 0.2, "red C {red}");
    }

    #[test]
    fn order_is_a_permutation_and_leaves_the_input_alone() {
        let input = presets().to_vec();
        let a = order_term_themes(&input, by_bg);
        let b = order_term_themes(&input, by_bg);
        assert_eq!(a.len(), 39);
        assert_eq!(input, presets().to_vec(), "the input is untouched");
        let want: BTreeSet<&str> = presets().iter().map(|p| p.id).collect();
        let got: BTreeSet<&str> = a.iter().map(|p| p.id).collect();
        assert_eq!(got, want);
        assert_eq!(ids(&a), ids(&b), "calling twice gives the same order");
    }

    #[test]
    fn every_neutral_theme_comes_before_every_coloured_one() {
        let o = order_term_themes(presets(), by_bg);
        let first = o
            .iter()
            .position(|p| oklch(p.bg).1 >= NEUTRAL_CHROMA)
            .expect("a coloured one");
        assert!(o[first..].iter().all(|p| oklch(p.bg).1 >= NEUTRAL_CHROMA));
    }

    #[test]
    fn each_group_runs_black_to_white() {
        let o = order_term_themes(presets(), by_bg);
        for pair in o.windows(2) {
            let (ca, cb) = (oklch(pair[0].bg), oklch(pair[1].bg));
            if (ca.1 >= NEUTRAL_CHROMA) == (cb.1 >= NEUTRAL_CHROMA) {
                assert!(ca.0 <= cb.0, "{} then {}", pair[0].id, pair[1].id);
            }
        }
    }

    #[test]
    fn the_eleven_neutral_grounds() {
        let mut neutral: Vec<&str> = presets()
            .iter()
            .filter(|p| oklch(p.bg).1 < NEUTRAL_CHROMA)
            .map(|p| p.id)
            .collect();
        neutral.sort_unstable();
        assert_eq!(
            neutral,
            [
                "campbell",
                "catppuccin-latte",
                "graphite",
                "gruvbox-dark",
                "mist",
                "monokai",
                "paper",
                "pitch",
                "prism",
                "pt-default",
                "volt"
            ]
        );
    }

    #[test]
    fn no_preset_ground_is_within_0_001_of_the_neutral_line() {
        for p in presets() {
            let c = oklch(p.bg).1;
            assert!(
                (c - NEUTRAL_CHROMA).abs() > 0.001,
                "{} sits on the line at {c}",
                p.id
            );
        }
    }

    #[test]
    fn ties_keep_the_list_order() {
        let list = [("b", "#101010"), ("a", "#101010")];
        let out = order_term_themes(&list, |t| t.1.to_string());
        assert_eq!(out, list.to_vec());
    }

    #[test]
    fn holds_the_whole_order() {
        let o = order_term_themes(presets(), by_bg);
        assert_eq!(ids(&o), fixture_ids("wall_order.json"));
    }

    #[test]
    fn the_wall_is_custom_then_the_default_then_the_rest() {
        let with = wall_ids(true, "pt-default");
        assert_eq!(&with[..2], ["custom", "pt-default"]);
        let without = wall_ids(false, "pt-default");
        assert_eq!(without[0], "pt-default");
        let rest: Vec<String> = without[1..].to_vec();
        let want: Vec<String> = fixture_ids("wall_order.json")
            .into_iter()
            .filter(|id| id != "pt-default")
            .collect();
        assert_eq!(rest, want);
        assert_eq!(with.len(), 40);
    }
}

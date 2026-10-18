//! Composites, legible ink, selection and the commit rule (Task 9). Source: PT `core/renderer/lib/colour.ts`
//! lines 196 to 425 and the rest of `colour.test.ts`.

use crate::ansi::{contrast_ratio, mix_hex, toward_room};
use crate::colour::{format, opaque, parse_colour, to_stored, with_alpha, ColourFormat, Rgba};
use crate::num::{clamp, js_round};

/// The opaque `#rrggbb` the eye gets when `c`, with its alpha, lies on the opaque `ground`.
pub fn composite(c: &str, ground: &str) -> String {
    let g = parse_colour(ground).unwrap_or(Rgba {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    });
    match parse_colour(c) {
        None => to_stored(&Rgba { a: 1.0, ..g }),
        Some(p) => to_stored(&Rgba {
            r: g.r + (p.r - g.r) * p.a,
            g: g.g + (p.g - g.g) * p.a,
            b: g.b + (p.b - g.b) * p.a,
            a: 1.0,
        }),
    }
}

/// `colour` (opaque) walked towards white or black, the LEAST distance that reaches `floor` (PT `towardsFloor`:
/// a bisection, so the result follows its input smoothly).
fn towards_floor(colour: &str, ground: &str, floor: f64) -> String {
    if contrast_ratio(colour, ground) >= floor {
        return colour.to_string();
    }
    let to = toward_room(ground);
    if contrast_ratio(to, ground) < floor {
        return to.to_string();
    }
    let (mut lo, mut hi) = (0.0_f64, 1.0_f64);
    for _ in 0..20 {
        let mid = (lo + hi) / 2.0;
        if contrast_ratio(&mix_hex(colour, to, mid), ground) >= floor {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    mix_hex(colour, to, hi)
}

/// A see-through colour that must stay legible, as the opaque colour it shows (PT `legibleOn`). The floor is
/// `min(floor, contrast of the opaque pick)`, so alpha never makes a colour less legible than the user's pick.
pub fn legible_on(c: &str, ground: &str, floor: f64) -> String {
    let g = opaque(ground, "#000000");
    let f = floor.min(contrast_ratio(&opaque(c, "#000000"), &g));
    towards_floor(&composite(c, &g), &g, f)
}

/// The plain floor, for what is floored even when opaque (PT `legibleOnStrict`).
pub fn legible_on_strict(c: &str, ground: &str, floor: f64) -> String {
    let g = opaque(ground, "#000000");
    towards_floor(&composite(c, &g), &g, floor)
}

/// Black or white text for a fill, chosen on what the eye sees (PT `inkOn`). An opaque fill keeps the strip's
/// rule: white unless the fill reads 12:1 against black. A see-through fill takes the better of the two on its
/// composite over `ground`.
pub fn ink_on(tint: &str, ground: &str) -> &'static str {
    let t = parse_colour(tint);
    if let Some(t) = t {
        if t.a < 1.0 && parse_colour(ground).is_some() {
            let fill = composite(tint, ground);
            return if contrast_ratio("#000000", &fill) >= contrast_ratio("#ffffff", &fill) {
                "#000000"
            } else {
                "#ffffff"
            };
        }
    }
    let fill = match t {
        Some(t) => to_stored(&Rgba { a: 1.0, ..t }),
        None => tint.to_string(),
    };
    if contrast_ratio("#000000", &fill) < 12.0 {
        "#ffffff"
    } else {
        "#000000"
    }
}

/// `(opaque fill, ink)` for a text-bearing selection fill (PT `selectionFor`). The ink clears 4.5:1 on the fill
/// as seen over every ground; if neither leaning ink can, the pair whose worst ground reads best is kept.
pub fn selection_for(fill: &str, grounds: &[&str]) -> (String, &'static str) {
    let p = parse_colour(fill).unwrap_or(Rgba {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    });
    let a = p.a;
    let solid = to_stored(&Rgba { a: 1.0, ..p });
    let gs: Vec<String> = if grounds.is_empty() {
        vec![solid.clone()]
    } else {
        grounds.iter().map(|g| opaque(g, "#000000")).collect()
    };
    let seen =
        |c: &str| -> Vec<String> { gs.iter().map(|g| composite(&with_alpha(c, a), g)).collect() };
    let worst = |c: &str, ink: &str| -> f64 {
        seen(c)
            .iter()
            .map(|s| contrast_ratio(ink, s))
            .fold(f64::INFINITY, f64::min)
    };
    let first = seen(&solid)[0].clone();
    let leaning = if contrast_ratio("#ffffff", &first) >= contrast_ratio("#0b0d12", &first) {
        "#ffffff"
    } else {
        "#0b0d12"
    };
    let mut best_fill = solid.clone();
    let mut best_ink = leaning;
    let mut best_score = -1.0_f64;
    let other = if leaning == "#ffffff" {
        "#0b0d12"
    } else {
        "#ffffff"
    };
    for ink in [leaning, other] {
        let towards = if ink == "#ffffff" {
            "#000000"
        } else {
            "#ffffff"
        };
        for i in 0..=25 {
            let c = mix_hex(&solid, towards, f64::from(i) * 0.04);
            let score = worst(&c, ink);
            if score >= 4.5 {
                return (c, ink);
            }
            if score > best_score {
                best_fill = c;
                best_ink = ink;
                best_score = score;
            }
        }
    }
    (best_fill, best_ink)
}

/// What a field may hold for alpha (PT `AlphaRange`). Default: alpha allowed, 0 to 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AlphaRange {
    pub alpha: bool,
    pub alpha_min: f64,
    pub alpha_max: f64,
}

impl Default for AlphaRange {
    fn default() -> Self {
        AlphaRange {
            alpha: true,
            alpha_min: 0.0,
            alpha_max: 1.0,
        }
    }
}

/// A colour held to a field's alpha rules (PT `fitAlpha`).
pub fn fit_alpha(c: &Rgba, r: &AlphaRange) -> Rgba {
    Rgba {
        a: if r.alpha {
            clamp(c.a, r.alpha_min, r.alpha_max)
        } else {
            1.0
        },
        ..*c
    }
}

/// The arguments of a functional colour: comma syntax, or space syntax with an optional `/ alpha`. A copy of
/// `colour.rs`'s private helper (that file belongs to another task); keep the two in step.
fn args(body: &str) -> Option<Vec<String>> {
    let s = body.trim();
    if s.contains(',') {
        let parts: Vec<String> = s.split(',').map(|p| p.trim().to_string()).collect();
        return parts.iter().all(|p| !p.is_empty()).then_some(parts);
    }
    let mut it = s.split('/');
    let main = it.next().unwrap_or("");
    let alpha = it.next();
    if it.next().is_some() {
        return None;
    }
    let mut parts: Vec<String> = main.split_whitespace().map(str::to_string).collect();
    if let Some(a) = alpha {
        let a = a.trim();
        if a.is_empty() {
            return None;
        }
        parts.push(a.to_string());
    }
    Some(parts)
}

/// Whether a typed code carries an alpha of its own: `#rgba`, `#rrggbbaa`, or a fourth argument to rgb()/hsl()
/// (PT `saysAlpha`). It reads the code, not the colour.
pub fn says_alpha(text: &str) -> bool {
    let v = text.trim().to_lowercase();
    let h = v.strip_prefix('#').unwrap_or(&v);
    if !h.is_empty() && h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return h.len() == 4 || h.len() == 8;
    }
    let Some(open) = v.find('(') else {
        return false;
    };
    let name = &v[..open];
    if !matches!(name, "rgb" | "rgba" | "hsl" | "hsla") || !v.ends_with(')') {
        return false;
    }
    let body = &v[open + 1..v.len() - 1];
    args(body).is_some_and(|a| a.len() == 4)
}

/// What a code field's blur or Enter commits, or None (PT `colourCommit`): only a typed draft that parses AND
/// names another colour, alpha included, in the stored form. A code with no alpha keeps the alpha in force.
pub fn colour_commit(draft: Option<&str>, value: &str, range: &AlphaRange) -> Option<String> {
    let draft = draft?;
    let typed = parse_colour(draft)?;
    let squash = |s: &str| s.to_lowercase().split_whitespace().collect::<String>();
    let was = parse_colour(value);
    if let Some(was) = was {
        let shown = fit_alpha(&was, range);
        let d = squash(draft);
        if [ColourFormat::Hex, ColourFormat::Rgba, ColourFormat::Hsla]
            .into_iter()
            .any(|f| squash(&format(&shown, f)) == d)
        {
            return None;
        }
    }
    let own = match was {
        Some(w) if !says_alpha(draft) => Rgba { a: w.a, ..typed },
        _ => typed,
    };
    let next = to_stored(&fit_alpha(&own, range));
    match was {
        Some(w) if next == to_stored(&fit_alpha(&w, range)) => None,
        _ => Some(next),
    }
}

/// Options for `snap_alpha_step` (PT `SnapOpts`). Default: min 0, max 1, no host steps.
#[derive(Clone, Copy, Default)]
pub struct SnapOpts<'a> {
    pub min: f64,
    pub max: f64,
    pub snap: Option<&'a dyn Fn(f64) -> f64>,
}

/// Where the alpha bar lands for a pointer or key value (PT `snapAlphaStep`).
pub fn snap_alpha_step(a: f64, held: f64, key: bool, opts: &SnapOpts) -> f64 {
    let fit = |n: f64| opts.max.min(opts.min.max(n));
    let want = fit(a);
    let Some(snap) = opts.snap else {
        return want;
    };
    let at = snap(want);
    let was = snap(held);
    if !key || at != was || (want - held).abs() > 0.011 {
        return at;
    }
    let dir: i64 = if want > held { 1 } else { -1 };
    let lo = js_round(opts.min * 100.0) as i64;
    let hi = js_round(opts.max * 100.0) as i64;
    let mut p = js_round(want * 100.0) as i64 + dir;
    while p >= lo && p <= hi {
        let next = snap(fit(p as f64 / 100.0));
        if next != was {
            return next;
        }
        p += dir;
    }
    was
}

/// The desktop a see-through window without acrylic is assumed to sit on: mid grey (Task 57; the measured
/// table in `tools/glass-light/contrast.csv`).
pub const ASSUMED_DESKTOP: Rgba = Rgba {
    r: 128.0,
    g: 128.0,
    b: 128.0,
    a: 1.0,
};

/// What the eye sees behind the text: the ground at `alpha` over the assumed desktop. Alpha 1 is the ground.
pub fn effective_ground(ground: &Rgba, alpha: f64, assumed_desktop: &Rgba) -> Rgba {
    let a = alpha.clamp(0.0, 1.0);
    Rgba {
        r: assumed_desktop.r + (ground.r - assumed_desktop.r) * a,
        g: assumed_desktop.g + (ground.g - assumed_desktop.g) * a,
        b: assumed_desktop.b + (ground.b - assumed_desktop.b) * a,
        a: 1.0,
    }
}

/// The sixteen for a see-through window without acrylic (alpha below 1): on a LIGHT ground each is walked
/// to the 3:1 ANSI floor against the effective ground (Task 57, DIFF-12 PROPOSED: readability beyond Prism
/// Terminal, which floors against the theme ground only). A solid window (alpha 1) and a dark ground come
/// back unchanged.
pub fn lift_ansi_for_see_through(
    ansi: &[Option<String>; 16],
    ground: &str,
    alpha: f64,
) -> [Option<String>; 16] {
    let opaque_ground = opaque(ground, "#000000");
    if alpha >= 1.0 || crate::ansi::luminance(&opaque_ground) <= crate::ground::LIGHT_GROUND {
        return ansi.clone();
    }
    let Some(g) = parse_colour(&opaque_ground) else {
        return ansi.clone();
    };
    let eff = to_stored(&effective_ground(&g, alpha, &ASSUMED_DESKTOP));
    ansi.clone()
        .map(|e| e.map(|c| legible_on_strict(&c, &eff, crate::ansi::ANSI_CONTRAST_FLOOR)))
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::colour::{hsl_to_rgb, Hsla};
    use crate::testutil::fixture;

    fn steps95(a: f64) -> f64 {
        if a >= 1.0 {
            1.0
        } else {
            a.min(0.95)
        }
    }

    fn snap_opts() -> SnapOpts<'static> {
        SnapOpts {
            min: 0.0,
            max: 1.0,
            snap: Some(&steps95 as &dyn Fn(f64) -> f64),
        }
    }

    #[test]
    fn composite_alpha_0_is_the_ground_and_alpha_1_the_colour() {
        assert_eq!(composite("#ff000000", "#102030"), "#102030");
        assert_eq!(composite("#ff0000", "#102030"), "#ff0000");
        assert_eq!(composite("#ffffff80", "#000000"), "#808080");
    }

    #[test]
    fn legible_on_at_alpha_1_returns_the_pick_unchanged() {
        assert_eq!(legible_on("#222222", "#111111", 4.5), "#222222");
        assert_eq!(legible_on("#d7dae1", "#0b0b0f", 4.5), "#d7dae1");
    }

    #[test]
    fn legible_on_is_never_less_legible_than_the_opaque_pick() {
        for ground in ["#0b0b0f", "#ffffff", "#3c3c64", "#808080"] {
            for pick in ["#e05561", "#d7dae1", "#4aa5f0", "#222222", "#fafafa"] {
                for a in 1..=10 {
                    let c = with_alpha(pick, f64::from(a) / 10.0);
                    let want = contrast_ratio(pick, ground).min(4.5);
                    let got = contrast_ratio(&legible_on(&c, ground, 4.5), ground);
                    assert!(got >= want - 0.02, "{c} on {ground}: {got} < {want}");
                }
            }
        }
    }

    #[test]
    fn legible_on_is_continuous_in_alpha() {
        let lum = |h: &str| -> i64 {
            (1..7)
                .step_by(2)
                .map(|i| i64::from_str_radix(&h[i..i + 2], 16).unwrap())
                .sum()
        };
        let mut prev = lum(&legible_on(&with_alpha("#4aa5f0", 0.0), "#0b0b0f", 4.5));
        for a in 1..=255 {
            let next = lum(&legible_on(
                &with_alpha("#4aa5f0", f64::from(a) / 255.0),
                "#0b0b0f",
                4.5,
            ));
            assert!((next - prev).abs() <= 12, "alpha {a}");
            prev = next;
        }
    }

    #[test]
    fn legible_on_strict_floors_even_an_opaque_pick() {
        assert!(contrast_ratio(&legible_on_strict("#222222", "#111111", 3.0), "#111111") >= 3.0);
    }

    #[test]
    fn ink_on_a_see_through_light_tint_over_dark_takes_white() {
        assert_eq!(ink_on("#ffd0a040", "#101010"), "#ffffff");
    }

    #[test]
    fn ink_on_a_see_through_tint_over_a_light_ground_takes_black() {
        assert_eq!(ink_on("#3b82f680", "#fafafa"), "#000000");
    }

    #[test]
    fn an_opaque_tint_keeps_the_strip_rule() {
        assert_eq!(ink_on("#ec9448", "#101010"), "#ffffff");
        assert_eq!(ink_on("#f0f0f0", "#101010"), "#000000");
    }

    #[test]
    fn any_see_through_tint_reads_4_5_on_its_composite() {
        for ground in ["#000000", "#1e1f29", "#808080", "#d4d4d4", "#ffffff"] {
            for h in (0..360).step_by(30) {
                let solid = to_stored(&hsl_to_rgb(&Hsla {
                    h: f64::from(h),
                    s: 80.0,
                    l: 55.0,
                    a: 1.0,
                }));
                for a in 1..10 {
                    let tint = with_alpha(&solid, f64::from(a) / 10.0);
                    let ratio = contrast_ratio(ink_on(&tint, ground), &composite(&tint, ground));
                    assert!(ratio >= 4.5, "{tint} on {ground}: {ratio}");
                }
            }
        }
    }

    #[test]
    fn selection_for_ink_clears_4_5_over_every_ground() {
        let grounds = ["#0b0b0f", "#1b1d24"];
        for accent in ["#4682fb", "#ec9448", "#5b5bd6", "#a6e22e"] {
            for a in [0.4, 0.6, 0.8, 1.0] {
                let (fill, ink) = selection_for(&with_alpha(accent, a), &grounds);
                assert_eq!(fill.len(), 7, "{fill}");
                for g in grounds {
                    let seen = composite(&with_alpha(&fill, a), g);
                    assert!(contrast_ratio(ink, &seen) >= 4.5, "{accent} {a} on {g}");
                }
            }
        }
    }

    #[test]
    fn selection_for_keeps_the_best_pair_when_no_fill_serves() {
        let (_, ink) = selection_for("#4682fb66", &["#000000", "#f5f5f5"]);
        assert!(ink == "#ffffff" || ink == "#0b0d12");
    }

    #[test]
    fn commit_nothing_typed() {
        assert_eq!(colour_commit(None, "#aabbcc", &AlphaRange::default()), None);
    }

    #[test]
    fn commit_nothing_for_the_same_colour_in_any_form() {
        let r = AlphaRange::default();
        assert_eq!(colour_commit(Some("#AABBCC"), "#aabbcc", &r), None);
        assert_eq!(colour_commit(Some("abc"), "#aabbcc", &r), None);
        assert_eq!(
            colour_commit(Some("rgb(170, 187, 204)"), "#aabbcc", &r),
            None
        );
        assert_eq!(colour_commit(Some("#aabbccff"), "#aabbcc", &r), None);
    }

    #[test]
    fn commit_a_new_alpha() {
        let r = AlphaRange::default();
        assert_eq!(
            colour_commit(Some("#aabbcc80"), "#aabbcc", &r).as_deref(),
            Some("#aabbcc80")
        );
        let hsla = "hsla(200, 50%, 40%, 0.5)";
        assert_eq!(
            colour_commit(Some(hsla), "#aabbcc", &r),
            Some(to_stored(&parse_colour(hsla).unwrap()))
        );
    }

    #[test]
    fn commit_nothing_for_junk() {
        assert_eq!(
            colour_commit(Some("zz"), "#aabbcc", &AlphaRange::default()),
            None
        );
    }

    #[test]
    fn alpha_false_drops_a_typed_alpha() {
        let r = AlphaRange {
            alpha: false,
            ..AlphaRange::default()
        };
        assert_eq!(
            colour_commit(Some("#11223380"), "#aabbcc", &r).as_deref(),
            Some("#112233")
        );
        assert_eq!(colour_commit(Some("#aabbcc80"), "#aabbcc", &r), None);
    }

    #[test]
    fn a_code_with_no_alpha_keeps_the_alpha_in_force() {
        let r = AlphaRange::default();
        assert_eq!(
            colour_commit(Some("#203040"), "#0b0d12cb", &r).as_deref(),
            Some("#203040cb")
        );
        assert_eq!(
            colour_commit(Some("203040"), "#0b0d12cb", &r).as_deref(),
            Some("#203040cb")
        );
        assert_eq!(
            colour_commit(Some("rgb(32, 48, 64)"), "#0b0d1280", &r).as_deref(),
            Some("#20304080")
        );
        let hsl = "hsl(210, 33%, 19%)";
        let want = to_stored(&Rgba {
            a: 128.0 / 255.0,
            ..parse_colour(hsl).unwrap()
        });
        assert_eq!(colour_commit(Some(hsl), "#0b0d1280", &r), Some(want));
    }

    #[test]
    fn a_code_that_says_an_alpha_sets_it_solid_included() {
        let r = AlphaRange::default();
        assert_eq!(
            colour_commit(Some("#203040ff"), "#0b0d12cb", &r).as_deref(),
            Some("#203040")
        );
        assert_eq!(
            colour_commit(Some("#20304080"), "#0b0d12cb", &r).as_deref(),
            Some("#20304080")
        );
        assert_eq!(
            colour_commit(Some("rgba(32, 48, 64, 1)"), "#0b0d1280", &r).as_deref(),
            Some("#203040")
        );
    }

    #[test]
    fn says_alpha_reads_the_code_not_the_colour() {
        assert_eq!(
            [
                says_alpha("#abc"),
                says_alpha("#abcd"),
                says_alpha("aabbcc"),
                says_alpha("#aabbcc80")
            ],
            [false, true, false, true]
        );
        assert_eq!(
            [
                says_alpha("rgb(1,2,3)"),
                says_alpha("rgba(1,2,3,0.5)"),
                says_alpha("hsl(1 2% 3% / 50%)")
            ],
            [false, true, true]
        );
    }

    #[test]
    fn commit_clamps_to_alpha_min_and_max() {
        let low = AlphaRange {
            alpha_min: 0.3,
            ..AlphaRange::default()
        };
        assert_eq!(
            colour_commit(Some("#11223300"), "#aabbcc", &low).as_deref(),
            Some("#1122334d")
        );
        let high = AlphaRange {
            alpha_max: 254.0 / 255.0,
            ..AlphaRange::default()
        };
        assert_eq!(
            colour_commit(Some("#112233"), "#aabbcc", &high).as_deref(),
            Some("#112233fe")
        );
    }

    #[test]
    fn shown_strings_reparse_and_commit_nothing() {
        let steps = [0.0, 51.0, 102.0, 153.0, 204.0, 255.0];
        for r in steps {
            for g in steps {
                for a in [0u32, 1, 37, 127, 128, 200, 255] {
                    let value = to_stored(&Rgba {
                        r,
                        g,
                        b: 77.0,
                        a: f64::from(a) / 255.0,
                    });
                    for f in [ColourFormat::Hex, ColourFormat::Rgba, ColourFormat::Hsla] {
                        let shown = format(&parse_colour(&value).unwrap(), f);
                        assert!(parse_colour(&shown).is_some(), "{shown}");
                        assert_eq!(
                            colour_commit(Some(&shown), &value, &AlphaRange::default()),
                            None,
                            "{shown}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn snap_without_host_steps_is_the_value() {
        assert_eq!(
            snap_alpha_step(
                0.42,
                1.0,
                true,
                &SnapOpts {
                    min: 0.0,
                    max: 1.0,
                    snap: None
                }
            ),
            0.42
        );
    }

    #[test]
    fn snap_shows_the_stored_step() {
        assert_eq!(snap_alpha_step(0.98, 1.0, false, &snap_opts()), 0.95);
    }

    #[test]
    fn one_key_press_down_from_solid_lands_on_95() {
        assert_eq!(snap_alpha_step(0.99, 1.0, true, &snap_opts()), 0.95);
    }

    #[test]
    fn one_key_press_up_from_95_reaches_solid() {
        assert_eq!(snap_alpha_step(0.96, 0.95, true, &snap_opts()), 1.0);
    }

    #[test]
    fn below_the_steps_a_key_press_moves_one_percent() {
        assert_eq!(snap_alpha_step(0.6, 0.61, true, &snap_opts()), 0.6);
    }

    #[test]
    fn oracle_commit_vectors() {
        let vectors = fixture("commit_vectors.json");
        let rows = vectors.as_array().unwrap();
        assert_eq!(rows.len(), 12);
        for row in rows {
            let draft = row["draft"].as_str();
            let value = row["value"].as_str().unwrap();
            let want = row["out"].as_str().map(str::to_string);
            assert_eq!(
                colour_commit(draft, value, &AlphaRange::default()),
                want,
                "{row}"
            );
        }
    }

    #[test]
    fn oracle_legible_selection_vectors() {
        let legible = fixture("legible_vectors.json");
        for row in legible.as_array().unwrap() {
            let c = row["c"].as_str().unwrap();
            let g = row["g"].as_str().unwrap();
            assert_eq!(
                legible_on(c, g, 4.5),
                row["floor45"].as_str().unwrap(),
                "floor45 {c} {g}"
            );
            assert_eq!(
                legible_on(c, g, 3.0),
                row["floor3"].as_str().unwrap(),
                "floor3 {c} {g}"
            );
            assert_eq!(
                legible_on_strict(c, g, 3.0),
                row["strict3"].as_str().unwrap(),
                "strict3 {c} {g}"
            );
            assert_eq!(ink_on(c, g), row["ink"].as_str().unwrap(), "ink {c} {g}");
            assert_eq!(
                composite(c, g),
                row["composite"].as_str().unwrap(),
                "composite {c} {g}"
            );
        }
        let selection = fixture("selection_vectors.json");
        for row in selection.as_array().unwrap() {
            let fill = row["fill"].as_str().unwrap();
            let grounds: Vec<&str> = row["grounds"]
                .as_array()
                .unwrap()
                .iter()
                .map(|g| g.as_str().unwrap())
                .collect();
            let (got_fill, got_ink) = selection_for(fill, &grounds);
            assert_eq!(
                got_fill,
                row["out"]["fill"].as_str().unwrap(),
                "fill {fill}"
            );
            assert_eq!(got_ink, row["out"]["ink"].as_str().unwrap(), "ink {fill}");
        }
    }
}

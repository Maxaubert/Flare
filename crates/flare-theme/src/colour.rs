//! Rgba, colour parsing, the stored form and conversions (Task 4). Port of Prism Terminal's `colour.ts`.

use crate::num::{clamp, hex_byte, js_round};

/// r, g, b 0..255 (fractions allowed on the way through), a 0..1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgba {
    pub r: f64,
    pub g: f64,
    pub b: f64,
    pub a: f64,
}

/// h 0..360, s and v 0..100, a 0..1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hsva {
    pub h: f64,
    pub s: f64,
    pub v: f64,
    pub a: f64,
}

/// h 0..360, s and l 0..100, a 0..1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hsla {
    pub h: f64,
    pub s: f64,
    pub l: f64,
    pub a: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColourFormat {
    Hex,
    Rgba,
    Hsla,
}

/// A number, or a number followed by `%` (scaled by `pct`), or None.
fn num(tok: &str, pct: Option<f64>) -> Option<f64> {
    let (body, has_pct) = match tok.strip_suffix('%') {
        Some(b) => (b, true),
        None => (tok, false),
    };
    let unsigned = body.strip_prefix(['+', '-']).unwrap_or(body);
    let (int, frac) = match unsigned.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (unsigned, None),
    };
    let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    let ok = match frac {
        None => !int.is_empty() && digits(int),
        Some(f) => digits(int) && digits(f) && (!int.is_empty() || !f.is_empty()),
    };
    if !ok {
        return None;
    }
    let n: f64 = body.parse().ok()?;
    if !n.is_finite() {
        return None;
    }
    if has_pct {
        pct.map(|p| n * p)
    } else {
        Some(n)
    }
}

/// An alpha token: 0..1 or 0%..100%.
fn alpha_tok(tok: Option<&str>) -> Option<f64> {
    match tok {
        None => Some(1.0),
        Some(t) => num(t, Some(0.01)).filter(|n| (0.0..=1.0).contains(n)),
    }
}

/// The arguments of a functional colour: comma syntax, or space syntax with an optional `/ alpha`.
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

/// Every form the code field takes: `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa` (the `#` optional), `rgb()`/`rgba()`
/// and `hsl()`/`hsla()` in comma or space syntax. Anything else, and any channel out of range, is None.
pub fn parse_colour(text: &str) -> Option<Rgba> {
    let v = text.trim().to_lowercase();
    if v.is_empty() {
        return None;
    }
    let h = v.strip_prefix('#').unwrap_or(&v);
    if matches!(h.len(), 3 | 4 | 6 | 8) && h.bytes().all(|b| b.is_ascii_hexdigit()) {
        let s: String = if h.len() <= 4 {
            h.chars().flat_map(|c| [c, c]).collect()
        } else {
            h.to_string()
        };
        let p = |i: usize| f64::from(u8::from_str_radix(&s[i..i + 2], 16).unwrap_or(0));
        return Some(Rgba {
            r: p(0),
            g: p(2),
            b: p(4),
            a: if s.len() == 8 { p(6) / 255.0 } else { 1.0 },
        });
    }
    let (name, rest) = v.split_once('(')?;
    let body = rest.strip_suffix(')')?;
    if !matches!(name, "rgb" | "rgba" | "hsl" | "hsla") {
        return None;
    }
    let parts = args(body)?;
    if parts.len() < 3 || parts.len() > 4 {
        return None;
    }
    let a = alpha_tok(parts.get(3).map(String::as_str))?;
    if name.starts_with("rgb") {
        let mut ch = [0.0; 3];
        for (i, slot) in ch.iter_mut().enumerate() {
            let c = num(&parts[i], Some(2.55))?;
            if !(0.0..=255.0).contains(&c) {
                return None;
            }
            *slot = c;
        }
        return Some(Rgba {
            r: ch[0],
            g: ch[1],
            b: ch[2],
            a,
        });
    }
    let hue_text = parts[0].strip_suffix("deg").unwrap_or(&parts[0]);
    let h = num(hue_text, None)?;
    let s = if parts[1].ends_with('%') {
        num(&parts[1], Some(1.0))?
    } else {
        return None;
    };
    let l = if parts[2].ends_with('%') {
        num(&parts[2], Some(1.0))?
    } else {
        return None;
    };
    if !(0.0..=100.0).contains(&s) || !(0.0..=100.0).contains(&l) {
        return None;
    }
    Some(hsl_to_rgb(&Hsla {
        h: ((h % 360.0) + 360.0) % 360.0,
        s,
        l,
        a,
    }))
}

/// The one stored form: lower-case `#rrggbb` when opaque, `#rrggbbaa` otherwise, alpha in 1/255 steps.
pub fn to_stored(c: &Rgba) -> String {
    let a = js_round(clamp(c.a, 0.0, 1.0) * 255.0);
    let tail = if a == 255.0 {
        String::new()
    } else {
        hex_byte(a)
    };
    format!(
        "#{}{}{}{}",
        hex_byte(c.r),
        hex_byte(c.g),
        hex_byte(c.b),
        tail
    )
}

/// A stored colour read back: the stored form of a 6 or 8 digit hex (with the `#`), or None.
pub fn canonical_stored(v: Option<&str>) -> Option<String> {
    let v = v?;
    let h = v.strip_prefix('#')?;
    if (h.len() == 6 || h.len() == 8) && h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return parse_colour(v).map(|c| to_stored(&c));
    }
    None
}

/// Alpha as text, at most two decimals, no trailing zeros.
fn alpha_text(a: f64) -> String {
    format!("{}", js_round(clamp(a, 0.0, 1.0) * 100.0) / 100.0)
}

/// How a field SHOWS a colour in the chosen format. Display only.
pub fn format(c: &Rgba, f: ColourFormat) -> String {
    match f {
        ColourFormat::Rgba => {
            let r = |n: f64| js_round(clamp(n, 0.0, 255.0));
            format!(
                "rgba({}, {}, {}, {})",
                r(c.r),
                r(c.g),
                r(c.b),
                alpha_text(c.a)
            )
        }
        ColourFormat::Hsla => {
            let h = rgb_to_hsl(c);
            format!(
                "hsla({}, {}%, {}%, {})",
                js_round(h.h) % 360.0,
                js_round(h.s),
                js_round(h.l),
                alpha_text(c.a)
            )
        }
        ColourFormat::Hex => to_stored(c),
    }
}

fn hue_of(r: f64, g: f64, b: f64, max: f64, d: f64) -> f64 {
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        ((g - b) / d) % 6.0
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    ((h * 60.0) % 360.0 + 360.0) % 360.0
}

pub fn rgb_to_hsv(c: &Rgba) -> Hsva {
    let (r, g, b) = (c.r / 255.0, c.g / 255.0, c.b / 255.0);
    let max = r.max(g).max(b);
    let d = max - r.min(g).min(b);
    Hsva {
        h: hue_of(r, g, b, max, d),
        s: if max != 0.0 { d / max * 100.0 } else { 0.0 },
        v: max * 100.0,
        a: c.a,
    }
}

pub fn hsv_to_rgb(c: &Hsva) -> Rgba {
    let s = clamp(c.s, 0.0, 100.0) / 100.0;
    let v = clamp(c.v, 0.0, 100.0) / 100.0;
    let h = ((c.h % 360.0) + 360.0) % 360.0;
    let f = |n: f64| {
        let k = (n + h / 60.0) % 6.0;
        (v - v * s * 0.0f64.max(k.min(4.0 - k).min(1.0))) * 255.0
    };
    Rgba {
        r: f(5.0),
        g: f(3.0),
        b: f(1.0),
        a: c.a,
    }
}

pub fn rgb_to_hsl(c: &Rgba) -> Hsla {
    let (r, g, b) = (c.r / 255.0, c.g / 255.0, c.b / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    let s = if d != 0.0 {
        d / (1.0 - (2.0 * l - 1.0).abs())
    } else {
        0.0
    };
    Hsla {
        h: hue_of(r, g, b, max, d),
        s: s * 100.0,
        l: l * 100.0,
        a: c.a,
    }
}

pub fn hsl_to_rgb(c: &Hsla) -> Rgba {
    let s = clamp(c.s, 0.0, 100.0) / 100.0;
    let l = clamp(c.l, 0.0, 100.0) / 100.0;
    let h = ((c.h % 360.0) + 360.0) % 360.0;
    let a = s * l.min(1.0 - l);
    let f = |n: f64| {
        let k = (n + h / 30.0) % 12.0;
        (l - a * (-1.0f64).max((k - 3.0).min(9.0 - k).min(1.0))) * 255.0
    };
    Rgba {
        r: f(0.0),
        g: f(8.0),
        b: f(4.0),
        a: c.a,
    }
}

/// The two hex digits for an alpha, `round(a * 255)`. No clamp: that is the caller's.
pub fn alpha_hex(a: f64) -> String {
    format!("{:02x}", js_round(a * 255.0) as i64)
}

/// An alpha in whole percent, where only opaque reads 100 and only clear reads 0.
pub fn alpha_pct(a: f64) -> u32 {
    if a >= 1.0 {
        100
    } else if a <= 0.0 {
        0
    } else {
        clamp(js_round(a * 100.0), 1.0, 99.0) as u32
    }
}

/// `#rrggbb` of any parseable colour, its alpha dropped; `fallback` else.
pub fn opaque(c: &str, fallback: &str) -> String {
    match parse_colour(c) {
        Some(p) => to_stored(&Rgba { a: 1.0, ..p }),
        None => fallback.to_string(),
    }
}

/// The alpha of any parseable colour, 1 for anything unparseable.
pub fn alpha_of(c: &str) -> f64 {
    parse_colour(c).map_or(1.0, |p| p.a)
}

/// The colour at alpha `a`, stored form.
pub fn with_alpha(c: &str, a: f64) -> String {
    let p = parse_colour(c).unwrap_or(Rgba {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    });
    to_stored(&Rgba {
        a: clamp(a, 0.0, 1.0),
        ..p
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgba(r: f64, g: f64, b: f64, a: f64) -> Rgba {
        Rgba { r, g, b, a }
    }
    fn close(a: &Rgba, b: &Rgba) {
        assert!(
            (a.r - b.r).abs() < 1e-9
                && (a.g - b.g).abs() < 1e-9
                && (a.b - b.b).abs() < 1e-9
                && (a.a - b.a).abs() < 1e-9,
            "{a:?} vs {b:?}"
        );
    }
    fn p(s: &str) -> Rgba {
        parse_colour(s).unwrap_or_else(|| panic!("{s} should parse"))
    }

    #[test]
    fn parse_reads_every_hex_form_with_or_without_the_hash() {
        close(&p("#abc"), &rgba(170.0, 187.0, 204.0, 1.0));
        close(&p("abc"), &rgba(170.0, 187.0, 204.0, 1.0));
        close(
            &p("#abcd"),
            &rgba(170.0, 187.0, 204.0, f64::from(0xddu8) / 255.0),
        );
        close(&p("#AABBCC"), &rgba(170.0, 187.0, 204.0, 1.0));
        close(
            &p("aabbcc80"),
            &rgba(170.0, 187.0, 204.0, f64::from(0x80u8) / 255.0),
        );
    }

    #[test]
    fn parse_reads_rgb_and_rgba_in_comma_and_space_syntax() {
        close(&p("rgb(1, 2, 3)"), &rgba(1.0, 2.0, 3.0, 1.0));
        close(&p("rgba(1,2,3,0.5)"), &rgba(1.0, 2.0, 3.0, 0.5));
        close(&p("rgba(1, 2, 3, 50%)"), &rgba(1.0, 2.0, 3.0, 0.5));
        close(&p("rgb(1 2 3 / 0.25)"), &rgba(1.0, 2.0, 3.0, 0.25));
        close(&p("RGBA(1 2 3 / 25%)"), &rgba(1.0, 2.0, 3.0, 0.25));
    }

    #[test]
    fn parse_reads_hsl_and_hsla() {
        assert_eq!(to_stored(&p("hsl(0, 100%, 50%)")), "#ff0000");
        assert_eq!(to_stored(&p("hsl(120deg 100% 25%)")), "#008000");
        assert_eq!(to_stored(&p("hsla(240, 100%, 50%, 0.5)")), "#0000ff80");
        assert_eq!(to_stored(&p("hsl(240 100% 50% / 50%)")), "#0000ff80");
    }

    #[test]
    fn parse_rejects_what_is_not_a_colour() {
        for s in [
            "",
            "   ",
            "#12345",
            "#1234567",
            "rgb(300,0,0)",
            "rgb(1,2)",
            "rgba(1,2,3,2)",
            "hsl(200, 50, 40)",
            "hsl(200, 50%, 40)",
            "red",
            "rgb(1,2,3",
            "#ggg",
        ] {
            assert!(parse_colour(s).is_none(), "{s:?} should be rejected");
        }
    }

    #[test]
    fn to_stored_is_six_digits_when_opaque_and_eight_otherwise() {
        assert_eq!(to_stored(&rgba(170.0, 187.0, 204.0, 1.0)), "#aabbcc");
        assert_eq!(to_stored(&rgba(170.0, 187.0, 204.0, 0.5)), "#aabbcc80");
        assert_eq!(to_stored(&rgba(0.0, 0.0, 0.0, 0.0)), "#00000000");
    }

    #[test]
    fn format_shows_hex_rgba_and_hsla() {
        let c = rgba(59.0, 130.0, 246.0, 0.5);
        assert_eq!(format(&c, ColourFormat::Hex), "#3b82f680");
        assert_eq!(format(&c, ColourFormat::Rgba), "rgba(59, 130, 246, 0.5)");
        let h = format(&c, ColourFormat::Hsla);
        assert!(
            h.starts_with("hsla(217, 91%, 60%, ") && h.ends_with("0.5)"),
            "{h}"
        );
        assert_eq!(
            format(&rgba(255.0, 0.0, 0.0, 1.0), ColourFormat::Hex),
            "#ff0000"
        );
    }

    #[test]
    fn every_typed_alpha_byte_reads_back_as_itself() {
        for a in 0..=255u32 {
            let text = format!("#123456{a:02x}");
            let want = if a == 255 {
                "#123456".to_string()
            } else {
                text.clone()
            };
            assert_eq!(to_stored(&p(&text)), want);
        }
    }

    #[test]
    fn hex8_to_rgba_to_hex8_and_hsla_round_trips_are_exact() {
        for r in (0..=255).step_by(51) {
            for g in (0..=255).step_by(51) {
                for b in (0..=255).step_by(51) {
                    for a in 0..256 {
                        let hex = format!("#{r:02x}{g:02x}{b:02x}{a:02x}");
                        let want = if a == 255 {
                            hex[..7].to_string()
                        } else {
                            hex.clone()
                        };
                        let c = p(&hex);
                        assert_eq!(to_stored(&c), want, "{hex}");
                        assert_eq!(to_stored(&hsl_to_rgb(&rgb_to_hsl(&c))), want, "hsl {hex}");
                    }
                }
            }
        }
    }

    #[test]
    fn hsv_to_rgb_to_hsv_within_half_a_unit() {
        for h in (0..360).step_by(30) {
            for s in [20.0, 50.0, 100.0] {
                for v in [30.0, 60.0, 100.0] {
                    let from = Hsva {
                        h: f64::from(h),
                        s,
                        v,
                        a: 1.0,
                    };
                    let back = rgb_to_hsv(&hsv_to_rgb(&from));
                    assert!((back.h - from.h).abs() <= 0.5, "h {from:?} {back:?}");
                    assert!((back.s - from.s).abs() <= 0.5, "s {from:?} {back:?}");
                    assert!((back.v - from.v).abs() <= 0.5, "v {from:?} {back:?}");
                }
            }
        }
    }

    #[test]
    fn alpha_hex_is_round_a_255_in_two_digits_no_clamp() {
        assert_eq!(alpha_hex(1.0), "ff");
        assert_eq!(alpha_hex(0.0), "00");
        assert_eq!(alpha_hex(0.5), "80");
        assert_eq!(alpha_hex(0.3), "4d");
    }

    #[test]
    fn opaque_alpha_of_and_with_alpha() {
        assert_eq!(opaque("#aabbcc80", "#000000"), "#aabbcc");
        assert_eq!(opaque("junk", "#010203"), "#010203");
        assert!((alpha_of("#aabbcc80") - 128.0 / 255.0).abs() < 1e-9);
        assert_eq!(alpha_of("#aabbcc"), 1.0);
        assert_eq!(with_alpha("#aabbcc", 0.5), "#aabbcc80");
        assert_eq!(with_alpha("#aabbcc80", 1.0), "#aabbcc");
    }

    #[test]
    fn alpha_pct_only_opaque_reads_100_and_only_clear_reads_0() {
        assert_eq!(alpha_pct(1.0), 100);
        assert_eq!(alpha_pct(254.0 / 255.0), 99);
        assert_eq!(alpha_pct(0.995), 99);
        assert_eq!(alpha_pct(0.0), 0);
        assert_eq!(alpha_pct(1.0 / 255.0), 1);
    }

    #[test]
    fn alpha_pct_is_the_whole_percent_elsewhere() {
        assert_eq!(alpha_pct(0.5), 50);
        assert_eq!(alpha_pct(0.3), 30);
        assert_eq!(alpha_pct(128.0 / 255.0), 50);
    }

    #[test]
    fn canonical_stored_accepts_only_six_or_eight_digits() {
        assert_eq!(
            canonical_stored(Some("#E07A2F")).as_deref(),
            Some("#e07a2f")
        );
        assert_eq!(
            canonical_stored(Some("#e07a2fff")).as_deref(),
            Some("#e07a2f")
        );
        assert_eq!(
            canonical_stored(Some("#e07a2f80")).as_deref(),
            Some("#e07a2f80")
        );
        for s in ["#fff", "orange", "#1234567", "#123456789", "#12345g"] {
            assert_eq!(canonical_stored(Some(s)), None, "{s}");
        }
        assert_eq!(canonical_stored(None), None);
    }

    #[test]
    fn oracle_parse_vectors() {
        let v = crate::testutil::fixture("parse_vectors.json");
        let rows = v.as_array().expect("array");
        assert_eq!(rows.len(), 28);
        for row in rows {
            let text = row["text"].as_str().expect("text");
            let got = parse_colour(text);
            if row["parsed"].is_null() {
                assert!(got.is_none(), "{text:?} should be None");
                continue;
            }
            let c = got.unwrap_or_else(|| panic!("{text:?} should parse"));
            for (k, val) in [("r", c.r), ("g", c.g), ("b", c.b), ("a", c.a)] {
                let want = row["parsed"][k].as_f64().expect("num");
                assert!((val - want).abs() < 1e-12, "{text:?} {k}: {val} vs {want}");
            }
            assert_eq!(
                to_stored(&c),
                row["stored"].as_str().expect("s"),
                "{text:?}"
            );
            assert_eq!(
                format(&c, ColourFormat::Hex),
                row["hex"].as_str().expect("s"),
                "{text:?}"
            );
            assert_eq!(
                format(&c, ColourFormat::Rgba),
                row["rgba"].as_str().expect("s"),
                "{text:?}"
            );
            assert_eq!(
                format(&c, ColourFormat::Hsla),
                row["hsla"].as_str().expect("s"),
                "{text:?}"
            );
        }
    }
}

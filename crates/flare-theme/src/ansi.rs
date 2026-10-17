//! Luminance, contrast, mixing and the derived ANSI palette (Task 3). Source: PT `termAnsi.ts`.

use crate::ground::DEFAULT_GROUND;
use crate::num::hex_byte;

/// The sixteen ANSI keys in PT's order (the order of the `Ansi16` object's keys).
pub const ANSI_KEYS: [&str; 16] = [
    "black",
    "red",
    "green",
    "yellow",
    "blue",
    "magenta",
    "cyan",
    "white",
    "brightBlack",
    "brightRed",
    "brightGreen",
    "brightYellow",
    "brightBlue",
    "brightMagenta",
    "brightCyan",
    "brightWhite",
];

/// Indices into an `Ansi16` in `ANSI_KEYS` order.
pub mod idx {
    pub const BLACK: usize = 0;
    pub const RED: usize = 1;
    pub const GREEN: usize = 2;
    pub const YELLOW: usize = 3;
    pub const BLUE: usize = 4;
    pub const MAGENTA: usize = 5;
    pub const CYAN: usize = 6;
    pub const WHITE: usize = 7;
    pub const BRIGHT_BLACK: usize = 8;
    pub const BRIGHT_RED: usize = 9;
    pub const BRIGHT_GREEN: usize = 10;
    pub const BRIGHT_YELLOW: usize = 11;
    pub const BRIGHT_BLUE: usize = 12;
    pub const BRIGHT_MAGENTA: usize = 13;
    pub const BRIGHT_CYAN: usize = 14;
    pub const BRIGHT_WHITE: usize = 15;
}

/// Text-on-terminal floor. 3:1 is WCAG's large-text line (PT's `ANSI_CONTRAST_FLOOR`).
pub const ANSI_CONTRAST_FLOOR: f64 = 3.0;

pub type Ansi16 = [String; 16];
pub type PartialAnsi = [Option<String>; 16];

// The hue seeds: the classic palette's identities, before adaptation.
const RED: &str = "#e05561";
const GREEN: &str = "#8cc265";
const YELLOW: &str = "#d1a54b";
const BLUE: &str = "#4aa5f0";
const MAGENTA: &str = "#c162de";
const CYAN: &str = "#42b3c2";
const BRIGHT_RED: &str = "#ff616e";
const BRIGHT_GREEN: &str = "#a5e075";
const BRIGHT_YELLOW: &str = "#f0c366";
const BRIGHT_BLUE: &str = "#4dc4ff";
const BRIGHT_MAGENTA: &str = "#de73ff";
const BRIGHT_CYAN: &str = "#4cd1e0";

fn is_hex(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// `^#[0-9a-f]{6}$` (either case), hand-checked (decision D15).
fn is_plain_hex(c: &str) -> bool {
    c.starts_with('#') && c.len() == 7 && is_hex(&c[1..])
}

/// One run of decimal digits after optional whitespace: PT's `\s*(\d+)`.
fn take_int(s: &str) -> Option<(f64, &str)> {
    let s = s.trim_start();
    let end = s.bytes().take_while(|b| b.is_ascii_digit()).count();
    if end == 0 {
        return None;
    }
    let n: f64 = s[..end].parse().ok()?;
    Some((n, &s[end..]))
}

/// A comma with optional whitespace on either side: PT's `\s*,\s*`.
fn take_comma(s: &str) -> Option<&str> {
    s.trim_start().strip_prefix(',')
}

/// Any CSS colour a style can publish, flattened to opaque lower-case `#rrggbb`: `#rgb`, `#rrggbb`,
/// `#rrggbbaa` (alpha dropped), and `rgb()` / `rgba()` (the first three integers). Anything else takes
/// `fallback`, which is never NaN maths (PT `normalizeColor`).
pub fn normalize_color(input: &str, fallback: &str) -> String {
    let v = input.trim();
    if let Some(h) = v.strip_prefix('#') {
        if h.len() == 3 && is_hex(h) {
            return format!(
                "#{}",
                h.chars()
                    .flat_map(|c| [c, c])
                    .collect::<String>()
                    .to_ascii_lowercase()
            );
        }
        if (h.len() == 6 || h.len() == 8) && is_hex(h) {
            return format!("#{}", h[..6].to_ascii_lowercase());
        }
        return fallback.to_string();
    }
    let lower = v.to_ascii_lowercase();
    let rest = match lower
        .strip_prefix("rgba(")
        .or_else(|| lower.strip_prefix("rgb("))
    {
        Some(r) => r,
        None => return fallback.to_string(),
    };
    let parsed = (|| {
        let (r, rest) = take_int(rest)?;
        let rest = take_comma(rest)?;
        let (g, rest) = take_int(rest)?;
        let rest = take_comma(rest)?;
        let (b, _) = take_int(rest)?;
        Some((r, g, b))
    })();
    match parsed {
        Some((r, g, b)) => rgb_to_hex(r, g, b),
        None => fallback.to_string(),
    }
}

/// `#rrggbb` (or `rrggbb`, or `#rgb`) into three channels. Bad input gives `[0, 0, 0]`; callers
/// normalise first, so PT's NaN is never reached.
pub fn hex_to_rgb(hex: &str) -> [f64; 3] {
    let h = hex.strip_prefix('#').unwrap_or(hex);
    let full: String = if h.len() == 3 && is_hex(h) {
        h.chars().flat_map(|c| [c, c]).collect()
    } else {
        h.to_string()
    };
    if full.len() != 6 || !is_hex(&full) {
        return [0.0; 3];
    }
    let channel = |i: usize| f64::from(u8::from_str_radix(&full[i..i + 2], 16).unwrap_or(0));
    [channel(0), channel(2), channel(4)]
}

/// Channels clamped to 0..=255 and rounded (PT's `rgbToHex`), as `#rrggbb`.
pub fn rgb_to_hex(r: f64, g: f64, b: f64) -> String {
    format!("#{}{}{}", hex_byte(r), hex_byte(g), hex_byte(b))
}

/// Linear interpolation: `t` is the share of `b` (PT `mixHex`).
pub fn mix_hex(a: &str, b: &str, t: f64) -> String {
    let [ar, ag, ab] = hex_to_rgb(a);
    let [br, bg, bb] = hex_to_rgb(b);
    rgb_to_hex(ar + (br - ar) * t, ag + (bg - ag) * t, ab + (bb - ab) * t)
}

/// WCAG relative luminance.
pub fn luminance(hex: &str) -> f64 {
    let lin = |n: f64| {
        let c = n / 255.0;
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let [r, g, b] = hex_to_rgb(hex);
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

/// WCAG contrast ratio, 1 to 21.
pub fn contrast_ratio(a: &str, b: &str) -> f64 {
    let la = luminance(a);
    let lb = luminance(b);
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// Which way a colour walks to clear a floor on `ground`: white or black, whichever has more room.
pub fn toward_room(ground: &str) -> &'static str {
    if contrast_ratio("#ffffff", ground) >= contrast_ratio("#000000", ground) {
        "#ffffff"
    } else {
        "#000000"
    }
}

/// `colour` untouched when it already reads; else up to 24 steps of 0.08 toward `toward_room`; else
/// the endpoint, even when that endpoint is short of the floor (PT `ensureContrast`).
pub fn ensure_contrast(colour: &str, bg: &str, floor: f64) -> String {
    if contrast_ratio(colour, bg) >= floor {
        return colour.to_string();
    }
    let towards = toward_room(bg);
    let mut c = colour.to_string();
    for _ in 0..24 {
        if contrast_ratio(&c, bg) >= floor {
            break;
        }
        c = mix_hex(&c, towards, 0.08);
    }
    if contrast_ratio(&c, bg) >= floor {
        c
    } else {
        towards.to_string()
    }
}

/// The sixteen, adapted to `bg` and `fg`: the greys come from the background and foreground, the
/// twelve hues are the seeds pushed to the floor (PT `deriveAnsi`).
pub fn derive_ansi(raw_bg: &str, raw_fg: &str) -> Ansi16 {
    let bg = normalize_color(raw_bg, DEFAULT_GROUND);
    let fg = normalize_color(raw_fg, "#e3e6ea");
    let dark = luminance(&bg) <= 0.35;
    let adapt = |c: &str| ensure_contrast(c, &bg, ANSI_CONTRAST_FLOOR);
    [
        mix_hex(&bg, &fg, if dark { 0.18 } else { 0.25 }),
        adapt(RED),
        adapt(GREEN),
        adapt(YELLOW),
        adapt(BLUE),
        adapt(MAGENTA),
        adapt(CYAN),
        adapt(&mix_hex(&fg, &bg, 0.18)),
        adapt(&mix_hex(&bg, &fg, 0.55)),
        adapt(BRIGHT_RED),
        adapt(BRIGHT_GREEN),
        adapt(BRIGHT_YELLOW),
        adapt(BRIGHT_BLUE),
        adapt(BRIGHT_MAGENTA),
        adapt(BRIGHT_CYAN),
        adapt(&fg),
    ]
}

/// Entries that are present are walked to the floor against `bg`; a plain `#rrggbb` keeps its case
/// and its bytes when it already reads, and an absent entry stays absent (PT `legiblePalette`).
pub fn legible_palette(ansi: &PartialAnsi, bg: &str) -> PartialAnsi {
    let plain = |c: &str, fallback: &str| {
        if is_plain_hex(c) {
            c.to_string()
        } else {
            normalize_color(c, fallback)
        }
    };
    let ground = plain(bg, DEFAULT_GROUND);
    ansi.clone()
        .map(|e| e.map(|c| ensure_contrast(&plain(&c, "#888888"), &ground, ANSI_CONTRAST_FLOOR)))
}

/// `legible_palette` for a full sixteen.
pub fn legible_ansi(ansi: &Ansi16, bg: &str) -> Ansi16 {
    let partial: PartialAnsi = ansi.clone().map(Some);
    legible_palette(&partial, bg).map(|e| e.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::fixture;

    const HUES: [&str; 15] = [
        "red",
        "green",
        "yellow",
        "blue",
        "magenta",
        "cyan",
        "brightRed",
        "brightGreen",
        "brightYellow",
        "brightBlue",
        "brightMagenta",
        "brightCyan",
        "brightBlack",
        "white",
        "brightWhite",
    ];

    fn named(a: &Ansi16, key: &str) -> String {
        let i = ANSI_KEYS.iter().position(|k| *k == key).unwrap();
        a[i].clone()
    }

    #[test]
    fn red_text_on_a_red_background_clears_the_floor() {
        let a = derive_ansi("#7a1111", "#f5e9e9");
        assert!(contrast_ratio(&named(&a, "red"), "#7a1111") >= ANSI_CONTRAST_FLOOR);
        assert!(contrast_ratio(&named(&a, "brightRed"), "#7a1111") >= ANSI_CONTRAST_FLOOR);
    }

    #[test]
    fn every_hue_reads_on_each_background() {
        let cases = [
            ("#000000", "#e6e6e6"),
            ("#ffffff", "#1a1a1a"),
            ("#0d1b3a", "#dbe4f5"),
            ("#0d2416", "#dcecdf"),
            ("#c2186f", "#ffe9f4"),
            ("#808080", "#101010"),
            ("#f6f0e4", "#2a241c"),
        ];
        for (bg, fg) in cases {
            let a = derive_ansi(bg, fg);
            for h in HUES {
                let c = contrast_ratio(&named(&a, h), bg);
                assert!(c >= 2.99, "{h} on {bg} is {c}");
            }
            for h in ["white", "brightWhite"] {
                assert!(contrast_ratio(&named(&a, h), bg) >= 2.99, "{h} on {bg}");
            }
        }
    }

    #[test]
    fn holds_over_a_sweep_of_arbitrary_backgrounds() {
        // PT's generator, in f64 (its `seed * 1103515245` overflows 2^53 the same way in both).
        let mut seed = 42.0_f64;
        let mut rnd = move || {
            seed = (seed * 1103515245.0 + 12345.0) % 2147483648.0;
            seed / 2147483648.0
        };
        for _ in 0..200 {
            let mut bg = String::from("#");
            for _ in 0..3 {
                let byte = (rnd() * 256.0).floor() as u32;
                bg.push_str(&format!("{byte:02x}"));
            }
            let fg = ensure_contrast("#c8c8c8", &bg, 4.5);
            let a = derive_ansi(&bg, &fg);
            for h in HUES {
                let c = contrast_ratio(&named(&a, h), &bg);
                assert!(c >= 2.99, "{h} on {bg} is {c}");
            }
        }
    }

    #[test]
    fn on_plain_dark_the_seeds_pass_through() {
        let a = derive_ansi("#101215", "#e3e6ea");
        assert_eq!(named(&a, "red"), "#e05561");
        assert_eq!(named(&a, "blue"), "#4aa5f0");
    }

    #[test]
    fn on_a_light_background_the_hues_darken_rather_than_invert() {
        let a = derive_ansi("#ffffff", "#1a1a1a");
        let red = named(&a, "red");
        let r = u8::from_str_radix(&red[1..3], 16).unwrap();
        let g = u8::from_str_radix(&red[3..5], 16).unwrap();
        let b = u8::from_str_radix(&red[5..7], 16).unwrap();
        assert!(r > g);
        assert!(r > b);
    }

    #[test]
    fn normalize_passes_hex_expands_short_and_drops_alpha() {
        assert_eq!(normalize_color("#101215", "#000000"), "#101215");
        assert_eq!(normalize_color("#abc", "#000000"), "#aabbcc");
        assert_eq!(normalize_color("#10121580", "#000000"), "#101215");
    }

    #[test]
    fn normalize_flattens_rgb_and_rgba() {
        assert_eq!(
            normalize_color("rgba(16, 18, 21, 0.6575)", "#000000"),
            "#101215"
        );
        assert_eq!(normalize_color("rgb(255,0,0)", "#000000"), "#ff0000");
    }

    #[test]
    fn normalize_unparseable_takes_the_fallback() {
        assert_eq!(normalize_color("transparent", "#101215"), "#101215");
        assert_eq!(normalize_color("", "#101215"), "#101215");
    }

    #[test]
    fn an_rgba_background_does_not_blacken_the_palette() {
        let a = derive_ansi("rgba(16, 18, 21, 0.6575)", "#e8eaf0");
        assert_ne!(named(&a, "red"), "#000000");
        assert_ne!(named(&a, "green"), "#000000");
        for h in ["red", "green", "yellow", "blue", "magenta", "cyan"] {
            assert!(contrast_ratio(&named(&a, h), "#101215") >= 2.99, "{h}");
        }
    }

    #[test]
    fn ensure_contrast_returns_the_input_when_it_reads_and_steps_toward_room_otherwise() {
        assert_eq!(ensure_contrast("#ffffff", "#000000", 3.0), "#ffffff");
        assert!(contrast_ratio(&ensure_contrast("#222222", "#111111", 3.0), "#111111") >= 3.0);
        assert_eq!(
            ensure_contrast("#777777", "#777777", 21.0),
            toward_room("#777777")
        );
    }

    #[test]
    fn luminance_and_ratio_are_wcag() {
        assert!((luminance("#ffffff") - 1.0).abs() < 1e-12);
        assert!(luminance("#000000").abs() < 1e-12);
        assert!((contrast_ratio("#ffffff", "#000000") - 21.0).abs() < 1e-12);
    }

    #[test]
    fn legible_palette_keeps_case_and_moves_only_the_failing() {
        let mut p: PartialAnsi = Default::default();
        p[0] = Some("#FFFFFF".to_string());
        p[1] = Some("#101215".to_string());
        let out = legible_palette(&p, "#ffffff");
        assert_ne!(out[0].as_deref(), Some("#FFFFFF"));
        assert!(contrast_ratio(out[0].as_deref().unwrap(), "#ffffff") >= 3.0);
        assert_eq!(out[1].as_deref(), Some("#101215"));
        assert_eq!(out[2], None);
    }

    #[test]
    fn oracle_ansi_vectors() {
        let vectors = fixture("ansi_vectors.json");
        let vectors = vectors.as_array().unwrap();
        assert_eq!(vectors.len(), 200);
        for v in vectors {
            let bg = v["bg"].as_str().unwrap();
            let fg = v["fg"].as_str().unwrap();
            let got = derive_ansi(bg, fg);
            for (i, key) in ANSI_KEYS.iter().enumerate() {
                assert_eq!(
                    got[i],
                    v["ansi"][key].as_str().unwrap(),
                    "{key} for bg {bg} fg {fg}"
                );
            }
        }
    }
}

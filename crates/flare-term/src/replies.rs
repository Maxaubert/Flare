//! What Flare answers a program that asks (Prism Terminal's termReplies.ts). Rule 10: every
//! byte here answers a query in the pty's own stream, or is the `?997` report a program
//! switched on. None holds CR, LF or BEL, so a shell can never act on it.

/// The look of the ground: a program asking for it is told dark or light.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroundMode {
    Dark,
    Light,
}

/// The least time between two bells that reach the host, in milliseconds.
pub const BELL_GAP_MS: u64 = 1000;

const ESC: char = '\x1b';

/// String terminator (ESC \).
const ST: &str = "\x1b\\";

/// The fallback ground when a colour does not parse (Prism's own dark ground).
const FALLBACK_HEX: &str = "#0b0b0f";

/// The colours the replies report.
pub struct Ground<'a> {
    /// Text colour, `#rrggbb`.
    pub fg: &'a str,
    /// Ground colour, `#rrggbb`.
    pub bg: &'a str,
}

/// The opaque RGB of a `#rgb`, `#rrggbb` or `#rrggbbaa` colour; `None` for anything else.
fn parse_rgb(hex: &str) -> Option<(u8, u8, u8)> {
    let h = hex.strip_prefix('#')?;
    let byte = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
    let nib = |i: usize| {
        u8::from_str_radix(h.get(i..i + 1)?, 16)
            .ok()
            .map(|n| n * 17)
    };
    match h.len() {
        3 => Some((nib(0)?, nib(1)?, nib(2)?)),
        6 | 8 => Some((byte(0)?, byte(2)?, byte(4)?)),
        _ => None,
    }
}

/// `#rrggbb` (any alpha dropped) as `rgb:rrrr/gggg/bbbb`, each byte doubled: xterm's own
/// reply form. A colour that does not parse is taken as black.
pub fn osc_colour(hex: &str) -> String {
    let (r, g, b) = parse_rgb(hex).unwrap_or((0, 0, 0));
    format!("rgb:{r:02x}{r:02x}/{g:02x}{g:02x}/{b:02x}{b:02x}")
}

/// The replies to an OSC 10 (text) or 11 (ground) whose every item is `?`. The OSC takes a
/// list: `10;?;?` asks for 10 then 11. `None` for a set, an empty one, or a list running
/// past 11 (the cursor and beyond), which the emulator answers by itself.
pub fn colour_query_replies(ident: u8, data: &str, g: &Ground) -> Option<Vec<String>> {
    let items: Vec<&str> = data.split(';').collect();
    if items.iter().any(|i| *i != "?") {
        return None;
    }
    if usize::from(ident) + items.len() - 1 > 11 {
        return None;
    }
    Some(
        items
            .iter()
            .enumerate()
            .map(|(i, _)| {
                let n = ident + i as u8;
                let colour = if n == 10 { g.fg } else { g.bg };
                format!("{ESC}]{n};{}{ST}", osc_colour(colour))
            })
            .collect(),
    )
}

/// WCAG relative luminance of an opaque colour component list.
fn luminance(rgb: (u8, u8, u8)) -> f64 {
    let lin = |c: u8| {
        let c = f64::from(c) / 255.0;
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(rgb.0) + 0.7152 * lin(rgb.1) + 0.0722 * lin(rgb.2)
}

/// Light or dark, measured from the ground's colour (alpha ignored). A ground above 0.4
/// luminance is light.
pub fn ground_mode(bg: &str) -> GroundMode {
    let rgb = parse_rgb(bg)
        .or_else(|| parse_rgb(FALLBACK_HEX))
        .unwrap_or((0, 0, 0));
    if luminance(rgb) > 0.4 {
        GroundMode::Light
    } else {
        GroundMode::Dark
    }
}

/// The answer to `CSI ? 996 n`, and the report mode 2031 pushes.
pub fn dsr_theme_reply(mode: GroundMode) -> String {
    match mode {
        GroundMode::Dark => format!("{ESC}[?997;1n"),
        GroundMode::Light => format!("{ESC}[?997;2n"),
    }
}

/// DECRQM's answer for a private mode: 1 set, 2 reset.
pub fn decrqm_reply(mode: u32, set: bool) -> String {
    format!("{ESC}[?{mode};{}$y", if set { 1 } else { 2 })
}

/// One CSI parameter as the parser hands it: a number, or a sub-parameter list.
pub enum Param {
    One(u32),
    Sub(Vec<u32>),
}

/// Flattens CSI parameters, sub-parameters in place.
pub fn mode_params(params: &[Param]) -> Vec<u32> {
    params
        .iter()
        .flat_map(|p| match p {
            Param::One(n) => vec![*n],
            Param::Sub(v) => v.clone(),
        })
        .collect()
}

/// XTVERSION's answer: `DCS > | name ST`. The name must not start with "xterm.js": Claude
/// reads that as VS Code's terminal and applies its scroll tuning.
pub fn xtversion_reply(flare: &str, alacritty: &str) -> String {
    format!("{ESC}P>|Flare {flare} (alacritty_terminal {alacritty}){ST}")
}

/// `CSI > q` and `CSI > 0 q` ask for the version; any other parameter is not XTVERSION.
pub fn xtversion_asked(params: &[u32]) -> bool {
    params.is_empty() || params == [0]
}

/// Whether a bell at `now` gets through, given the last one that did (`None`: none yet).
pub fn bell_gate(last: Option<u64>, now: u64) -> bool {
    match last {
        None => true,
        Some(prev) => now.saturating_sub(prev) >= BELL_GAP_MS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const COLOURS: Ground = Ground {
        fg: "#3d2a1f",
        bg: "#e6d8c0",
    };

    #[test]
    fn osc_colour_doubles_each_byte() {
        assert_eq!(osc_colour("#f5f5f0"), "rgb:f5f5/f5f5/f0f0");
    }

    #[test]
    fn osc_colour_drops_alpha() {
        assert_eq!(osc_colour("#f5f5f0cc"), "rgb:f5f5/f5f5/f0f0");
    }

    #[test]
    fn colour_query_one_query() {
        assert_eq!(
            colour_query_replies(11, "?", &COLOURS),
            Some(vec!["\x1b]11;rgb:e6e6/d8d8/c0c0\x1b\\".to_string()])
        );
        assert_eq!(
            colour_query_replies(10, "?", &COLOURS),
            Some(vec!["\x1b]10;rgb:3d3d/2a2a/1f1f\x1b\\".to_string()])
        );
    }

    #[test]
    fn colour_query_list_in_order() {
        assert_eq!(
            colour_query_replies(10, "?;?", &COLOURS),
            Some(vec![
                "\x1b]10;rgb:3d3d/2a2a/1f1f\x1b\\".to_string(),
                "\x1b]11;rgb:e6e6/d8d8/c0c0\x1b\\".to_string(),
            ])
        );
    }

    #[test]
    fn colour_query_leaves_sets_alone() {
        for (ident, data) in [
            (11, "rgb:00/00/00"),
            (10, "?;#000000"),
            (11, "?;?"),
            (11, ""),
        ] {
            assert_eq!(colour_query_replies(ident, data, &COLOURS), None, "{data}");
        }
    }

    #[test]
    fn colour_query_replies_hold_no_control_bytes() {
        let replies = colour_query_replies(10, "?;?", &COLOURS).unwrap_or_default();
        for r in replies {
            assert!(!r.contains(['\r', '\n', '\x07']), "{r:?}");
        }
    }

    #[test]
    fn ground_mode_light_grounds() {
        assert_eq!(ground_mode("#e6d8c0"), GroundMode::Light);
        assert_eq!(ground_mode("#f6f4ee"), GroundMode::Light);
    }

    #[test]
    fn ground_mode_dark_grounds() {
        assert_eq!(ground_mode("#171717"), GroundMode::Dark);
        assert_eq!(ground_mode("#0b0b0f"), GroundMode::Dark);
    }

    #[test]
    fn ground_mode_ignores_alpha() {
        assert_eq!(ground_mode("#e6d8c040"), GroundMode::Light);
    }

    #[test]
    fn dsr_theme_reply_values() {
        assert_eq!(dsr_theme_reply(GroundMode::Dark), "\x1b[?997;1n");
        assert_eq!(dsr_theme_reply(GroundMode::Light), "\x1b[?997;2n");
    }

    #[test]
    fn decrqm_reply_values() {
        assert_eq!(decrqm_reply(2031, true), "\x1b[?2031;1$y");
        assert_eq!(decrqm_reply(2031, false), "\x1b[?2031;2$y");
    }

    #[test]
    fn mode_params_flattens() {
        let params = [Param::One(1004), Param::Sub(vec![2031, 1]), Param::One(25)];
        assert_eq!(mode_params(&params), vec![1004, 2031, 1, 25]);
    }

    #[test]
    fn xtversion_reply_shape() {
        assert_eq!(
            xtversion_reply("0.1.0", "0.26.0"),
            "\x1bP>|Flare 0.1.0 (alacritty_terminal 0.26.0)\x1b\\"
        );
    }

    #[test]
    fn xtversion_name_is_not_xterm_js() {
        let reply = xtversion_reply("0.1.0", "0.26.0");
        let name = reply.strip_prefix("\x1bP>|").unwrap_or_default();
        assert!(!name.starts_with("xterm.js"));
    }

    #[test]
    fn xtversion_reply_holds_no_control_bytes() {
        let reply = xtversion_reply("0.1.0", "0.26.0");
        assert!(!reply.contains(['\r', '\n', '\x07']));
    }

    #[test]
    fn xtversion_asked_only_for_none_or_zero() {
        assert!(xtversion_asked(&[]));
        assert!(xtversion_asked(&[0]));
        assert!(!xtversion_asked(&[1]));
        assert!(!xtversion_asked(&[0, 0]));
    }

    #[test]
    fn bell_gate_first_bell() {
        assert!(bell_gate(None, 5000));
    }

    #[test]
    fn bell_gate_drops_a_quick_second_bell() {
        assert!(!bell_gate(Some(5000), 5300));
        assert!(bell_gate(Some(5000), 6100));
    }

    #[test]
    fn bell_gap_is_one_second() {
        assert_eq!(BELL_GAP_MS, 1000);
        assert!(!bell_gate(Some(0), 999));
        assert!(bell_gate(Some(0), 1000));
    }
}

//! Colours: RGBA values, the terminal colour model and the pt-default palette.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// Parses `#rrggbb` or `#rrggbbaa`.
    pub fn from_hex(s: &str) -> Option<Self> {
        let hex = s.strip_prefix('#')?;
        if hex.len() != 6 && hex.len() != 8 {
            return None;
        }
        let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
        Some(Self {
            r: byte(0)?,
            g: byte(2)?,
            b: byte(4)?,
            a: if hex.len() == 8 { byte(6)? } else { 255 },
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Color {
    #[default]
    DefaultFg,
    DefaultBg,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Palette {
    pub fg: Rgba,
    pub bg: Rgba,
    pub cursor: Rgba,
    pub cursor_accent: Rgba,
    pub ansi: [Rgba; 16],
}

impl Palette {
    /// Prism Terminal's `pt-default` preset (termTheme.ts).
    pub fn pt_default() -> Self {
        let bg = Rgba::rgb(0x17, 0x17, 0x17);
        let fg = Rgba::rgb(0xde, 0xda, 0xcf);
        let hex = |s: &str| Rgba::from_hex(s).expect("valid literal");
        Self {
            fg,
            bg,
            cursor: hex("#bbbbbb"),
            cursor_accent: bg,
            ansi: [
                hex("#646464"),
                hex("#ff615a"),
                hex("#b1e969"),
                hex("#ebd99c"),
                hex("#5da9f6"),
                hex("#e86aff"),
                hex("#82fff7"),
                hex("#dedacf"),
                hex("#6b6b6b"),
                hex("#f58c80"),
                hex("#ddf88f"),
                hex("#eee5b2"),
                hex("#a5c7ff"),
                hex("#ddaaff"),
                hex("#b7fff9"),
                hex("#ffffff"),
            ],
        }
    }
}

/// The xterm 256-colour palette: 0..15 from the theme, 16..231 a 6x6x6 cube, 232..255 greys.
pub fn indexed(p: &Palette, i: u8) -> Rgba {
    const LEVELS: [u8; 6] = [0x00, 0x5f, 0x87, 0xaf, 0xd7, 0xff];
    match i {
        0..=15 => p.ansi[i as usize],
        16..=231 => {
            let n = i - 16;
            Rgba::rgb(
                LEVELS[(n / 36) as usize],
                LEVELS[((n / 6) % 6) as usize],
                LEVELS[(n % 6) as usize],
            )
        }
        232..=255 => {
            let g = 8 + 10 * (i - 232);
            Rgba::rgb(g, g, g)
        }
    }
}

/// Foreground colour; bold brightens the low eight indices (xterm drawBoldTextInBrightColors).
pub fn resolve_fg(p: &Palette, c: Color, bold: bool) -> Rgba {
    match c {
        Color::DefaultFg => p.fg,
        Color::DefaultBg => p.bg,
        Color::Indexed(i) if bold && i < 8 => p.ansi[i as usize + 8],
        Color::Indexed(i) => indexed(p, i),
        Color::Rgb(r, g, b) => Rgba::rgb(r, g, b),
    }
}

/// Background colour; the bold rule does not apply.
pub fn resolve_bg(p: &Palette, c: Color) -> Rgba {
    match c {
        Color::DefaultFg => p.fg,
        Color::DefaultBg => p.bg,
        Color::Indexed(i) => indexed(p, i),
        Color::Rgb(r, g, b) => Rgba::rgb(r, g, b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_parses_rgb_and_rgba() {
        assert_eq!(Rgba::from_hex("#171717"), Some(Rgba::rgb(0x17, 0x17, 0x17)));
        assert_eq!(Rgba::from_hex("#ffffff1c").unwrap().a, 0x1c);
        assert_eq!(Rgba::from_hex("171717"), None);
        assert_eq!(Rgba::from_hex("#12"), None);
    }

    #[test]
    fn pt_default_matches_prism_terminal() {
        let p = Palette::pt_default();
        assert_eq!(p.bg, Rgba::from_hex("#171717").unwrap());
        assert_eq!(p.fg, Rgba::from_hex("#dedacf").unwrap());
        assert_eq!(p.ansi[1], Rgba::from_hex("#ff615a").unwrap());
        assert_eq!(p.ansi[15], Rgba::from_hex("#ffffff").unwrap());
        assert_eq!(p.cursor_accent, p.bg);
    }

    #[test]
    fn cube_corners() {
        let p = Palette::pt_default();
        assert_eq!(indexed(&p, 16), Rgba::rgb(0, 0, 0));
        assert_eq!(indexed(&p, 21), Rgba::rgb(0, 0, 255));
        assert_eq!(indexed(&p, 196), Rgba::rgb(255, 0, 0));
        assert_eq!(indexed(&p, 231), Rgba::rgb(255, 255, 255));
        assert_eq!(indexed(&p, 67), Rgba::rgb(0x5f, 0x87, 0xaf));
    }

    #[test]
    fn grey_ramp() {
        let p = Palette::pt_default();
        assert_eq!(indexed(&p, 232), Rgba::rgb(8, 8, 8));
        assert_eq!(indexed(&p, 255), Rgba::rgb(238, 238, 238));
    }

    #[test]
    fn bold_brightens_low_eight_only() {
        let p = Palette::pt_default();
        assert_eq!(resolve_fg(&p, Color::Indexed(1), true), p.ansi[9]);
        assert_eq!(resolve_fg(&p, Color::Indexed(9), true), p.ansi[9]);
        assert_eq!(resolve_fg(&p, Color::Indexed(100), true), indexed(&p, 100));
        assert_eq!(resolve_fg(&p, Color::Indexed(1), false), p.ansi[1]);
    }

    #[test]
    fn truecolour_passes_through() {
        let p = Palette::pt_default();
        assert_eq!(
            resolve_fg(&p, Color::Rgb(1, 2, 3), true),
            Rgba::rgb(1, 2, 3)
        );
    }

    #[test]
    fn defaults() {
        let p = Palette::pt_default();
        assert_eq!(resolve_fg(&p, Color::DefaultFg, false), p.fg);
        assert_eq!(resolve_bg(&p, Color::DefaultBg), p.bg);
        assert_eq!(resolve_bg(&p, Color::DefaultFg), p.fg);
    }
}

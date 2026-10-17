//! The tab agent icon: which icon a tab wears and its artwork (Task 13). Ported from Prism
//! Terminal's `agentIcon.ts`. The artwork is data; the drawing is Slint's (Task 24).

use crate::kinds::DetectedAgent;

/// Which icon a tab wears.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconKind {
    Claude,
    Codex,
    Shell,
    Settings,
}

/// The agent in its shell, else the shell's own icon; Settings wins over all.
pub fn icon_kind(agent: Option<DetectedAgent>, settings: bool) -> IconKind {
    if settings {
        return IconKind::Settings;
    }
    match agent {
        Some(DetectedAgent::Claude) => IconKind::Claude,
        Some(DetectedAgent::Codex) => IconKind::Codex,
        Some(DetectedAgent::Other) | None => IconKind::Shell,
    }
}

/// Clawd's colour, as Claude Code draws it in the terminal.
pub const CLAWD_ORANGE: &str = "#d77757";

/// Clawd's view box: x, y, width, height (in pixel units of the 18 x 6 grid, doubled).
pub const CLAWD_VIEWBOX: (f32, f32, f32, f32) = (0.5, -2.5, 17.0, 17.0);

/// Clawd's pixels, [x, y] on an 18 x 6 grid, one quadrant of a character cell each. The same
/// order as Prism Terminal's `CLAWD_PIXELS`.
pub fn clawd_pixels() -> Vec<(u8, u8)> {
    let mut px: Vec<(u8, u8)> = vec![
        // The head's top row, the left eye's quarter and the right eye.
        (3, 0),
        (3, 1),
        (4, 0),
        (5, 0),
        (4, 1),
        (12, 0),
        (13, 0),
        (13, 1),
        (14, 0),
        (14, 1),
        // The arms.
        (1, 2),
        (2, 2),
        (3, 2),
        (3, 3),
        (14, 2),
        (15, 2),
        (14, 3),
        (16, 2),
        // The legs.
        (4, 4),
        (6, 4),
        (11, 4),
        (13, 4),
    ];
    for x in 6..=11 {
        px.push((x, 0));
        px.push((x, 1));
    }
    for x in 4..=13 {
        px.push((x, 2));
        px.push((x, 3));
    }
    px
}

/// Clawd as an SVG path: each pixel is 1.02 wide and 2.04 tall (a terminal cell is twice as
/// tall as it is wide, so Clawd is not squat).
pub fn clawd_path() -> String {
    clawd_pixels()
        .iter()
        .map(|&(x, y)| format!("M{x} {} h1.02 v2.04 h-1.02 z", u32::from(y) * 2))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The OpenAI mark (Simple Icons 15, CC0 path data), copied from Prism Terminal's `agentIcon.ts`.
pub const OPENAI_PATH: &str = "M22.2819 9.8211a5.9847 5.9847 0 0 0-.5157-4.9108 6.0462 6.0462 0 0 0-6.5098-2.9A6.0651 6.0651 0 0 0 4.9807 4.1818a5.9847 5.9847 0 0 0-3.9977 2.9 6.0462 6.0462 0 0 0 .7427 7.0966 5.98 5.98 0 0 0 .511 4.9107 6.051 6.051 0 0 0 6.5146 2.9001A5.9847 5.9847 0 0 0 13.2599 24a6.0557 6.0557 0 0 0 5.7718-4.2058 5.9894 5.9894 0 0 0 3.9977-2.9001 6.0557 6.0557 0 0 0-.7475-7.0729zm-9.022 12.6081a4.4755 4.4755 0 0 1-2.8764-1.0408l.1419-.0804 4.7783-2.7582a.7948.7948 0 0 0 .3927-.6813v-6.7369l2.02 1.1686a.071.071 0 0 1 .038.052v5.5826a4.504 4.504 0 0 1-4.4945 4.4944zm-9.6607-4.1254a4.4708 4.4708 0 0 1-.5346-3.0137l.142.0852 4.783 2.7582a.7712.7712 0 0 0 .7806 0l5.8428-3.3685v2.3324a.0804.0804 0 0 1-.0332.0615L9.74 19.9502a4.4992 4.4992 0 0 1-6.1408-1.6464zM2.3408 7.8956a4.485 4.485 0 0 1 2.3655-1.9728V11.6a.7664.7664 0 0 0 .3879.6765l5.8144 3.3543-2.0201 1.1685a.0757.0757 0 0 1-.071 0l-4.8303-2.7865A4.504 4.504 0 0 1 2.3408 7.872zm16.5963 3.8558L13.1038 8.364 15.1192 7.2a.0757.0757 0 0 1 .071 0l4.8303 2.7913a4.4944 4.4944 0 0 1-.6765 8.1042v-5.6772a.79.79 0 0 0-.407-.667zm2.0107-3.0231l-.142-.0852-4.7735-2.7818a.7759.7759 0 0 0-.7854 0L9.409 9.2297V6.8974a.0662.0662 0 0 1 .0284-.0615l4.8303-2.7866a4.4992 4.4992 0 0 1 6.6802 4.66zM8.3065 12.863l-2.02-1.1638a.0804.0804 0 0 1-.038-.0567V6.0742a4.4992 4.4992 0 0 1 7.3757-3.4537l-.142.0805L8.704 5.459a.7948.7948 0 0 0-.3927.6813zm1.0976-2.3654l2.602-1.4998 2.6069 1.4998v2.9994l-2.5974 1.4997-2.6067-1.4997Z";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_icon_is_the_agent_in_its_shell_the_prompt_for_any_other_the_cog_for_settings() {
        assert_eq!(
            icon_kind(Some(DetectedAgent::Claude), false),
            IconKind::Claude
        );
        assert_eq!(
            icon_kind(Some(DetectedAgent::Codex), false),
            IconKind::Codex
        );
        assert_eq!(
            icon_kind(Some(DetectedAgent::Other), false),
            IconKind::Shell
        );
        assert_eq!(icon_kind(None, false), IconKind::Shell);
        assert_eq!(
            icon_kind(Some(DetectedAgent::Claude), true),
            IconKind::Settings
        );
        assert_eq!(icon_kind(None, true), IconKind::Settings);
    }

    #[test]
    fn clawd_is_claude_codes_own_block_art() {
        // Each block character is a 2 x 2 cell of quadrants: TL, TR, BL, BR.
        let quadrants = |ch: char| -> [bool; 4] {
            match ch {
                ' ' => [false, false, false, false],
                '█' => [true, true, true, true],
                '▐' => [false, true, false, true],
                '▌' => [true, false, true, false],
                '▛' => [true, true, true, false],
                '▜' => [true, true, false, true],
                '▝' => [false, true, false, false],
                '▘' => [true, false, false, false],
                other => panic!("unexpected block character {other:?}"),
            }
        };
        let art = [" ▐▛███▜▌", "▝▜█████▛▘", "  ▘▘ ▝▝"];
        let mut want = std::collections::BTreeSet::new();
        for (row, line) in art.iter().enumerate() {
            for (col, ch) in line.chars().enumerate() {
                let [tl, tr, bl, br] = quadrants(ch);
                let (x, y) = (col * 2, row * 2);
                if tl {
                    want.insert((x, y));
                }
                if tr {
                    want.insert((x + 1, y));
                }
                if bl {
                    want.insert((x, y + 1));
                }
                if br {
                    want.insert((x + 1, y + 1));
                }
            }
        }
        let got: std::collections::BTreeSet<(usize, usize)> = clawd_pixels()
            .iter()
            .map(|&(x, y)| (usize::from(x), usize::from(y)))
            .collect();
        assert_eq!(got, want);
        assert_eq!(clawd_pixels().len(), 54);
        assert_eq!(got.len(), 54, "no duplicates");
        assert_eq!(CLAWD_ORANGE, "#d77757");
    }

    #[test]
    fn the_glyph_data_is_what_prism_terminal_ships() {
        assert_eq!(OPENAI_PATH.len(), 1460);
        assert!(OPENAI_PATH.starts_with("M22.2819 9.8211a5.98"));
        assert!(OPENAI_PATH.ends_with(".4997-2.6067-1.4997Z"));
        assert_eq!(clawd_path().matches('M').count(), 54);
        assert!(clawd_path().starts_with("M3 0 h1.02 v2.04 h-1.02 z"));
    }
}

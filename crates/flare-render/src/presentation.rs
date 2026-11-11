//! Emoji or text presentation for a cluster (UTS #51), the rule Terminal.app, kitty and Ghostty
//! follow: a character whose default presentation is text (U+23FA, U+2764, U+2122 and the like)
//! is drawn from a text face, and from a colour emoji face only with VS16 (U+FE0F) after it.

/// VS15: asks for text presentation.
const VS15: char = '\u{FE0E}';
/// VS16: asks for emoji presentation.
const VS16: char = '\u{FE0F}';
/// COMBINING ENCLOSING KEYCAP: a keycap sequence is an emoji even without VS16.
const KEYCAP: char = '\u{20E3}';

/// Whether `cluster` should be drawn as a colour emoji. VS15 forces text and VS16 forces emoji;
/// otherwise the first character's Emoji_Presentation property decides. Characters that are not
/// emoji at all have it false, so they keep text.
pub fn wants_emoji(cluster: &str) -> bool {
    if cluster.contains(VS15) {
        return false;
    }
    if cluster.contains(VS16) || cluster.contains(KEYCAP) {
        return true;
    }
    cluster.chars().next().is_some_and(is_emoji_presentation)
}

/// The Emoji_Presentation property: a character drawn as an emoji by default.
pub fn is_emoji_presentation(c: char) -> bool {
    let c = c as u32;
    EMOJI_PRESENTATION
        .binary_search_by(|&(lo, hi)| {
            if hi < c {
                std::cmp::Ordering::Less
            } else if lo > c {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

/// Emoji_Presentation=Yes, as merged inclusive ranges. Generated from Unicode 16.0's
/// `emoji-data.txt` (https://www.unicode.org/Public/16.0.0/ucd/emoji/emoji-data.txt, dated
/// 2024-05-01): every `Emoji_Presentation` line, sorted, adjacent ranges joined. No crate in the
/// tree exposes the property (swash has only Emoji and Extended_Pictographic).
const EMOJI_PRESENTATION: &[(u32, u32)] = &[
    (0x231A, 0x231B),
    (0x23E9, 0x23EC),
    (0x23F0, 0x23F0),
    (0x23F3, 0x23F3),
    (0x25FD, 0x25FE),
    (0x2614, 0x2615),
    (0x2648, 0x2653),
    (0x267F, 0x267F),
    (0x2693, 0x2693),
    (0x26A1, 0x26A1),
    (0x26AA, 0x26AB),
    (0x26BD, 0x26BE),
    (0x26C4, 0x26C5),
    (0x26CE, 0x26CE),
    (0x26D4, 0x26D4),
    (0x26EA, 0x26EA),
    (0x26F2, 0x26F3),
    (0x26F5, 0x26F5),
    (0x26FA, 0x26FA),
    (0x26FD, 0x26FD),
    (0x2705, 0x2705),
    (0x270A, 0x270B),
    (0x2728, 0x2728),
    (0x274C, 0x274C),
    (0x274E, 0x274E),
    (0x2753, 0x2755),
    (0x2757, 0x2757),
    (0x2795, 0x2797),
    (0x27B0, 0x27B0),
    (0x27BF, 0x27BF),
    (0x2B1B, 0x2B1C),
    (0x2B50, 0x2B50),
    (0x2B55, 0x2B55),
    (0x1F004, 0x1F004),
    (0x1F0CF, 0x1F0CF),
    (0x1F18E, 0x1F18E),
    (0x1F191, 0x1F19A),
    (0x1F1E6, 0x1F1FF),
    (0x1F201, 0x1F201),
    (0x1F21A, 0x1F21A),
    (0x1F22F, 0x1F22F),
    (0x1F232, 0x1F236),
    (0x1F238, 0x1F23A),
    (0x1F250, 0x1F251),
    (0x1F300, 0x1F320),
    (0x1F32D, 0x1F335),
    (0x1F337, 0x1F37C),
    (0x1F37E, 0x1F393),
    (0x1F3A0, 0x1F3CA),
    (0x1F3CF, 0x1F3D3),
    (0x1F3E0, 0x1F3F0),
    (0x1F3F4, 0x1F3F4),
    (0x1F3F8, 0x1F43E),
    (0x1F440, 0x1F440),
    (0x1F442, 0x1F4FC),
    (0x1F4FF, 0x1F53D),
    (0x1F54B, 0x1F54E),
    (0x1F550, 0x1F567),
    (0x1F57A, 0x1F57A),
    (0x1F595, 0x1F596),
    (0x1F5A4, 0x1F5A4),
    (0x1F5FB, 0x1F64F),
    (0x1F680, 0x1F6C5),
    (0x1F6CC, 0x1F6CC),
    (0x1F6D0, 0x1F6D2),
    (0x1F6D5, 0x1F6D7),
    (0x1F6DC, 0x1F6DF),
    (0x1F6EB, 0x1F6EC),
    (0x1F6F4, 0x1F6FC),
    (0x1F7E0, 0x1F7EB),
    (0x1F7F0, 0x1F7F0),
    (0x1F90C, 0x1F93A),
    (0x1F93C, 0x1F945),
    (0x1F947, 0x1F9FF),
    (0x1FA70, 0x1FA7C),
    (0x1FA80, 0x1FA89),
    (0x1FA8F, 0x1FAC6),
    (0x1FACE, 0x1FADC),
    (0x1FADF, 0x1FAE9),
    (0x1FAF0, 0x1FAF8),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_presentation_rule_follows_uts_51() {
        let cases: &[(&str, bool)] = &[
            ("\u{23FA}", false),
            ("\u{23FA}\u{FE0F}", true),
            ("\u{1F600}", true),
            ("\u{2764}", false),
            ("\u{2764}\u{FE0F}", true),
            ("\u{2B50}", true),
            ("\u{2B50}\u{FE0E}", false),
            ("\u{25B6}", false),
            ("\u{2600}", false),
            ("\u{2122}", false),
            ("\u{A9}", false),
            ("\u{2194}", false),
            ("1\u{FE0F}\u{20E3}", true),
            ("\u{1F1F3}\u{1F1F4}", true),
            ("a", false),
            ("", false),
        ];
        for &(text, emoji) in cases {
            assert_eq!(wants_emoji(text), emoji, "{text:?}");
        }
    }

    #[test]
    fn the_table_is_sorted_and_disjoint() {
        for w in EMOJI_PRESENTATION.windows(2) {
            assert!(w[0].0 <= w[0].1 && w[0].1 + 1 < w[1].0, "{w:?}");
        }
    }
}

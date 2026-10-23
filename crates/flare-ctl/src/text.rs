//! Words that come from outside (a web page, an answer over a pipe) made safe to hand to a model.
//! One copy, used by the app side (`flare-surfaces`) and the agent tools (`flare-mcp`), so the two
//! layers of cleaning can never differ.

/// True for characters that print nothing (or nothing a person can read) yet can carry words a model
/// reads: zero-width and joiner characters, direction marks, overrides and isolates, the byte-order
/// mark, variation selectors, the Unicode tag characters (U+E0000..U+E007F, the "ASCII smuggling"
/// way to hide instructions), the soft hyphen, fillers, other default-ignorable code points, private
/// use and noncharacters.
pub fn invisible(c: char) -> bool {
    let u = u32::from(c);
    matches!(
        u,
        0x00AD
            | 0x034F
            | 0x061C
            | 0x115F..=0x1160
            | 0x17B4..=0x17B5
            | 0x180B..=0x180F
            | 0x200B..=0x200F
            | 0x202A..=0x202E
            | 0x2060..=0x206F
            | 0x3164
            | 0xE000..=0xF8FF // private use
            | 0xFDD0..=0xFDEF // noncharacters
            | 0xFE00..=0xFE0F
            | 0xFEFF
            | 0xFFA0
            | 0xFFF0..=0xFFFB
            | 0x1BCA0..=0x1BCA3
            | 0x1D173..=0x1D17A
            | 0xE0000..=0xE0FFF // tags, variation selectors supplement
            | 0xF0000..=0x10FFFF // private use planes
    ) || (u & 0xFFFE) == 0xFFFE
}

/// Text made safe to hand on: one line, no control or invisible characters, runs of white space
/// (line and paragraph separators included) collapsed, at most `max` characters (a cut ends in `...`).
pub fn clean_text(s: &str, max: usize) -> String {
    let mut out = String::new();
    let mut count = 0usize;
    let mut gap = false;
    let mut cut = false;
    for c in s.chars() {
        if invisible(c) {
            continue;
        }
        if c.is_whitespace() || c.is_control() {
            gap = !out.is_empty();
            continue;
        }
        let need = usize::from(gap) + 1;
        if count + need > max {
            cut = true;
            break;
        }
        if gap {
            out.push(' ');
            count += 1;
            gap = false;
        }
        out.push(c);
        count += 1;
    }
    if cut {
        out.push_str("...");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_characters_of_every_kind_are_dropped() {
        // Tag characters spelling "ignore", variation selectors, zero-width, bidi, fillers, private use.
        let tags: String = "ignore"
            .chars()
            .map(|c| char::from_u32(0xE0000 + u32::from(c)).unwrap())
            .collect();
        let s = format!(
            "a{tags}b\u{FE0F}c\u{E0100}d\u{180E}e\u{034F}f\u{3164}g\u{115F}h\u{202E}i\u{200B}j\u{E000}k\u{FFFF}l"
        );
        assert_eq!(clean_text(&s, 100), "abcdefghijkl");
    }

    #[test]
    fn line_separators_collapse_to_a_space() {
        assert_eq!(clean_text("a\u{2028}b\u{2029}c\r\nd", 100), "a b c d");
    }

    #[test]
    fn real_words_and_emoji_stay() {
        assert_eq!(
            clean_text("Caf\u{e9} \u{1F600} \u{65E5}\u{672C}", 100),
            "Caf\u{e9} \u{1F600} \u{65E5}\u{672C}"
        );
    }

    #[test]
    fn a_long_text_is_cut_with_dots() {
        assert_eq!(clean_text("abcdef", 3), "abc...");
    }
}

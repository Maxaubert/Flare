//! Wrapping a text preview's lines to the columns that fit (no Slint, no Win32).
//!
//! The text panel is monospace and virtualised: it builds only the rows in view, which needs every row to be one
//! line tall. So a long line is cut into several rows of at most `cols` characters here, once, when the panel's
//! width settles (never per drag step), and the window draws the rows. The line-number gutter shows the number on
//! a line's first row only; the continuation rows carry 0.
//!
//! A cut falls after a space or a dash when one sits in the last quarter of the row (a sentence breaks between
//! words), else it is a hard cut at `cols` characters (a minified line, a long path). Characters count as one
//! column each, as the panel's widest-line measure does.

/// The fewest columns a wrapped row has (a very narrow slot still shows something).
pub const MIN_COLS: usize = 8;

/// `line` as rows of at most `cols` characters, in order; an empty line is one empty row and a line that fits is
/// itself. Never loses a character: the rows joined are the line.
pub fn wrap_line(line: &str, cols: usize) -> Vec<&str> {
    let cols = cols.max(1);
    let mut rows = Vec::new();
    let mut rest = line;
    loop {
        // Byte offset of the (cols + 1)-th character: where the row would overflow.
        let Some((over, _)) = rest.char_indices().nth(cols) else {
            rows.push(rest);
            return rows;
        };
        let mut cut = over;
        // Prefer a break after a space or a dash in the last quarter of the row.
        let from = cols - cols / 4;
        let mut best = None;
        for (n, (at, c)) in rest.char_indices().enumerate().take(cols) {
            if n >= from && (c == ' ' || c == '-') {
                best = Some(at + c.len_utf8());
            }
        }
        if let Some(b) = best
            && b < over
            && b > 0
        {
            cut = b;
        }
        rows.push(&rest[..cut]);
        rest = &rest[cut..];
        if rest.is_empty() {
            return rows;
        }
    }
}

/// How many rows `line` takes at `cols`.
pub fn row_count(line: &str, cols: usize) -> usize {
    wrap_line(line, cols).len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_that_fits_is_one_row_and_an_empty_one_is_one_empty_row() {
        assert_eq!(wrap_line("", 10), [""]);
        assert_eq!(wrap_line("short", 10), ["short"]);
        assert_eq!(wrap_line("exactly10!", 10), ["exactly10!"]);
    }

    #[test]
    fn a_long_line_without_spaces_is_cut_hard_at_the_columns() {
        let line = "x".repeat(500);
        let rows = wrap_line(&line, 100);
        assert_eq!(rows.len(), 5);
        assert!(rows.iter().all(|r| r.chars().count() == 100));
        assert_eq!(rows.concat(), line);
    }

    #[test]
    fn a_sentence_breaks_after_a_space_in_the_last_quarter() {
        let rows = wrap_line("the quick brown fox jumps over the lazy dog", 16);
        assert_eq!(
            rows,
            ["the quick brown ", "fox jumps over ", "the lazy dog"]
        );
        assert!(rows.iter().all(|r| r.chars().count() <= 16));
    }

    #[test]
    fn no_character_is_ever_lost_or_split_in_two() {
        for cols in [1, 2, 3, 7, 8, 16, 33, 200] {
            for line in [
                "plain ascii text that goes on and on and on".to_string(),
                "h\u{e9}llo w\u{f6}rld \u{2014} \u{1f600}\u{1f600} \u{65e5}\u{672c}\u{8a9e} text"
                    .repeat(5),
                " ".repeat(40),
                "-".repeat(40),
            ] {
                let rows = wrap_line(&line, cols);
                assert_eq!(rows.concat(), line, "cols {cols}");
                assert!(
                    rows.iter()
                        .all(|r| r.chars().count() <= cols.max(1) && !r.is_empty()),
                    "cols {cols}: {rows:?}"
                );
            }
        }
    }

    #[test]
    fn the_row_count_matches_the_rows() {
        let line = "a b ".repeat(100);
        assert_eq!(row_count(&line, 30), wrap_line(&line, 30).len());
        assert_eq!(row_count("", 30), 1);
    }
}

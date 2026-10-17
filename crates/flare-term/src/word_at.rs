//! Word ranges under a cell for the double click, ported from xterm's
//! `SelectionService._getWordAt` and `OptionsService.wordSeparator`.

use crate::cells::CellInfo;

/// xterm's default `wordSeparator`: space ( ) [ ] { } ' , " and backtick.
pub const WORD_SEPARATORS: &str = " ()[]{}',\"`";

/// The word under cell `at` as `[start, end)` cells of the line. The clicked cell is always inside.
/// - `at` on a wide character's spacer (width 0) is moved to the wide cell first.
/// - a blank cell (chars "" or " ") with `allow_whitespace_only`: the run of blank cells around it;
///   without it: `None`.
/// - otherwise: left while the cell before `start` is not a separator, right while the cell after
///   `end - 1` is not a separator. A width-0 cell is never a separator. The end covers a trailing
///   wide character's spacer.
/// - `None` when `at >= cells.len()`.
pub fn word_range(
    cells: &[CellInfo],
    at: usize,
    allow_whitespace_only: bool,
) -> Option<(usize, usize)> {
    if at >= cells.len() {
        return None;
    }
    // A spacer belongs to the wide character before it.
    let at = if cells[at].width == 0 && at > 0 {
        at - 1
    } else {
        at
    };

    if is_blank(&cells[at]) {
        if !allow_whitespace_only {
            return None;
        }
        let mut start = at;
        while start > 0 && is_blank(&cells[start - 1]) {
            start -= 1;
        }
        let mut end = at + 1;
        while end < cells.len() && is_blank(&cells[end]) {
            end += 1;
        }
        return Some((start, end));
    }

    let mut start = at;
    while start > 0 && !is_separator(&cells[start - 1]) {
        start -= 1;
    }
    let mut end = at + 1;
    while end < cells.len() && !is_separator(&cells[end]) {
        end += 1;
    }
    Some((start, end))
}

/// An empty cell or a space: a blank. A spacer (width 0) is never blank.
fn is_blank(c: &CellInfo) -> bool {
    c.width != 0 && (c.chars.is_empty() || c.chars == " ")
}

/// A blank is a separator (xterm treats an empty cell as a space); a spacer never is.
fn is_separator(c: &CellInfo) -> bool {
    if c.width == 0 {
        return false;
    }
    if c.chars.is_empty() {
        return true;
    }
    let mut it = c.chars.chars();
    match (it.next(), it.next()) {
        (Some(ch), None) => WORD_SEPARATORS.contains(ch),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(chars: &str, width: u8) -> CellInfo {
        CellInfo {
            chars: chars.to_string(),
            width,
        }
    }

    fn cells(s: &str) -> Vec<CellInfo> {
        s.chars().map(|c| cell(&c.to_string(), 1)).collect()
    }

    fn blank() -> CellInfo {
        cell("", 1)
    }

    #[test]
    fn a_word_is_the_run_between_separators() {
        let c = cells("git status");
        assert_eq!(word_range(&c, 1, true), Some((0, 3)));
        assert_eq!(word_range(&c, 6, true), Some((4, 10)));
    }

    #[test]
    fn a_clicked_bracket_keeps_itself_and_runs_on() {
        let c = cells("(foo) bar");
        assert_eq!(word_range(&c, 0, true), Some((0, 4)));
        assert_eq!(word_range(&c, 4, true), Some((1, 5)));
    }

    #[test]
    fn a_blank_run_when_allowed() {
        let c = cells("a   b");
        assert_eq!(word_range(&c, 2, true), Some((1, 4)));
        assert_eq!(word_range(&c, 2, false), None);
    }

    #[test]
    fn empty_cells_are_blanks() {
        let c = vec![cell("a", 1), blank(), blank(), cell("b", 1)];
        assert_eq!(word_range(&c, 1, true), Some((1, 3)));
    }

    #[test]
    fn a_wide_character_is_one_unit() {
        let c = vec![
            cell("日", 2),
            cell("", 0),
            cell("本", 2),
            cell("", 0),
            cell("a", 1),
        ];
        for at in 0..5 {
            assert_eq!(word_range(&c, at, true), Some((0, 5)), "at {at}");
        }
    }

    #[test]
    fn a_combining_cluster_is_one_cell() {
        let c = vec![cell("e\u{301}", 1), cell("x", 1)];
        assert_eq!(word_range(&c, 0, true), Some((0, 2)));
    }

    #[test]
    fn the_end_is_exclusive_and_stops_at_the_line_end() {
        let c = cells("ab");
        assert_eq!(word_range(&c, 1, true), Some((0, 2)));
    }

    #[test]
    fn a_comma_a_quote_and_a_backtick_separate() {
        let c = cells("a,b");
        assert_eq!(word_range(&c, 0, true), Some((0, 1)));
        assert_eq!(word_range(&c, 2, true), Some((2, 3)));
        assert_eq!(word_range(&c, 1, true), Some((0, 3)));
        assert_eq!(word_range(&cells("a`b"), 0, true), Some((0, 1)));
    }

    #[test]
    fn out_of_range_is_none() {
        assert_eq!(word_range(&cells("ab"), 2, true), None);
    }
}

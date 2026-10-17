//! Find in scrollback: a literal search over logical lines (the xterm search addon's rules,
//! Prism Terminal's `TermFind`). Pure: it reads a `BufferLike` and owns no terminal state.
//! Text comes from `cells::logical_line`, so wraps and wide characters are right.

use crate::cells::{BufferLike, LineLike, LogicalLine, logical_line};
use crate::scan_plan::SLICE_MS;

/// The most matches the bar highlights and counts (xterm `highlightLimit`).
pub const HIGHLIGHT_LIMIT: usize = 1000;

/// A match: its first cell (absolute row, column) and its length in cells along the logical
/// line, so a match that wraps has `col + size > cols`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FindMatch {
    pub row: usize,
    pub col: usize,
    pub size: usize,
}

impl FindMatch {
    /// The pieces of the match per row: `(row, col, width)`; the first from `col`, later ones from 0.
    pub fn pieces(&self, cols: usize) -> Vec<(usize, usize, usize)> {
        let mut out = Vec::new();
        if cols == 0 {
            return out;
        }
        let mut left = self.size;
        let mut row = self.row;
        let mut col = self.col;
        while left > 0 {
            let w = left.min(cols - col);
            out.push((row, col, w));
            left -= w;
            row += 1;
            col = 0;
        }
        out
    }

    fn key(&self, cols: usize) -> usize {
        self.row * cols + self.col
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FindOpts {
    /// No control sets it (D4); the engine has it.
    pub case_sensitive: bool,
}

fn fold(c: char, opts: FindOpts) -> char {
    if opts.case_sensitive {
        c
    } else {
        c.to_lowercase().next().unwrap_or(c)
    }
}

pub struct Finder {
    term: Vec<char>,
    opts: FindOpts,
}

impl Finder {
    /// None for an empty term.
    pub fn new(term: &str, opts: FindOpts) -> Option<Self> {
        if term.is_empty() {
            return None;
        }
        Some(Self {
            term: term.chars().map(|c| fold(c, opts)).collect(),
            opts,
        })
    }

    /// Every non-overlapping match of one logical line, in reading order.
    fn line_matches(&self, ll: &LogicalLine, cols: usize) -> Vec<FindMatch> {
        let text: Vec<char> = ll.text.chars().map(|c| fold(c, self.opts)).collect();
        let m = self.term.len();
        if cols == 0 || text.len() < m {
            return Vec::new();
        }
        // Char index -> the cell holding it (spacers hold nothing).
        let starts: Vec<(usize, usize)> = (0..ll.widths.len())
            .filter(|&i| ll.widths[i] != 0)
            .map(|i| (i, ll.index[i]))
            .collect();
        let mut char_cell = vec![0usize; text.len()];
        for (k, &(cell, s)) in starts.iter().enumerate() {
            let e = starts.get(k + 1).map_or(text.len(), |n| n.1);
            for slot in char_cell.iter_mut().take(e.min(text.len())).skip(s) {
                *slot = cell;
            }
        }
        let mut out = Vec::new();
        let mut p = 0;
        while p + m <= text.len() {
            if text[p..p + m] == self.term[..] {
                let start = char_cell[p];
                let q = p + m;
                let end = if q < text.len() {
                    char_cell[q]
                } else {
                    ll.widths.len()
                };
                out.push(FindMatch {
                    row: ll.first + start / cols,
                    col: start % cols,
                    size: end.saturating_sub(start).max(1),
                });
                p = q;
            } else {
                p += 1;
            }
        }
        out
    }

    /// The matches that START in logical lines overlapping rows `first..=last`, in reading order.
    pub fn matches_in(
        &self,
        b: &dyn BufferLike,
        cols: usize,
        first: usize,
        last: usize,
    ) -> Vec<FindMatch> {
        let mut out = Vec::new();
        if b.is_empty() {
            return out;
        }
        let last = last.min(b.len() - 1);
        let mut y = first;
        while y <= last {
            let ll = logical_line(b, cols, y);
            out.extend(self.line_matches(&ll, cols));
            y = ll.last + 1;
        }
        out
    }

    /// The first match whose start key is `>= from` (linear cell key), scanning down.
    fn first_from(&self, b: &dyn BufferLike, cols: usize, from: usize) -> Option<FindMatch> {
        if b.is_empty() || cols == 0 {
            return None;
        }
        let mut y = from / cols;
        while y < b.len() {
            let ll = logical_line(b, cols, y);
            if let Some(m) = self
                .line_matches(&ll, cols)
                .into_iter()
                .find(|m| m.key(cols) >= from)
            {
                return Some(m);
            }
            y = ll.last + 1;
        }
        None
    }

    /// The last match whose start key is `< limit`, scanning up.
    fn last_before(&self, b: &dyn BufferLike, cols: usize, limit: usize) -> Option<FindMatch> {
        if b.is_empty() || cols == 0 {
            return None;
        }
        let mut y = (limit / cols).min(b.len() - 1);
        loop {
            let ll = logical_line(b, cols, y);
            if let Some(m) = self
                .line_matches(&ll, cols)
                .into_iter()
                .rev()
                .find(|m| m.key(cols) < limit)
            {
                return Some(m);
            }
            if ll.first == 0 {
                return None;
            }
            y = ll.first - 1;
        }
    }

    /// xterm `findNextWithSelection`. Just after `prev` when `same_term`, at `prev` when the term
    /// changed, from the top without `prev`; wraps to the top.
    pub fn next(
        &self,
        b: &dyn BufferLike,
        cols: usize,
        prev: Option<&FindMatch>,
        same_term: bool,
    ) -> Option<FindMatch> {
        let start = match prev {
            None => 0,
            Some(p) if same_term => p.key(cols) + p.size.max(1),
            Some(p) => p.key(cols),
        };
        self.first_from(b, cols, start)
            .or_else(|| self.first_from(b, cols, 0))
    }

    /// xterm `findPreviousWithSelection`. Without `prev` the last match in the buffer; with a
    /// changed term a match at or after `prev` in its own logical line first; else the last match
    /// before `prev`; then wrap to the bottom.
    pub fn previous(
        &self,
        b: &dyn BufferLike,
        cols: usize,
        prev: Option<&FindMatch>,
        same_term: bool,
    ) -> Option<FindMatch> {
        let Some(p) = prev else {
            return self.last_before(b, cols, usize::MAX);
        };
        if !same_term && !b.is_empty() && p.row < b.len() {
            let ll = logical_line(b, cols, p.row);
            let k = p.key(cols);
            if let Some(m) = self
                .line_matches(&ll, cols)
                .into_iter()
                .find(|m| m.key(cols) >= k)
            {
                return Some(m);
            }
        }
        self.last_before(b, cols, p.key(cols))
            .or_else(|| self.last_before(b, cols, usize::MAX))
    }
}

/// The bar's readout: "" for an empty query; "none" for 0 matches; "N+" when there are matches
/// but the active one is not among the counted; else "i of n" (1-based).
pub fn readout(query_empty: bool, count: usize, index: Option<usize>) -> String {
    if query_empty {
        String::new()
    } else if count == 0 {
        "none".to_string()
    } else if let Some(i) = index {
        format!("{} of {}", i + 1, count)
    } else {
        format!("{count}+")
    }
}

/// The active match's place in `results`, None when absent (xterm: -1).
pub fn index_of(results: &[FindMatch], active: &FindMatch) -> Option<usize> {
    results.iter().position(|m| m == active)
}

/// Lines to scroll (positive toward newer) so `row` is visible: None when it already is in
/// `top..top + rows`, else `row - top - rows / 2` (xterm `_selectResult`).
pub fn scroll_to_show(row: usize, top: usize, rows: usize) -> Option<i32> {
    if row >= top && row < top + rows {
        None
    } else {
        Some(row as i32 - top as i32 - (rows / 2) as i32)
    }
}

/// The sliced count of every match, so the bar's number can grow without holding the window.
/// Walks bottom-up in slices of `SLICE_MS` (the `scan_plan` budget rule).
pub struct CountJob {
    finder: Finder,
    /// Next row to visit, or -1 when finished.
    at: i64,
    found: Vec<FindMatch>,
}

impl CountJob {
    pub fn new(finder: Finder, total_rows: usize) -> Self {
        Self {
            finder,
            at: total_rows as i64 - 1,
            found: Vec::new(),
        }
    }

    /// One slice by `now` (a millisecond clock); the clock is read after each visited row and a
    /// slice always holds at least one row. True when the whole buffer has been walked.
    pub fn step(&mut self, b: &dyn BufferLike, cols: usize, now: &dyn Fn() -> f64) -> bool {
        if self.at >= b.len() as i64 {
            self.at = b.len() as i64 - 1;
        }
        let start = now();
        while self.at >= 0 {
            let row = self.at as usize;
            self.at -= 1;
            let continuation = b.line(row).is_some_and(LineLike::is_wrapped);
            if !continuation {
                let ll = logical_line(b, cols, row);
                self.found.extend(self.finder.line_matches(&ll, cols));
            }
            if now() - start >= SLICE_MS {
                break;
            }
        }
        self.done()
    }

    /// The first `HIGHLIGHT_LIMIT` matches in reading order so far; `done` says if it is final.
    pub fn results(&self) -> Vec<FindMatch> {
        let mut v = self.found.clone();
        v.sort_by_key(|m| (m.row, m.col));
        v.truncate(HIGHLIGHT_LIMIT);
        v
    }

    pub fn done(&self) -> bool {
        self.at < 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cells::ScratchCell;
    use std::cell::Cell;

    struct Row {
        cells: Vec<(String, u8)>,
        wrapped: bool,
    }
    impl LineLike for Row {
        fn is_wrapped(&self) -> bool {
            self.wrapped
        }
        fn load_cell(&self, x: usize, into: &mut ScratchCell) -> bool {
            match self.cells.get(x) {
                Some((s, w)) => {
                    into.chars = s.clone();
                    into.width = *w;
                }
                None => {
                    into.chars = String::new();
                    into.width = 1;
                }
            }
            true
        }
    }
    struct Buf {
        rows: Vec<Row>,
    }
    impl BufferLike for Buf {
        fn len(&self) -> usize {
            self.rows.len()
        }
        fn line(&self, y: usize) -> Option<&dyn LineLike> {
            self.rows.get(y).map(|r| r as &dyn LineLike)
        }
    }
    fn row(text: &str, wrapped: bool) -> Row {
        let mut cells = Vec::new();
        for c in text.chars() {
            if (c as u32) >= 0x1100 {
                cells.push((c.to_string(), 2));
                cells.push((String::new(), 0));
            } else {
                cells.push((c.to_string(), 1));
            }
        }
        Row { cells, wrapped }
    }
    fn buf(rows: &[(&str, bool)]) -> Buf {
        Buf {
            rows: rows.iter().map(|&(t, w)| row(t, w)).collect(),
        }
    }
    fn m(row: usize, col: usize, size: usize) -> FindMatch {
        FindMatch { row, col, size }
    }
    fn finder(t: &str) -> Finder {
        Finder::new(t, FindOpts::default()).unwrap()
    }

    #[test]
    fn finds_case_insensitively_by_default() {
        let b = buf(&[("Error: boom", false), ("an ERROR", false)]);
        let got = finder("error").matches_in(&b, 10, 0, 1);
        assert_eq!(got, vec![m(0, 0, 5), m(1, 3, 5)]);
    }

    #[test]
    fn case_sensitive_is_available_to_the_engine() {
        let b = buf(&[("an ERROR", false)]);
        let opts = FindOpts {
            case_sensitive: true,
        };
        let lower = Finder::new("error", opts).unwrap();
        assert!(lower.matches_in(&b, 10, 0, 0).is_empty());
        let upper = Finder::new("ERROR", opts).unwrap();
        assert_eq!(upper.matches_in(&b, 10, 0, 0).len(), 1);
    }

    #[test]
    fn a_match_across_a_wrap_has_pieces_on_both_rows() {
        let b = buf(&[("hello wor", false), ("ld", true)]);
        let got = finder("world").matches_in(&b, 9, 0, 1);
        assert_eq!(got, vec![m(0, 6, 5)]);
        assert_eq!(got[0].pieces(9), vec![(0, 6, 3), (1, 0, 2)]);
    }

    #[test]
    fn wide_characters_count_two_cells() {
        let b = buf(&[("日本語x", false)]);
        assert_eq!(finder("本").matches_in(&b, 10, 0, 0), vec![m(0, 2, 2)]);
    }

    #[test]
    fn empty_term_is_none() {
        assert!(Finder::new("", FindOpts::default()).is_none());
    }

    #[test]
    fn matches_do_not_overlap() {
        let b = buf(&[("aaaa", false)]);
        assert_eq!(
            finder("aa").matches_in(&b, 10, 0, 0),
            vec![m(0, 0, 2), m(0, 2, 2)]
        );
    }

    fn three() -> Buf {
        buf(&[("x ab", false), ("ab y", false), ("zz ab", false)])
    }

    #[test]
    fn next_without_a_previous_match_is_the_first() {
        assert_eq!(
            finder("ab").next(&three(), 10, None, true),
            Some(m(0, 2, 2))
        );
    }

    #[test]
    fn next_after_the_same_term_steps_on_and_wraps() {
        let b = three();
        let f = finder("ab");
        assert_eq!(f.next(&b, 10, Some(&m(0, 2, 2)), true), Some(m(1, 0, 2)));
        assert_eq!(f.next(&b, 10, Some(&m(2, 3, 2)), true), Some(m(0, 2, 2)));
    }

    #[test]
    fn next_with_a_changed_term_keeps_the_current_match() {
        let b = buf(&[("abc", false), ("x", false), ("abc", false)]);
        let prev = m(0, 0, 2);
        assert_eq!(
            finder("abc").next(&b, 10, Some(&prev), false),
            Some(m(0, 0, 3))
        );
    }

    #[test]
    fn a_single_match_finds_itself_again() {
        let b = buf(&[("hi ab", false)]);
        let only = m(0, 3, 2);
        assert_eq!(finder("ab").next(&b, 10, Some(&only), true), Some(only));
        assert_eq!(finder("ab").previous(&b, 10, Some(&only), true), Some(only));
    }

    #[test]
    fn previous_without_a_previous_match_is_the_last() {
        assert_eq!(
            finder("ab").previous(&three(), 10, None, true),
            Some(m(2, 3, 2))
        );
    }

    #[test]
    fn previous_steps_back_and_wraps_to_the_bottom() {
        let b = three();
        let f = finder("ab");
        assert_eq!(
            f.previous(&b, 10, Some(&m(1, 0, 2)), true),
            Some(m(0, 2, 2))
        );
        assert_eq!(
            f.previous(&b, 10, Some(&m(0, 2, 2)), true),
            Some(m(2, 3, 2))
        );
    }

    #[test]
    fn previous_with_a_changed_term_grows_in_place_first() {
        let b = buf(&[("ab", false), ("ab", false), (" abc", false)]);
        let prev = m(2, 1, 2);
        assert_eq!(
            finder("abc").previous(&b, 10, Some(&prev), false),
            Some(m(2, 1, 3))
        );
    }

    #[test]
    fn the_readout_words() {
        assert_eq!(readout(true, 5, Some(0)), "");
        assert_eq!(readout(false, 0, None), "none");
        assert_eq!(readout(false, 1000, None), "1000+");
        assert_eq!(readout(false, 12, Some(2)), "3 of 12");
    }

    #[test]
    fn index_of_finds_the_active_match() {
        let r = [m(0, 0, 1), m(1, 0, 1)];
        assert_eq!(index_of(&r, &m(1, 0, 1)), Some(1));
        assert_eq!(index_of(&r, &m(5, 0, 1)), None);
    }

    #[test]
    fn scrolling_to_show_goes_to_the_middle() {
        assert_eq!(scroll_to_show(500, 100, 24), Some(500 - 100 - 12));
        assert_eq!(scroll_to_show(105, 100, 24), None);
        assert_eq!(scroll_to_show(99, 100, 24), Some(99 - 100 - 12));
    }

    #[test]
    fn the_count_job_walks_in_slices_and_counts_in_reading_order() {
        let b = Buf {
            rows: (0..10_000).map(|_| row("a", false)).collect(),
        };
        let mut job = CountJob::new(finder("a"), b.len());
        let ticks = Cell::new(0.0f64);
        let now = || {
            ticks.set(ticks.get() + 1.0);
            ticks.get()
        };
        let mut calls = 0;
        loop {
            let before = job.found.len();
            let done = job.step(&b, 10, &now);
            calls += 1;
            assert!(job.found.len() - before <= 8);
            if done {
                break;
            }
            assert!(calls < 1250, "finished late");
        }
        assert_eq!(calls, 1250);
        let r = job.results();
        assert_eq!(r.len(), 1000);
        assert!(
            r.windows(2)
                .all(|w| (w[0].row, w[0].col) < (w[1].row, w[1].col))
        );
        assert!(job.done());
    }

    #[test]
    fn the_count_job_skips_wrapped_continuations() {
        let b = buf(&[("hello wor", false), ("ld", true)]);
        let mut job = CountJob::new(finder("world"), b.len());
        let now = || 0.0;
        assert!(job.step(&b, 9, &now));
        assert_eq!(job.results(), vec![m(0, 6, 5)]);
    }
}

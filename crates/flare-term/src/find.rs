//! Find in scrollback: a literal search over logical lines (the xterm search addon's rules,
//! Prism Terminal's `TermFind`). Pure: it reads a `BufferLike` and owns no terminal state.
//! Text comes from `cells::logical_line`, so wraps and wide characters are right.

use std::collections::VecDeque;

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
    /// The first match whose start key is `>= from`, scanning down logical lines that start at or
    /// before row `last_row` (a wrap-around pass stops at the line it began on).
    fn first_from(
        &self,
        b: &dyn BufferLike,
        cols: usize,
        from: usize,
        last_row: usize,
    ) -> Option<FindMatch> {
        if b.is_empty() || cols == 0 {
            return None;
        }
        let mut y = from / cols;
        while y < b.len() && y <= last_row {
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

    /// The last match whose start key is `< limit`, scanning up logical lines down to the one
    /// that holds row `stop_row` (a wrap-around pass stops at the line it began on).
    fn last_before(
        &self,
        b: &dyn BufferLike,
        cols: usize,
        limit: usize,
        stop_row: usize,
    ) -> Option<FindMatch> {
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
            if ll.first == 0 || ll.first <= stop_row {
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
        // The wrap pass ends at the line the first pass began on: nothing below it can match
        // that the first pass did not already see.
        self.first_from(b, cols, start, usize::MAX)
            .or_else(|| self.first_from(b, cols, 0, start / cols))
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
            return self.last_before(b, cols, usize::MAX, 0);
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
        self.last_before(b, cols, p.key(cols), 0)
            .or_else(|| self.last_before(b, cols, usize::MAX, p.row))
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

/// Whether a slice may visit one more row: its elapsed time plus the costliest row it has seen
/// must stay under `SLICE_MS`, so the slice ends before the row that would cross the budget.
pub fn slice_may_continue(elapsed_ms: f64, max_row_ms: f64) -> bool {
    elapsed_ms + max_row_ms < SLICE_MS
}

/// Which way a search walks: Enter and the down button go next, Shift+Enter and the up button
/// go previous.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Next,
    Prev,
}

/// One pass of a search, the same walks as `Finder::first_from` and `Finder::last_before`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Walk {
    /// Down from row `from / cols` through the logical lines that start at or before `last_row`;
    /// a match counts when its start key is `>= from`.
    Forward { from: usize, last_row: usize },
    /// Up from row `limit / cols` through the logical lines, down to the one that holds
    /// `stop_row`; a match counts when its start key is `< limit`.
    Backward { limit: usize, stop_row: usize },
}

/// What one unit of a walk found.
enum Unit {
    Found(FindMatch),
    Exhausted,
    Continue,
}

/// A search cut into slices of at most `SLICE_MS`. Run to the end it gives exactly the answer of
/// `Finder::next` or `Finder::previous` (the unsliced reference). One unit of work is one logical
/// line. Output moves the buffer between slices: a change in its length restarts the walk from the
/// anchor, and so does `restart` (the emulator calls it when output or a trim arrives).
pub struct StepJob {
    finder: Finder,
    dir: Dir,
    prev: Option<FindMatch>,
    same_term: bool,
    /// The buffer length the walk was planned against.
    total_rows: usize,
    planned: bool,
    passes: Vec<Walk>,
    /// The running pass.
    pass: usize,
    /// The row the running pass reads next; None before that pass starts.
    at: Option<usize>,
    cancelled: bool,
}

impl StepJob {
    pub fn new(
        finder: Finder,
        dir: Dir,
        prev: Option<FindMatch>,
        same_term: bool,
        total_rows: usize,
    ) -> Self {
        Self {
            finder,
            dir,
            prev,
            same_term,
            total_rows,
            planned: false,
            passes: Vec::new(),
            pass: 0,
            at: None,
            cancelled: false,
        }
    }

    /// Starts the walk again from its anchor: the buffer moved (output, or rows trimmed off the top).
    pub fn restart(&mut self) {
        self.planned = false;
        self.passes.clear();
        self.pass = 0;
        self.at = None;
    }

    /// Ends the walk: the next step answers "no match" and does no work.
    pub fn cancel(&mut self) {
        self.cancelled = true;
    }

    /// One slice. None = not finished; Some(None) = finished with no match; Some(Some(m)) = found.
    pub fn step(
        &mut self,
        b: &dyn BufferLike,
        cols: usize,
        now: &dyn Fn() -> f64,
    ) -> Option<Option<FindMatch>> {
        if self.cancelled {
            return Some(None);
        }
        if b.len() != self.total_rows {
            self.total_rows = b.len();
            self.restart();
        }
        if !self.planned {
            self.passes = self.plan(b, cols);
            self.planned = true;
        }
        let start = now();
        let mut elapsed = 0.0;
        let mut max_row = 0.0f64;
        let mut first = true;
        loop {
            if self.pass >= self.passes.len() {
                return Some(None);
            }
            if !first && !slice_may_continue(elapsed, max_row) {
                return None;
            }
            first = false;
            let unit_start = now();
            let unit = self.unit(b, cols);
            let unit_end = now();
            max_row = max_row.max(unit_end - unit_start);
            elapsed = unit_end - start;
            match unit {
                Unit::Found(m) => return Some(Some(m)),
                Unit::Exhausted => {
                    self.pass += 1;
                    self.at = None;
                }
                Unit::Continue => {}
            }
        }
    }

    /// The answer with no budget: the walk runs to its end in this call.
    pub fn run(&mut self, b: &dyn BufferLike, cols: usize) -> Option<FindMatch> {
        let no_budget = || 0.0;
        loop {
            if let Some(answer) = self.step(b, cols, &no_budget) {
                return answer;
            }
        }
    }

    /// The passes of this search, in the order `Finder::next` or `Finder::previous` makes them.
    fn plan(&self, b: &dyn BufferLike, cols: usize) -> Vec<Walk> {
        if b.is_empty() || cols == 0 {
            return Vec::new();
        }
        match (self.dir, self.prev) {
            (Dir::Next, prev) => {
                let start = match prev {
                    None => 0,
                    Some(p) if self.same_term => p.key(cols) + p.size.max(1),
                    Some(p) => p.key(cols),
                };
                vec![
                    Walk::Forward {
                        from: start,
                        last_row: usize::MAX,
                    },
                    Walk::Forward {
                        from: 0,
                        last_row: start / cols,
                    },
                ]
            }
            (Dir::Prev, None) => vec![Walk::Backward {
                limit: usize::MAX,
                stop_row: 0,
            }],
            (Dir::Prev, Some(p)) => {
                let k = p.key(cols);
                let mut passes = Vec::with_capacity(3);
                if !self.same_term && p.row < b.len() {
                    passes.push(Walk::Forward {
                        from: k,
                        last_row: p.row,
                    });
                }
                passes.push(Walk::Backward {
                    limit: k,
                    stop_row: 0,
                });
                passes.push(Walk::Backward {
                    limit: usize::MAX,
                    stop_row: p.row,
                });
                passes
            }
        }
    }

    /// One logical line of the running pass.
    fn unit(&mut self, b: &dyn BufferLike, cols: usize) -> Unit {
        if b.is_empty() {
            return Unit::Exhausted;
        }
        match self.passes[self.pass] {
            Walk::Forward { from, last_row } => {
                let y = self.at.unwrap_or(from / cols);
                if y >= b.len() || y > last_row {
                    return Unit::Exhausted;
                }
                let ll = logical_line(b, cols, y);
                if let Some(m) = self
                    .finder
                    .line_matches(&ll, cols)
                    .into_iter()
                    .find(|m| m.key(cols) >= from)
                {
                    return Unit::Found(m);
                }
                self.at = Some(ll.last + 1);
                Unit::Continue
            }
            Walk::Backward { limit, stop_row } => {
                let y = self.at.unwrap_or((limit / cols).min(b.len() - 1));
                let ll = logical_line(b, cols, y);
                if let Some(m) = self
                    .finder
                    .line_matches(&ll, cols)
                    .into_iter()
                    .rev()
                    .find(|m| m.key(cols) < limit)
                {
                    return Unit::Found(m);
                }
                if ll.first == 0 || ll.first <= stop_row {
                    return Unit::Exhausted;
                }
                self.at = Some(ll.first - 1);
                Unit::Continue
            }
        }
    }
}

/// The sliced count of every match, so the bar's number can grow without holding the window.
/// Walks bottom-up in slices of `SLICE_MS` (the `scan_plan` budget rule).
pub struct CountJob {
    finder: Finder,
    /// Next row to visit, or -1 when finished.
    at: i64,
    /// The first `HIGHLIGHT_LIMIT` matches in reading order found so far. The walk goes up, so
    /// every logical line's matches go in FRONT and the ones past the limit fall off the back:
    /// a slice never sorts or copies more than the limit.
    found: VecDeque<FindMatch>,
}

impl CountJob {
    pub fn new(finder: Finder, total_rows: usize) -> Self {
        Self {
            finder,
            at: total_rows as i64 - 1,
            found: VecDeque::new(),
        }
    }

    /// One slice by `now` (a millisecond clock). A slice always holds at least one row, and before
    /// each further row it stops when `slice_may_continue` says the next row would cross the budget.
    /// True when the whole buffer has been walked.
    pub fn step(&mut self, b: &dyn BufferLike, cols: usize, now: &dyn Fn() -> f64) -> bool {
        if self.at >= b.len() as i64 {
            self.at = b.len() as i64 - 1;
        }
        let start = now();
        let mut elapsed = 0.0;
        let mut max_row = 0.0f64;
        let mut first = true;
        while self.at >= 0 {
            if !first && !slice_may_continue(elapsed, max_row) {
                break;
            }
            first = false;
            let row_start = now();
            let row = self.at as usize;
            self.at -= 1;
            let continuation = b.line(row).is_some_and(LineLike::is_wrapped);
            if !continuation {
                let ll = logical_line(b, cols, row);
                for m in self.finder.line_matches(&ll, cols).into_iter().rev() {
                    self.found.push_front(m);
                }
                self.found.truncate(HIGHLIGHT_LIMIT);
            }
            let row_end = now();
            max_row = max_row.max(row_end - row_start);
            elapsed = row_end - start;
        }
        self.done()
    }

    /// The first `HIGHLIGHT_LIMIT` matches in reading order so far; `done` says if it is final.
    pub fn results(&self) -> Vec<FindMatch> {
        self.found.iter().copied().collect()
    }

    pub fn done(&self) -> bool {
        self.at < 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cells::ScratchCell;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

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

    /// A buffer whose rows cost `cost` ms the first time a walk reads them: the fake clock moves
    /// only when a row is read, so a slice's length is the work of the rows it visited.
    struct Timed {
        rows: Vec<Row>,
        clock: Rc<Cell<f64>>,
        cost: f64,
        charged: RefCell<Vec<bool>>,
    }
    impl Timed {
        fn new(rows: Vec<Row>, clock: &Rc<Cell<f64>>, cost: f64) -> Self {
            let n = rows.len();
            Self {
                rows,
                clock: Rc::clone(clock),
                cost,
                charged: RefCell::new(vec![false; n]),
            }
        }
    }
    impl BufferLike for Timed {
        fn len(&self) -> usize {
            self.rows.len()
        }
        fn line(&self, y: usize) -> Option<&dyn LineLike> {
            let mut charged = self.charged.borrow_mut();
            if let Some(seen) = charged.get_mut(y).filter(|seen| !**seen) {
                *seen = true;
                self.clock.set(self.clock.get() + self.cost);
            }
            drop(charged);
            self.rows.get(y).map(|r| r as &dyn LineLike)
        }
    }
    fn clock_of(c: &Rc<Cell<f64>>) -> impl Fn() -> f64 {
        let c = Rc::clone(c);
        move || c.get()
    }

    #[test]
    fn the_count_job_walks_in_slices_and_counts_in_reading_order() {
        let clock = Rc::new(Cell::new(0.0));
        let b = Timed::new((0..10_000).map(|_| row("a", false)).collect(), &clock, 1.0);
        let mut job = CountJob::new(finder("a"), b.len());
        let now = clock_of(&clock);
        let mut calls = 0;
        loop {
            let before = job.found.len();
            let done = job.step(&b, 10, &now);
            calls += 1;
            assert!(job.found.len() - before <= 8);
            if done {
                break;
            }
            assert!(calls < 2000, "finished late");
        }
        // 1 ms a row: a slice takes 7 rows (7 + 1 >= 8 refuses the eighth), 10,000 rows in 1,429 slices.
        assert_eq!(calls, 1429);
        let r = job.results();
        assert_eq!(r.len(), 1000);
        assert!(
            r.windows(2)
                .all(|w| (w[0].row, w[0].col) < (w[1].row, w[1].col))
        );
        assert!(job.done());
    }

    #[test]
    fn a_sliced_search_equals_the_unsliced_one() {
        let mut seed = 0x2545_f491_u64;
        let mut rnd = move |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        for case in 0..300 {
            let rows_n = 1 + rnd(40) as usize;
            let rows: Vec<(String, bool)> = (0..rows_n)
                .map(|i| {
                    let len = rnd(12) as usize;
                    let text: String = (0..len).map(|_| ['a', 'b', ' '][rnd(3) as usize]).collect();
                    (text, i > 0 && rnd(4) == 0)
                })
                .collect();
            let b = Buf {
                rows: rows.iter().map(|(t, w)| row(t, *w)).collect(),
            };
            let query = ["a", "ab", "b", "ba"][rnd(4) as usize];
            let cols = 3 + rnd(6) as usize;
            let prev = match rnd(4) {
                0 => None,
                1 => Some(m(
                    rnd(rows_n as u64) as usize,
                    rnd(cols as u64) as usize,
                    rnd(3) as usize,
                )),
                _ => {
                    let all = finder(query).matches_in(&b, cols, 0, rows_n - 1);
                    if all.is_empty() {
                        None
                    } else {
                        Some(all[rnd(all.len() as u64) as usize])
                    }
                }
            };
            let same_term = rnd(2) == 0;
            let dir = if rnd(2) == 0 { Dir::Next } else { Dir::Prev };
            let expected = match dir {
                Dir::Next => finder(query).next(&b, cols, prev.as_ref(), same_term),
                Dir::Prev => finder(query).previous(&b, cols, prev.as_ref(), same_term),
            };
            let ticks = Cell::new(0.0f64);
            let now = || {
                ticks.set(ticks.get() + 1.0);
                ticks.get()
            };
            let mut job = StepJob::new(finder(query), dir, prev, same_term, b.len());
            let mut steps = 0;
            let got = loop {
                steps += 1;
                assert!(steps < 100_000, "case {case} never finished");
                if let Some(answer) = job.step(&b, cols, &now) {
                    break answer;
                }
            };
            assert_eq!(got, expected, "case {case}");
        }
    }

    #[test]
    fn a_no_match_search_over_10000_lines_takes_many_short_steps() {
        let clock = Rc::new(Cell::new(0.0));
        let b = Timed::new((0..10_000).map(|_| row("x", false)).collect(), &clock, 0.05);
        let now = clock_of(&clock);
        let mut job = StepJob::new(finder("zz"), Dir::Next, None, false, b.len());
        let mut steps = 0;
        let mut worst = 0.0f64;
        loop {
            let before = clock.get();
            let done = job.step(&b, 10, &now).is_some();
            worst = worst.max(clock.get() - before);
            steps += 1;
            if done {
                break;
            }
        }
        assert!(steps >= 60, "{steps} steps");
        assert!(worst <= SLICE_MS, "the worst slice took {worst} ms");
        assert!((clock.get() - 500.0).abs() < 1e-6, "{}", clock.get());
    }

    #[test]
    fn a_costly_row_never_pushes_a_slice_past_the_budget() {
        let clock = Rc::new(Cell::new(0.0));
        let b = Timed::new((0..50).map(|_| row("x", false)).collect(), &clock, 5.0);
        let now = clock_of(&clock);
        let mut job = StepJob::new(finder("zz"), Dir::Next, None, false, b.len());
        let mut slices = Vec::new();
        loop {
            let before = clock.get();
            if job.step(&b, 10, &now).is_some() {
                break;
            }
            slices.push(clock.get() - before);
        }
        // Each slice holds one row: 5 + 5 >= 8 refuses the second. The first also reads the
        // wrap flag of row 1, so its slice is charged for two rows.
        assert!(slices.len() >= 49, "{} slices", slices.len());
        assert!(slices[0] <= 10.0);
        assert!(slices.iter().skip(1).all(|&d| d <= 5.0), "{slices:?}");
    }

    #[test]
    fn count_slices_stay_at_or_under_the_budget() {
        let clock = Rc::new(Cell::new(0.0));
        let b = Timed::new((0..10_000).map(|_| row("a", false)).collect(), &clock, 0.8);
        let now = clock_of(&clock);
        let mut job = CountJob::new(finder("a"), b.len());
        let mut calls = 0;
        loop {
            let before = clock.get();
            let done = job.step(&b, 10, &now);
            let took = clock.get() - before;
            assert!(took <= SLICE_MS, "a slice took {took} ms");
            calls += 1;
            if done {
                break;
            }
        }
        assert!(calls > 1000, "{calls} slices");
    }

    #[test]
    fn find_with_a_match_near_the_top_is_found_in_one_step() {
        let clock = Rc::new(Cell::new(0.0));
        let mut texts = vec!["x"; 10_000];
        texts[2] = "foo";
        let b = Timed::new(texts.iter().map(|t| row(t, false)).collect(), &clock, 0.05);
        let now = clock_of(&clock);
        let mut job = StepJob::new(finder("foo"), Dir::Next, None, false, b.len());
        assert_eq!(job.step(&b, 10, &now), Some(Some(m(2, 0, 3))));
    }

    #[test]
    fn output_between_slices_neither_skips_nor_repeats_a_row() {
        // History at capacity: every feed pushes a row in at the bottom and trims one off the top.
        let mut b = Buf {
            rows: (0..200)
                .map(|i| row(if i % 7 == 0 { "foo" } else { "x" }, false))
                .collect(),
        };
        let ticks = Cell::new(0.0f64);
        let now = || {
            ticks.set(ticks.get() + 1.0);
            ticks.get()
        };
        let mut job = StepJob::new(finder("foo"), Dir::Next, None, false, b.len());
        let mut feeds = 0;
        let got = loop {
            if feeds < 50 {
                for i in 0..1000 {
                    b.rows
                        .push(row(if i % 5 == 0 { "foo" } else { "y" }, false));
                    b.rows.remove(0);
                }
                // What the emulator does on output: the walk starts again from its anchor.
                job.restart();
                feeds += 1;
            }
            if let Some(answer) = job.step(&b, 10, &now) {
                break answer;
            }
        };
        assert_eq!(got, finder("foo").next(&b, 10, None, false));
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

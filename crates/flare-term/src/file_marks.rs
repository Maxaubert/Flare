//! File links by marker (spec 2026-11-11, `docs/superpowers/specs/2026-11-11-file-link-markers.md`). A session
//! writes a file path between `⟦` (U+27E6) and `⟧` (U+27E7); this reader finds each span and reads what is between
//! the marks exactly. Flare guesses no file paths any more: only what is written between marks is a file link.
//!
//! A span may run over rows: a row the terminal wrapped is joined with nothing dropped, and a row the program broke
//! by hand (Claude Code breaks its own lines and indents the next row) is joined with the next row's leading spaces
//! and the last row's trailing blanks dropped. A trailing `:line` or `:line:col` is taken off and kept. A lone `⟦`
//! never eats the screen: a span ends within `MARK_ROWS` rows and `MARK_CHARS` characters, or is no link.

use crate::cells::{BufferLike, CellInfo, ScratchCell, buffer_row_cells};
use crate::link_scan::Piece;
use std::collections::HashMap;

/// The start mark.
pub const MARK_OPEN: char = '\u{27E6}';
/// The end mark.
pub const MARK_CLOSE: char = '\u{27E7}';
/// The most rows one span may cover, its first row included.
pub const MARK_ROWS: usize = 8;
/// The most characters between the marks.
pub const MARK_CHARS: usize = 1024;
/// The most other spellings a span is tried as (a space where a row broke).
const ALTS_MAX: usize = 8;

/// One span between marks: the path it names, its line and column if it gave them, and the cells it covers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileMark {
    /// The path, as written, rows joined with nothing between them.
    pub text: String,
    pub line: Option<u32>,
    pub col: Option<u32>,
    /// The same path with one space where a row broke by hand (each break alone, then all of them): a click tries
    /// these, in order, when `text` names nothing.
    pub alts: Vec<String>,
    /// One piece per row, marks included. `row` is absolute.
    pub pieces: Vec<Piece>,
}

/// The rows a scan reads, each read once.
struct Rows<'a> {
    b: &'a dyn BufferLike,
    cols: usize,
    got: HashMap<usize, Option<Vec<CellInfo>>>,
    scratch: ScratchCell,
}

impl Rows<'_> {
    fn cells(&mut self, y: usize) -> Option<&Vec<CellInfo>> {
        if !self.got.contains_key(&y) {
            let row = buffer_row_cells(self.b.line(y), self.cols, &mut self.scratch);
            self.got.insert(y, row);
        }
        self.got.get(&y).and_then(Option::as_ref)
    }

    fn wrapped(&self, y: usize) -> bool {
        self.b.line(y).is_some_and(|l| l.is_wrapped())
    }
}

fn is_mark(cell: &CellInfo, mark: char) -> bool {
    cell.width > 0 && cell.chars.starts_with(mark)
}

fn is_blank(cell: &CellInfo) -> bool {
    cell.chars.is_empty() || cell.chars.chars().all(char::is_whitespace)
}

/// The cell just past the last one that holds anything.
fn text_end(row: &[CellInfo]) -> usize {
    row.iter().rposition(|c| !is_blank(c)).map_or(0, |i| i + 1)
}

/// How a read from a start mark ended.
enum Read {
    /// A whole span, and the cell of its end mark.
    Mark(FileMark, usize, usize),
    /// Another start mark came first: the read starts again from it.
    Again(usize, usize),
    /// No end mark within the limits.
    Lost,
}

/// Every span that starts at most `MARK_ROWS - 1` rows above `first` and covers a row of `first..=last`, in order.
pub fn find_marks(b: &dyn BufferLike, cols: usize, first: usize, last: usize) -> Vec<FileMark> {
    let mut out = Vec::new();
    if cols == 0 || b.is_empty() {
        return out;
    }
    let last = last.min(b.len() - 1);
    let mut rows = Rows {
        b,
        cols,
        got: HashMap::new(),
        scratch: ScratchCell::default(),
    };
    let (mut y, mut x) = (first.saturating_sub(MARK_ROWS - 1), 0);
    while y <= last {
        let open = rows
            .cells(y)
            .and_then(|r| (x..r.len()).find(|&i| is_mark(&r[i], MARK_OPEN)));
        let Some(c) = open else {
            (y, x) = (y + 1, 0);
            continue;
        };
        match read_from(&mut rows, y, c) {
            Read::Mark(m, ey, ex) => {
                if m.pieces.iter().any(|p| p.row >= first && p.row <= last) {
                    out.push(m);
                }
                (y, x) = (ey, ex + 1);
            }
            Read::Again(ry, rx) => (y, x) = (ry, rx),
            Read::Lost => x = c + 1,
        }
    }
    out
}

/// Reads one span from the start mark at row `y0`, cell `c0`.
fn read_from(rows: &mut Rows, y0: usize, c0: usize) -> Read {
    let cols = rows.cols;
    let mut text = String::new();
    let mut n = 0usize;
    let mut breaks: Vec<usize> = Vec::new();
    let mut pieces: Vec<Piece> = Vec::new();
    let (mut y, mut from, mut start) = (y0, c0 + 1, c0);
    loop {
        let Some(row) = rows.cells(y).cloned() else {
            return Read::Lost;
        };
        let mut blank_tail = 0usize;
        for (x, cell) in row.iter().enumerate().skip(from) {
            if cell.width == 0 {
                continue;
            }
            if is_mark(cell, MARK_CLOSE) {
                pieces.push(Piece {
                    row: y,
                    col: start,
                    width: x + 1 - start,
                });
                return match parse(&text, &breaks) {
                    Some(m) => Read::Mark(FileMark { pieces, ..m }, y, x),
                    None => Read::Lost,
                };
            }
            if is_mark(cell, MARK_OPEN) {
                return Read::Again(y, x);
            }
            let s = if cell.chars.is_empty() {
                " "
            } else {
                cell.chars.as_str()
            };
            text.push_str(s);
            n += s.chars().count();
            blank_tail = if is_blank(cell) { blank_tail + 1 } else { 0 };
            if n > MARK_CHARS {
                return Read::Lost;
            }
        }
        let next = y + 1;
        if next - y0 >= MARK_ROWS || next >= rows.b.len() {
            return Read::Lost;
        }
        if rows.wrapped(next) {
            pieces.push(Piece {
                row: y,
                col: start,
                width: cols - start,
            });
            (y, from, start) = (next, 0, 0);
            continue;
        }
        // Broken by hand: the blanks after the row's text are not the path's, nor the next row's indent.
        for _ in 0..blank_tail {
            text.pop();
        }
        n -= blank_tail;
        let end = text_end(&row).max(start + 1);
        pieces.push(Piece {
            row: y,
            col: start,
            width: end - start,
        });
        let Some(lead) = rows
            .cells(next)
            .and_then(|r| r.iter().position(|c| !is_blank(c)))
        else {
            return Read::Lost;
        };
        breaks.push(n);
        (y, from, start) = (next, lead, lead);
    }
}

/// The path, line and column a span's text names; None for one that names no file.
fn parse(text: &str, breaks: &[usize]) -> Option<FileMark> {
    let lead = text.chars().take_while(|c| c.is_whitespace()).count();
    let body = text.trim();
    if body.is_empty() || body.contains("://") || body.chars().any(char::is_control) {
        return None;
    }
    let (path, line, col) = split_line(body);
    if path.is_empty() {
        return None;
    }
    let len = path.chars().count();
    let at: Vec<usize> = breaks
        .iter()
        .filter_map(|b| b.checked_sub(lead))
        .filter(|&b| b > 0 && b < len)
        .collect();
    let spaced = |cuts: &[usize]| -> String {
        let mut s = String::with_capacity(path.len() + cuts.len());
        for (i, c) in path.chars().enumerate() {
            if cuts.contains(&i) {
                s.push(' ');
            }
            s.push(c);
        }
        s
    };
    let mut alts: Vec<String> = at.iter().map(|&b| spaced(&[b])).collect();
    if at.len() > 1 {
        alts.push(spaced(&at));
    }
    alts.truncate(ALTS_MAX);
    Some(FileMark {
        text: path.to_string(),
        line,
        col,
        alts,
        pieces: Vec::new(),
    })
}

/// `:line` or `:line:col` off the end of a path.
fn split_line(s: &str) -> (&str, Option<u32>, Option<u32>) {
    let num = |t: &str| -> Option<(usize, u32)> {
        let i = t.rfind(':')?;
        let d = &t[i + 1..];
        if d.is_empty() || !d.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        Some((i, d.parse().ok()?))
    };
    let Some((i, last)) = num(s) else {
        return (s, None, None);
    };
    let head = &s[..i];
    match num(head) {
        Some((j, line)) => (&head[..j], Some(line), Some(last)),
        None => (head, Some(last), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cells::LineLike;

    struct Row {
        text: String,
        wrapped: bool,
    }
    impl LineLike for Row {
        fn is_wrapped(&self) -> bool {
            self.wrapped
        }
        fn load_cell(&self, x: usize, into: &mut ScratchCell) -> bool {
            into.chars = self
                .text
                .chars()
                .nth(x)
                .filter(|c| *c != ' ')
                .map(|c| c.to_string())
                .unwrap_or_default();
            into.width = 1;
            true
        }
    }
    struct Buf(Vec<Row>);
    impl BufferLike for Buf {
        fn len(&self) -> usize {
            self.0.len()
        }
        fn line(&self, y: usize) -> Option<&dyn LineLike> {
            self.0.get(y).map(|r| r as &dyn LineLike)
        }
    }
    fn buf(rows: &[(&str, bool)]) -> Buf {
        Buf(rows
            .iter()
            .map(|&(t, w)| Row {
                text: t.to_string(),
                wrapped: w,
            })
            .collect())
    }
    fn marks(rows: &[(&str, bool)], cols: usize) -> Vec<FileMark> {
        let b = buf(rows);
        find_marks(&b, cols, 0, rows.len() - 1)
    }

    #[test]
    fn a_path_between_marks_on_one_row_is_one_link_with_its_line() {
        let m = marks(&[("see ⟦C:\\repo\\src\\main.rs:42⟧ now", false)], 40);
        assert_eq!(m.len(), 1, "{m:?}");
        assert_eq!(m[0].text, "C:\\repo\\src\\main.rs");
        assert_eq!((m[0].line, m[0].col), (Some(42), None));
        assert_eq!(
            m[0].pieces,
            vec![Piece {
                row: 0,
                col: 4,
                width: 24
            }]
        );
        assert!(m[0].alts.is_empty());
        let m = marks(&[("⟦C:\\a.rs:7:3⟧", false)], 20);
        assert_eq!(
            (m[0].text.as_str(), m[0].line, m[0].col),
            ("C:\\a.rs", Some(7), Some(3))
        );
    }

    #[test]
    fn a_span_the_terminal_wrapped_is_joined_with_nothing_dropped() {
        // The row ends with a space that is part of the path: a wrapped row keeps it.
        let m = marks(&[("x ⟦C:\\My D", false), ("ocs\\a.txt⟧ y", true)], 10);
        assert_eq!(m.len(), 1, "{m:?}");
        assert_eq!(m[0].text, "C:\\My Docs\\a.txt");
        assert_eq!(
            m[0].pieces,
            vec![
                Piece {
                    row: 0,
                    col: 2,
                    width: 8
                },
                Piece {
                    row: 1,
                    col: 0,
                    width: 10
                }
            ]
        );
    }

    #[test]
    fn a_span_broken_by_hand_drops_the_indent_of_the_next_row() {
        // Claude Code: the row is cut at its own width and the next one is indented two spaces.
        let m = marks(
            &[
                ("  Saved ⟦C:\\Users\\Admin\\Documents\\Claude\\Gith", false),
                ("  ub\\web-app\\src\\app\\auth\\form-state.ts:17⟧ ok", false),
            ],
            48,
        );
        assert_eq!(m.len(), 1, "{m:?}");
        assert_eq!(
            m[0].text,
            "C:\\Users\\Admin\\Documents\\Claude\\Github\\web-app\\src\\app\\auth\\form-state.ts"
        );
        assert_eq!(m[0].line, Some(17));
        assert_eq!(m[0].pieces.len(), 2);
        assert_eq!((m[0].pieces[1].row, m[0].pieces[1].col), (1, 2));
        assert_eq!(
            m[0].alts,
            vec![
                "C:\\Users\\Admin\\Documents\\Claude\\Gith ub\\web-app\\src\\app\\auth\\form-state.ts"
            ]
        );
    }

    #[test]
    fn a_space_at_a_hand_break_is_offered_as_the_second_spelling() {
        let m = marks(&[("⟦C:\\My", false), ("   Docs\\a.txt⟧", false)], 16);
        assert_eq!(m[0].text, "C:\\MyDocs\\a.txt");
        assert_eq!(m[0].alts, vec!["C:\\My Docs\\a.txt"]);
        // Three rows: each break alone, then both.
        let m = marks(
            &[("⟦C:\\a", false), (" b\\c", false), (" d.txt⟧", false)],
            8,
        );
        assert_eq!(m[0].text, "C:\\ab\\cd.txt");
        assert_eq!(
            m[0].alts,
            vec!["C:\\a b\\cd.txt", "C:\\ab\\c d.txt", "C:\\a b\\c d.txt"]
        );
    }

    #[test]
    fn a_start_mark_with_no_end_mark_is_no_link_and_eats_nothing() {
        let mut rows: Vec<(&str, bool)> = vec![("⟦C:\\never\\closed", false)];
        rows.extend((0..10).map(|_| ("more text here", false)));
        rows.push(("late⟧", false));
        assert!(marks(&rows, 20).is_empty());
        // A blank row ends the span too.
        assert!(marks(&[("⟦C:\\a", false), ("", false), ("b.txt⟧", false)], 10).is_empty());
        // So does a span longer than the character limit.
        let long = format!("⟦C:\\{}⟧", "a".repeat(MARK_CHARS + 1));
        assert!(marks(&[(&long, false)], long.chars().count()).is_empty());
    }

    #[test]
    fn two_links_on_one_row_are_two_marks() {
        let m = marks(&[("⟦C:\\a.rs⟧ and ⟦C:\\b.rs:2⟧", false)], 40);
        assert_eq!(m.len(), 2, "{m:?}");
        assert_eq!(m[0].text, "C:\\a.rs");
        assert_eq!((m[1].text.as_str(), m[1].line), ("C:\\b.rs", Some(2)));
        assert!(m[0].pieces[0].col + m[0].pieces[0].width <= m[1].pieces[0].col);
    }

    #[test]
    fn a_second_start_mark_starts_the_span_again() {
        let m = marks(&[("⟦oops ⟦C:\\a.rs⟧", false)], 30);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].text, "C:\\a.rs");
        assert_eq!(m[0].pieces[0].col, 6);
    }

    #[test]
    fn marks_around_a_web_address_are_no_file_link() {
        assert!(marks(&[("⟦https://a.io/x⟧", false)], 30).is_empty());
        assert!(marks(&[("⟦   ⟧ ⟦⟧", false)], 30).is_empty());
    }

    #[test]
    fn windows_and_posix_paths_are_both_read() {
        let m = marks(
            &[("⟦/Users/me/a b.txt:3⟧ ⟦~/notes.md⟧ ⟦C:/x/y.rs⟧", false)],
            60,
        );
        let texts: Vec<&str> = m.iter().map(|k| k.text.as_str()).collect();
        assert_eq!(texts, ["/Users/me/a b.txt", "~/notes.md", "C:/x/y.rs"]);
        assert_eq!(m[0].line, Some(3));
    }

    #[test]
    fn a_relative_path_is_read_as_written() {
        let m = marks(&[("⟦src\\main.rs:9⟧", false)], 30);
        assert_eq!((m[0].text.as_str(), m[0].line), ("src\\main.rs", Some(9)));
    }

    #[test]
    fn only_spans_that_touch_the_rows_asked_are_given() {
        let b = buf(&[
            ("⟦C:\\a.rs⟧", false),
            ("", false),
            ("⟦C:\\long", false),
            ("  \\b.rs⟧", false),
        ]);
        let m = find_marks(&b, 20, 3, 3);
        assert_eq!(m.len(), 1, "{m:?}");
        assert_eq!(m[0].text, "C:\\long\\b.rs");
        assert_eq!(m[0].pieces[0].row, 2);
    }
}

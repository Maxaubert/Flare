//! The link scanner over the viewport (Prism Terminal's termLinkPaint.ts, without the painting).
//! Web links come from `links::find_links_in`, file links only from the marks a session writes around a path
//! (`file_marks`, 2026-11-11: no file path is guessed any more), OSC 8 links from the per-cell hyperlinks of the
//! buffer (decision D18). Only the viewport is scanned, one logical line at a time, and a line whose text,
//! hyperlinks and ink are unchanged is not scanned again. No decoration is stored: the marks are rebuilt from the
//! per-line cache on every call.

use crate::cells::{BufferLike, LogicalLine, SCAN_WINDOW_ROWS, logical_line_win};
use crate::file_marks::find_marks;
use crate::links::find_links_in;
use crate::osc8::is_web_link;
use crate::path_grammar::is_relative;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkTarget {
    Web(String),
    /// A path a session wrote between the marks (`file_marks`), with the line and column it named. `alts` are the
    /// other spellings a click tries when `text` names nothing (a space where a row broke).
    Path {
        text: String,
        line: Option<u32>,
        col: Option<u32>,
        alts: Vec<String>,
    },
    Osc8 {
        uri: String,
    },
}

impl LinkTarget {
    /// A file link to `text` with no line (tests and the probes).
    pub fn path(text: &str) -> Self {
        Self::Path {
            text: text.to_string(),
            line: None,
            col: None,
            alts: Vec::new(),
        }
    }
}

/// The cells of a mark on one row. `row` is ABSOLUTE.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Piece {
    pub row: usize,
    pub col: usize,
    pub width: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkMark {
    pub target: LinkTarget,
    pub pieces: Vec<Piece>,
}

pub struct LinkCtx<'a> {
    /// The tab's folder: a relative path between marks is a link only when it is known.
    pub cwd: Option<&'a str>,
    /// Changes when the link colour changes: everything is scanned again.
    pub ink_key: u32,
    /// The hyperlink on a cell (absolute row, column) as (id, uri).
    pub hyperlink: &'a dyn Fn(usize, usize) -> Option<(String, String)>,
}

type Link = Option<(String, String)>;

pub struct LinkScanner {
    cache: HashMap<usize, (u64, Vec<LinkMark>)>,
    hidden: bool,
    scanned: usize,
    /// A repaint was asked; the cache is dropped once, at the next scan (the frame).
    repaint_pending: bool,
    repaints: usize,
}

impl Default for LinkScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl LinkScanner {
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
            hidden: false,
            scanned: 0,
            repaint_pending: false,
            repaints: 0,
        }
    }

    /// Hidden: `scan` returns nothing and does no work. Shown again: the cache is dropped.
    pub fn set_hidden(&mut self, hidden: bool) {
        if self.hidden && !hidden {
            self.repaint();
        }
        self.hidden = hidden;
    }

    /// Asks for everything to be scanned again. Asks within one frame are one: the cache is dropped at the
    /// next scan, once (core/renderer/lib/lookApply.ts, oncePerFrame).
    pub fn repaint(&mut self) {
        self.repaint_pending = true;
    }

    /// Cache drops actually done (a test reads it).
    pub fn repaints_done(&self) -> usize {
        self.repaints
    }

    /// Logical lines actually scanned so far.
    pub fn lines_scanned(&self) -> usize {
        self.scanned
    }

    /// The marks on the logical lines overlapping absolute rows `first..=last`. `b` must hold the
    /// `file_marks::MARK_ROWS` rows above and below them too, so a file link that starts above the view is whole.
    pub fn scan(
        &mut self,
        b: &dyn BufferLike,
        cols: usize,
        first: usize,
        last: usize,
        ctx: &mut LinkCtx,
    ) -> Vec<LinkMark> {
        if self.hidden || cols == 0 || b.is_empty() {
            return Vec::new();
        }
        if std::mem::take(&mut self.repaint_pending) {
            self.cache.clear();
            self.repaints += 1;
        }
        let last = last.min(b.len() - 1);
        let mut out = Vec::new();
        let mut next: HashMap<usize, (u64, Vec<LinkMark>)> = HashMap::new();
        let mut y = first;
        let mut floor = 0;
        // Where the mark the last piece left for this one starts (it touched that piece's bottom edge).
        let mut carry: Option<(usize, usize)> = None;
        while y <= last {
            // Across rows a program broke by hand too (`cells::by_hand`): a web address it cut at the
            // edge is read whole, and each row keeps its own piece of the mark.
            // A very long line is read in pieces of a window of rows, each from where the last ended.
            let ll = logical_line_win(b, cols, y, true, floor, SCAN_WINDOW_ROWS);
            let row_end = ll.last;
            let links = line_links(&ll, cols, ctx);
            let blank = ll.text_end == 0 && links.iter().all(Option::is_none);
            if blank {
                carry = None;
            } else {
                let key = line_key(&ll, cols, &links, ctx);
                let marks = match self.cache.remove(&ll.first) {
                    Some((k, marks)) if k == key => marks,
                    _ => {
                        self.scanned += 1;
                        scan_line(&ll, cols, &links)
                    }
                };
                // A piece cut from a longer line: a mark touching the cut edge is only half a link, left to
                // the piece that has its other half; a mark wholly in the overlap was the last piece's.
                // A mark the last piece dropped at ITS bottom edge is this piece's, even wholly above `y`.
                let owed = carry;
                out.extend(
                    marks
                        .iter()
                        .filter(|m| !(ll.cut_top && touches_top(m, &ll)))
                        .filter(|m| !(ll.cut_bottom && touches_bottom(m, &ll, cols)))
                        .filter(|m| {
                            m.pieces.iter().any(|p| p.row >= y)
                                || owed.is_some_and(|c| (m.pieces[0].row, m.pieces[0].col) >= c)
                        })
                        .cloned(),
                );
                // The start of the first mark this piece leaves for the next (touching its bottom edge).
                carry = if ll.cut_bottom {
                    marks
                        .iter()
                        .filter(|m| touches_bottom(m, &ll, cols))
                        .map(|m| (m.pieces[0].row, m.pieces[0].col))
                        .min()
                } else {
                    None
                };
                next.insert(ll.first, (key, marks));
            }
            y = row_end + 1;
            // A line cut at the window's bottom goes on in the next piece, which starts a few rows back
            // so a link straddling the cut is read whole in it.
            // The next piece starts a row above the mark left at the bottom edge, if that is further back, so
            // a link longer than the overlap is read whole.
            floor = if ll.cut_bottom {
                let mut back = y.saturating_sub(CUT_OVERLAP_ROWS);
                if let Some((row, _)) = carry {
                    back = back.min(row.saturating_sub(1));
                }
                back.max(ll.first + 1)
            } else {
                y
            };
        }
        self.cache = next;
        let files = file_links(b, cols, first, last, ctx.cwd);
        if !files.is_empty() {
            // The session marked these on purpose: a web address or a label under one gives way.
            out.retain(|m| !files.iter().any(|f| overlaps(m, f)));
            out.extend(files);
            out.sort_by_key(|m| (m.pieces[0].row, m.pieces[0].col));
        }
        out
    }
}

/// Rows a piece of a very long line starts back into the previous one.
const CUT_OVERLAP_ROWS: usize = 6;

/// The file links between marks on rows `first..=last`: a relative path only when the tab's folder is known.
fn file_links(
    b: &dyn BufferLike,
    cols: usize,
    first: usize,
    last: usize,
    cwd: Option<&str>,
) -> Vec<LinkMark> {
    find_marks(b, cols, first, last)
        .into_iter()
        .filter(|m| cwd.is_some() || !is_relative(&m.text))
        .map(|m| LinkMark {
            target: LinkTarget::Path {
                text: m.text,
                line: m.line,
                col: m.col,
                alts: m.alts,
            },
            pieces: m.pieces,
        })
        .collect()
}

fn overlaps(a: &LinkMark, b: &LinkMark) -> bool {
    a.pieces.iter().any(|p| {
        b.pieces
            .iter()
            .any(|q| p.row == q.row && p.col < q.col + q.width && q.col < p.col + p.width)
    })
}

fn touches_top(m: &LinkMark, ll: &LogicalLine) -> bool {
    m.pieces.iter().any(|p| p.row == ll.first && p.col == 0)
}

fn touches_bottom(m: &LinkMark, ll: &LogicalLine, cols: usize) -> bool {
    m.pieces
        .iter()
        .any(|p| p.row == ll.last && p.col + p.width >= cols)
}

/// The hyperlink of every cell of the line (a wide character's spacer shares its character's).
fn line_links(ll: &LogicalLine, cols: usize, ctx: &LinkCtx) -> Vec<Link> {
    let mut out: Vec<Link> = Vec::with_capacity(ll.widths.len());
    for (i, w) in ll.widths.iter().enumerate() {
        if *w == 0 && i > 0 {
            let prev = out[i - 1].clone();
            out.push(prev);
        } else {
            out.push((ctx.hyperlink)(ll.first + i / cols, i % cols));
        }
    }
    out
}

fn line_key(ll: &LogicalLine, cols: usize, links: &[Link], ctx: &LinkCtx) -> u64 {
    let mut h = DefaultHasher::new();
    ll.text.hash(&mut h);
    ll.last.hash(&mut h);
    cols.hash(&mut h);
    ctx.ink_key.hash(&mut h);
    ll.widths.hash(&mut h);
    ll.joins.hash(&mut h);
    links.hash(&mut h);
    h.finish()
}

/// The pieces for cells `start..end`, one per row of `cols`.
fn pieces(ll: &LogicalLine, cols: usize, start: usize, end: usize) -> Vec<Piece> {
    let mut out = Vec::new();
    let mut c = start;
    while c < end {
        let row = c / cols;
        let stop = end.min((row + 1) * cols);
        let width = stop - c;
        if width > 0 {
            out.push(Piece {
                row: ll.first + row,
                col: c % cols,
                width,
            });
        }
        c = stop;
    }
    out
}

/// The cell range of the char range `start..end`.
fn cell_range(ll: &LogicalLine, start: usize, end: usize) -> (usize, usize) {
    let n = ll.widths.len();
    let s = (0..n)
        .find(|&i| ll.index[i] >= start && ll.widths[i] > 0)
        .unwrap_or(n);
    let e = (0..n).find(|&i| ll.index[i] >= end).unwrap_or(n);
    (s, e.max(s))
}

fn scan_line(ll: &LogicalLine, cols: usize, links: &[Link]) -> Vec<LinkMark> {
    let chars: Vec<char> = ll.text.chars().collect();
    let mut spans: Vec<(LinkMark, usize, usize)> = Vec::new();
    for r in find_links_in(&ll.text, &ll.joins) {
        let (s, e) = cell_range(ll, r.start, r.end);
        let ps = pieces(ll, cols, s, e);
        if ps.is_empty() {
            continue;
        }
        let target = LinkTarget::Web(chars[r.start..r.end.min(chars.len())].iter().collect());
        spans.push((LinkMark { target, pieces: ps }, s, e));
    }
    let mut i = 0;
    while i < links.len() {
        let Some(link) = &links[i] else {
            i += 1;
            continue;
        };
        let mut j = i + 1;
        // A run stops where a hand break starts a row: two labels there, as the stream spans
        // Prism Terminal paints from are, and `mark_chars` reads one row.
        while j < links.len()
            && links[j].as_ref() == Some(link)
            && !(j % cols == 0 && ll.index.get(j).is_some_and(|c| ll.joins.contains(c)))
        {
            j += 1;
        }
        let uri = &link.1;
        if is_web_link(uri) {
            let dup = spans.iter().any(|(m, s, e)| {
                *s <= i
                    && j <= *e
                    && match &m.target {
                        LinkTarget::Web(u) => u == uri,
                        LinkTarget::Path { .. } => true,
                        LinkTarget::Osc8 { .. } => false,
                    }
            });
            let ps = pieces(ll, cols, i, j);
            if !dup && !ps.is_empty() {
                let target = LinkTarget::Osc8 { uri: uri.clone() };
                spans.push((LinkMark { target, pieces: ps }, i, j));
            }
        }
        i = j;
    }
    spans.sort_by_key(|(_, s, _)| *s);
    spans.into_iter().map(|(m, _, _)| m).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cells::{LineLike, ScratchCell};

    struct Row {
        text: String,
        wrapped: bool,
        loads: std::rc::Rc<std::cell::Cell<usize>>,
    }
    impl LineLike for Row {
        fn is_wrapped(&self) -> bool {
            self.wrapped
        }
        fn load_cell(&self, x: usize, into: &mut ScratchCell) -> bool {
            self.loads.set(self.loads.get() + 1);
            into.chars = self
                .text
                .chars()
                .nth(x)
                .map(|c| c.to_string())
                .unwrap_or_default();
            into.width = 1;
            true
        }
    }
    struct Buf {
        rows: Vec<Row>,
        loads: std::rc::Rc<std::cell::Cell<usize>>,
    }
    impl BufferLike for Buf {
        fn len(&self) -> usize {
            self.rows.len()
        }
        fn line(&self, y: usize) -> Option<&dyn LineLike> {
            self.rows.get(y).map(|r| r as &dyn LineLike)
        }
    }
    fn buf(rows: &[(&str, bool)]) -> Buf {
        let loads = std::rc::Rc::new(std::cell::Cell::new(0));
        Buf {
            rows: rows
                .iter()
                .map(|&(t, w)| Row {
                    text: t.to_string(),
                    wrapped: w,
                    loads: loads.clone(),
                })
                .collect(),
            loads,
        }
    }

    /// (row, first col, end col, id, uri)
    type Runs = Vec<(usize, usize, usize, &'static str, &'static str)>;

    struct H {
        scanner: LinkScanner,
        runs: Runs,
        ink: u32,
        cwd: Option<&'static str>,
    }
    impl H {
        fn new() -> Self {
            Self {
                scanner: LinkScanner::new(),
                runs: Vec::new(),
                ink: 1,
                cwd: None,
            }
        }
        fn scan(&mut self, b: &Buf, cols: usize, first: usize, last: usize) -> Vec<LinkMark> {
            let runs = self.runs.clone();
            let f = move |r: usize, c: usize| {
                runs.iter()
                    .find(|(rr, a, e, _, _)| *rr == r && c >= *a && c < *e)
                    .map(|(_, _, _, id, uri)| (id.to_string(), uri.to_string()))
            };
            let mut ctx = LinkCtx {
                cwd: self.cwd,
                ink_key: self.ink,
                hyperlink: &f,
            };
            self.scanner.scan(b, cols, first, last, &mut ctx)
        }
    }

    #[test]
    fn a_printed_url_is_one_mark_with_its_cells() {
        let mut h = H::new();
        let b = buf(&[("see https://example.com/1 ok", false)]);
        let m = h.scan(&b, 40, 0, 0);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].target, LinkTarget::Web("https://example.com/1".into()));
        assert_eq!(
            m[0].pieces,
            vec![Piece {
                row: 0,
                col: 4,
                width: 21
            }]
        );
        let b = buf(&[("go https://a.io/x.", false)]);
        let m = h.scan(&b, 40, 0, 0);
        assert_eq!(m[0].target, LinkTarget::Web("https://a.io/x".into()));
    }

    #[test]
    fn a_wrapped_link_has_a_piece_per_row() {
        let mut h = H::new();
        let b = buf(&[("go https://exa", false), ("mple.com/path", true)]);
        let m = h.scan(&b, 14, 0, 1);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].pieces.len(), 2);
        assert_eq!(m[0].pieces[0].col + m[0].pieces[0].width, 14);
        assert_eq!(m[0].pieces[1].row, 1);
        assert_eq!(m[0].pieces[1].col, 0);
    }

    fn three_links() -> Buf {
        buf(&[
            ("https://a.io/1", false),
            ("https://a.io/2", false),
            ("https://a.io/3", false),
            ("", false),
            ("", false),
        ])
    }

    #[test]
    fn an_unchanged_line_is_not_scanned_again() {
        let mut h = H::new();
        let b = three_links();
        let first = h.scan(&b, 20, 0, 4);
        assert_eq!(h.scanner.lines_scanned(), 3);
        let second = h.scan(&b, 20, 0, 4);
        assert_eq!(h.scanner.lines_scanned(), 3);
        assert_eq!(first, second);
        assert_eq!(first.len(), 3);
    }

    #[test]
    fn link_repaints_coalesce_to_one_per_frame() {
        let mut h = H::new();
        let b = three_links();
        h.scan(&b, 20, 0, 4);
        assert_eq!(h.scanner.lines_scanned(), 3);
        // A burst of repaints inside one frame (a colour dragged) drops the cache once, at the frame.
        for _ in 0..5 {
            h.scanner.repaint();
        }
        assert_eq!(h.scanner.repaints_done(), 0);
        h.scan(&b, 20, 0, 4);
        assert_eq!(h.scanner.repaints_done(), 1);
        assert_eq!(h.scanner.lines_scanned(), 6);
        // The next frame with no repaint asked scans nothing and drops nothing.
        h.scan(&b, 20, 0, 4);
        assert_eq!(h.scanner.repaints_done(), 1);
        assert_eq!(h.scanner.lines_scanned(), 6);
    }
    #[test]
    fn only_the_changed_line_is_scanned_again() {
        let mut h = H::new();
        let mut b = three_links();
        h.scan(&b, 20, 0, 4);
        b.rows[1].text = "https://a.io/22".into();
        let m = h.scan(&b, 20, 0, 4);
        assert_eq!(h.scanner.lines_scanned(), 4);
        assert_eq!(m[1].target, LinkTarget::Web("https://a.io/22".into()));
    }

    #[test]
    fn scrolling_does_not_duplicate_marks() {
        let mut h = H::new();
        let b = buf(&[
            ("https://a.io/1", false),
            ("https://a.io/2", false),
            ("https://a.io/3", false),
            ("https://a.io/4", false),
        ]);
        assert_eq!(h.scan(&b, 20, 0, 2).len(), 3);
        let m = h.scan(&b, 20, 1, 3);
        assert_eq!(m.len(), 3);
        assert_eq!(m[0].pieces[0].row, 1);
    }

    #[test]
    fn a_new_ink_scans_everything_again() {
        let mut h = H::new();
        let b = buf(&[
            ("https://a.io/1", false),
            ("https://a.io/2", false),
            ("plain text", false),
        ]);
        h.scan(&b, 20, 0, 2);
        assert_eq!(h.scanner.lines_scanned(), 3);
        h.ink = 2;
        h.scan(&b, 20, 0, 2);
        assert_eq!(h.scanner.lines_scanned(), 6);
    }

    #[test]
    fn a_hidden_tab_scans_nothing_and_catches_up() {
        let mut h = H::new();
        let b = three_links();
        h.scanner.set_hidden(true);
        assert!(h.scan(&b, 20, 0, 4).is_empty());
        assert_eq!(h.scanner.lines_scanned(), 0);
        h.scanner.set_hidden(false);
        assert_eq!(h.scan(&b, 20, 0, 4).len(), 3);
    }

    #[test]
    fn a_burst_of_scrollback_is_not_scanned() {
        let mut h = H::new();
        let loads = std::rc::Rc::new(std::cell::Cell::new(0));
        let b = Buf {
            rows: (0..44)
                .map(|i| Row {
                    text: format!("https://a.io/{i}"),
                    wrapped: false,
                    loads: loads.clone(),
                })
                .collect(),
            loads,
        };
        h.scan(&b, 30, 40, 43);
        assert_eq!(h.scanner.lines_scanned(), 4);
    }

    /// Review #37: a URL Claude's Ink hard-wraps at the edge is one link to the whole address.
    #[test]
    fn a_url_broken_by_hand_at_the_edge_is_one_mark_to_the_whole_address() {
        let mut h = H::new();
        let b = buf(&[("go https://git", false), ("hub.com/x/1234", false)]);
        let m = h.scan(&b, 14, 0, 1);
        assert_eq!(m.len(), 1, "{m:?}");
        assert_eq!(
            m[0].target,
            LinkTarget::Web("https://github.com/x/1234".into()),
            "the click must open the whole address, never the first row's prefix"
        );
        assert_eq!(m[0].pieces.len(), 2);
        assert_eq!((m[0].pieces[1].row, m[0].pieces[1].col), (1, 0));
    }

    /// Review #39: one huge logical line (a megabyte of JSON on one line) is read in a window around
    /// the viewport, not rebuilt whole on every scan.
    #[test]
    fn a_huge_logical_line_is_scanned_in_a_window_not_whole() {
        let mut h = H::new();
        let filler = "aaaaaaaaaaaaaaaaaaa ";
        let mut rows: Vec<(&str, bool)> = (0..6000).map(|_| (filler, true)).collect();
        rows[0].1 = false;
        rows[3000] = ("see https://a.io/xy ", true);
        let b = buf(&rows);
        let m = h.scan(&b, 20, 2995, 3005);
        assert_eq!(m.len(), 1, "{} marks", m.len());
        assert_eq!(m[0].target, LinkTarget::Web("https://a.io/xy".into()));
        assert_eq!(m[0].pieces[0].row, 3000);
        let cells = b.loads.get();
        assert!(
            cells < 20 * 200,
            "{cells} cells read: the whole 6000-row line must not be built for one scan"
        );
        // Scrolling on reads another window, never duplicating a mark.
        let m = h.scan(&b, 20, 2999, 3009);
        assert_eq!(m.len(), 1);
    }

    /// A link that straddles the cut between two pieces of a long line is one whole mark, not two
    /// truncated ones (NEW-F).
    #[test]
    fn a_link_across_the_cut_of_a_long_line_is_one_whole_mark() {
        let mut h = H::new();
        let filler = "aaaaaaaaaaaaaaaaaaa ";
        let mut rows: Vec<(&str, bool)> = (0..200).map(|_| (filler, true)).collect();
        rows[0].1 = false;
        // 20 columns: the first window ends after row 24, so this address crosses rows 24 and 25.
        rows[24] = ("aaaaaaaaaa https://a", true);
        rows[25] = (".io/xyz aaaaaaaaaaa ", true);
        let b = buf(&rows);
        let m = h.scan(&b, 20, 0, 100);
        let webs: Vec<&LinkMark> = m
            .iter()
            .filter(|k| matches!(k.target, LinkTarget::Web(_)))
            .collect();
        assert_eq!(webs.len(), 1, "{webs:?}");
        assert_eq!(webs[0].target, LinkTarget::Web("https://a.io/xyz".into()));
        assert_eq!(webs[0].pieces.len(), 2);
    }

    /// R2: a link longer than the overlap, and one ending exactly at the cut column, are marks too.
    #[test]
    fn a_link_longer_than_the_overlap_or_ending_at_the_cut_column_is_kept() {
        let mut h = H::new();
        let filler = "aaaaaaaaaaaaaaaaaaa ";
        let mk = || -> Vec<(&str, bool)> {
            let mut rows: Vec<(&str, bool)> = (0..200).map(|_| (filler, true)).collect();
            rows[0].1 = false;
            rows
        };
        // Nine rows long, straddling the cut after row 24.
        let mut rows = mk();
        rows[20] = ("aaaaa https://a.io/x", true);
        for r in rows.iter_mut().take(28).skip(21) {
            *r = ("xxxxxxxxxxxxxxxxxxxx", true);
        }
        rows[28] = ("xxxx aaaaaaaaaaaaaaa", true);
        let b = buf(&rows);
        let m = h.scan(&b, 20, 0, 100);
        let webs: Vec<&LinkMark> = m
            .iter()
            .filter(|k| matches!(k.target, LinkTarget::Web(_)))
            .collect();
        assert_eq!(webs.len(), 1, "{webs:?}");
        let LinkTarget::Web(u) = &webs[0].target else {
            unreachable!()
        };
        assert_eq!(u.len(), "https://a.io/x".len() + 7 * 20 + 4, "{u}");
        assert_eq!(webs[0].pieces.len(), 9);
        // Ending on the last column of the last row of the first piece.
        let mut h = H::new();
        let mut rows = mk();
        rows[24] = ("aaaa https://a.io/xy", true);
        rows[25] = (" aaaaaaaaaaaaaaaaaaa", true);
        let b = buf(&rows);
        let m = h.scan(&b, 20, 0, 100);
        let webs: Vec<&LinkMark> = m
            .iter()
            .filter(|k| matches!(k.target, LinkTarget::Web(_)))
            .collect();
        assert_eq!(webs.len(), 1, "{webs:?}");
        assert_eq!(webs[0].target, LinkTarget::Web("https://a.io/xy".into()));
    }

    #[test]
    fn osc8_runs_become_marks() {
        let mut h = H::new();
        h.runs.push((0, 2, 9, "1", "https://x.io/pr/165"));
        let b = buf(&[("  PR #165  x", false)]);
        let m = h.scan(&b, 20, 0, 0);
        assert_eq!(m.len(), 1);
        assert_eq!(
            m[0].target,
            LinkTarget::Osc8 {
                uri: "https://x.io/pr/165".into()
            }
        );
        assert_eq!(
            m[0].pieces,
            vec![Piece {
                row: 0,
                col: 2,
                width: 7
            }]
        );
    }

    #[test]
    fn a_file_uri_label_is_not_a_link() {
        let mut h = H::new();
        h.runs.push((0, 0, 5, "1", "file:///C:/x"));
        let b = buf(&[("label", false)]);
        assert!(h.scan(&b, 20, 0, 0).is_empty());
    }

    #[test]
    fn two_adjacent_hyperlinks_with_different_ids_are_two_marks() {
        let mut h = H::new();
        h.runs.push((0, 0, 3, "1", "https://x.io/a"));
        h.runs.push((0, 3, 6, "2", "https://x.io/a"));
        let b = buf(&[("abcdef", false)]);
        assert_eq!(h.scan(&b, 20, 0, 0).len(), 2);
    }

    #[test]
    fn an_osc8_label_over_a_wrap_has_pieces_on_both_rows() {
        let mut h = H::new();
        h.runs.push((0, 6, 8, "1", "https://x.io/a"));
        h.runs.push((1, 0, 3, "1", "https://x.io/a"));
        let b = buf(&[("abcdefgh", false), ("ijk", true)]);
        let m = h.scan(&b, 8, 0, 1);
        assert_eq!(m.len(), 1);
        assert_eq!(
            m[0].pieces,
            vec![
                Piece {
                    row: 0,
                    col: 6,
                    width: 2
                },
                Piece {
                    row: 1,
                    col: 0,
                    width: 3
                }
            ]
        );
    }

    #[test]
    fn an_osc8_label_cut_by_hand_is_a_mark_per_row() {
        let mut h = H::new();
        h.runs.push((0, 3, 12, "1", "https://x.io/a"));
        h.runs.push((1, 0, 9, "1", "https://x.io/a"));
        let b = buf(&[("at C:\\t\\scra", false), ("tch\\a.exe", false)]);
        let m = h.scan(&b, 12, 0, 1);
        assert_eq!(m.len(), 2, "{m:?}");
        assert!(m.iter().all(|k| k.pieces.len() == 1), "{m:?}");
    }
    #[test]
    fn a_hyperlink_that_repeats_a_web_link_is_one_mark() {
        let mut h = H::new();
        h.runs.push((0, 0, 16, "1", "https://a.io/xyz"));
        let b = buf(&[("https://a.io/xyz", false)]);
        let m = h.scan(&b, 20, 0, 0);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].target, LinkTarget::Web("https://a.io/xyz".into()));
    }

    #[test]
    fn a_marked_path_is_a_file_link_with_its_line() {
        let mut h = H::new();
        let b = buf(&[("Saved \u{27E6}C:\\r\\src\\main.rs:42\u{27E7} now", false)]);
        let m = h.scan(&b, 40, 0, 0);
        assert_eq!(m.len(), 1, "{m:?}");
        assert_eq!(
            m[0].target,
            LinkTarget::Path {
                text: "C:\\r\\src\\main.rs".into(),
                line: Some(42),
                col: None,
                alts: Vec::new(),
            }
        );
        assert_eq!((m[0].pieces[0].col, m[0].pieces[0].width), (6, 21));
    }

    #[test]
    fn a_relative_marked_path_needs_the_tabs_folder() {
        let mut h = H::new();
        let b = buf(&[("\u{27E6}src\\main.rs\u{27E7}", false)]);
        assert!(h.scan(&b, 30, 0, 0).is_empty());
        h.cwd = Some("C:\\w");
        h.scanner.repaint();
        assert_eq!(
            h.scan(&b, 30, 0, 0),
            vec![LinkMark {
                target: LinkTarget::path("src\\main.rs"),
                pieces: vec![Piece {
                    row: 0,
                    col: 0,
                    width: 13
                }],
            }]
        );
    }

    #[test]
    fn a_marked_path_that_starts_above_the_view_is_whole() {
        let mut h = H::new();
        let b = buf(&[
            ("see \u{27E6}C:\\a\\long", false),
            ("  \\b.txt\u{27E7} ok", false),
        ]);
        let m = h.scan(&b, 16, 1, 1);
        assert_eq!(m.len(), 1, "{m:?}");
        assert_eq!(
            m[0].target,
            LinkTarget::Path {
                text: "C:\\a\\long\\b.txt".into(),
                line: None,
                col: None,
                alts: vec!["C:\\a\\long \\b.txt".into()],
            }
        );
        assert_eq!(m[0].pieces.len(), 2);
    }

    /// The anti-goal (spec 2026-11-11): a plain shell's output is no wall of guessed file links again.
    #[test]
    fn a_plain_listing_has_no_file_links() {
        let mut h = H::new();
        h.cwd = Some("C:\\w");
        let b = buf(&[
            (" Directory of C:\\w\\src", false),
            ("11/11/2026  10:00     1,234 main.rs", false),
            ("error: C:\\w\\src\\main.rs:4:2 expected", false),
            ("see https://a.io/x", false),
        ]);
        let m = h.scan(&b, 40, 0, 3);
        assert!(
            m.iter().all(|k| matches!(k.target, LinkTarget::Web(_))),
            "{m:?}"
        );
        assert_eq!(m.len(), 1);
    }

    #[test]
    fn a_web_address_beside_a_marked_path_stays_a_web_link() {
        let mut h = H::new();
        let b = buf(&[("https://a.io/x\u{27E6}C:\\a.rs\u{27E7}", false)]);
        let m = h.scan(&b, 40, 0, 0);
        assert_eq!(m.len(), 2, "{m:?}");
        assert_eq!(m[0].target, LinkTarget::Web("https://a.io/x".into()));
        assert_eq!(m[1].target, LinkTarget::path("C:\\a.rs"));
    }
}

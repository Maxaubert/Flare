//! The link scanner over the viewport (Prism Terminal's termLinkPaint.ts, without the painting).
//! Web links and known paths come from `PathLinks::link_ranges`, OSC 8 links from the per-cell
//! hyperlinks of the buffer (decision D18). Only the viewport is scanned, one logical line at a time,
//! and a line whose text, hyperlinks, ink and cwd are unchanged is not scanned again. No decoration
//! is stored: the marks are rebuilt from the per-line cache on every call.

use crate::cells::{BufferLike, LogicalLine, logical_line};
use crate::osc8::is_web_link;
use crate::path_links::PathLinks;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkTarget {
    Web(String),
    Path { text: String },
    Osc8 { uri: String },
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
    pub paths: &'a mut PathLinks,
    /// The tab id.
    pub owner: &'a str,
    pub cwd: Option<&'a str>,
    pub now_ms: u64,
    /// Changes when the link colour changes: everything is scanned again.
    pub ink_key: u32,
    /// The hyperlink on a cell (absolute row, column) as (id, uri).
    pub hyperlink: &'a dyn Fn(usize, usize) -> Option<(String, String)>,
}

type Link = Option<(String, String)>;

pub struct LinkScanner {
    cache: HashMap<usize, (u64, Vec<LinkMark>)>,
    waiting: HashSet<usize>,
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
            waiting: HashSet::new(),
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

    /// A path answer arrived: the lines that were waiting for one are scanned again.
    pub fn revisit(&mut self) {
        for row in self.waiting.drain() {
            self.cache.remove(&row);
        }
    }

    /// Logical lines actually scanned so far.
    pub fn lines_scanned(&self) -> usize {
        self.scanned
    }

    /// The marks on the logical lines overlapping absolute rows `first..=last`.
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
            self.waiting.clear();
            self.repaints += 1;
        }
        let last = last.min(b.len() - 1);
        let mut out = Vec::new();
        let mut next: HashMap<usize, (u64, Vec<LinkMark>)> = HashMap::new();
        let mut y = first;
        while y <= last {
            let ll = logical_line(b, cols, y);
            let row_end = ll.last;
            let links = line_links(&ll, cols, ctx);
            let blank = ll.text_end == 0 && links.iter().all(Option::is_none);
            if blank {
                self.waiting.remove(&ll.first);
            } else {
                let key = line_key(&ll, cols, &links, ctx);
                let marks = match self.cache.remove(&ll.first) {
                    Some((k, marks)) if k == key => marks,
                    _ => {
                        self.waiting.remove(&ll.first);
                        self.scanned += 1;
                        let marks = scan_line(&ll, cols, &links, ctx);
                        if ctx.paths.take_asked(ctx.owner) {
                            self.waiting.insert(ll.first);
                        }
                        marks
                    }
                };
                out.extend(marks.iter().cloned());
                next.insert(ll.first, (key, marks));
            }
            y = row_end + 1;
        }
        self.waiting.retain(|r| next.contains_key(r));
        self.cache = next;
        out
    }
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
    ctx.cwd.hash(&mut h);
    ll.widths.hash(&mut h);
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

fn scan_line(ll: &LogicalLine, cols: usize, links: &[Link], ctx: &mut LinkCtx) -> Vec<LinkMark> {
    let chars: Vec<char> = ll.text.chars().collect();
    let ranges = ctx
        .paths
        .link_ranges(&ll.text, ctx.cwd, ctx.owner, ctx.now_ms);
    let mut spans: Vec<(LinkMark, usize, usize)> = Vec::new();
    for r in ranges {
        let (s, e) = cell_range(ll, r.start, r.end);
        let ps = pieces(ll, cols, s, e);
        if ps.is_empty() {
            continue;
        }
        let target = match r.path {
            Some(text) => LinkTarget::Path { text },
            None => LinkTarget::Web(chars[r.start..r.end.min(chars.len())].iter().collect()),
        };
        spans.push((LinkMark { target, pieces: ps }, s, e));
    }
    let mut i = 0;
    while i < links.len() {
        let Some(link) = &links[i] else {
            i += 1;
            continue;
        };
        let mut j = i + 1;
        while j < links.len() && links[j].as_ref() == Some(link) {
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
    use crate::path_open::{PathHit, PathKind};

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
                .map(|c| c.to_string())
                .unwrap_or_default();
            into.width = 1;
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
    fn buf(rows: &[(&str, bool)]) -> Buf {
        Buf {
            rows: rows
                .iter()
                .map(|&(t, w)| Row {
                    text: t.to_string(),
                    wrapped: w,
                })
                .collect(),
        }
    }

    /// (row, first col, end col, id, uri)
    type Runs = Vec<(usize, usize, usize, &'static str, &'static str)>;

    struct H {
        paths: PathLinks,
        scanner: LinkScanner,
        runs: Runs,
        ink: u32,
        cwd: Option<&'static str>,
    }
    impl H {
        fn new() -> Self {
            Self {
                paths: PathLinks::new(),
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
                paths: &mut self.paths,
                owner: "t1",
                cwd: self.cwd,
                now_ms: 0,
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
        let b = Buf {
            rows: (0..44)
                .map(|i| Row {
                    text: format!("https://a.io/{i}"),
                    wrapped: false,
                })
                .collect(),
        };
        h.scan(&b, 30, 40, 43);
        assert_eq!(h.scanner.lines_scanned(), 4);
    }

    fn hit() -> PathHit {
        PathHit {
            kind: PathKind::File,
            abs: "C:\\w\\src/main.rs".into(),
        }
    }

    fn answer_all(h: &mut H) {
        h.paths.advance(100);
        while let Some(ask) = h.paths.take_ask() {
            let hits = ask.paths.iter().map(|_| Some(hit())).collect();
            h.paths.answer(&ask, Some(hits), 100);
        }
    }

    #[test]
    fn a_path_that_is_not_yet_known_is_asked_and_the_line_waits() {
        let mut h = H::new();
        h.cwd = Some("C:\\w");
        let b = buf(&[("edit src/main.rs now", false)]);
        assert!(h.scan(&b, 30, 0, 0).is_empty());
        answer_all(&mut h);
        h.scanner.revisit();
        let m = h.scan(&b, 30, 0, 0);
        assert_eq!(m.len(), 1);
        assert_eq!(
            m[0].target,
            LinkTarget::Path {
                text: "src/main.rs".into()
            }
        );
    }

    #[test]
    fn an_unchanged_waiting_line_is_scanned_again_only_after_revisit() {
        let mut h = H::new();
        h.cwd = Some("C:\\w");
        let b = buf(&[("edit src/main.rs now", false)]);
        h.scan(&b, 30, 0, 0);
        answer_all(&mut h);
        h.scan(&b, 30, 0, 0);
        assert_eq!(h.scanner.lines_scanned(), 1);
        h.scanner.revisit();
        h.scan(&b, 30, 0, 0);
        assert_eq!(h.scanner.lines_scanned(), 2);
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
    fn a_hyperlink_that_repeats_a_web_link_is_one_mark() {
        let mut h = H::new();
        h.runs.push((0, 0, 16, "1", "https://a.io/xyz"));
        let b = buf(&[("https://a.io/xyz", false)]);
        let m = h.scan(&b, 20, 0, 0);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].target, LinkTarget::Web("https://a.io/xyz".into()));
    }
}

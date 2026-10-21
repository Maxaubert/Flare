//! OSC 8 hyperlinks (Prism Terminal's termOsc8.ts, #169). A program prints a label and says,
//! in-band, which uri it stands for. The emulator does not keep the link, so Flare follows the
//! stream: where the cursor was at the open and at the close, and the text between. Cells are
//! not characters (rule 21): every cell-to-text step goes through `cells`.

use std::cell::Cell;
use std::rc::Rc;

use crate::cells::{CellInfo, cell_text};
use crate::web_url::is_web_url;

/// Spans kept for one terminal by default.
pub const OSC8_CAP: usize = 300;

/// The uri an OSC 8 sequence carries (empty is the close), taken after the first `;`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Osc8Uri {
    pub uri: String,
}

/// Parses an OSC 8 payload ("params;uri"). `None` when there is no `;` at all.
pub fn parse_osc8(data: &str) -> Option<Osc8Uri> {
    let at = data.find(';')?;
    Some(Osc8Uri {
        uri: data[at + 1..].to_string(),
    })
}

/// Only http and https links are opened.
pub fn is_web_link(uri: &str) -> bool {
    is_web_url(uri)
}

/// The host (with a port when one is given), lowercased; `None` if the address does not parse.
fn host_of(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    let host = parsed.host_str()?.to_ascii_lowercase();
    Some(match parsed.port() {
        Some(port) => format!("{host}:{port}"),
        None => host,
    })
}

/// Where an OSC 8 link goes on a click. A label that shows an address on another host is taken
/// at its word; any other label opens its uri. Hosts, not whole urls.
pub fn osc8_target(uri: &str, shown: Option<&str>) -> String {
    let Some(shown) = shown else {
        return uri.to_string();
    };
    let want = host_of(uri);
    if want.is_some() && want == host_of(shown) {
        uri.to_string()
    } else {
        shown.to_string()
    }
}

#[derive(Debug)]
struct AnchorInner {
    line: Cell<i64>,
    disposed: Cell<bool>,
}

/// Where a span's first row is now. Shared, so the span follows the buffer as it trims.
#[derive(Clone, Debug)]
pub struct Anchor(Rc<AnchorInner>);

impl Anchor {
    pub fn new(line: i64) -> Self {
        Anchor(Rc::new(AnchorInner {
            line: Cell::new(line),
            disposed: Cell::new(false),
        }))
    }

    pub fn line(&self) -> i64 {
        self.0.line.get()
    }

    pub fn set_line(&self, line: i64) {
        self.0.line.set(line);
    }

    pub fn dispose(&self) {
        self.0.disposed.set(true);
    }

    pub fn is_disposed(&self) -> bool {
        self.0.disposed.get()
    }
}

/// One OSC 8 link: from its first cell to the cell just past its last one.
#[derive(Debug)]
pub struct Osc8Span {
    pub anchor: Anchor,
    /// The first cell, on the anchor's row.
    pub x: usize,
    /// How many rows below the anchor the span ends.
    pub rows: usize,
    /// The cell just past the last one, on the end row.
    pub end_x: usize,
    pub uri: String,
    /// What the cells held when the link was printed; the span stands only while they still do.
    pub text: String,
    /// On the alternate screen.
    pub alt: bool,
}

type Pos = (i64, usize);

fn live(s: &Osc8Span) -> bool {
    !s.anchor.is_disposed() && s.anchor.line() >= 0
}

fn start_of(s: &Osc8Span) -> Pos {
    (s.anchor.line(), s.x)
}

fn end_of(s: &Osc8Span) -> Pos {
    (s.anchor.line() + s.rows as i64, s.end_x)
}

fn overlaps(a: &Osc8Span, b: &Osc8Span) -> bool {
    start_of(a) < end_of(b) && start_of(b) < end_of(a)
}

/// The spans kept for a terminal, oldest first.
#[derive(Debug)]
pub struct Osc8Spans {
    spans: Vec<Osc8Span>,
    cap: usize,
}

impl Default for Osc8Spans {
    fn default() -> Self {
        Self::new(OSC8_CAP)
    }
}

impl Osc8Spans {
    pub fn new(cap: usize) -> Self {
        Self {
            spans: Vec::new(),
            cap,
        }
    }

    /// Adds a span. A newer span over the same cells replaces the older one (a TUI redraws its
    /// links in place); past the cap the oldest goes.
    pub fn add(&mut self, span: Osc8Span) {
        let mut kept = Vec::with_capacity(self.spans.len() + 1);
        for old in self.spans.drain(..) {
            if live(&old) && (old.alt != span.alt || !overlaps(&old, &span)) {
                kept.push(old);
            } else {
                old.anchor.dispose();
            }
        }
        kept.push(span);
        self.spans = kept;
        while self.spans.len() > self.cap {
            self.spans.remove(0).anchor.dispose();
        }
    }

    /// The newest span over cell (`line`, `x`) of one screen.
    pub fn at(&self, line: i64, x: usize, alt: bool) -> Option<&Osc8Span> {
        let p: Pos = (line, x);
        self.spans
            .iter()
            .rev()
            .find(|s| s.alt == alt && live(s) && start_of(s) <= p && p < end_of(s))
    }

    /// The spans with a cell on rows `first` to `last` of one screen.
    pub fn overlapping(&self, first: i64, last: i64, alt: bool) -> Vec<&Osc8Span> {
        self.spans
            .iter()
            .filter(|s| {
                s.alt == alt
                    && live(s)
                    && s.anchor.line() <= last
                    && s.anchor.line() + s.rows as i64 >= first
            })
            .collect()
    }

    /// Forgets one screen's spans (`Some`), or all of them (`None`).
    pub fn clear(&mut self, alt: Option<bool>) {
        let mut kept = Vec::with_capacity(self.spans.len());
        for s in self.spans.drain(..) {
            if alt.is_some_and(|a| s.alt != a) {
                kept.push(s);
            } else {
                s.anchor.dispose();
            }
        }
        self.spans = kept;
    }
}

/// A stretch of cells on one buffer row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowPiece {
    pub line: i64,
    pub x: usize,
    pub width: usize,
}

/// The span's cells, one piece per row it crosses.
pub fn span_rows(span: &Osc8Span, cols: usize) -> Vec<RowPiece> {
    (0..=span.rows)
        .filter_map(|r| {
            let from = if r == 0 { span.x } else { 0 };
            let to = if r == span.rows { span.end_x } else { cols };
            (to > from).then(|| RowPiece {
                line: span.anchor.line() + r as i64,
                x: from,
                width: to - from,
            })
        })
        .collect()
}

/// True when any piece shares a cell with any taken piece (a URL match inside an OSC 8 link
/// is that link, painted once).
pub fn pieces_overlap(pieces: &[RowPiece], taken: &[RowPiece]) -> bool {
    pieces.iter().any(|p| {
        taken
            .iter()
            .any(|t| t.line == p.line && t.x < p.x + p.width && p.x < t.x + t.width)
    })
}

/// The char range of the text that cells `from` to `to` of a row cover (a wide character is
/// two cells and one character).
pub fn span_text_range(row: &[CellInfo], from: usize, to: usize) -> (usize, usize) {
    let index = cell_text(row).index;
    let last = index.len() - 1;
    (index[from.min(last)], index[to.min(last)])
}

/// What the span's cells hold now; `None` when a row of it is gone.
pub fn span_text(
    span: &Osc8Span,
    cols: usize,
    cells_of: &dyn Fn(i64) -> Option<Vec<CellInfo>>,
) -> Option<String> {
    let mut text = String::new();
    for piece in span_rows(span, cols) {
        let row = cells_of(piece.line)?;
        let (start, end) = span_text_range(&row, piece.x, piece.x + piece.width);
        let row_text = cell_text(&row).text;
        text.extend(row_text.chars().skip(start).take(end.saturating_sub(start)));
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn span(line: i64, x: usize, rows: usize, end_x: usize, alt: bool, text: &str) -> Osc8Span {
        Osc8Span {
            anchor: Anchor::new(line),
            x,
            rows,
            end_x,
            uri: "https://a.b/c".to_string(),
            text: text.to_string(),
            alt,
        }
    }

    fn cells(s: &str) -> Vec<CellInfo> {
        s.chars()
            .map(|c| CellInfo {
                chars: if c == ' ' {
                    String::new()
                } else {
                    c.to_string()
                },
                width: 1,
            })
            .collect()
    }

    #[test]
    fn osc8_target_label_not_an_address() {
        assert_eq!(
            osc8_target("https://github.com/o/r/pull/165", None),
            "https://github.com/o/r/pull/165"
        );
    }

    #[test]
    fn osc8_target_same_host() {
        assert_eq!(
            osc8_target(
                "https://github.com/o/r/pull/165",
                Some("https://github.com/o/r")
            ),
            "https://github.com/o/r/pull/165"
        );
        assert_eq!(
            osc8_target("https://GitHub.com/x", Some("https://github.com/y")),
            "https://GitHub.com/x"
        );
    }

    #[test]
    fn osc8_target_other_host_opens_the_shown_address() {
        assert_eq!(
            osc8_target("https://evil.example/", Some("https://github.com/org/repo")),
            "https://github.com/org/repo"
        );
        assert_eq!(
            osc8_target("https://github.com:8443/", Some("https://github.com/")),
            "https://github.com/"
        );
    }

    #[test]
    fn osc8_target_unparseable_uri_opens_the_label() {
        assert_eq!(
            osc8_target("https://", Some("https://github.com/")),
            "https://github.com/"
        );
    }

    #[test]
    fn parse_osc8_reads_the_uri() {
        assert_eq!(
            parse_osc8("id=x;https://a.b/c").map(|o| o.uri),
            Some("https://a.b/c".to_string())
        );
        assert_eq!(
            parse_osc8(";https://a.b/c").map(|o| o.uri),
            Some("https://a.b/c".to_string())
        );
    }

    #[test]
    fn parse_osc8_keeps_semicolons_in_the_uri() {
        assert_eq!(
            parse_osc8(";https://a.b/c?x=1;y=2").map(|o| o.uri),
            Some("https://a.b/c?x=1;y=2".to_string())
        );
    }

    #[test]
    fn parse_osc8_empty_uri_is_the_close() {
        assert_eq!(parse_osc8(";").map(|o| o.uri), Some(String::new()));
        assert_eq!(parse_osc8("id=x;").map(|o| o.uri), Some(String::new()));
    }

    #[test]
    fn parse_osc8_not_osc8_is_none() {
        assert_eq!(parse_osc8("garbage"), None);
        assert_eq!(parse_osc8(""), None);
    }

    #[test]
    fn is_web_link_http_https_only() {
        assert!(is_web_link("https://a.b"));
        assert!(is_web_link("HTTP://a.b"));
        assert!(!is_web_link("file:///C:/x"));
        assert!(!is_web_link("javascript:alert(1)"));
        assert!(!is_web_link("ms-settings:display"));
    }

    #[test]
    fn spans_find_inside_and_miss_outside() {
        let mut spans = Osc8Spans::new(OSC8_CAP);
        spans.add(span(5, 10, 0, 19, false, "x"));
        assert_eq!(
            spans.at(5, 10, false).map(|s| s.uri.as_str()),
            Some("https://a.b/c")
        );
        assert!(spans.at(5, 18, false).is_some());
        assert!(spans.at(5, 19, false).is_none());
        assert!(spans.at(5, 9, false).is_none());
        assert!(spans.at(4, 12, false).is_none());
        assert!(spans.at(5, 12, true).is_none());
    }

    #[test]
    fn spans_follow_wraps() {
        let mut spans = Osc8Spans::new(OSC8_CAP);
        spans.add(span(5, 70, 1, 4, false, "x"));
        assert!(spans.at(5, 75, false).is_some());
        assert!(spans.at(6, 2, false).is_some());
        assert!(spans.at(6, 4, false).is_none());
        assert_eq!(spans.overlapping(6, 6, false).len(), 1);
        assert_eq!(spans.overlapping(7, 9, false).len(), 0);
    }

    #[test]
    fn spans_follow_the_anchor() {
        let mut spans = Osc8Spans::new(OSC8_CAP);
        let sp = span(500, 0, 0, 5, false, "x");
        let anchor = sp.anchor.clone();
        spans.add(sp);
        anchor.set_line(400);
        assert!(spans.at(400, 2, false).is_some());
        assert!(spans.at(500, 2, false).is_none());
    }

    #[test]
    fn spans_forget_disposed_anchors() {
        let mut spans = Osc8Spans::new(OSC8_CAP);
        let sp = span(1, 0, 0, 5, false, "x");
        let anchor = sp.anchor.clone();
        spans.add(sp);
        anchor.dispose();
        assert!(spans.at(1, 2, false).is_none());
        assert!(spans.overlapping(0, 10, false).is_empty());
    }

    #[test]
    fn newer_span_replaces_older() {
        let mut spans = Osc8Spans::new(OSC8_CAP);
        let old = span(3, 2, 0, 9, true, "x");
        let old_anchor = old.anchor.clone();
        spans.add(old);
        let new = Osc8Span {
            uri: "https://new/".to_string(),
            ..span(3, 5, 0, 12, true, "x")
        };
        spans.add(new);
        assert_eq!(
            spans.at(3, 6, true).map(|s| s.uri.as_str()),
            Some("https://new/")
        );
        assert!(spans.at(3, 3, true).is_none());
        assert!(old_anchor.is_disposed());
    }

    #[test]
    fn spans_side_by_side_are_kept() {
        let mut spans = Osc8Spans::new(OSC8_CAP);
        spans.add(span(3, 0, 0, 4, true, "x"));
        spans.add(span(3, 4, 0, 8, true, "x"));
        assert_eq!(spans.overlapping(3, 3, true).len(), 2);
    }

    #[test]
    fn spans_cap_drops_the_oldest() {
        let mut spans = Osc8Spans::new(3);
        let first = span(0, 0, 0, 2, false, "x");
        let first_anchor = first.anchor.clone();
        spans.add(first);
        for i in 1..=3 {
            spans.add(span(i, 0, 0, 2, false, "x"));
        }
        assert!(spans.at(0, 1, false).is_none());
        assert!(first_anchor.is_disposed());
        assert!(spans.at(3, 1, false).is_some());
    }

    #[test]
    fn clear_one_screen() {
        let mut spans = Osc8Spans::new(OSC8_CAP);
        spans.add(span(1, 0, 0, 2, true, "x"));
        spans.add(span(1, 0, 0, 2, false, "x"));
        spans.clear(Some(true));
        assert!(spans.at(1, 1, true).is_none());
        assert!(spans.at(1, 1, false).is_some());
        spans.clear(None);
        assert!(spans.at(1, 1, false).is_none());
    }

    #[test]
    fn span_rows_one_piece_per_row() {
        assert_eq!(
            span_rows(&span(5, 70, 2, 4, false, "x"), 80),
            vec![
                RowPiece {
                    line: 5,
                    x: 70,
                    width: 10
                },
                RowPiece {
                    line: 6,
                    x: 0,
                    width: 80
                },
                RowPiece {
                    line: 7,
                    x: 0,
                    width: 4
                },
            ]
        );
        assert_eq!(
            span_rows(&span(2, 3, 0, 8, false, "x"), 80),
            vec![RowPiece {
                line: 2,
                x: 3,
                width: 5
            }]
        );
    }

    #[test]
    fn span_rows_empty_span() {
        assert!(span_rows(&span(2, 3, 0, 3, false, "x"), 80).is_empty());
    }

    #[test]
    fn span_text_reads_the_covered_cells() {
        let mut rows: HashMap<i64, Vec<CellInfo>> = HashMap::new();
        rows.insert(4, cells("see OSC8LABEL here"));
        let sp = span(4, 4, 0, 13, false, "x");
        assert_eq!(
            span_text(&sp, 18, &|l| rows.get(&l).cloned()),
            Some("OSC8LABEL".to_string())
        );
    }

    #[test]
    fn span_text_reads_across_a_wrap() {
        let mut rows: HashMap<i64, Vec<CellInfo>> = HashMap::new();
        rows.insert(0, cells("abcdeOSC"));
        rows.insert(1, cells("8LABEL  "));
        let sp = span(0, 5, 1, 6, false, "x");
        assert_eq!(
            span_text(&sp, 8, &|l| rows.get(&l).cloned()),
            Some("OSC8LABEL".to_string())
        );
    }

    #[test]
    fn span_text_none_when_a_row_is_gone() {
        let sp = span(0, 0, 0, 3, false, "x");
        assert_eq!(span_text(&sp, 8, &|_| None), None);
    }

    #[test]
    fn span_text_wide_character() {
        let mut row = vec![
            CellInfo {
                chars: "界".to_string(),
                width: 2,
            },
            CellInfo {
                chars: String::new(),
                width: 0,
            },
            CellInfo {
                chars: String::new(),
                width: 1,
            },
        ];
        row.extend(cells("PR #165"));
        let sp = span(0, 3, 0, 10, false, "x");
        assert_eq!(
            span_text(&sp, row.len(), &|_| Some(row.clone())),
            Some("PR #165".to_string())
        );
        assert_eq!(span_text_range(&row, 3, 10), (2, 9));
    }

    #[test]
    fn pieces_overlap_true_on_a_shared_cell() {
        let span_pieces = vec![
            RowPiece {
                line: 4,
                x: 70,
                width: 10,
            },
            RowPiece {
                line: 5,
                x: 0,
                width: 12,
            },
        ];
        assert!(pieces_overlap(
            &[RowPiece {
                line: 4,
                x: 70,
                width: 10
            }],
            &span_pieces
        ));
        assert!(pieces_overlap(
            &[RowPiece {
                line: 5,
                x: 11,
                width: 4
            }],
            &span_pieces
        ));
    }

    #[test]
    fn pieces_overlap_false_beside() {
        let span_pieces = vec![
            RowPiece {
                line: 4,
                x: 70,
                width: 10,
            },
            RowPiece {
                line: 5,
                x: 0,
                width: 12,
            },
        ];
        assert!(!pieces_overlap(
            &[RowPiece {
                line: 4,
                x: 60,
                width: 10
            }],
            &span_pieces
        ));
        assert!(!pieces_overlap(
            &[RowPiece {
                line: 5,
                x: 12,
                width: 8
            }],
            &span_pieces
        ));
        assert!(!pieces_overlap(
            &[RowPiece {
                line: 6,
                x: 0,
                width: 12
            }],
            &span_pieces
        ));
        assert!(!pieces_overlap(
            &[RowPiece {
                line: 4,
                x: 70,
                width: 10
            }],
            &[]
        ));
    }
}

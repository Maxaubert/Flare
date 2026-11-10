//! Markdown views (2026-11-10): a `.md` or `.markdown` file in a panel opens compiled, every time, and the header
//! switches it between three views ([`MdView`]). Compiled and Markdown are pages Flare writes itself and shows in the
//! panel's browser as a mini-app of their own; Code is the native text view.
//!
//! The file is untrusted. Flare writes every tag of the page itself from the parser's events (pulldown-cmark, built
//! with no HTML writer), so raw HTML in the file shows as text, a picture shows its alt text, a link has no address
//! (its target shows as a tooltip) unless it is a `#` jump to a heading or a footnote Flare named, and the page has no
//! script at all. Its policy ([`CSP`]) is the first thing in it. Every function here is for a worker (rule 34): a
//! long render stops when `stale` says the panel has moved on.
//!
//! Not in this version: links that leave the page and pictures (each needs its own safety rules: opening an address
//! from an untrusted file, reading local image files).

use std::collections::{HashMap, HashSet};

use pulldown_cmark::{
    Alignment, BlockQuoteKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd,
};

/// How a markdown file shows in its panel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum MdView {
    /// The formatted document (the default, every time a file opens).
    #[default]
    Compiled,
    /// The text with its markdown coloured.
    Markdown,
    /// The native text view, with line numbers and wrap.
    Code,
}

impl MdView {
    /// The three views in the header's order.
    pub const ALL: [MdView; 3] = [MdView::Compiled, MdView::Markdown, MdView::Code];

    /// The name in `panels.json`.
    pub fn name(self) -> &'static str {
        match self {
            MdView::Compiled => "compiled",
            MdView::Markdown => "markdown",
            MdView::Code => "code",
        }
    }

    /// A stored name; anything else is None (read as the default and never written back, rule 40).
    pub fn parse(s: &str) -> Option<MdView> {
        MdView::ALL.into_iter().find(|v| v.name() == s)
    }

    /// The view is a page Flare writes (shown in the panel's browser), not the native text view.
    pub fn is_page(self) -> bool {
        self != MdView::Code
    }

    /// The header control's index.
    pub fn index(self) -> i32 {
        match self {
            MdView::Compiled => 0,
            MdView::Markdown => 1,
            MdView::Code => 2,
        }
    }

    pub fn from_index(i: i32) -> Option<MdView> {
        usize::try_from(i)
            .ok()
            .and_then(|i| MdView::ALL.get(i).copied())
    }
}

/// The panel's look the page follows: light or dark, and the panel's own ground and ink when they are opaque.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Look {
    pub light: bool,
    pub ground: Option<[u8; 3]>,
    pub ink: Option<[u8; 3]>,
}

/// The page's policy: no script, no request of any kind, inline style only and `data:` pictures (none are written).
pub const CSP: &str = "default-src 'none'; style-src 'unsafe-inline'; img-src data:; base-uri 'none'; form-action 'none'";
/// The most HTML a page grows to (bytes); the rest of the file is left out with a note.
pub const PAGE_MAX: usize = 32 * 1024 * 1024;
/// The most elements open at once: deeper nesting (a thousand `>` in a row) shows its text without more boxes.
pub const DEPTH_MAX: usize = 48;
/// The most characters of a link's target in its tooltip.
const TIP_MAX: usize = 500;
/// The most characters of a heading's text used for its anchor, of a picture's alt text and of a code block's language.
const SLUG_MAX: usize = 80;
const ALT_MAX: usize = 1000;
const LANG_MAX: usize = 40;
/// How many parser events pass between two looks at `stale`.
const STALE_EVERY: usize = 2048;
/// The note under a page whose file was cut short.
pub const CUT_NOTE: &str = "Only the start of the file is shown.";

/// The parser's extensions: GitHub's tables, task lists, strikethrough, footnotes and callouts, and a YAML header.
fn options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_GFM
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS
}

/// Text as it may sit in HTML, an element or an attribute. Control characters other than a line break or a tab show as
/// a replacement mark; a carriage return goes.
fn esc_into(out: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            '\r' => {}
            '\n' | '\t' => out.push(c),
            c if c.is_control() => out.push('\u{FFFD}'),
            c => out.push(c),
        }
    }
}

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    esc_into(&mut out, s);
    out
}

/// At most `n` characters of `s`, with an ellipsis when it was longer.
fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n).collect::<String>() + "..."
    }
}

/// A heading's anchor as GitHub makes it: lower case, letters, digits, `-` and `_`, a space as `-`.
fn slug(text: &str) -> String {
    let mut s = String::new();
    for c in text.trim().chars().flat_map(char::to_lowercase) {
        if s.chars().count() >= SLUG_MAX {
            break;
        }
        if c.is_alphanumeric() || c == '-' || c == '_' {
            s.push(c);
        } else if c.is_whitespace() {
            s.push('-');
        }
    }
    s
}

/// The anchors named so far, and for each base the next number to try after it (as GitHub's slugger keeps it), so a
/// file of a million headings of one name costs a million steps, not a million squared.
#[derive(Default)]
struct Names {
    used: HashSet<String>,
    next: HashMap<String, u64>,
}

impl Names {
    /// `base`, or `base-1`, `base-2` and so on: the first that is not taken yet. A number is tried once per base, and
    /// `base-n` has only one base, so every try that fails is a name taken earlier: the work grows with the names.
    fn unique(&mut self, base: String) -> String {
        let base = if base.is_empty() {
            "section".to_string()
        } else {
            base
        };
        if self.used.insert(base.clone()) {
            return base;
        }
        let n = self.next.entry(base.clone()).or_insert(1);
        loop {
            let c = format!("{base}-{n}");
            *n += 1;
            if self.used.insert(c.clone()) {
                return c;
            }
        }
    }
}

/// The anchors Flare names in the page: one per heading in order, one per footnote by its label. Only these are ever
/// the target of an `href`.
struct Anchors {
    headings: Vec<String>,
    notes: HashMap<String, String>,
    ids: HashSet<String>,
}

fn anchors(src: &str, stale: &dyn Fn() -> bool) -> Option<Anchors> {
    let mut names = Names::default();
    let mut notes = HashMap::new();
    let mut texts = Vec::new();
    let mut cur: Option<String> = None;
    for (i, ev) in Parser::new_ext(src, options()).enumerate() {
        if i % STALE_EVERY == 0 && stale() {
            return None;
        }
        match ev {
            Event::Start(Tag::Heading { .. }) => cur = Some(String::new()),
            Event::End(TagEnd::Heading(_)) => texts.push(cur.take().unwrap_or_default()),
            Event::Text(t) | Event::Code(t) => {
                if let Some(c) = cur.as_mut()
                    && c.len() < SLUG_MAX * 4
                {
                    c.push_str(&t);
                }
            }
            Event::Start(Tag::FootnoteDefinition(label)) if !notes.contains_key(label.as_ref()) => {
                let id = names.unique(format!("fn-{}", slug(&label)));
                notes.insert(label.to_string(), id);
            }
            _ => {}
        }
    }
    let mut headings = Vec::with_capacity(texts.len());
    for (i, t) in texts.into_iter().enumerate() {
        if i % STALE_EVERY == 0 && stale() {
            return None;
        }
        headings.push(names.unique(slug(&t)));
    }
    Some(Anchors {
        headings,
        notes,
        ids: names.used,
    })
}

impl Anchors {
    /// The anchor a `#` link jumps to, when Flare named it.
    fn jump(&self, dest: &str) -> Option<&str> {
        let frag = dest.strip_prefix('#')?;
        self.ids
            .get(frag)
            .or_else(|| self.ids.get(&frag.to_lowercase()))
            .map(String::as_str)
    }
}

/// Writes the compiled document: every tag from the parser's events, every piece of text escaped.
struct Emit<'a> {
    out: String,
    anchors: &'a Anchors,
    heading: usize,
    /// The closing text of each open element ("" for one past [`DEPTH_MAX`], which writes no box).
    stack: Vec<&'static str>,
    aligns: Vec<Alignment>,
    in_head: bool,
    cell: usize,
    /// A picture being read: its target, its alt text and how many of its own elements are open.
    image: Option<(String, String, usize)>,
}

fn tip(dest: &str, title: &str) -> String {
    let mut t = clip(dest, TIP_MAX);
    if !title.is_empty() {
        t.push('\n');
        t.push_str(&clip(title, TIP_MAX));
    }
    t
}

impl Emit<'_> {
    fn open(&mut self, html: &str, close: &'static str) {
        if self.stack.len() >= DEPTH_MAX {
            self.stack.push("");
            return;
        }
        self.out.push_str(html);
        self.stack.push(close);
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => self.open("<p>", "</p>"),
            Tag::Heading { level, .. } => {
                let id = self
                    .anchors
                    .headings
                    .get(self.heading)
                    .map_or_else(String::new, |s| esc(s));
                self.heading += 1;
                let (n, close) = match level {
                    HeadingLevel::H1 => (1, "</h1>"),
                    HeadingLevel::H2 => (2, "</h2>"),
                    HeadingLevel::H3 => (3, "</h3>"),
                    HeadingLevel::H4 => (4, "</h4>"),
                    HeadingLevel::H5 => (5, "</h5>"),
                    HeadingLevel::H6 => (6, "</h6>"),
                };
                self.open(&format!("<h{n} id=\"{id}\">"), close);
            }
            Tag::BlockQuote(kind) => {
                let callout = kind.map(|k| match k {
                    BlockQuoteKind::Note => ("note", "Note"),
                    BlockQuoteKind::Tip => ("tip", "Tip"),
                    BlockQuoteKind::Important => ("important", "Important"),
                    BlockQuoteKind::Warning => ("warning", "Warning"),
                    BlockQuoteKind::Caution => ("caution", "Caution"),
                });
                match callout {
                    Some((class, words)) => self.open(
                        &format!(
                            "<blockquote class=\"callout {class}\"><p class=\"callout-title\">{words}</p>"
                        ),
                        "</blockquote>",
                    ),
                    None => self.open("<blockquote>", "</blockquote>"),
                }
            }
            Tag::CodeBlock(kind) => {
                let lang = match kind {
                    pulldown_cmark::CodeBlockKind::Fenced(info) => info
                        .split_whitespace()
                        .next()
                        .map(|l| clip(l, LANG_MAX))
                        .unwrap_or_default(),
                    pulldown_cmark::CodeBlockKind::Indented => String::new(),
                };
                if lang.is_empty() {
                    self.open("<pre class=\"code\"><code>", "</code></pre>");
                } else {
                    self.open(
                        &format!("<pre class=\"code\" data-lang=\"{}\"><code>", esc(&lang)),
                        "</code></pre>",
                    );
                }
            }
            Tag::HtmlBlock => self.open("<pre class=\"raw\">", "</pre>"),
            Tag::List(Some(n)) => self.open(&format!("<ol start=\"{n}\">"), "</ol>"),
            Tag::List(None) => self.open("<ul>", "</ul>"),
            Tag::Item => self.open("<li>", "</li>"),
            Tag::FootnoteDefinition(label) => {
                let id = self
                    .anchors
                    .notes
                    .get(label.as_ref())
                    .map_or_else(String::new, |s| esc(s));
                self.open(
                    &format!(
                        "<div class=\"footnote\" id=\"{id}\"><span class=\"fn-label\">{}</span>",
                        esc(&label)
                    ),
                    "</div>",
                );
            }
            Tag::DefinitionList => self.open("<dl>", "</dl>"),
            Tag::DefinitionListTitle => self.open("<dt>", "</dt>"),
            Tag::DefinitionListDefinition => self.open("<dd>", "</dd>"),
            Tag::Table(aligns) => {
                self.aligns = aligns;
                self.open("<div class=\"table\"><table>", "</tbody></table></div>");
            }
            Tag::TableHead => {
                self.in_head = true;
                self.cell = 0;
                self.open("<thead><tr>", "</tr></thead><tbody>");
            }
            Tag::TableRow => {
                self.cell = 0;
                self.open("<tr>", "</tr>");
            }
            Tag::TableCell => {
                let align = match self.aligns.get(self.cell) {
                    Some(Alignment::Left) => " style=\"text-align:left\"",
                    Some(Alignment::Center) => " style=\"text-align:center\"",
                    Some(Alignment::Right) => " style=\"text-align:right\"",
                    _ => "",
                };
                self.cell += 1;
                if self.in_head {
                    self.open(&format!("<th{align}>"), "</th>");
                } else {
                    self.open(&format!("<td{align}>"), "</td>");
                }
            }
            Tag::Emphasis => self.open("<em>", "</em>"),
            Tag::Strong => self.open("<strong>", "</strong>"),
            Tag::Strikethrough => self.open("<del>", "</del>"),
            Tag::Superscript => self.open("<sup>", "</sup>"),
            Tag::Subscript => self.open("<sub>", "</sub>"),
            Tag::Link {
                dest_url, title, ..
            } => {
                let words = esc(&tip(&dest_url, &title));
                match self.anchors.jump(&dest_url) {
                    Some(id) => {
                        let id = esc(id);
                        self.open(&format!("<a href=\"#{id}\" title=\"{words}\">"), "</a>");
                    }
                    None => self.open(&format!("<a class=\"away\" title=\"{words}\">"), "</a>"),
                }
            }
            Tag::Image { dest_url, .. } => {
                self.image = Some((dest_url.to_string(), String::new(), 1));
            }
            Tag::MetadataBlock(_) => self.open("<pre class=\"meta\">", "</pre>"),
        }
    }

    fn end(&mut self, end: TagEnd) {
        if end == TagEnd::TableHead {
            self.in_head = false;
        }
        if let Some(close) = self.stack.pop() {
            self.out.push_str(close);
        }
    }

    /// A picture's own events: its alt text is kept, and at its end the alt text is written in its place.
    fn in_image(&mut self, ev: &Event<'_>) -> bool {
        let Some((dest, alt, open)) = self.image.as_mut() else {
            return false;
        };
        match ev {
            Event::Start(_) => *open += 1,
            Event::End(_) => {
                *open -= 1;
                if *open == 0 {
                    let words = if alt.trim().is_empty() {
                        "picture".to_string()
                    } else {
                        clip(alt.trim(), ALT_MAX)
                    };
                    let title = esc(&format!("Picture: {}", clip(dest, TIP_MAX)));
                    let words = esc(&words);
                    self.image = None;
                    self.out.push_str(&format!(
                        "<span class=\"pic\" title=\"{title}\">{words}</span>"
                    ));
                }
            }
            Event::Text(t) | Event::Code(t) if alt.len() < ALT_MAX * 4 => alt.push_str(t),
            _ => {}
        }
        true
    }

    fn event(&mut self, ev: Event<'_>) {
        if self.in_image(&ev) {
            return;
        }
        match ev {
            Event::Start(tag) => self.start(tag),
            Event::End(end) => self.end(end),
            Event::Text(t) | Event::Html(t) => esc_into(&mut self.out, &t),
            Event::Code(t) => {
                self.out.push_str("<code>");
                esc_into(&mut self.out, &t);
                self.out.push_str("</code>");
            }
            Event::InlineMath(t) | Event::DisplayMath(t) => {
                self.out.push_str("<code class=\"math\">");
                esc_into(&mut self.out, &t);
                self.out.push_str("</code>");
            }
            Event::InlineHtml(t) => {
                self.out.push_str("<span class=\"raw\">");
                esc_into(&mut self.out, &t);
                self.out.push_str("</span>");
            }
            Event::FootnoteReference(label) => match self.anchors.notes.get(label.as_ref()) {
                Some(id) => {
                    let (id, label) = (esc(id), esc(&label));
                    self.out.push_str(&format!(
                        "<sup class=\"fnref\"><a href=\"#{id}\">{label}</a></sup>"
                    ));
                }
                None => {
                    self.out.push_str("<sup>[");
                    esc_into(&mut self.out, &label);
                    self.out.push_str("]</sup>");
                }
            },
            Event::SoftBreak => self.out.push('\n'),
            Event::HardBreak => self.out.push_str("<br>"),
            Event::Rule => self.out.push_str("<hr>"),
            Event::TaskListMarker(done) => self.out.push_str(if done {
                "<span class=\"task done\">\u{2611}</span>"
            } else {
                "<span class=\"task\">\u{2610}</span>"
            }),
        }
    }
}

/// The compiled document's body, at most `max` bytes (then the rest of the file is left out). None when `stale`.
fn compiled_body(src: &str, max: usize, stale: &dyn Fn() -> bool) -> Option<(String, bool)> {
    let anchors = anchors(src, stale)?;
    let mut e = Emit {
        out: String::with_capacity(src.len() + src.len() / 2),
        anchors: &anchors,
        heading: 0,
        stack: Vec::new(),
        aligns: Vec::new(),
        in_head: false,
        cell: 0,
        image: None,
    };
    let mut full = false;
    for (i, ev) in Parser::new_ext(src, options()).enumerate() {
        if i % STALE_EVERY == 0 && stale() {
            return None;
        }
        if e.out.len() > max {
            full = true;
            break;
        }
        e.event(ev);
    }
    while let Some(close) = e.stack.pop() {
        e.out.push_str(close);
    }
    Some((e.out, full))
}

/// The length of a list item's marker at the start of `s` (`-`, `*`, `+`, `1.`, `1)`), 0 when there is none.
fn marker_len(s: &str) -> usize {
    let b = s.as_bytes();
    match b.first() {
        Some(b'-' | b'*' | b'+') => 1,
        Some(c) if c.is_ascii_digit() => {
            let d = b.iter().take_while(|c| c.is_ascii_digit()).count();
            if matches!(b.get(d), Some(b'.' | b')')) {
                d + 1
            } else {
                0
            }
        }
        _ => 0,
    }
}

/// The coloured stretches of the source: byte ranges and their class, ordered so an outer one comes before the ones
/// inside it.
fn source_spans(src: &str, stale: &dyn Fn() -> bool) -> Option<Vec<(usize, usize, &'static str)>> {
    let mut spans = Vec::new();
    for (i, (ev, r)) in Parser::new_ext(src, options())
        .into_offset_iter()
        .enumerate()
    {
        if i % STALE_EVERY == 0 && stale() {
            return None;
        }
        let class = match ev {
            Event::Start(Tag::Heading { .. }) => "h",
            Event::Start(Tag::Emphasis) => "em",
            Event::Start(Tag::Strong) => "st",
            Event::Start(Tag::Strikethrough) => "del",
            Event::Start(Tag::CodeBlock(_)) | Event::Code(_) => "code",
            Event::Start(Tag::Link { .. } | Tag::Image { .. }) | Event::FootnoteReference(_) => {
                "link"
            }
            Event::Start(Tag::BlockQuote(_)) => "quote",
            Event::Start(Tag::HtmlBlock) | Event::InlineHtml(_) => "raw",
            Event::Start(Tag::MetadataBlock(_)) => "meta",
            Event::Rule | Event::TaskListMarker(_) => "mark",
            Event::Start(Tag::Item) => {
                let n = src.get(r.start..).map_or(0, marker_len);
                if n > 0 {
                    spans.push((r.start, r.start + n, "mark"));
                }
                continue;
            }
            _ => continue,
        };
        // A block's range takes its line break with it; the colour stops at the end of its text.
        let end = src
            .get(r.start..r.end)
            .map_or(r.end, |s| r.start + s.trim_end_matches(['\r', '\n']).len());
        spans.push((r.start, end, class));
    }
    spans.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
    Some(spans)
}

/// The source with its stretches wrapped in spans: every character of the file, escaped, in order.
fn source_body(src: &str, stale: &dyn Fn() -> bool) -> Option<String> {
    let spans = source_spans(src, stale)?;
    let mut out = String::with_capacity(src.len() + src.len() / 2);
    let mut ends: Vec<usize> = Vec::new();
    let mut pos = 0;
    for (k, &(s, e, class)) in spans.iter().enumerate() {
        if k % STALE_EVERY == 0 && stale() {
            return None;
        }
        // A stretch that ended before this one starts is closed first.
        while let Some(&top) = ends.last() {
            if top > s {
                break;
            }
            esc_into(&mut out, &src[pos..top]);
            pos = top;
            out.push_str("</span>");
            ends.pop();
        }
        let e = ends.last().map_or(e, |&t| e.min(t)).min(src.len());
        if s < pos || e <= s || !src.is_char_boundary(s) || !src.is_char_boundary(e) {
            continue;
        }
        esc_into(&mut out, &src[pos..s]);
        pos = s;
        out.push_str("<span class=\"");
        out.push_str(class);
        out.push_str("\">");
        ends.push(e);
    }
    while let Some(top) = ends.pop() {
        esc_into(&mut out, &src[pos..top]);
        pos = top;
        out.push_str("</span>");
    }
    esc_into(&mut out, &src[pos..]);
    Some(out)
}

fn hex(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

/// The page's colours: the panel's ground and ink when it gave them, and inks that read on it.
fn vars(look: Look) -> String {
    let (scheme, bg, ink, accent, mark, code, raw, note, tip, important, warning, caution) =
        if look.light {
            (
                "light", "#ffffff", "#1f2328", "#0969da", "#8250df", "#953800", "#9a6700",
                "#0969da", "#1a7f37", "#8250df", "#9a6700", "#cf222e",
            )
        } else {
            (
                "dark", "#1e1f22", "#dcdfe4", "#6cb6ff", "#d2a8ff", "#f0b37e", "#e3b341",
                "#4493f8", "#3fb950", "#ab7df8", "#d29922", "#f85149",
            )
        };
    let bg = look.ground.map_or_else(|| bg.to_string(), hex);
    let ink = look.ink.map_or_else(|| ink.to_string(), hex);
    format!(
        ":root{{color-scheme:{scheme};--bg:{bg};--ink:{ink};--accent:{accent};--mark:{mark};--code:{code};--raw:{raw};\
--note:{note};--tip:{tip};--important:{important};--warning:{warning};--caution:{caution};\
--dim:color-mix(in srgb,var(--ink) 62%,var(--bg));--rule:color-mix(in srgb,var(--ink) 18%,var(--bg));\
--soft:color-mix(in srgb,var(--ink) 7%,var(--bg))}}"
    )
}

/// The page's style: a document that reads well in a narrow panel (headings, lists, tables, code, quotes, task
/// lists), and the Markdown view's colours.
const STYLE: &str = "\
html{background:var(--bg)}\
body{margin:0;padding:18px 22px 40px;color:var(--ink);background:var(--bg);font:14px/1.6 \"Segoe UI Variable Text\",\"Segoe UI\",system-ui,sans-serif;overflow-wrap:anywhere}\
main{max-width:880px;margin:0 auto}\
main>:first-child{margin-top:0}\
h1,h2,h3,h4,h5,h6{line-height:1.25;margin:1.4em 0 .6em;font-weight:600}\
h1{font-size:1.8em;padding-bottom:.3em;border-bottom:1px solid var(--rule)}\
h2{font-size:1.4em;padding-bottom:.3em;border-bottom:1px solid var(--rule)}\
h3{font-size:1.18em}h4{font-size:1.05em}h5{font-size:.95em}h6{font-size:.9em;color:var(--dim)}\
p,ul,ol,dl,pre,blockquote,.table{margin:0 0 1em}\
ul,ol{padding-left:1.8em}li+li{margin-top:.25em}li>ul,li>ol{margin:.25em 0 0}li>p{margin:0 0 .5em}\
li:has(>.task),li:has(>p>.task){list-style:none}\
.task{display:inline-block;width:1.4em;margin-left:-1.4em;color:var(--dim)}.task.done{color:var(--accent)}\
a{color:var(--accent);text-decoration:none}a[href]:hover{text-decoration:underline}\
a.away{text-decoration:underline dotted;text-underline-offset:2px;cursor:help}\
code,pre{font-family:\"Cascadia Mono\",\"Cascadia Code\",Consolas,monospace;font-size:.88em}\
code{background:var(--soft);padding:.12em .35em;border-radius:4px}\
pre.code,pre.raw,pre.meta{position:relative;background:var(--soft);padding:12px 14px;border-radius:6px;overflow:auto;line-height:1.45}\
pre code{background:none;padding:0;font-size:1em}\
pre[data-lang]::before{content:attr(data-lang);position:absolute;top:3px;right:8px;font-size:.78em;color:var(--dim)}\
pre.raw,.raw{color:var(--raw)}span.raw{font-family:\"Cascadia Mono\",Consolas,monospace;font-size:.88em}\
pre.meta{color:var(--dim)}\
blockquote{margin-left:0;padding:0 1em;color:var(--dim);border-left:3px solid var(--rule)}\
blockquote.callout{--tone:var(--note);color:var(--ink);border-left-color:var(--tone)}\
.callout.tip{--tone:var(--tip)}.callout.important{--tone:var(--important)}.callout.warning{--tone:var(--warning)}.callout.caution{--tone:var(--caution)}\
.callout-title{font-weight:600;color:var(--tone);margin:0 0 .3em}\
hr{border:0;border-top:1px solid var(--rule);margin:1.6em 0}\
.table{overflow-x:auto}table{border-collapse:collapse}\
th,td{border:1px solid var(--rule);padding:6px 12px;vertical-align:top}th{background:var(--soft);font-weight:600}\
tbody tr:nth-child(2n){background:color-mix(in srgb,var(--soft) 55%,transparent)}\
.pic{display:inline-block;padding:0 .45em;border:1px dashed var(--rule);border-radius:4px;color:var(--dim);font-size:.9em}\
.footnote{font-size:.9em;color:var(--dim);border-top:1px solid var(--rule);padding-top:.5em;margin-top:1.5em}\
.fn-label{font-weight:600;margin-right:.5em}\
dt{font-weight:600}dd{margin:0 0 .5em 1.5em}del{color:var(--dim)}\
.cut{color:var(--dim);font-style:italic;border-top:1px solid var(--rule);padding-top:.6em;margin-top:1.5em}\
::selection{background:color-mix(in srgb,var(--accent) 35%,transparent)}\
body.source{padding:12px 16px 32px}\
pre.src{margin:0;white-space:pre-wrap;overflow-wrap:anywhere;font-size:13px;line-height:1.55}\
.src .h{color:var(--accent);font-weight:600}.src .em{font-style:italic}.src .st{font-weight:700}\
.src .del{text-decoration:line-through;color:var(--dim)}.src .code{color:var(--code)}.src .link{color:var(--accent)}\
.src .quote{color:var(--dim)}.src .raw{color:var(--raw)}.src .mark{color:var(--mark);font-weight:600}.src .meta{color:var(--dim)}";

/// The page around a body: the policy first, then the look.
fn page(title: &str, look: Look, class: &str, body: &str, note: bool) -> String {
    let mut out = String::with_capacity(body.len() + STYLE.len() + 1024);
    out.push_str("<!doctype html><meta charset=\"utf-8\"><meta http-equiv=\"Content-Security-Policy\" content=\"");
    out.push_str(CSP);
    out.push_str("\"><meta http-equiv=\"x-dns-prefetch-control\" content=\"off\"><meta name=\"referrer\" content=\"no-referrer\">\n<meta name=\"color-scheme\" content=\"");
    out.push_str(if look.light { "light" } else { "dark" });
    out.push_str("\"><title>");
    esc_into(&mut out, &clip(title, 200));
    out.push_str("</title>\n<style>");
    out.push_str(&vars(look));
    out.push_str(STYLE);
    out.push_str("</style>\n<body class=\"");
    out.push_str(class);
    out.push_str("\">");
    if class == "source" {
        out.push_str("<pre class=\"src\">");
        out.push_str(body);
        out.push_str("</pre>");
    } else {
        out.push_str("<main>");
        out.push_str(body);
        out.push_str("</main>");
    }
    if note {
        out.push_str("<p class=\"cut\">");
        out.push_str(CUT_NOTE);
        out.push_str("</p>");
    }
    out.push_str("</body>\n");
    out
}

/// The page of `view` for markdown `src` named `title`; `cut` says the file was longer than what was read. None for the
/// Code view, and when `stale` says the panel moved on.
pub fn page_for(
    view: MdView,
    src: &str,
    title: &str,
    look: Look,
    cut: bool,
    stale: &dyn Fn() -> bool,
) -> Option<String> {
    match view {
        MdView::Compiled => {
            let (body, full) = compiled_body(src, PAGE_MAX, stale)?;
            Some(page(title, look, "md", &body, cut || full))
        }
        MdView::Markdown => {
            let body = source_body(src, stale)?;
            Some(page(title, look, "source", &body, cut))
        }
        MdView::Code => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn never() -> bool {
        false
    }

    fn compiled(src: &str) -> String {
        page_for(
            MdView::Compiled,
            src,
            "a.md",
            Look::default(),
            false,
            &never,
        )
        .unwrap()
    }

    fn source(src: &str) -> String {
        page_for(
            MdView::Markdown,
            src,
            "a.md",
            Look::default(),
            false,
            &never,
        )
        .unwrap()
    }

    /// The body alone (the style holds words such as `attr(`, never a tag).
    fn body(page: &str) -> &str {
        let at = page.find("<body").unwrap();
        &page[at..]
    }

    #[test]
    fn a_view_reads_back_by_its_name_and_an_unknown_one_is_none() {
        for v in MdView::ALL {
            assert_eq!(MdView::parse(v.name()), Some(v));
            assert_eq!(MdView::from_index(v.index()), Some(v));
        }
        assert_eq!(MdView::parse("hologram"), None);
        assert_eq!(MdView::parse("Compiled"), None);
        assert_eq!(MdView::from_index(3), None);
        assert_eq!(MdView::from_index(-1), None);
        assert_eq!(MdView::default(), MdView::Compiled);
        assert!(
            MdView::Compiled.is_page() && MdView::Markdown.is_page() && !MdView::Code.is_page()
        );
    }

    #[test]
    fn the_policy_comes_first_and_lets_in_no_script_and_no_request() {
        for p in [compiled("# Hi"), source("# Hi")] {
            assert!(p.starts_with(
                "<!doctype html><meta charset=\"utf-8\"><meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; style-src 'unsafe-inline'; img-src data:;"
            ));
            assert!(!p.contains("<script"), "no script anywhere");
            assert!(!p.contains("<img"), "no picture element");
            assert!(!p.contains("src="), "nothing to fetch");
        }
        assert!(
            !CSP.contains("script-src"),
            "default-src 'none' refuses every script"
        );
    }

    #[test]
    fn raw_html_in_the_file_shows_as_text() {
        let src = "<script>alert(1)</script>\n\nText <img src=x onerror=alert(1)> and <b onclick=x>bold</b>.\n\n<iframe src=\"https://e.example\"></iframe>\n";
        for p in [compiled(src), source(src)] {
            let b = body(&p);
            assert!(!b.contains("<script"), "{b}");
            assert!(!b.contains("<img"), "{b}");
            assert!(!b.contains("<b "), "{b}");
            assert!(!b.contains("<iframe"), "{b}");
            assert!(b.contains("&lt;script&gt;alert(1)&lt;/script&gt;"), "{b}");
            assert!(b.contains("&lt;img src=x onerror=alert(1)&gt;"), "{b}");
        }
    }

    #[test]
    fn a_link_has_no_address_and_shows_its_target_as_a_tooltip() {
        let p = compiled(
            "[site](https://e.example/x \"Hi\") and [run](javascript:alert(1)) and <https://auto.example>\n",
        );
        let b = body(&p);
        assert!(!b.contains("href"), "{b}");
        assert!(
            b.contains("<a class=\"away\" title=\"https://e.example/x\nHi\">site</a>"),
            "{b}"
        );
        assert!(b.contains("title=\"javascript:alert(1)\""), "{b}");
        assert!(b.contains("title=\"https://auto.example\""), "{b}");
    }

    #[test]
    fn a_jump_to_a_heading_flare_named_works_and_any_other_hash_does_not() {
        let p = compiled(
            "# Install it\n\nSee [below](#usage), [gone](#nowhere), [case](#Install-It).\n\n## Usage\n\n## Usage\n",
        );
        let b = body(&p);
        assert!(b.contains("<h1 id=\"install-it\">Install it</h1>"), "{b}");
        assert!(b.contains("<h2 id=\"usage\">Usage</h2>"), "{b}");
        assert!(
            b.contains("<h2 id=\"usage-1\">Usage</h2>"),
            "two of a name differ"
        );
        assert!(
            b.contains("<a href=\"#usage\" title=\"#usage\">below</a>"),
            "{b}"
        );
        assert!(
            b.contains("<a href=\"#install-it\""),
            "a jump in another case finds it"
        );
        assert!(
            b.contains("<a class=\"away\" title=\"#nowhere\">gone</a>"),
            "{b}"
        );
    }

    #[test]
    fn headings_of_one_name_number_in_order_around_names_already_taken() {
        let b = compiled("## a-1\n\n## a\n\n## a\n\n## a\n\n## a-2\n\n#\n\n#\n");
        let b = body(&b);
        for want in [
            "<h2 id=\"a-1\">a-1</h2>",
            "<h2 id=\"a\">a</h2>",
            "<h2 id=\"a-2\">a</h2>",
            "<h2 id=\"a-3\">a</h2>",
            "<h2 id=\"a-2-1\">a-2</h2>",
            "<h1 id=\"section\"></h1>",
            "<h1 id=\"section-1\"></h1>",
        ] {
            assert!(b.contains(want), "{want} in {b}");
        }
    }

    #[test]
    fn many_headings_of_one_name_get_their_anchors_in_a_time_that_grows_with_the_file() {
        // Each name used to be found by trying base-1, base-2 and so on from 1: a file of n empty headings took n
        // squared steps, and a 4 MB file of `#` lines held its worker for good.
        let src = "#\n".repeat(60_000);
        let started = std::time::Instant::now();
        let a = anchors(&src, &never).unwrap();
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "took {:?}",
            started.elapsed()
        );
        assert_eq!(a.headings.len(), 60_000);
        assert_eq!(a.headings[0], "section");
        assert_eq!(a.headings[59_999], "section-59999");
        // Footnote labels that differ only in punctuation share the slug `fn-a`.
        let label = |i: u32| -> String {
            (0..16)
                .map(|b| if i >> b & 1 == 1 { '!' } else { '?' })
                .collect()
        };
        let notes: String = (0..20_000)
            .map(|i| format!("[^a{}]: x\n\n", label(i)))
            .collect();
        let started = std::time::Instant::now();
        let a = anchors(&notes, &never).unwrap();
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "took {:?}",
            started.elapsed()
        );
        assert_eq!(a.notes.len(), 20_000);
        assert!(a.ids.contains("fn-a") && a.ids.contains("fn-a-19999"));
    }

    #[test]
    fn naming_the_headings_stops_when_the_panel_moved_on() {
        let src = "#\n".repeat(3 * STALE_EVERY);
        // Every look the parse makes says "go on"; the first look after it says "stop".
        let parse_looks = Parser::new_ext(&src, options())
            .count()
            .div_ceil(STALE_EVERY);
        let looks = std::cell::Cell::new(0);
        let stale = || {
            looks.set(looks.get() + 1);
            looks.get() > parse_looks
        };
        assert!(anchors(&src, &stale).is_none());
    }

    #[test]
    fn a_heading_cannot_name_an_anchor_that_breaks_out() {
        let p = compiled("# a\" onmouseover=\"x <b>\n");
        let b = body(&p);
        assert!(b.contains("<h1 id=\"a-onmouseoverx\">"), "{b}");
        assert!(!b.contains("onmouseover=\"x"), "{b}");
        assert!(!b.contains("<b>"), "{b}");
    }

    #[test]
    fn a_picture_shows_its_alt_text() {
        let p =
            compiled("![a *cat* on a mat](C:/secret/cat.png) and ![](https://e.example/p.png)\n");
        let b = body(&p);
        assert!(!b.contains("<img"), "{b}");
        assert!(
            b.contains(
                "<span class=\"pic\" title=\"Picture: C:/secret/cat.png\">a cat on a mat</span>"
            ),
            "{b}"
        );
        assert!(
            b.contains(">picture</span>"),
            "an empty alt says picture: {b}"
        );
    }

    #[test]
    fn a_code_fence_language_cannot_break_out_of_its_attribute() {
        let p = compiled("```\"><script>x</script>\nlet a = 1 < 2;\n```\n");
        let b = body(&p);
        assert!(!b.contains("<script"), "{b}");
        assert!(
            b.contains("data-lang=\"&quot;&gt;&lt;script&gt;x&lt;/script&gt;\""),
            "{b}"
        );
        assert!(b.contains("let a = 1 &lt; 2;"), "{b}");
    }

    #[test]
    fn the_document_has_lists_tables_tasks_quotes_and_footnotes() {
        let src = "- [x] done\n- [ ] todo\n\n3. three\n4. four\n\n| a | b |\n|:--|--:|\n| 1 | 2 |\n\n> quoted\n\n> [!WARNING]\n> careful\n\nNote[^1].\n\n[^1]: The note.\n\n---\n\n~~old~~ **bold** *it* `code`\n";
        let b = compiled(src);
        let b = body(&b);
        assert!(
            b.contains("<span class=\"task done\">\u{2611}</span>"),
            "{b}"
        );
        assert!(b.contains("<span class=\"task\">\u{2610}</span>"), "{b}");
        assert!(b.contains("<ol start=\"3\">"), "{b}");
        assert!(b.contains("<thead><tr><th style=\"text-align:left\">a</th><th style=\"text-align:right\">b</th></tr></thead><tbody><tr><td style=\"text-align:left\">1</td>"), "{b}");
        assert!(b.contains("</tbody></table></div>"), "{b}");
        assert!(b.contains("<blockquote>"), "{b}");
        assert!(
            b.contains(
                "<blockquote class=\"callout warning\"><p class=\"callout-title\">Warning</p>"
            ),
            "{b}"
        );
        assert!(
            b.contains("<sup class=\"fnref\"><a href=\"#fn-1\">1</a></sup>"),
            "{b}"
        );
        assert!(b.contains("<div class=\"footnote\" id=\"fn-1\">"), "{b}");
        assert!(b.contains("<hr>"), "{b}");
        assert!(
            b.contains("<del>old</del> <strong>bold</strong> <em>it</em> <code>code</code>"),
            "{b}"
        );
    }

    #[test]
    fn deep_nesting_writes_no_more_than_the_cap_of_boxes() {
        let src = format!("{}deep\n", "> ".repeat(500));
        let b = compiled(&src);
        let b = body(&b);
        assert!(b.matches("<blockquote>").count() <= DEPTH_MAX);
        assert_eq!(
            b.matches("<blockquote>").count(),
            b.matches("</blockquote>").count(),
            "every box opened is closed"
        );
        assert!(b.contains("deep"));
    }

    #[test]
    fn a_page_past_its_size_leaves_the_rest_out_and_closes_what_it_opened() {
        let src = "- item\n".repeat(1000);
        let (out, full) = compiled_body(&src, 200, &never).unwrap();
        assert!(full);
        assert!(out.len() < 400, "{}", out.len());
        assert_eq!(out.matches("<li>").count(), out.matches("</li>").count());
        assert!(out.ends_with("</ul>"), "{out}");
    }

    #[test]
    fn a_file_that_was_cut_says_so_under_the_page() {
        let p = page_for(
            MdView::Compiled,
            "# a",
            "a.md",
            Look::default(),
            true,
            &never,
        )
        .unwrap();
        assert!(p.contains(CUT_NOTE));
        assert!(!compiled("# a").contains(CUT_NOTE));
    }

    #[test]
    fn the_markdown_view_colours_the_syntax_and_keeps_every_character() {
        let src = "# Title\n\nSome *em* and **strong** with `code` and [a](#x).\n\n- one\n1. two\n> quote <b>\n";
        let p = source(src);
        let b = body(&p);
        assert!(b.contains("<span class=\"h\"># Title</span>"), "{b}");
        assert!(b.contains("<span class=\"em\">*em*</span>"), "{b}");
        assert!(b.contains("<span class=\"st\">**strong**</span>"), "{b}");
        assert!(b.contains("<span class=\"code\">`code`</span>"), "{b}");
        assert!(b.contains("<span class=\"link\">[a](#x)</span>"), "{b}");
        assert!(b.contains("<span class=\"mark\">-</span> one"), "{b}");
        assert!(b.contains("<span class=\"mark\">1.</span> two"), "{b}");
        // Every character of the file is there once the spans are taken away.
        let inner = &b[b.find("<pre class=\"src\">").unwrap() + 17..b.find("</pre>").unwrap()];
        let mut text = String::new();
        let mut in_tag = false;
        for c in inner.chars() {
            match c {
                '<' => in_tag = true,
                '>' if in_tag => in_tag = false,
                c if !in_tag => text.push(c),
                _ => {}
            }
        }
        let text = text
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&#39;", "'")
            .replace("&amp;", "&");
        assert_eq!(text, src);
    }

    #[test]
    fn the_markdown_view_of_a_crlf_file_drops_only_the_carriage_returns() {
        let b = source("# A\r\n\r\ntext\r\n");
        assert!(
            body(&b).contains("<span class=\"h\"># A</span>\n\ntext\n"),
            "{b}"
        );
    }

    #[test]
    fn a_render_stops_when_the_panel_moved_on() {
        let stale = || true;
        let src = "# a\n".repeat(10);
        assert!(page_for(MdView::Compiled, &src, "a", Look::default(), false, &stale).is_none());
        assert!(page_for(MdView::Markdown, &src, "a", Look::default(), false, &stale).is_none());
        assert!(page_for(MdView::Code, &src, "a", Look::default(), false, &never).is_none());
    }

    #[test]
    fn the_page_follows_the_panels_look() {
        let dark = compiled("x");
        assert!(
            dark.contains("color-scheme:dark;--bg:#1e1f22"),
            "the dark default"
        );
        let light = page_for(
            MdView::Compiled,
            "x",
            "a",
            Look {
                light: true,
                ground: Some([250, 250, 249]),
                ink: Some([17, 17, 17]),
            },
            false,
            &never,
        )
        .unwrap();
        assert!(
            light.contains("color-scheme:light;--bg:#fafaf9;--ink:#111111"),
            "the panel's own ground and ink"
        );
        assert!(light.contains("<meta name=\"color-scheme\" content=\"light\">"));
    }

    #[test]
    fn the_title_cannot_break_out_of_the_page() {
        let p = page_for(
            MdView::Compiled,
            "x",
            "</title><script>x</script>",
            Look::default(),
            false,
            &never,
        )
        .unwrap();
        assert!(!p.contains("<script"));
        assert!(p.contains("<title>&lt;/title&gt;&lt;script&gt;"));
    }

    #[test]
    fn control_characters_never_reach_the_page() {
        let b = compiled("a\u{0}b\u{7}c");
        assert!(!body(&b).contains('\u{0}') && !body(&b).contains('\u{7}'));
    }
}

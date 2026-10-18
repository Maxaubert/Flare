//! M9 Task 5: the release-notes parser.
//!
//! A GitHub release body, turned into something safe to show. The body is NEVER rendered: it is
//! reduced here to a short list of plain strings and the window prints each one as text. There is
//! no markdown renderer and no HTML on this path, so a string that says `<script>` is only a
//! string that says so. The stripping below is about reading well, not about safety.
//!
//! Ported from Prism Terminal v0.55.6 `releaseNotes.ts`. Lengths are counted in UTF-16 units, like
//! JavaScript, so the caps agree with the golden fixtures to the unit. Every regex here runs in
//! linear time (the Rust `regex` crate), so no input can make it backtrack.

use std::sync::LazyLock;

use regex::Regex;

/// How many entries the window lists; the rest are counted, not shown.
pub const MAX_ENTRIES: usize = 20;
/// One entry is one line or two in the window, never a paragraph (UTF-16 units).
pub const MAX_ENTRY_CHARS: usize = 160;
/// Only the head of a body is ever read (UTF-16 units).
pub const MAX_BODY_CHARS: usize = 20_000;

/// What stands in for a release that came with no notes, or none worth a line.
pub const NO_NOTES: &str = "No notes were published with this release.";

/// The parsed notes of one release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseNotes {
    /// Plain text, never empty strings, never more than [`MAX_ENTRIES`].
    pub entries: Vec<String>,
    /// How many further entries were read and left out.
    pub more: usize,
    /// True when the body gave nothing and `entries` is the one stand-in line.
    pub empty: bool,
}

/// The line under a capped list.
pub fn more_line(n: usize) -> String {
    format!("+ {n} more")
}

/// Length in UTF-16 units, the way JavaScript counts a string.
pub fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// Cut to `limit` UTF-16 units, ellipsis included, never through a surrogate pair. A string that
/// fits is returned unchanged. Rust strings cannot hold half a pair, so the half pair is dropped
/// whole.
pub fn cut_utf16(s: &str, limit: usize) -> String {
    if utf16_len(s) <= limit {
        return s.to_string();
    }
    let room = limit.saturating_sub(1);
    let mut used = 0;
    let mut end = 0;
    for (i, c) in s.char_indices() {
        let n = c.len_utf16();
        if used + n > room {
            break;
        }
        used += n;
        end = i + c.len_utf8();
    }
    let mut out = s[..end].trim_end().to_string();
    out.push('\u{2026}');
    out
}

fn re(p: &str) -> Regex {
    Regex::new(p).expect("a fixed pattern")
}

static HTML_COMMENT: LazyLock<Regex> = LazyLock::new(|| re(r"(?s)<!--.*?-->"));
// The TypeScript has one regex with a back-reference; Rust has none, so two in turn.
static HTML_SCRIPT: LazyLock<Regex> = LazyLock::new(|| re(r"(?is)<script(?-u:\b).*?</script\s*>"));
static HTML_STYLE: LazyLock<Regex> = LazyLock::new(|| re(r"(?is)<style(?-u:\b).*?</style\s*>"));
static HTML_TAG: LazyLock<Regex> = LazyLock::new(|| re(r"</?[a-zA-Z][^<>]*>"));
static TAG_LIKE: LazyLock<Regex> = LazyLock::new(|| re(r"<[a-zA-Z!/]"));
static TAG_OPEN: LazyLock<Regex> = LazyLock::new(|| re(r"<([a-zA-Z!/])"));
static HEADING: LazyLock<Regex> = LazyLock::new(|| re(r"^\s{0,3}#{1,6}(?:\s|$)"));
static BULLET: LazyLock<Regex> =
    LazyLock::new(|| re(r"^\s*(?:[*+-]|[0-9]{1,9}[.)])(?:\s+|$)(?:\[[ xX]\]\s+)?"));
static FULL_CHANGELOG: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)^[^A-Za-z0-9_]*full changelog(?-u:\b)"));
static NEW_CONTRIBUTORS: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)new contributors"));
static AUTHOR_TAIL: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)\s+by\s+@[A-Za-z0-9_-]+(?:\[bot\])?(?:\s+in\s+(https?://\S+))?\s*$"));
static PR_NUMBER: LazyLock<Regex> = LazyLock::new(|| re(r"/(?:pull|issues)/([0-9]{1,9})/?$"));
static MD_LINK: LazyLock<Regex> =
    LazyLock::new(|| re(r"!?\[([^\[\]]*)\]\([^()\s]*(?:\([^()\s]*\))?[^()\s]*\)"));
static SPACES: LazyLock<Regex> = LazyLock::new(|| re(r"\s+"));
static LINE_BREAK: LazyLock<Regex> = LazyLock::new(|| re(r"\r\n|\r|\n"));

/// C0 and C1 controls (tab, LF and CR are kept: the body is split on them), zero-width
/// characters, and the bidirectional overrides and isolates. U+202E makes the text after it read
/// backwards, which is how "exe.txt" is made to look like "txt.exe".
fn is_unprintable(c: char) -> bool {
    matches!(c,
        '\u{0000}'..='\u{0008}'
        | '\u{000B}'
        | '\u{000C}'
        | '\u{000E}'..='\u{001F}'
        | '\u{007F}'..='\u{009F}'
        | '\u{200B}'..='\u{200F}'
        | '\u{202A}'..='\u{202E}'
        | '\u{2060}'..='\u{2069}'
        | '\u{FEFF}')
}

/// Tags out. A tag can be hidden inside a tag (`<scr<script>ipt>`), so one pass can MAKE a tag;
/// a few passes, and then whatever still opens one loses its bracket. A lone `<` that opens
/// nothing is left: "x < y" is only text.
fn strip_html(s: &str) -> String {
    let mut out = HTML_COMMENT.replace_all(s, " ").into_owned();
    out = HTML_SCRIPT.replace_all(&out, " ").into_owned();
    out = HTML_STYLE.replace_all(&out, " ").into_owned();
    for _ in 0..4 {
        if !TAG_LIKE.is_match(&out) {
            break;
        }
        out = HTML_TAG.replace_all(&out, " ").into_owned();
    }
    if TAG_LIKE.is_match(&out) {
        out = TAG_OPEN.replace_all(&out, "$1").into_owned();
    }
    out
}

/// A line of three or more of the SAME character from `-*_`, spaces allowed between.
fn is_rule(line: &str) -> bool {
    let t = line.trim();
    let Some(first) = t.chars().next() else {
        return false;
    };
    if !matches!(first, '-' | '*' | '_') {
        return false;
    }
    let mut count = 0;
    for c in t.chars() {
        if c == first {
            count += 1;
        } else if !c.is_whitespace() {
            return false;
        }
    }
    count >= 3
}

/// Does `text` hold `needle` at a place where an ASCII digit does not follow?
fn has_ref(text: &str, needle: &str) -> bool {
    text.match_indices(needle).any(|(i, m)| {
        !text[i + m.len()..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit())
    })
}

/// One raw entry (a bullet and whatever wrapped under it) as a plain line.
fn clean_entry(raw: &str) -> String {
    let mut text = raw.to_string();
    let mut reference = String::new();
    if let Some(tail) = AUTHOR_TAIL.captures(&text) {
        let start = tail.get(0).map_or(text.len(), |m| m.start());
        if let Some(n) = tail
            .get(1)
            .and_then(|u| PR_NUMBER.captures(u.as_str()))
            .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
        {
            reference = format!("#{n}");
        }
        text.truncate(start);
    }
    let text = MD_LINK.replace_all(&text, "$1").into_owned();
    let text = text.replace("**", "").replace("~~", "").replace('`', "");
    let text = SPACES.replace_all(&text, " ").trim().to_string();
    if text.is_empty() {
        return String::new();
    }
    // The number is kept as TEXT, and only when the title does not already say it.
    let suffix = if !reference.is_empty() && !has_ref(&text, &reference) {
        format!(" ({reference})")
    } else {
        String::new()
    };
    let mut out = cut_utf16(&text, MAX_ENTRY_CHARS.saturating_sub(utf16_len(&suffix)));
    out.push_str(&suffix);
    out
}

/// The body cut to the head that is read: [`MAX_BODY_CHARS`] units, back to the last whole line.
fn head_of(body: &str) -> &str {
    if utf16_len(body) <= MAX_BODY_CHARS {
        return body;
    }
    let mut used = 0;
    let mut end = 0;
    for (i, c) in body.char_indices() {
        let n = c.len_utf16();
        if used + n > MAX_BODY_CHARS {
            break;
        }
        used += n;
        end = i + c.len_utf8();
    }
    let head = &body[..end];
    match head.rfind('\n') {
        Some(nl) if nl > 0 => &head[..nl],
        _ => head,
    }
}

/// The entries of a release body. `None` is a body that is not a string: missing, null and
/// "not a string" all mean there are no notes.
pub fn parse_release_notes(body: Option<&str>) -> ReleaseNotes {
    let none = || ReleaseNotes {
        entries: vec![NO_NOTES.to_string()],
        more: 0,
        empty: true,
    };
    let Some(body) = body else {
        return none();
    };

    let printable: String = head_of(body)
        .chars()
        .filter(|&c| !is_unprintable(c))
        .collect();
    let text = strip_html(&printable);

    let mut raw: Vec<String> = Vec::new();
    let mut skipping = false; // inside "New Contributors", until the next heading
    let mut open = false; // the last entry can still take a wrapped line
    let mut para = false; // the last entry is a paragraph still running
    for line in LINE_BREAK.split(&text) {
        if line.trim().is_empty() {
            open = false;
            para = false;
            continue;
        }
        if HEADING.is_match(line) {
            skipping = NEW_CONTRIBUTORS.is_match(line);
            open = false;
            para = false;
            continue;
        }
        if skipping || is_rule(line) || FULL_CHANGELOG.is_match(line) {
            open = false;
            para = false;
            continue;
        }
        if let Some(bullet) = BULLET.find(line) {
            raw.push(line[bullet.end()..].to_string());
            open = true;
            para = false;
        } else if (open && line.chars().next().is_some_and(char::is_whitespace)) || para {
            if let Some(last) = raw.last_mut() {
                last.push(' ');
                last.push_str(line.trim());
            }
        } else {
            raw.push(line.to_string());
            open = false;
            para = true;
        }
    }

    let all: Vec<String> = raw
        .iter()
        .map(|r| clean_entry(r))
        .filter(|e| !e.is_empty())
        .collect();
    if all.is_empty() {
        return none();
    }
    let more = all.len().saturating_sub(MAX_ENTRIES);
    ReleaseNotes {
        entries: all.into_iter().take(MAX_ENTRIES).collect(),
        more,
        empty: false,
    }
}

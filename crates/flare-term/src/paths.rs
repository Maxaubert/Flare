//! Text that may be a path (Prism Terminal's termPaths.ts). This only says where a path
//! COULD be in a line of text; whether one exists is main's to answer. Positions are char
//! indices. A URL is not a path (links.rs has it).

use regex::Regex;
use std::sync::LazyLock;

/// A candidate path in the text, char indices, end exclusive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathCandidate {
    pub start: usize,
    pub end: usize,
    /// As written: relative to the shell's folder, or absolute.
    pub path: String,
}

/// Every candidate path in `text`, in order.
pub fn find_paths(text: &str) -> Vec<PathCandidate> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if is_break(chars[i]) {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < chars.len() && !is_break(chars[j]) {
            j += 1;
        }
        let word: String = chars[i..j].iter().collect();
        if let Some((from, path)) = candidate(&word) {
            let start = i + from;
            let end = start + path.chars().count();
            out.push(PathCandidate { start, end, path });
        }
        i = j;
    }
    out
}

/// Whitespace, quotes and the characters a Windows path can never hold.
fn is_break(c: char) -> bool {
    c.is_whitespace() || matches!(c, '"' | '\'' | '`' | '<' | '>' | '|' | '*' | '?')
}

/// A word's path, with the sentence's punctuation off, and how many leading chars it lost.
fn candidate(word: &str) -> Option<(usize, String)> {
    if word.contains("://") {
        return None;
    }
    let trimmed = word.trim_start_matches(['(', '[', '{']);
    let from = word.chars().count() - trimmed.chars().count();
    let mut w = trimmed.to_string();
    // Sentence punctuation and a line number, in either order ("a.ts:12)." ).
    for _ in 0..3 {
        let before = w.clone();
        w = TRAIL.replace(&w, "").into_owned();
        w = LINE_NO.replace(&w, "").into_owned();
        if w == before {
            break;
        }
    }
    let len = w.chars().count();
    if !(2..=MAX).contains(&len) {
        return None;
    }
    if !w.chars().any(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    if w.starts_with('-') {
        return None;
    }
    let separated = w.contains(['\\', '/']);
    if !separated && !EXTENSION.is_match(&w) {
        return None;
    }
    if BARE_DRIVE.is_match(&w) || SEPARATORS_ONLY.is_match(&w) {
        return None;
    }
    Some((from, w))
}

/// The longest text asked about: MAX_PATH.
const MAX: usize = 260;

static TRAIL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[.,;:!)\]}]+$").expect("static regex"));
static LINE_NO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r":[0-9]+(?::[0-9]+)?$").expect("static regex"));
static EXTENSION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[A-Za-z0-9_-]\.[A-Za-z0-9]{1,8}$").expect("static regex"));
static BARE_DRIVE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z]:[\\/]?$").expect("static regex"));
static SEPARATORS_ONLY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[\\/.]+$").expect("static regex"));

#[cfg(test)]
mod tests {
    use super::*;

    fn slice(text: &str, start: usize, end: usize) -> String {
        text.chars().skip(start).take(end - start).collect()
    }

    fn paths(text: &str) -> Vec<String> {
        find_paths(text).into_iter().map(|p| p.path).collect()
    }

    #[test]
    fn find_paths_finds_the_screenshot_paths() {
        assert_eq!(
            paths(
                "- Rapport: docs/sign-off/rapport.pdf, 9 sider. PNG i docs/wireframes/png/. Kilde i docs/wireframes/kilde/, som"
            ),
            vec![
                "docs/sign-off/rapport.pdf",
                "docs/wireframes/png/",
                "docs/wireframes/kilde/"
            ]
        );
    }

    #[test]
    fn find_paths_says_where() {
        let text = "see (src/app.ts:12:3).";
        let p = find_paths(text)[0].clone();
        assert_eq!(slice(text, p.start, p.end), "src/app.ts");
    }

    #[test]
    fn find_paths_knows_windows_paths() {
        assert_eq!(
            paths("C:\\Users\\me\\notes.txt and docs\\sign-off\\rapport.pdf"),
            vec!["C:\\Users\\me\\notes.txt", "docs\\sign-off\\rapport.pdf"]
        );
        assert_eq!(paths("AGENTS.md points there"), vec!["AGENTS.md"]);
        assert_eq!(
            paths("../up/one.txt ./here.md ~/home.cfg"),
            vec!["../up/one.txt", "./here.md", "~/home.cfg"]
        );
    }

    #[test]
    fn find_paths_leaves_urls() {
        assert!(paths("https://github.com/a/b/pull/11 and file:///C:/x").is_empty());
    }

    #[test]
    fn find_paths_asks_nothing_about_words() {
        assert!(paths("hello world 1.5 0.16.1 --force -rf 12:30 C: / ...").is_empty());
    }

    #[test]
    fn find_paths_quotes_break_words() {
        assert_eq!(
            paths("open \"docs/a b.pdf\" or 'x/y.md'"),
            vec!["docs/a", "b.pdf", "x/y.md"]
        );
    }
}

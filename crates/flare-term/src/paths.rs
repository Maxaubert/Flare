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
    // A quoted run that holds spaces and is rooted like a path is ONE candidate ('C:/Users/X/OneDrive -
    // Contoso/plan.docx'), never the prefix before its first space. Its chars are blanked for the word pass.
    let mut masked = chars.clone();
    // The platform's own forms first (macOS: `file://` URLs and words with escaped spaces).
    for s in crate::path_grammar::special_candidates(&chars) {
        for m in &mut masked[s.mask.0..s.mask.1] {
            *m = ' ';
        }
        out.push(PathCandidate {
            start: s.start,
            end: s.end,
            path: s.path,
        });
    }
    let mut i = 0;
    while i < chars.len() {
        if let Some(close) = quoted_path_end(&masked, i) {
            let inner: String = masked[i + 1..close].iter().collect();
            if let Some((from, path)) = candidate(&inner) {
                let start = i + 1 + from;
                let end = start + path.chars().count();
                out.push(PathCandidate { start, end, path });
                for m in &mut masked[i + 1..close] {
                    *m = ' ';
                }
                i = close + 1;
                continue;
            }
        }
        i += 1;
    }
    let chars = masked;
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
    out.sort_by_key(|c| c.start);
    out
}

/// Where a quoted run opened at `at` closes, when it is a path with spaces in it: the quote opens a word
/// (not an apostrophe inside one), closes on the same quote before one more word starts, holds white
/// space, and begins like a path (a drive, a root, `./`, `../`, `~`).
fn quoted_path_end(chars: &[char], at: usize) -> Option<usize> {
    let q = *chars.get(at)?;
    if !matches!(q, '"' | '\'' | '`') {
        return None;
    }
    let opens = at == 0 || chars[at - 1].is_whitespace() || "([{=,:".contains(chars[at - 1]);
    if !opens {
        return None;
    }
    let close = (at + 1..chars.len()).find(|&k| chars[k] == q)?;
    if chars.get(close + 1).is_some_and(|c| c.is_alphanumeric()) {
        return None;
    }
    let inner = &chars[at + 1..close];
    if inner.is_empty()
        || inner[0].is_whitespace()
        || inner[inner.len() - 1].is_whitespace()
        || !inner.iter().any(|c| c.is_whitespace())
    {
        return None;
    }
    let head: String = inner.iter().take(3).collect();
    let rooted = head.starts_with(['/', '\\', '~'])
        || head.starts_with("./")
        || head.starts_with(".\\")
        || head.starts_with("../")
        || head.starts_with("..\\")
        || (head.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            && head[1..].starts_with(':'));
    rooted.then_some(close)
}

/// Whitespace, quotes and the characters a Windows path can never hold.
fn is_break(c: char) -> bool {
    c.is_whitespace() || matches!(c, '"' | '\'' | '`' | '<' | '>' | '|' | '*' | '?')
}

/// A word's path, with the sentence's punctuation off, and how many leading chars it lost.
pub(crate) fn candidate(word: &str) -> Option<(usize, String)> {
    if word.contains("://") {
        return None;
    }
    let trimmed = word.trim_start_matches(['(', '[', '{']);
    let from = word.chars().count() - trimmed.chars().count();
    let mut w = trimmed.to_string();
    // Sentence punctuation and a line number, in either order ("a.ts:12)." ).
    for _ in 0..3 {
        let before = w.clone();
        // "(line,col)" first: the trailing punctuation would eat its closing bracket.
        w = PAREN_LINE.replace(&w, "").into_owned();
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
/// MSBuild's and tsc's "(12,5)" (and "(12)"), with the punctuation after it.
static PAREN_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\([0-9]+(?:,[0-9]+)?\)[.,;:!]*$").expect("static regex"));
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

    #[cfg(windows)]
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

    #[test]
    fn a_quoted_path_with_spaces_is_one_candidate_not_its_prefix() {
        let text = "open 'C:/Users/X/OneDrive - Contoso/plan.docx' now";
        let got = find_paths(text);
        assert_eq!(
            got.iter().map(|p| p.path.as_str()).collect::<Vec<_>>(),
            vec!["C:/Users/X/OneDrive - Contoso/plan.docx"],
            "never 'C:/Users/X/OneDrive' on its own"
        );
        assert_eq!(
            slice(text, got[0].start, got[0].end),
            "C:/Users/X/OneDrive - Contoso/plan.docx"
        );
        assert_eq!(paths("cat \"./my docs/a.txt\":12"), vec!["./my docs/a.txt"]);
        // An apostrophe inside prose is no quote.
        assert_eq!(paths("it's in docs/a.md, the dev's"), vec!["docs/a.md"]);
    }

    #[test]
    fn a_trailing_line_and_column_in_parentheses_is_not_part_of_the_path() {
        assert_eq!(paths("src/a.ts(12,5): error"), vec!["src/a.ts"]);
        assert_eq!(paths("src/a.ts(12):"), vec!["src/a.ts"]);
    }

    #[cfg(not(windows))]
    #[test]
    fn find_paths_reads_a_local_file_url_and_leaves_web_ones() {
        assert!(paths("https://github.com/a/b/pull/11").is_empty());
        assert_eq!(paths("open file:///C:/x"), vec!["/C:/x"]);
        assert_eq!(
            paths("file:///Users/me/a%20b.md and file://far/x.md"),
            vec!["/Users/me/a b.md"]
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn find_paths_knows_posix_paths() {
        assert_eq!(
            paths("/Users/me/notes.txt and ~/src/main.rs:10:2 and ./a/b.md"),
            vec!["/Users/me/notes.txt", "~/src/main.rs", "./a/b.md"]
        );
        let text = "edit My\\ Docs/plan\\ v2.md, then";
        let got = find_paths(text);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].path, "My Docs/plan v2.md");
        assert_eq!(
            slice(text, got[0].start, got[0].end),
            "My\\ Docs/plan\\ v2.md"
        );
    }
}

//! The http(s) links in a line of text (Prism Terminal's termLinks.ts, findLinks only;
//! linkColor is flare-theme's, M4). Positions are char indices.

use regex::Regex;
use std::sync::LazyLock;

/// Where one link sits in the text, char indices, end exclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinkSpan {
    pub start: usize,
    pub end: usize,
}

/// The http(s) links in `text`. A link runs to the next space, quote or angle bracket; then
/// whatever belongs to the SENTENCE is given back: a full stop, comma, colon, etc. after it,
/// and a closing bracket the link never opened.
pub fn find_links(text: &str) -> Vec<LinkSpan> {
    let mut out = Vec::new();
    for m in CANDIDATE.find_iter(text) {
        let mut url = m.as_str();
        loop {
            match url.as_bytes().last().copied() {
                Some(b'.' | b',' | b';' | b':' | b'!' | b'?') => url = &url[..url.len() - 1],
                Some(c @ (b')' | b']' | b'}')) => {
                    let opener = match c {
                        b')' => b'(',
                        b']' => b'[',
                        _ => b'{',
                    };
                    if count(url, c) > count(url, opener) {
                        url = &url[..url.len() - 1];
                    } else {
                        break;
                    }
                }
                _ => break,
            }
        }
        if !SCHEME_AND_HOST.is_match(url) {
            continue;
        }
        let start = text[..m.start()].chars().count();
        out.push(LinkSpan {
            start,
            end: start + url.chars().count(),
        });
    }
    out
}

static CANDIDATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)https?://[^\s"'<>`]+"#).expect("static regex"));
static SCHEME_AND_HOST: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^https?://[^/?#]").expect("static regex"));

fn count(s: &str, ch: u8) -> usize {
    s.bytes().filter(|&b| b == ch).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slice(text: &str, start: usize, end: usize) -> String {
        text.chars().skip(start).take(end - start).collect()
    }

    fn found(text: &str) -> Vec<String> {
        find_links(text)
            .into_iter()
            .map(|l| slice(text, l.start, l.end))
            .collect()
    }

    #[test]
    fn find_links_leaves_the_full_stop() {
        assert_eq!(
            found("  https://go.microsoft.com/fwlink/?LinkID=108518."),
            vec!["https://go.microsoft.com/fwlink/?LinkID=108518"]
        );
    }

    #[test]
    fn find_links_finds_several() {
        assert_eq!(
            found("see http://a.io/x and https://b.dev/y?z=1#top, then C:\\Users\\me"),
            vec!["http://a.io/x", "https://b.dev/y?z=1#top"]
        );
    }

    #[test]
    fn find_links_stops_at_wrappers() {
        assert_eq!(
            found("(https://a.io/x) \"https://b.io/y\" <https://c.io/z>"),
            vec!["https://a.io/x", "https://b.io/y", "https://c.io/z"]
        );
    }

    #[test]
    fn find_links_keeps_its_own_bracket() {
        assert_eq!(
            found("https://en.wikipedia.org/wiki/Atlas_(optics) is one"),
            vec!["https://en.wikipedia.org/wiki/Atlas_(optics)"]
        );
    }

    #[test]
    fn find_links_bare_scheme_is_nothing() {
        assert!(found("https:// and http://").is_empty());
    }

    #[test]
    fn find_links_positions_are_chars_after_wide_text() {
        let text = "日本 https://a.io/x";
        let l = find_links(text)[0];
        assert_eq!((l.start, l.end), (3, 3 + "https://a.io/x".chars().count()));
    }
}

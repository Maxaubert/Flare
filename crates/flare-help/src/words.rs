//! Words of the help search: tokenise, meaningful words, stem, expand, one-slip match (M7 Task 6).
//! A port of the word half of `PT/core/shared/help/search.ts`. JavaScript counts string lengths in
//! UTF-16 units and Rust in bytes, so every length that decides a result goes through `u16len`.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

/// A query is cut here before anything reads it.
pub(crate) const MAX_QUERY_CHARS: usize = 400;
/// No real word is longer.
const MAX_WORD_CHARS: usize = 40;
/// How many distinct meaningful words of a query are scored.
pub(crate) const MAX_QUERY_WORDS: usize = 24;

/// The length JavaScript reports: UTF-16 code units.
pub(crate) fn u16len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// `s.slice(0, n)` in UTF-16 units, cut at a character boundary at or below `n`.
pub(crate) fn truncate_u16(s: &str, n: usize) -> &str {
    let mut units = 0;
    for (i, c) in s.char_indices() {
        units += c.len_utf16();
        if units > n {
            return &s[..i];
        }
    }
    s
}

/// The JavaScript `\s` set (also what `trim` strips).
pub(crate) fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}

/// `trim()` with the JavaScript whitespace set.
pub(crate) fn js_trim(s: &str) -> &str {
    s.trim_matches(is_js_space)
}

fn is_apostrophe(c: char) -> bool {
    matches!(c, '\'' | '\u{2019}' | '\u{02BC}' | '`')
}

/// Lower-case words, nothing else: punctuation gone, accents folded, and an apostrophe closed up
/// rather than split on, so "what's" is "whats". A command splits into its parts.
pub fn tokenise(text: &str) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let flat: String = text
        .nfkd()
        .filter(|c| !is_combining_mark(*c) && !is_apostrophe(*c))
        .collect::<String>()
        .to_lowercase();
    flat.split(|c: char| !(c.is_alphabetic() || c.is_numeric()))
        .filter(|w| !w.is_empty())
        .map(|w| truncate_u16(w, MAX_WORD_CHARS).to_string())
        .collect()
}

/// The scaffolding of a question. Deliberately not here, because in a shell they are the point:
/// all, not, no, up, down, out, back, off, kill, who, where, new, last, first, set, run, use.
const STOP_WORDS: &str = "a an the this that these those there here it its \
    i im ive id me my mine we our you your \
    is are am be been was were do does did doing done \
    can could would should will shall may might must \
    how what whats which why when \
    to of in on at by for with from into onto as about than then so and or but if \
    please way ways want wants need needs like just some any thing things something someone \
    tell know let lets help get one s t";

fn stop_words() -> &'static HashSet<&'static str> {
    static SET: OnceLock<HashSet<&'static str>> = OnceLock::new();
    SET.get_or_init(|| STOP_WORDS.split(' ').filter(|w| !w.is_empty()).collect())
}

/// The words of a query that are worth scoring, each once, in the order they were said. A query
/// made of nothing but scaffolding keeps all of its words.
pub fn meaningful_words(query: &str) -> Vec<String> {
    let all = tokenise(truncate_u16(query, MAX_QUERY_CHARS));
    let stops = stop_words();
    let kept: Vec<&String> = all.iter().filter(|w| !stops.contains(w.as_str())).collect();
    let source: Vec<&String> = if kept.is_empty() {
        all.iter().collect()
    } else {
        kept
    };
    let mut seen: HashSet<&str> = HashSet::new();
    let mut out = Vec::new();
    for w in source {
        if !seen.insert(w.as_str()) {
            continue;
        }
        out.push(w.clone());
        if out.len() == MAX_QUERY_WORDS {
            break;
        }
    }
    out
}

const IRREGULAR: [(&str, &str); 15] = [
    ("using", "use"),
    ("used", "use"),
    ("uses", "use"),
    ("going", "go"),
    ("goes", "go"),
    ("went", "go"),
    ("ran", "run"),
    ("made", "make"),
    ("took", "take"),
    ("taken", "take"),
    ("wrote", "write"),
    ("written", "write"),
    ("found", "find"),
    ("gone", "go"),
    ("dirs", "dir"),
];

/// -er and -est are folded only onto these.
const COMPARABLE: [&str; 17] = [
    "big", "large", "small", "old", "fast", "slow", "long", "short", "high", "low", "early",
    "heavy", "great", "few", "deep", "wide", "recent",
];

fn is_vowel(w: &[u8], i: usize) -> bool {
    let c = w[i];
    matches!(c, b'a' | b'e' | b'i' | b'o' | b'u') || (c == b'y' && i > 0)
}

fn has_vowel(w: &[u8]) -> bool {
    (0..w.len()).any(|i| is_vowel(w, i))
}

fn ends_any(w: &str, suffixes: &[&str]) -> bool {
    suffixes.iter().any(|s| w.ends_with(s))
}

/// What is left once -ing / -ed / -er / -est has come off, put back into a word. None when too
/// little is left to be a word at all.
fn repair(root: &str) -> Option<String> {
    let b = root.as_bytes();
    let n = b.len();
    if n < 3 || !has_vowel(b) {
        return None;
    }
    if b[n - 1] == b[n - 2] && !is_vowel(b, n - 1) && !matches!(b[n - 1], b'l' | b's' | b'z') {
        return Some(if n > 3 {
            root[..n - 1].to_string()
        } else {
            root.to_string()
        });
    }
    if ends_any(root, &["at", "iz", "bl", "rg", "ang", "ur"]) {
        return Some(format!("{root}e"));
    }
    let cvc = !is_vowel(b, n - 1) && is_vowel(b, n - 2) && !is_vowel(b, n - 3);
    if cvc
        && !matches!(b[n - 1], b'w' | b'x' | b'y')
        && !ends_any(root, &["en", "er", "on", "or", "el", "al", "it"])
    {
        return Some(format!("{root}e"));
    }
    Some(root.to_string())
}

/// A light stemmer: plurals, -ing, -ed, and -er / -est on a short list of adjectives.
pub fn stem(word: &str) -> String {
    let plain = !word.is_empty() && word.bytes().all(|b| b.is_ascii_lowercase());
    if !plain || word.len() <= 3 || word.len() > MAX_WORD_CHARS {
        return word.to_string();
    }
    if let Some((_, to)) = IRREGULAR.iter().find(|(from, _)| *from == word) {
        return (*to).to_string();
    }
    let mut w: String = word.to_string();
    if w.ends_with("sses") {
        w.truncate(w.len() - 2);
    } else if w.ends_with("ies") && w.len() > 4 {
        w.truncate(w.len() - 3);
        w.push('y');
    } else if ends_any(&w, &["ches", "shes", "xes"]) {
        w.truncate(w.len() - 2);
    } else if w.ends_with('s') && !ends_any(&w, &["ss", "us", "is", "as"]) {
        w.truncate(w.len() - 1);
    }
    if w.len() <= 3 {
        return w;
    }
    if w.ends_with("ing") {
        return repair(&w[..w.len() - 3]).unwrap_or(w);
    }
    if w.ends_with("ied") && w.len() > 4 {
        return format!("{}y", &w[..w.len() - 3]);
    }
    if w.ends_with("ed") && !w.ends_with("eed") {
        return repair(&w[..w.len() - 2]).unwrap_or(w);
    }
    for (suffix, y) in [("iest", true), ("ier", true), ("est", false), ("er", false)] {
        if !w.ends_with(suffix) {
            continue;
        }
        let cut = &w[..w.len() - suffix.len()];
        let root = if y {
            Some(format!("{cut}y"))
        } else {
            repair(cut)
        };
        if let Some(r) = root
            && COMPARABLE.contains(&r.as_str())
        {
            return r;
        }
        break;
    }
    w
}

/// A short form and its long form are one word, so they compare equal.
const LONG_FORM: [(&str, &str); 5] = [
    ("admin", "administrator"),
    ("env", "environment"),
    ("app", "application"),
    ("repo", "repository"),
    ("config", "configuration"),
];

/// What two words are compared by: the stem (short forms lengthened), less a final e on anything of
/// five letters or more.
pub(crate) fn key_of(word: &str) -> String {
    let mut k = stem(word);
    if let Some((_, long)) = LONG_FORM.iter().find(|(s, _)| *s == k) {
        k = (*long).to_string();
    }
    if u16len(&k) >= 5 && k.ends_with('e') {
        k.pop();
    }
    k
}

/// The everyday vocabulary of this domain and nothing more.
const SYNONYM_GROUPS: [&[&str]; 27] = [
    &["big", "large", "huge", "size", "heavy"],
    &["delete", "remove", "erase", "rm", "del", "trash"],
    &["folder", "directory", "dir"],
    &["find", "search", "locate", "look"],
    &["show", "list", "see", "display", "view", "print"],
    &["stop", "kill", "end", "terminate", "quit"],
    &["make", "create", "new"],
    &["rename", "move", "mv"],
    &["copy", "duplicate", "cp"],
    &["text", "string", "word"],
    &["process", "task", "program", "app", "application"],
    &["port", "listening"],
    &["ip", "address"],
    &["path", "environment", "env", "variable"],
    &["undo", "revert", "discard", "restore"],
    &["go", "change", "navigate", "cd"],
    &["hidden", "invisible", "dotfile"],
    &["unzip", "extract", "decompress", "unpack"],
    &["zip", "compress", "archive"],
    &["admin", "administrator", "elevated", "sudo"],
    &["history", "previous", "last", "recent"],
    &["newest", "newer", "latest"],
    &["clear", "cls", "clean", "wipe"],
    &["install", "setup"],
    &["download", "fetch"],
    &["file", "document"],
    &["frozen", "hung", "stuck", "unresponsive"],
];

fn synonyms_by_key() -> &'static HashMap<String, Vec<String>> {
    static MAP: OnceLock<HashMap<String, Vec<String>>> = OnceLock::new();
    MAP.get_or_init(|| {
        let mut m: HashMap<String, Vec<String>> = HashMap::new();
        for group in SYNONYM_GROUPS {
            for w in group {
                let list = m.entry(key_of(w)).or_default();
                for other in group {
                    if other != w && !list.iter().any(|x| x == other) {
                        list.push((*other).to_string());
                    }
                }
            }
        }
        m
    })
}

/// The synonym words (as written in the table) of a word that is already a key.
pub(crate) fn synonym_keys(key: &str) -> &'static [String] {
    synonyms_by_key().get(key).map_or(&[], Vec::as_slice)
}

/// The other everyday words for this one, as written in the table (never the word itself). Empty
/// for a word the table does not know.
pub fn expand(word: &str) -> Vec<String> {
    if word.is_empty() {
        return Vec::new();
    }
    let w = word.to_lowercase();
    if w == "running" {
        let mut out = vec!["process".to_string()];
        out.extend(synonym_keys(&key_of("process")).iter().cloned());
        return out;
    }
    let k = key_of(&w);
    synonym_keys(&k)
        .iter()
        .filter(|s| key_of(s) != k)
        .cloned()
        .collect()
}

fn from(a: &[u16], i: usize) -> &[u16] {
    &a[i.min(a.len())..]
}

/// True when `a` is `b` with at most one slip: a letter wrong, missing, extra, or two neighbours
/// swapped. Works on UTF-16 units, as the original does.
pub fn within_one_edit(a: &str, b: &str) -> bool {
    let a: Vec<u16> = a.encode_utf16().collect();
    let b: Vec<u16> = b.encode_utf16().collect();
    within_one_edit_units(&a, &b)
}

pub(crate) fn within_one_edit_units(a: &[u16], b: &[u16]) -> bool {
    let (la, lb) = (a.len(), b.len());
    if la.abs_diff(lb) > 1 {
        return false;
    }
    let n = la.min(lb);
    let mut i = 0;
    while i < n && a[i] == b[i] {
        i += 1;
    }
    if i == la && i == lb {
        return true;
    }
    if la == lb {
        if from(a, i + 1) == from(b, i + 1) {
            return true;
        }
        return i + 1 < la
            && a[i] == b[i + 1]
            && a[i + 1] == b[i]
            && from(a, i + 2) == from(b, i + 2);
    }
    if la > lb {
        from(a, i + 1) == from(b, i)
    } else {
        from(a, i) == from(b, i + 1)
    }
}

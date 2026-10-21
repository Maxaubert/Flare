//! The help search and its index (M7 Task 6): a port of `PT/core/shared/help/search.ts`, held to
//! Prism Terminal's own answers by `tests/golden.rs`.
//!
//! The query is read the way a person wrote it: the scaffolding of a question is dropped, words
//! are folded to a stem, a small synonym table covers the everyday vocabulary, a word still being
//! typed matches by prefix and a long word that matches nothing is allowed one slip. A query that
//! is a command's own name puts that entry first.
//!
//! THE SCORE, highest tier first: 12000 / 10000 the query is the command / its leading words;
//! 6000 the same for a variant; 2000 the query starts the command or names a later pipeline stage;
//! 1000 every meaningful word matched; under 1000 the words themselves (per word, the best field:
//! task 10, keywords 6, summary 3, command 2, the rest 1, scaled by how it matched and how rare
//! the word is) plus up to 30 for a whole phrase. Ties break by the shorter task, then the id.
//!
//! Port notes: lengths are UTF-16 units; the vocabulary and the per-entry field table keep
//! insertion order because two decisions depend on it; `Math.round` is `floor(x + 0.5)`.

use crate::shells::shown_in;
use crate::types::{HelpCategory, HelpEntry, HelpShell, HelpShellChoice};
use crate::words::{
    MAX_QUERY_CHARS, MAX_QUERY_WORDS, expand, is_js_space, js_trim, key_of, meaningful_words,
    synonym_keys, tokenise, truncate_u16, u16len, within_one_edit_units,
};
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Clone, Copy, Debug, Default)]
pub struct HelpSearchOptions {
    /// Keep the entries shown in this shell (`shown_in`); `Any` or `Windows` keeps the entries
    /// written for exactly that.
    pub shell: Option<HelpShell>,
    pub category: Option<HelpCategory>,
    /// At most this many results; `None` or 0 is no limit.
    pub limit: Option<usize>,
}

#[derive(Clone, Copy, Debug)]
pub struct HelpSearchHit<'a> {
    pub entry: &'a HelpEntry,
    pub score: f64,
}

const W_TASK: f64 = 10.0;
const W_KEYWORDS: f64 = 6.0;
const W_SUMMARY: f64 = 3.0;
const W_COMMAND: f64 = 2.0;
const W_REST: f64 = 1.0;
// A variant is a row of its own in the panel, so its name and hidden words weigh a step under the
// entry's own.
const W_VARIANT_NAME: f64 = 7.0;
const W_VARIANT_KEY: f64 = 4.5;
/// A second field holding the same word is a little more evidence.
const ALSO: f64 = 0.25;

const M_EXACT: f64 = 1.0;
const M_SYNONYM: f64 = 0.55;
const M_PREFIX: f64 = 0.5;
const M_FUZZY: f64 = 0.4;

const BONUS_COMMAND_IS: f64 = 12000.0;
const BONUS_COMMAND_LEADS: f64 = 10000.0;
const BONUS_VARIANT: f64 = 6000.0;
const BONUS_COMMAND_PARTIAL: f64 = 2000.0;
const BONUS_ALL_WORDS: f64 = 1000.0;
const BONUS_PHRASE_IS: f64 = 30.0;
const BONUS_PHRASE_INSIDE: f64 = 15.0;
const BONUS_PHRASE_AROUND: f64 = 12.0;

struct Posting {
    at: Vec<usize>,
    worth: Vec<f64>,
}

struct Index {
    count: usize,
    postings: HashMap<String, Posting>,
    /// The keys in the order they were first seen.
    vocabulary: Vec<String>,
    rarity: HashMap<String, f64>,
    commands: Vec<String>,
    variants: Vec<Vec<String>>,
    stages: Vec<Vec<String>>,
    phrases: Vec<Vec<String>>,
}

/// Lower-case, the JavaScript whitespace collapsed to single spaces, trimmed.
fn flat_command(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut pending_space = false;
    for c in s.to_lowercase().chars() {
        if is_js_space(c) {
            pending_space = true;
        } else {
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            pending_space = false;
            out.push(c);
        }
    }
    out
}

/// The first word of each later stage of a pipeline (`||`, `&&`, `|`, `;`).
fn stages_of(command: &str) -> Vec<String> {
    let mut parts: Vec<&str> = Vec::new();
    let b = command.as_bytes();
    let (mut start, mut i) = (0, 0);
    while i < b.len() {
        let two = i + 1 < b.len()
            && ((b[i] == b'|' && b[i + 1] == b'|') || (b[i] == b'&' && b[i + 1] == b'&'));
        if two {
            parts.push(&command[start..i]);
            i += 2;
            start = i;
        } else if b[i] == b'|' || b[i] == b';' {
            parts.push(&command[start..i]);
            i += 1;
            start = i;
        } else {
            i += 1;
        }
    }
    parts.push(&command[start..]);
    parts
        .into_iter()
        .skip(1)
        .map(|s| {
            let t = js_trim(s);
            let t = t.strip_prefix("sudo ").unwrap_or(t);
            t.split(' ').next().unwrap_or("").to_string()
        })
        .filter(|s| !s.is_empty())
        .collect()
}

#[cfg(test)]
static BUILDS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn build_index(entries: &[HelpEntry]) -> Index {
    #[cfg(test)]
    BUILDS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let count = entries.len();
    let mut postings: HashMap<String, Posting> = HashMap::new();
    let mut vocabulary: Vec<String> = Vec::new();
    let mut commands = Vec::with_capacity(count);
    let mut variants = Vec::with_capacity(count);
    let mut stages = Vec::with_capacity(count);
    let mut phrases = Vec::with_capacity(count);

    for (i, entry) in entries.iter().enumerate() {
        // The fields in insertion order: the order decides the order words enter the postings.
        let mut fields: Vec<(String, Vec<f64>)> = Vec::new();
        let mut field_at: HashMap<String, usize> = HashMap::new();
        let mut phrase: Vec<String> = Vec::new();
        let mut read = |text: &str, worth: f64, as_phrase: bool| {
            let keys: Vec<String> = tokenise(text).iter().map(|w| key_of(w)).collect();
            for k in &keys {
                match field_at.get(k) {
                    Some(&at) => {
                        let held = &mut fields[at].1;
                        if !held.contains(&worth) {
                            held.push(worth);
                        }
                    }
                    None => {
                        field_at.insert(k.clone(), fields.len());
                        fields.push((k.clone(), vec![worth]));
                    }
                }
            }
            if as_phrase && !keys.is_empty() {
                phrase.push(format!(" {} ", keys.join(" ")));
            }
        };

        read(&entry.task, W_TASK, true);
        for k in &entry.keywords {
            read(k, W_KEYWORDS, true);
        }
        read(&entry.summary, W_SUMMARY, false);
        read(&entry.command, W_COMMAND, false);
        // A variant is a row: its label is a name and its keywords are hidden words for that row.
        for v in &entry.variants {
            read(&v.label, W_VARIANT_NAME, true);
            for k in &v.keywords {
                read(k, W_VARIANT_KEY, true);
            }
            read(&v.command, W_REST, false);
        }
        for v in entry.placeholders.values() {
            read(v, W_REST, false);
        }
        if let Some(d) = &entry.danger {
            read(d, W_REST, false);
        }

        for (k, mut held) in fields {
            held.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
            let mut worth = held[0];
            for h in &held[1..] {
                worth += ALSO * h;
            }
            match postings.get_mut(&k) {
                Some(p) => {
                    p.at.push(i);
                    p.worth.push(worth);
                }
                None => {
                    vocabulary.push(k.clone());
                    postings.insert(
                        k,
                        Posting {
                            at: vec![i],
                            worth: vec![worth],
                        },
                    );
                }
            }
        }

        let command = flat_command(&entry.command);
        stages.push(stages_of(&command));
        commands.push(command);
        variants.push(
            entry
                .variants
                .iter()
                .map(|v| flat_command(&v.command))
                .filter(|c| !c.is_empty())
                .collect(),
        );
        phrases.push(phrase);
    }

    // A word in every other entry says less than one in two. Between 1 and 2.
    let mut rarity = HashMap::with_capacity(postings.len());
    let ln = ((count + 1) as f64).ln();
    let scale = if ln == 0.0 { 1.0 } else { ln };
    for (k, p) in &postings {
        rarity.insert(
            k.clone(),
            1.0 + (((count + 1) as f64) / ((p.at.len() + 1) as f64)).ln() / scale,
        );
    }

    Index {
        count,
        postings,
        vocabulary,
        rarity,
        commands,
        variants,
        stages,
        phrases,
    }
}

type IndexCache = Mutex<Vec<(usize, usize, u64, Arc<Index>)>>;

fn fingerprint(entries: &[HelpEntry]) -> u64 {
    let mut h = DefaultHasher::new();
    for e in entries {
        e.id.hash(&mut h);
        e.task.hash(&mut h);
        e.summary.hash(&mut h);
        e.command.hash(&mut h);
        e.keywords.hash(&mut h);
        for v in &e.variants {
            v.label.hash(&mut h);
            v.command.hash(&mut h);
            v.keywords.hash(&mut h);
        }
        for (k, v) in &e.placeholders {
            k.hash(&mut h);
            v.hash(&mut h);
        }
        e.danger.hash(&mut h);
    }
    h.finish()
}

/// One index per catalogue slice, found again by its address and length. A fingerprint of the
/// text guards against a freed slice whose address is reused by a different catalogue.
fn index_for(entries: &[HelpEntry]) -> Arc<Index> {
    static CACHE: OnceLock<IndexCache> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(Vec::new()));
    let (ptr, len) = (entries.as_ptr() as usize, entries.len());
    let fp = fingerprint(entries);
    let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(i) = guard
        .iter()
        .position(|(p, l, f, _)| *p == ptr && *l == len && *f == fp)
    {
        return guard[i].3.clone();
    }
    let ix = Arc::new(build_index(entries));
    guard.retain(|(p, ..)| *p != ptr);
    if guard.len() >= 8 {
        guard.remove(0);
    }
    guard.push((ptr, len, fp, ix.clone()));
    ix
}

fn is_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

fn first_unit(s: &str) -> Option<u16> {
    s.encode_utf16().next()
}

/// Every way this word of the query may meet a word of the catalogue.
fn candidates(word: &str, ix: &Index) -> HashMap<String, f64> {
    let mut out: HashMap<String, f64> = HashMap::new();
    let key = key_of(word);
    out.insert(key.clone(), M_EXACT);
    let others = expand(word);
    for s in &others {
        out.entry(key_of(s)).or_insert(M_SYNONYM);
    }
    // A word the catalogue holds needs no guessing, and a number is a value for a placeholder.
    if ix.postings.contains_key(&key) || is_digits(word) {
        return out;
    }

    // A guessed word brings its synonyms with it.
    let guess = |out: &mut HashMap<String, f64>, v: &str, how: f64| {
        out.insert(v.to_string(), how);
        for s in synonym_keys(v) {
            out.entry(key_of(s)).or_insert(how * M_SYNONYM);
        }
    };
    let wlen = u16len(word);
    if wlen >= 3 {
        let key_long = u16len(&key) >= 3;
        for v in &ix.vocabulary {
            if u16len(v) > wlen
                && (v.starts_with(word) || (key_long && v.starts_with(key.as_str())))
                && !out.contains_key(v)
            {
                guess(&mut out, v, M_PREFIX);
            }
        }
    }
    // One typo, only in a word long enough for one slip to leave it recognisable, and only when
    // nothing else explained the word. The first letter has to be right.
    if out.len() == 1 && others.is_empty() && wlen >= 5 {
        let w16: Vec<u16> = word.encode_utf16().collect();
        let k16: Vec<u16> = key.encode_utf16().collect();
        let w0 = first_unit(word);
        for v in &ix.vocabulary {
            if out.contains_key(v) {
                continue;
            }
            let v16: Vec<u16> = v.encode_utf16().collect();
            if v16.len() >= 4
                && Some(v16[0]) == w0
                && (within_one_edit_units(&w16, &v16) || within_one_edit_units(&k16, &v16))
            {
                guess(&mut out, v, M_FUZZY);
            }
        }
    }
    out
}

fn command_bonus(q: &str, i: usize, ix: &Index) -> f64 {
    if q.is_empty() || u16len(q) > 200 {
        return 0.0;
    }
    let command = &ix.commands[i];
    if command == q {
        return BONUS_COMMAND_IS;
    }
    let lead = format!("{q} ");
    if command.starts_with(&lead) {
        return BONUS_COMMAND_LEADS;
    }
    let vs = &ix.variants[i];
    if vs.iter().any(|v| v == q || v.starts_with(&lead)) {
        return BONUS_VARIANT;
    }
    if ix.stages[i].iter().any(|s| s == q) {
        return BONUS_COMMAND_PARTIAL;
    }
    // Still being typed: one letter is too little to mean a command, except a slash.
    if (u16len(q) >= 2 || q == "/")
        && (command.starts_with(q) || vs.iter().any(|v| v.starts_with(q)))
    {
        return BONUS_COMMAND_PARTIAL;
    }
    0.0
}

fn phrase_bonus(padded: &str, word_count: usize, phrases: &[String]) -> f64 {
    let mut best = 0.0;
    for p in phrases {
        if p == padded {
            return BONUS_PHRASE_IS;
        }
        // A one-word keyword inside a sentence is just a word, already scored.
        let several = p
            .get(1..)
            .and_then(|r| r.find(' '))
            .is_some_and(|at| at + 1 < p.len() - 1);
        if best < BONUS_PHRASE_INSIDE && several && padded.contains(p.as_str()) {
            best = BONUS_PHRASE_INSIDE;
        } else if best < BONUS_PHRASE_AROUND && word_count >= 2 && p.contains(padded) {
            best = BONUS_PHRASE_AROUND;
        }
    }
    best
}

fn js_round(x: f64) -> f64 {
    (x + 0.5).floor()
}

fn choice_of(shell: HelpShell) -> Option<HelpShellChoice> {
    match shell {
        HelpShell::Powershell => Some(HelpShellChoice::Powershell),
        HelpShell::Cmd => Some(HelpShellChoice::Cmd),
        HelpShell::Bash => Some(HelpShellChoice::Bash),
        HelpShell::Any | HelpShell::Windows => None,
    }
}

/// Search the catalogue. An empty query is not a search: it answers with the entries of that
/// shell in catalogue order, score 0, so the panel can browse by category. A query that matches
/// nothing answers with nothing. Never panics, whatever it is handed.
pub fn search_help<'a>(
    entries: &'a [HelpEntry],
    query: &str,
    opts: &HelpSearchOptions,
) -> Vec<HelpSearchHit<'a>> {
    if entries.is_empty() {
        return Vec::new();
    }
    let limit = match opts.limit {
        Some(n) if n >= 1 => n,
        _ => usize::MAX,
    };
    let wanted = |entry: &HelpEntry| -> bool {
        let shell_ok = match opts.shell {
            None => true,
            Some(s) => match choice_of(s) {
                Some(choice) => shown_in(entry.shell, choice),
                None => entry.shell == s,
            },
        };
        shell_ok && opts.category.is_none_or(|c| entry.category == c)
    };

    let text = truncate_u16(query, MAX_QUERY_CHARS);
    if js_trim(text).is_empty() {
        let mut all = Vec::new();
        for entry in entries {
            if wanted(entry) {
                all.push(HelpSearchHit { entry, score: 0.0 });
            }
            if all.len() >= limit {
                break;
            }
        }
        return all;
    }

    let ix = index_for(entries);
    let n = ix.count;
    let words = meaningful_words(text);
    let mut sum = vec![0.0f64; n];
    let mut matched = vec![0u8; n];
    let mut best = vec![0.0f64; n];
    // A number is never required: "port 3000" is fully answered by the entry about ports.
    let mut required: usize = 0;

    for word in &words {
        let counts = !is_digits(word);
        if counts {
            required += 1;
        }
        best.fill(0.0);
        for (key, how) in candidates(word, &ix) {
            let Some(p) = ix.postings.get(&key) else {
                continue;
            };
            let scale = how * ix.rarity.get(&key).copied().unwrap_or(1.0);
            for (j, &at) in p.at.iter().enumerate() {
                let v = p.worth[j] * scale;
                if v > best[at] {
                    best[at] = v;
                }
            }
        }
        for i in 0..n {
            if best[i] == 0.0 {
                continue;
            }
            sum[i] += best[i];
            if counts {
                matched[i] = matched[i].saturating_add(1);
            }
        }
    }

    let q = flat_command(text);
    let spoken: Vec<String> = tokenise(text)
        .iter()
        .take(MAX_QUERY_WORDS * 2)
        .map(|w| key_of(w))
        .collect();
    let padded = format!(" {} ", spoken.join(" "));

    let mut hits: Vec<HelpSearchHit<'a>> = Vec::new();
    for (i, entry) in entries.iter().enumerate() {
        if !wanted(entry) {
            continue;
        }
        let named = command_bonus(&q, i, &ix);
        let phrase = if spoken.is_empty() {
            0.0
        } else {
            phrase_bonus(&padded, spoken.len(), &ix.phrases[i])
        };
        if sum[i] == 0.0 && named == 0.0 && phrase == 0.0 {
            continue;
        }
        let every_word = if required > 0 && usize::from(matched[i]) == required {
            BONUS_ALL_WORDS
        } else {
            0.0
        };
        hits.push(HelpSearchHit {
            entry,
            score: js_round((sum[i] + phrase + every_word + named) * 100.0) / 100.0,
        });
    }

    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| u16len(&a.entry.task).cmp(&u16len(&b.entry.task)))
            .then_with(|| a.entry.id.cmp(&b.entry.id))
    });
    hits.truncate(limit);
    hits
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entries::all_help;
    use std::sync::atomic::Ordering;

    fn entry(id: &str, task: &str, keyword: &str) -> HelpEntry {
        serde_json::from_value(serde_json::json!({
            "id": id, "shell": "any", "category": "files", "task": task, "summary": "x",
            "command": "frob", "keywords": [keyword]
        }))
        .unwrap()
    }

    #[test]
    fn the_index_is_built_once_per_catalogue_and_not_confused_by_a_reused_address() {
        let before = BUILDS.load(Ordering::SeqCst);
        let throwaway = vec![entry("a", "Frobnicate a widget", "zzz")];
        let opts = HelpSearchOptions::default();
        search_help(&throwaway, "frobnicate", &opts);
        let built = BUILDS.load(Ordering::SeqCst);
        assert!(built > before);
        search_help(&throwaway, "widget", &opts);
        assert_eq!(
            BUILDS.load(Ordering::SeqCst),
            built,
            "the second search rebuilt"
        );
        // The real catalogue: built at most once however many searches follow.
        search_help(all_help(), "kill process", &opts);
        let after_first = BUILDS.load(Ordering::SeqCst);
        search_help(all_help(), "delete a folder", &opts);
        search_help(all_help(), "port 3000", &opts);
        assert_eq!(BUILDS.load(Ordering::SeqCst), after_first);
        // Different text at the same length is a different catalogue.
        let other = vec![entry("b", "Quuxify a gadget", "yyy")];
        assert!(search_help(&other, "gadget", &opts).len() == 1);
        assert!(search_help(&other, "widget", &opts).is_empty());
    }

    #[test]
    fn stages_split_on_every_pipe_kind() {
        assert_eq!(
            stages_of("a | sudo grep x && b y; c || d"),
            vec!["grep", "b", "c", "d"]
        );
        assert!(stages_of("single").is_empty());
    }

    #[test]
    fn flat_command_collapses_javascript_whitespace() {
        assert_eq!(flat_command("  A\t\u{A0}B \n"), "a b");
        assert_eq!(flat_command(""), "");
    }
}

//! Cleaning of heard text (Task 5). Ported rule for rule from Prism Terminal's
//! `core/renderer/lib/dictationClean.ts` at e5848cb (#13).
//!
//! What is done to Whisper's text before it is pasted. Deterministic only (owner's decision: no second
//! model), and deliberately timid: text typed into a prompt cannot be taken back, so every rule here
//! removes something that is certainly not speech, and anything that MIGHT be what the user said is kept.
//! Whisper's own punctuation and capitalisation are never rewritten.
//!
//! ONE RULE IS NOT ABOUT TIDINESS: the result never carries a line break or any other control character.
//! The text travels as a bracketed paste, which an ESC in the text could close early, so control characters
//! go first, and the last thing done is a whitespace collapse that leaves nothing but single spaces.

/// Inside [ ], ( ) or * *, lowercased, with _ and - read as spaces. Whisper writes these for sound that
/// is not speech; nobody dictates them in brackets.
const ARTIFACTS: &[&str] = &[
    "blank audio",
    "silence",
    "no speech",
    "no audio",
    "inaudible",
    "unintelligible",
    "indistinct",
    "crosstalk",
    "music",
    "music playing",
    "applause",
    "clapping",
    "cheering",
    "laughter",
    "laughs",
    "laughing",
    "sighs",
    "sigh",
    "coughs",
    "cough",
    "coughing",
    "sniffs",
    "gasps",
    "groans",
    "breathing",
    "clears throat",
    "noise",
    "background noise",
    "static",
    "beep",
    "click",
    "clicking",
    "typing",
    "keyboard clicking",
    "wind",
    "footsteps",
    "pause",
];

/// Pairs people say on purpose. Small by design: a missed stutter costs the user one backspaced word,
/// a wrongly joined pair changes what they said.
const DELIBERATE_PAIRS: &[&str] = &[
    "that", "had", "bye", "no", "very", "really", "yeah", "ha", "knock", "night", "there",
];

/// The filler alternatives in PT's order, each as runs of one letter (letter, minimum run). Every run
/// is taken as far as it goes: a shorter run would leave a letter of the same family behind the match,
/// which the word-boundary check always refuses, so the greedy reading is the only one that can match.
const FILLER_ALTS: &[&[(char, usize)]] = &[
    &[('u', 1), ('m', 1)],
    &[('u', 1), ('h', 1), ('m', 0)],
    &[('e', 1), ('r', 1), ('m', 1)],
    &[('h', 1), ('m', 1)],
];

/// Letters and digits (PT's `\p{L}` and `\p{N}`, approximated by Rust's `is_alphabetic` and `is_numeric`).
fn is_word(c: char) -> bool {
    c.is_alphabetic() || c.is_numeric()
}

/// What makes a filler or a repeated word a WORD: a letter, a digit, an apostrophe or a hyphen next to it.
fn is_glued(c: char) -> bool {
    is_word(c) || matches!(c, '\'' | '\u{2019}' | '-')
}

/// JavaScript's `\s`: Unicode white space and the byte order mark.
fn js_space(c: char) -> bool {
    c.is_whitespace() || c == '\u{feff}'
}

fn has_word(s: &str) -> bool {
    s.chars().any(is_word)
}

/// `.replace(/\s+/g, ' ').trim()`.
fn squash(s: &str) -> String {
    s.split(js_space)
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Line breaks of every kind, ESC, NUL and the rest of C0 and C1, as spaces.
fn without_controls(raw: &str) -> String {
    raw.chars()
        .map(|c| {
            let n = c as u32;
            if n < 0x20 || (0x7f..=0x9f).contains(&n) || n == 0x2028 || n == 0x2029 {
                ' '
            } else {
                c
            }
        })
        .collect()
}

/// The music notes Whisper brackets singing with. Sharps and flats are not in it.
fn is_note(c: char) -> bool {
    matches!(
        c,
        '\u{2669}'..='\u{266c}' | '\u{1f3b5}' | '\u{1f3b6}' | '\u{1f3bc}'
    )
}

/// "(upbeat music)", "[dramatic music playing]": one describing word, then music, then at most one
/// of a few verbs. Lowercase ASCII only, as the PT pattern is.
fn music_cue(norm: &str) -> bool {
    let is_plain = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_lowercase());
    let suffix = |s: &str| matches!(s, "playing" | "plays" | "continues" | "fades");
    let parts: Vec<&str> = norm.split(' ').collect();
    match parts.as_slice() {
        ["music"] => true,
        ["music", s] => suffix(s),
        [w, "music"] => is_plain(w),
        [w, "music", s] => is_plain(w) && suffix(s),
        _ => false,
    }
}

/// `[ ]`, `(...)`: noise with no word in it, or a listed engine artifact, or a music cue.
fn is_artifact(content: &str) -> bool {
    if !has_word(content) {
        return true;
    }
    let spaced: String = content
        .to_lowercase()
        .chars()
        .map(|c| if c == '_' || c == '-' { ' ' } else { c })
        .collect();
    let norm = squash(&spaced);
    ARTIFACTS.contains(&norm.as_str()) || music_cue(&norm)
}

fn drop_artifact(whole: &str, content: &str) -> String {
    if is_artifact(content) {
        " ".to_string()
    } else {
        whole.to_string()
    }
}

/// `\*[a-z]+(?: [a-z]+){0,2}\*` at `i`: returns the index just past the closing asterisk.
fn stage_cue_end(c: &[char], i: usize) -> Option<usize> {
    if c.get(i) != Some(&'*') {
        return None;
    }
    let n = c.len();
    let mut k = i + 1;
    let start = k;
    while k < n && c[k].is_ascii_alphabetic() {
        k += 1;
    }
    if k == start {
        return None;
    }
    for _ in 0..2 {
        if k < n && c[k] == ' ' {
            let word = k + 1;
            let mut q = word;
            while q < n && c[q].is_ascii_alphabetic() {
                q += 1;
            }
            if q == word {
                break;
            }
            k = q;
        } else {
            break;
        }
    }
    (k < n && c[k] == '*').then_some(k + 1)
}

fn replace_stage_cues(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < c.len() {
        if let Some(end) = stage_cue_end(&c, i) {
            out.push(' ');
            i = end;
        } else {
            out.push(c[i]);
            i += 1;
        }
    }
    out
}

/// `open [^open close]* close`, global, each match replaced by `drop_artifact`.
fn replace_brackets(s: &str, open: char, close: char) -> String {
    let c: Vec<char> = s.chars().collect();
    let n = c.len();
    let mut out = String::new();
    let mut i = 0;
    while i < n {
        if c[i] == open {
            let mut j = i + 1;
            while j < n && c[j] != open && c[j] != close {
                j += 1;
            }
            if j < n && c[j] == close {
                let content: String = c[i + 1..j].iter().collect();
                let whole: String = c[i..=j].iter().collect();
                out.push_str(&drop_artifact(&whole, &content));
                i = j + 1;
                continue;
            }
        }
        out.push(c[i]);
        i += 1;
    }
    out
}

/// FILLER (um, uh, erm, hm in any case) at `p`, with NOT_BEFORE checked: returns the index just past it.
fn filler_end(c: &[char], p: usize) -> Option<usize> {
    'alts: for alt in FILLER_ALTS {
        let mut k = p;
        for &(letter, min) in alt.iter() {
            let start = k;
            while k < c.len() && c[k].to_ascii_lowercase() == letter {
                k += 1;
            }
            if k - start < min {
                continue 'alts;
            }
        }
        if !c.get(k).is_some_and(|&ch| is_glued(ch)) {
            return Some(k);
        }
    }
    None
}

fn is_leading_punct(c: char) -> bool {
    matches!(c, '.' | '!' | '?' | '\u{2026}')
}

fn is_tail_punct(c: char) -> bool {
    matches!(c, ',' | '.' | '!' | '?' | '\u{2026}')
}

/// `(^\s*|[.!?…]\s+)FILLER(?!glued)[,.!?…]*\s*(\p{L})?` at `p`: returns the lead text kept, the letter
/// to capitalise (if any), and the index just past the match.
fn leading_filler_at(c: &[char], p: usize) -> Option<(String, Option<char>, usize)> {
    let n = c.len();
    let mut candidates = Vec::new();
    if p == 0 {
        let mut q = 0;
        while q < n && js_space(c[q]) {
            q += 1;
        }
        candidates.push(q);
    }
    if is_leading_punct(c[p]) {
        let mut q = p + 1;
        while q < n && js_space(c[q]) {
            q += 1;
        }
        if q > p + 1 {
            candidates.push(q);
        }
    }
    for q in candidates {
        if let Some(fe) = filler_end(c, q) {
            let mut r = fe;
            while r < n && is_tail_punct(c[r]) {
                r += 1;
            }
            while r < n && js_space(c[r]) {
                r += 1;
            }
            let (letter, end) = if r < n && c[r].is_alphabetic() {
                (Some(c[r]), r + 1)
            } else {
                (None, r)
            };
            let lead: String = c[p..q].iter().collect();
            return Some((lead, letter, end));
        }
    }
    None
}

/// A filler that LEADS a sentence goes with whatever punctuation hung off it, and hands its capital to
/// the word that now leads instead.
fn lead_filler_pass(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < c.len() {
        match leading_filler_at(&c, i) {
            Some((lead, letter, end)) => {
                out.push_str(&lead);
                if let Some(l) = letter {
                    out.extend(l.to_uppercase());
                }
                i = end;
            }
            None => {
                out.push(c[i]);
                i += 1;
            }
        }
    }
    out
}

/// Anywhere else a filler goes with a comma that directly follows it, and nothing more.
fn filler_word_pass(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let n = c.len();
    let mut out = String::new();
    let mut i = 0;
    while i < n {
        let not_after = i == 0 || !is_glued(c[i - 1]);
        if not_after {
            if let Some(fe) = filler_end(&c, i) {
                let mut e = fe;
                if e < n && c[e] == ',' {
                    e += 1;
                }
                i = e;
                continue;
            }
        }
        out.push(c[i]);
        i += 1;
    }
    out
}

fn same_letter_ignoring_case(a: char, b: char) -> bool {
    a.to_lowercase().eq(b.to_lowercase())
}

/// `NOT_AFTER (\p{L}[\p{L}'’]*)\s+\1 NOT_BEFORE`, global: the second word goes, unless the pair is one
/// people say on purpose.
fn doubled_pass(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let n = c.len();
    let mut out = String::new();
    let mut i = 0;
    while i < n {
        let not_after = i == 0 || !is_glued(c[i - 1]);
        if not_after && c[i].is_alphabetic() {
            let mut g = i + 1;
            while g < n && (c[g].is_alphabetic() || c[g] == '\'' || c[g] == '\u{2019}') {
                g += 1;
            }
            let mut space_end = g;
            while space_end < n && js_space(c[space_end]) {
                space_end += 1;
            }
            let word = &c[i..g];
            let after = space_end + word.len();
            if space_end > g
                && after <= n
                && word
                    .iter()
                    .zip(&c[space_end..after])
                    .all(|(a, b)| same_letter_ignoring_case(*a, *b))
                && !c.get(after).is_some_and(|&ch| is_glued(ch))
            {
                let word_text: String = word.iter().collect();
                if DELIBERATE_PAIRS.contains(&word_text.to_lowercase().as_str()) {
                    out.extend(&c[i..after]);
                } else {
                    out.push_str(&word_text);
                }
                i = after;
                continue;
            }
        }
        out.push(c[i]);
        i += 1;
    }
    out
}

fn is_mark(c: char) -> bool {
    matches!(c, '.' | ',' | '!' | '?' | ';' | ':' | '\u{2026}')
}

/// `([.,!?;:…])\s+([.,])(?=\s|$)`, global: a removal left stranded punctuation; a full stop beats the
/// comma before it, otherwise the first mark stands.
fn stranded_pass(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let n = c.len();
    let mut out = String::new();
    let mut i = 0;
    while i < n {
        if is_mark(c[i]) {
            let mut q = i + 1;
            while q < n && js_space(c[q]) {
                q += 1;
            }
            if q > i + 1
                && q < n
                && (c[q] == '.' || c[q] == ',')
                && (q + 1 >= n || js_space(c[q + 1]))
            {
                out.push(if c[i] == ',' { c[q] } else { c[i] });
                i = q + 1;
                continue;
            }
        }
        out.push(c[i]);
        i += 1;
    }
    out
}

/// The run of marks that starts at `start` (one or more of `\.{3,}`, a single mark, or a dot not followed
/// by a dot), and the index where it ends if the lookahead `(?=\s|$|["'’”)\]])` holds there.
fn mark_group_end(c: &[char], start: usize) -> Option<usize> {
    let n = c.len();
    let mut q = start;
    let mut tokens = 0;
    while q < n {
        if c[q] == '.' {
            let mut d = q;
            while d < n && c[d] == '.' {
                d += 1;
            }
            let run = d - q;
            if run >= 3 {
                q = d;
            } else if run == 1 {
                q += 1;
            } else {
                break;
            }
            tokens += 1;
        } else if matches!(c[q], ',' | '!' | '?' | ';' | ':' | '\u{2026}') {
            q += 1;
            tokens += 1;
        } else {
            break;
        }
    }
    if tokens == 0 {
        return None;
    }
    let ok = q >= n
        || js_space(c[q])
        || matches!(c[q], '"' | '\'' | '\u{2019}' | '\u{201d}' | ')' | ']');
    ok.then_some(q)
}

/// `\s+((?:\.{3,}|[,!?;:…]|\.(?!\.))+)(?=\s|$|["'’”)\]])`, global, replaced by the marks: a space before
/// punctuation goes, but only where the mark ends something.
fn space_before_mark_pass(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let n = c.len();
    let mut out = String::new();
    let mut i = 0;
    while i < n {
        if js_space(c[i]) {
            let mut run_end = i;
            while run_end < n && js_space(c[run_end]) {
                run_end += 1;
            }
            if let Some(q) = mark_group_end(&c, run_end) {
                out.extend(&c[run_end..q]);
                i = q;
                continue;
            }
        }
        out.push(c[i]);
        i += 1;
    }
    out
}

/// `^(?:[,;:\s]|[.!?…]+(?=\s|$))+`: marks left at the very front. A dot is only stripped when it stands
/// alone, so ".gitignore" keeps its own.
fn strip_leading_marks(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let n = c.len();
    let strong = |ch: char| matches!(ch, '.' | '!' | '?' | '\u{2026}');
    let mut i = 0;
    loop {
        if i < n && (matches!(c[i], ',' | ';' | ':') || js_space(c[i])) {
            i += 1;
            continue;
        }
        if i < n && strong(c[i]) {
            let mut j = i;
            while j < n && strong(c[j]) {
                j += 1;
            }
            if j == n || js_space(c[j]) {
                i = j;
                continue;
            }
        }
        break;
    }
    c[i..].iter().collect()
}

/// Apply one global pass until nothing changes (the JavaScript loops `for (prev = ''; prev !== text;)`).
fn to_standstill(mut text: String, pass: fn(&str) -> String) -> String {
    loop {
        let next = pass(&text);
        if next == text {
            return text;
        }
        text = next;
    }
}

fn pass(input: &str) -> String {
    let text: String = input
        .chars()
        .map(|c| if is_note(c) { ' ' } else { c })
        .collect();
    let text = replace_stage_cues(&text);
    let text = replace_brackets(&text, '[', ']');
    let text = replace_brackets(&text, '(', ')');
    // One at a time from the front: each removal can put the NEXT filler at the head of the sentence.
    let text = to_standstill(text, lead_filler_pass);
    let text = filler_word_pass(&text);
    // To a standstill, since a global pass pairs "I I I" as (I I) I and stops.
    let text = to_standstill(text, doubled_pass);
    let text = to_standstill(text, stranded_pass);
    let text = space_before_mark_pass(&text);
    let text = strip_leading_marks(&text);
    squash(&text)
}

/// Whisper's raw text in, the text to paste out. Returns "" when nothing that was said is left (silence,
/// a lone "[BLANK_AUDIO]", a lone "Um."), which the caller shows as "Heard nothing" and pastes nothing for.
pub fn clean_transcript(raw: &str) -> String {
    let mut text = squash(&without_controls(raw));
    // A removal can uncover the next one, so run to a standstill. The cap is only there so a bug cannot
    // hang a paste.
    for _ in 0..8 {
        let next = pass(&text);
        if next == text {
            break;
        }
        text = next;
    }
    if has_word(&text) {
        text
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The PT list of `[ ]`, `( )` and `* *` contents that are engine noise, as Whisper writes them.
    const ARTIFACT_CASES: &[&str] = &[
        "[BLANK_AUDIO]",
        "[ Silence ]",
        "[silence]",
        "(music)",
        "[Music]",
        "[MUSIC PLAYING]",
        "(applause)",
        "[inaudible]",
        "[NO_SPEECH]",
        "(upbeat music)",
        "*sighs*",
        "*clears throat*",
        "\u{266a}",
        "\u{266a}\u{266a} \u{266b}",
        "[ ]",
        "(...)",
        "[BLANK_AUDIO] [BLANK_AUDIO]",
        "[BLANK_AUDIO].",
    ];

    /// Control characters, notes, the bracketed-paste end and friends: a seeded generator glues them
    /// together in orders nobody wrote down, so the rule is tested as a property. (Rust strings cannot
    /// hold a lone surrogate, so the PT piece for one is not in the list.)
    const PIECES: &[&str] = &[
        "\n",
        "\r",
        "\r\n",
        "\n\n\n",
        "\t",
        "\u{b}",
        "\u{c}",
        "\u{2028}",
        "\u{2029}",
        "\u{85}",
        "\u{1b}[201~",
        "\u{0}",
        "\u{7}",
        "\u{a0}",
        " ",
        "  ",
        "um",
        "Um,",
        "uh",
        "[BLANK_AUDIO]",
        "(music)",
        "[",
        "]",
        "(",
        ")",
        "*",
        "*sighs*",
        "\u{266a}",
        "the",
        "the",
        "The",
        "word",
        ".",
        ",",
        "!",
        "?",
        ";",
        ":",
        "...",
        "ls -la",
        "rm -rf /",
        "&&",
        "\"",
        "'",
        "\u{e5}",
        "\u{65e5}\u{672c}\u{8a9e}",
        "\u{1f3b5}",
        "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
    ];

    /// PT's generator: a linear congruential seed, pieces joined with a space two times out of three.
    fn generated(count: usize) -> Vec<String> {
        let mut seed: u32 = 0x02f6_e2b1;
        let mut next = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            seed
        };
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            let n = 1 + (next() % 12) as usize;
            let mut s = String::new();
            for _ in 0..n {
                s.push_str(PIECES[(next() % PIECES.len() as u32) as usize]);
                if next() % 3 != 0 {
                    s.push(' ');
                }
            }
            out.push(s);
        }
        out
    }

    fn assert_no_breaks_or_controls(out: &str, raw: &str) {
        assert!(
            !out.chars().any(|c| matches!(
                c,
                '\n' | '\r' | '\u{2028}' | '\u{2029}' | '\u{85}' | '\u{b}' | '\u{c}'
            )),
            "line break in {out:?} from {raw:?}"
        );
        assert!(
            !out.chars().any(|c| (c as u32) < 0x20 || c as u32 == 0x7f),
            "control character in {out:?} from {raw:?}"
        );
        assert_eq!(out, out.trim(), "untrimmed edge from {raw:?}");
        assert!(!out.contains("  "), "double space in {out:?} from {raw:?}");
    }

    #[test]
    fn leaves_punctuation_and_capitalisation_alone() {
        assert_eq!(
            clean_transcript(" Ask not what your country can do for you."),
            "Ask not what your country can do for you."
        );
        assert_eq!(
            clean_transcript("Wait, really? Yes! It's 3.5 GB; see: the log."),
            "Wait, really? Yes! It's 3.5 GB; see: the log."
        );
    }

    #[test]
    fn does_not_capitalise_a_sentence_whisper_left_lowercase() {
        assert_eq!(clean_transcript("git status"), "git status");
    }

    #[test]
    fn keeps_an_ellipsis() {
        assert_eq!(clean_transcript("Well... maybe."), "Well... maybe.");
    }

    #[test]
    fn artifacts_alone_are_nothing() {
        for raw in ARTIFACT_CASES {
            assert_eq!(clean_transcript(raw), "", "{raw:?}");
        }
    }

    #[test]
    fn takes_artifacts_out_of_the_middle_of_speech() {
        assert_eq!(
            clean_transcript("Run the tests [BLANK_AUDIO] and then push."),
            "Run the tests and then push."
        );
        assert_eq!(clean_transcript("Hello (music) world."), "Hello world.");
        assert_eq!(clean_transcript("Okay *sighs* fine."), "Okay fine.");
        assert_eq!(
            clean_transcript("\u{266a} Happy birthday to you \u{266a}"),
            "Happy birthday to you"
        );
    }

    #[test]
    fn does_not_strip_parentheses_a_speaker_dictated_around_real_words() {
        assert_eq!(
            clean_transcript("Add a note (see the README) at the end."),
            "Add a note (see the README) at the end."
        );
        assert_eq!(
            clean_transcript("Index [0] and step (1)."),
            "Index [0] and step (1)."
        );
        assert_eq!(
            clean_transcript("The (music) folder is separate from (my music library)."),
            "The folder is separate from (my music library)."
        );
    }

    #[test]
    fn does_not_take_a_multiplication_for_a_stage_direction() {
        assert_eq!(clean_transcript("2 * 3 * 4"), "2 * 3 * 4");
        assert_eq!(
            clean_transcript("Match *.ts and *.tsx files"),
            "Match *.ts and *.tsx files"
        );
    }

    #[test]
    fn unwraps_a_nest_of_them() {
        assert_eq!(clean_transcript("[ [music] ] hi"), "hi");
    }

    #[test]
    fn removes_um_uh_erm_uhm_and_hmm_as_whole_words_with_the_comma_that_follows() {
        assert_eq!(
            clean_transcript("I think, um, that works."),
            "I think, that works."
        );
        assert_eq!(
            clean_transcript("So uh we ship it erm today uhm yes hmm."),
            "So we ship it today yes."
        );
        assert_eq!(clean_transcript("It is UM, fine."), "It is fine.");
        assert_eq!(clean_transcript("It is ummm, fine, uhh."), "It is fine.");
    }

    #[test]
    fn does_not_touch_words_that_contain_them() {
        let s = "The umbrella will hum; Uhura hummed, the drummer erred.";
        assert_eq!(clean_transcript(s), s);
    }

    #[test]
    fn leaves_a_hyphenated_word_whole() {
        assert_eq!(
            clean_transcript("He said uh-huh twice."),
            "He said uh-huh twice."
        );
    }

    #[test]
    fn capitalises_a_sentence_its_filler_used_to_lead() {
        assert_eq!(
            clean_transcript("Um, so this is the plan."),
            "So this is the plan."
        );
        assert_eq!(clean_transcript("uh list the files"), "List the files");
        assert_eq!(clean_transcript("Um. Uh, okay then."), "Okay then.");
        assert_eq!(clean_transcript("Done. Um, next one."), "Done. Next one.");
        assert_eq!(clean_transcript("[BLANK_AUDIO] Um, hello."), "Hello.");
    }

    #[test]
    fn leaves_no_orphaned_punctuation_behind() {
        assert_eq!(clean_transcript("Right. Um. Okay."), "Right. Okay.");
        assert_eq!(clean_transcript("That works, uh."), "That works.");
        assert_eq!(clean_transcript("Um."), "");
        assert_eq!(clean_transcript("Um, uh, erm..."), "");
    }

    #[test]
    fn collapses_an_immediate_double_keeping_the_first() {
        assert_eq!(clean_transcript("Open the the file."), "Open the file.");
        assert_eq!(clean_transcript("The the file."), "The file.");
        assert_eq!(clean_transcript("I I I think so."), "I think so.");
        assert_eq!(clean_transcript("go to to the the root"), "go to the root");
    }

    #[test]
    fn collapses_one_a_filler_was_hiding() {
        assert_eq!(
            clean_transcript("Open the, um, the file."),
            "Open the, the file."
        );
        assert_eq!(clean_transcript("Open the um the file."), "Open the file.");
    }

    #[test]
    fn never_touches_digits() {
        assert_eq!(
            clean_transcript("Version 2 2 and 10 10."),
            "Version 2 2 and 10 10."
        );
    }

    #[test]
    fn keeps_the_pairs_people_say_on_purpose() {
        let s = "I know that that is what he had had. No no, bye bye.";
        assert_eq!(clean_transcript(s), s);
    }

    #[test]
    fn does_not_see_a_double_inside_longer_words() {
        assert_eq!(
            clean_transcript("the theme, an answer, is isolated"),
            "the theme, an answer, is isolated"
        );
    }

    #[test]
    fn works_past_ascii() {
        assert_eq!(
            clean_transcript("Jeg s\u{e5} s\u{e5} filen og og \u{e5}pnet den."),
            "Jeg s\u{e5} filen og \u{e5}pnet den."
        );
    }

    #[test]
    fn does_not_reach_across_a_full_stop_or_a_comma() {
        assert_eq!(clean_transcript("Yes. Yes, it is."), "Yes. Yes, it is.");
    }

    #[test]
    fn turns_segment_breaks_and_tabs_into_single_spaces_and_trims() {
        assert_eq!(
            clean_transcript("  First line.\n Second line.\r\n\tThird.  "),
            "First line. Second line. Third."
        );
    }

    #[test]
    fn removes_a_space_before_punctuation() {
        assert_eq!(
            clean_transcript("Hello , world ! Is it ? Yes ; here : now ."),
            "Hello, world! Is it? Yes; here: now."
        );
    }

    #[test]
    fn does_not_glue_a_dotfile_or_a_relative_path_to_the_word_before_it() {
        assert_eq!(
            clean_transcript("open .gitignore and cd .."),
            "open .gitignore and cd .."
        );
        assert_eq!(
            clean_transcript(".gitignore is listed"),
            ".gitignore is listed"
        );
        assert_eq!(clean_transcript("wait ... what"), "wait... what");
    }

    #[test]
    fn returns_nothing_for_nothing() {
        assert_eq!(clean_transcript(""), "");
        assert_eq!(clean_transcript(" \n\t "), "");
        assert_eq!(clean_transcript("..."), "");
    }

    #[test]
    fn never_returns_a_line_break_a_control_character_or_an_untrimmed_edge() {
        let mut inputs: Vec<String> = PIECES.iter().map(|p| (*p).to_string()).collect();
        inputs.push(PIECES.concat());
        inputs.push(PIECES.join("\n"));
        inputs.extend(generated(3000));
        for raw in &inputs {
            let out = clean_transcript(raw);
            assert_no_breaks_or_controls(&out, raw);
        }
    }

    #[test]
    fn is_stable_cleaning_a_cleaned_text_changes_nothing() {
        for raw in generated(1500) {
            let once = clean_transcript(&raw);
            assert_eq!(clean_transcript(&once), once, "{raw:?}");
        }
    }

    #[test]
    fn cannot_be_made_to_close_the_bracketed_paste_it_travels_in() {
        assert!(!clean_transcript("echo hi\u{1b}[201~\nrm -rf x").contains('\u{1b}'));
    }

    #[test]
    fn output_never_holds_esc_or_a_paste_end() {
        for raw in generated(2000) {
            let out = clean_transcript(&raw);
            assert!(!out.contains('\u{1b}'), "ESC in {out:?} from {raw:?}");
            assert!(
                !out.contains("\u{1b}[201~"),
                "paste end in {out:?} from {raw:?}"
            );
            assert_no_breaks_or_controls(&out, &raw);
        }
    }
}

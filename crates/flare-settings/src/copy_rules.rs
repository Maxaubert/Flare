//! The copy rules of the Settings tab, checked by tests (M7 Task 7).
//!
//! Port of Prism Terminal's `core/shared/settingsCopy.ts`. Settings copy is plain words: letters,
//! digits, spaces, commas and full stops only. A subtext is at most eight words and names no key.
//! The one row that breaks the word limit on purpose is See-through (O8, decision D5).

/// A subtext is ONE short line: at most this many words.
pub const SUB_MAX_WORDS: usize = 8;

/// Plain copy: an ASCII letter or digit, a space, a comma or a full stop.
fn is_plain(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == ' ' || c == ',' || c == '.'
}

/// A character of a word for the whole-word test (JavaScript's `\w`).
fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// The symbol rule: every character other than plain copy, distinct, joined by spaces.
fn symbol_problem(text: &str) -> Option<String> {
    if text.chars().all(is_plain) {
        return None;
    }
    let mut bad: Vec<char> = Vec::new();
    for c in text.chars() {
        if !is_plain(c) && !bad.contains(&c) {
            bad.push(c);
        }
    }
    let joined = bad
        .iter()
        .map(char::to_string)
        .collect::<Vec<_>>()
        .join(" ");
    Some(format!(
        "uses symbols other than comma and full stop: {joined}"
    ))
}

/// Whether a whole word is a key name: Ctrl, Control, Shift, Alt, Enter, Escape, Esc, Tab,
/// Backspace, Win, or F followed by one or two digits.
fn is_key_name(word: &str) -> bool {
    const NAMES: [&str; 10] = [
        "Ctrl",
        "Control",
        "Shift",
        "Alt",
        "Enter",
        "Escape",
        "Esc",
        "Tab",
        "Backspace",
        "Win",
    ];
    if NAMES.contains(&word) {
        return true;
    }
    let Some(digits) = word.strip_prefix('F') else {
        return false;
    };
    (1..=2).contains(&digits.len()) && digits.chars().all(|c| c.is_ascii_digit())
}

/// What is wrong with one piece of copy (a label, a subtext or a description), or None.
/// Symbols other than comma and full stop come first, then a whole word that names a key.
pub fn copy_problem(text: &str) -> Option<String> {
    if let Some(problem) = symbol_problem(text) {
        return Some(problem);
    }
    text.split(|c: char| !is_word_char(c))
        .find(|w| is_key_name(w))
        .map(|w| format!("names a key: {w}"))
}

/// Whether a subtext runs past `SUB_MAX_WORDS`.
pub fn sub_too_long(text: &str) -> bool {
    text.split_whitespace().count() > SUB_MAX_WORDS
}

/// What is wrong with a row LABEL, or None: the symbol rule only. "Tab width" names a tab, not the
/// Tab key, so a label is never checked for key names.
pub fn label_problem(text: &str) -> Option<String> {
    symbol_problem(text)
}

/// The text of a quoted literal that starts `s` (single, double or back quote), or None when the
/// literal is unclosed or crosses a line break (the scan reads one line of copy at a time).
fn quoted(s: &str) -> Option<&str> {
    let q = s.chars().next()?;
    if !matches!(q, '\'' | '"' | '`') {
        return None;
    }
    let body = &s[1..];
    let mut escaped = false;
    for (i, c) in body.char_indices() {
        if escaped {
            escaped = false;
            if c == '\n' {
                return None;
            }
            continue;
        }
        match c {
            '\\' => escaped = true,
            '\n' => return None,
            _ if c == q => return Some(&body[..i]),
            _ => {}
        }
    }
    None
}

/// Every `field:` (a whole word, not after `-`, `_` or a letter) whose value is a quoted literal,
/// with the byte position of the field, in source order.
fn field_literals(source: &str, field: &str) -> Vec<(usize, String)> {
    let pat = format!("{field}:");
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(off) = source[from..].find(&pat) {
        let at = from + off;
        from = at + pat.len();
        if source[..at]
            .chars()
            .next_back()
            .is_some_and(|c| is_word_char(c) || c == '-')
        {
            continue;
        }
        if let Some(lit) = quoted(source[from..].trim_start()) {
            out.push((at, lit.to_string()));
        }
    }
    out
}

/// The body of a `{...}` expression that starts `rest` (up to its matching brace, or the end).
fn braced(rest: &str) -> &str {
    let mut depth = 0usize;
    for (i, c) in rest.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &rest[1..i];
                }
            }
            _ => {}
        }
    }
    &rest[1..]
}

/// The index of the closing quote `q` in `body`, with backslash escapes skipped.
fn closing(body: &str, q: char) -> Option<usize> {
    let mut escaped = false;
    for (i, c) in body.char_indices() {
        if escaped {
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == q {
            return Some(i);
        }
    }
    None
}

/// A template literal with its `${...}` holes removed.
fn strip_holes(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(at) = rest.find("${") {
        let Some(len) = rest[at..].find('}') else {
            break;
        };
        out.push_str(&rest[..at]);
        rest = &rest[at + len + 1..];
    }
    out.push_str(rest);
    out
}

/// The literals of a `{...}` expression that are copy: a text with a space, or one that ends in a
/// full stop (a key or a class name is not copy).
fn expr_literals(expr: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = expr;
    while let Some((i, q)) = rest
        .char_indices()
        .find(|(_, c)| matches!(c, '\'' | '"' | '`'))
    {
        let body = &rest[i + q.len_utf8()..];
        let Some(end) = closing(body, q) else {
            break;
        };
        let text = strip_holes(&body[..end]).trim().to_string();
        if text.chars().any(char::is_whitespace) || text.ends_with('.') {
            out.push(text);
        }
        rest = &body[end + q.len_utf8()..];
    }
    out
}

/// The literals of one `attr=` attribute (`attr="..."` or `attr={...}`), with positions.
fn attribute_literals(source: &str, attr: &str) -> Vec<(usize, String)> {
    let pat = format!("{attr}=");
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(off) = source[from..].find(&pat) {
        let at = from + off;
        from = at + pat.len();
        if source[..at].chars().next_back().is_some_and(is_word_char) {
            continue;
        }
        let rest = &source[from..];
        if let Some(body) = rest.strip_prefix('"') {
            if let Some(end) = body.find('"') {
                out.push((at, body[..end].to_string()));
            }
        } else if rest.starts_with('{') {
            out.extend(expr_literals(braced(rest)).into_iter().map(|s| (at, s)));
        }
    }
    out
}

/// The `label:` and `sub:` string literals of an option list or a row table, in source order.
pub fn settings_list_copy(source: &str) -> (Vec<String>, Vec<String>) {
    let labels = field_literals(source, "label")
        .into_iter()
        .map(|(_, s)| s)
        .collect();
    let subs = field_literals(source, "sub")
        .into_iter()
        .map(|(_, s)| s)
        .collect();
    (labels, subs)
}

/// Every description a settings source declares: the literals of `hint=`, `sub=`, `what=` and
/// `warn=` attributes (a `{...}` expression's literals each count), and `sub:` and `note:` fields,
/// in source order.
pub fn settings_descriptions(source: &str) -> Vec<String> {
    let mut found: Vec<(usize, String)> = Vec::new();
    for attr in ["hint", "sub", "what", "warn"] {
        found.extend(attribute_literals(source, attr));
    }
    found.extend(field_literals(source, "sub"));
    found.extend(field_literals(source, "note"));
    found.sort_by_key(|(at, _)| *at);
    found
        .into_iter()
        .map(|(_, s)| s)
        .filter(|s| !s.trim().is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// The row tables of the pages (Task 10).
    const ROWS: &str = include_str!("rows.rs");

    /// The one subtext allowed past the word limit: See-through's nine words (O8, decision D5).
    const SEE_THROUGH_SUB: &str = "How much of the desktop shows behind every surface.";

    /// The sub literal that follows the row id `term-acrylic`, the See-through row (O8).
    fn see_through_sub(source: &str) -> Option<String> {
        let at = source.find("\"term-acrylic\"")?;
        settings_list_copy(&source[at..]).1.into_iter().next()
    }

    /// Every settings source file under `crates/flare/ui/settings`, except `help_panel.slint`
    /// (its words are Prism Terminal's help panel copy, ported verbatim, not settings copy).
    fn slint_settings_sources() -> Vec<(String, String)> {
        let dir: PathBuf = [env!("CARGO_MANIFEST_DIR"), "..", "flare", "ui", "settings"]
            .iter()
            .collect();
        let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
            .expect("the settings ui folder")
            .map(|e| e.expect("a directory entry").path())
            .filter(|p| p.extension().is_some_and(|x| x == "slint"))
            .filter(|p| p.file_name().is_some_and(|n| n != "help_panel.slint"))
            .collect();
        paths.sort();
        paths
            .into_iter()
            .map(|p| {
                let text = std::fs::read_to_string(&p).expect("a settings ui file");
                (p.display().to_string(), text)
            })
            .collect()
    }

    #[test]
    fn plain_words_pass() {
        assert_eq!(
            copy_problem("The shell that new terminals start with."),
            None
        );
        assert_eq!(
            copy_problem("The official CUDA 12.4 build, which runs in under a second."),
            None
        );
    }

    #[test]
    fn any_other_symbol_is_refused() {
        for s in [
            "Needs Windows 11: yes",
            "The theme's colour",
            "Asking Windows\u{2026}",
            "a - b",
            "a (b)",
            "\"quoted\"",
            "50%",
            "x/y",
        ] {
            assert!(copy_problem(s).is_some(), "{s} should be refused");
        }
    }

    #[test]
    fn a_key_name_is_refused_but_not_a_word_containing_one() {
        assert!(
            copy_problem("Press F1 to open it.")
                .unwrap()
                .contains("key")
        );
        assert!(
            copy_problem("Hold Ctrl while you scroll.")
                .unwrap()
                .contains("key")
        );
        assert_eq!(copy_problem("Needs Windows 11."), None);
        assert_eq!(copy_problem("The tabs and the title bar."), None);
    }

    #[test]
    fn descriptions_are_found_in_attributes_and_notes() {
        let src = "sub: \"One.\"\nnote: \"Four five.\"\n<Pref hint=\"Six.\" />\nlet r = Row { sub: \"Two words.\" };";
        assert_eq!(
            settings_descriptions(src),
            vec!["One.", "Four five.", "Six.", "Two words."]
        );
    }

    #[test]
    fn a_subtext_is_held_to_eight_words() {
        assert!(!sub_too_long("Lines between panels and around the window."));
        assert!(!sub_too_long("Speech to text on this PC, never sent."));
        assert!(sub_too_long(
            "The colour of the chosen page, buttons, progress bar and visualizer."
        ));
    }

    #[test]
    fn a_label_is_checked_for_symbols_only() {
        assert_eq!(label_problem("Tab width"), None);
        assert_eq!(label_problem("Exact status from Claude Code"), None);
        assert!(label_problem("The theme's colour").is_some());
    }

    #[test]
    fn list_copy_reads_labels_and_subs_only() {
        let src = "{ id: \"a\", label: \"Tab width\", sub: \"Sized to the name.\", keywords: \"x y\" }\n{ id: \"b\", label: \"Two\", sub: `Three.` }";
        assert_eq!(
            settings_list_copy(src),
            (
                vec!["Tab width".to_string(), "Two".to_string()],
                vec!["Sized to the name.".to_string(), "Three.".to_string()]
            )
        );
    }

    #[test]
    fn the_row_tables_describe_every_setting_in_plain_words() {
        let (labels, subs) = settings_list_copy(ROWS);
        for t in &labels {
            assert_eq!(label_problem(t), None, "label {t}");
        }
        for t in subs.iter().cloned().chain(settings_descriptions(ROWS)) {
            assert_eq!(copy_problem(&t), None, "subtext {t}");
        }
    }

    #[test]
    fn every_subtext_of_the_tables_is_at_most_eight_words_except_the_see_through_one() {
        let (_, subs) = settings_list_copy(ROWS);
        let exception = see_through_sub(ROWS);
        if let Some(e) = &exception {
            assert_eq!(e, SEE_THROUGH_SUB, "the term-acrylic row's subtext changed");
        }
        let long: Vec<&String> = subs
            .iter()
            .filter(|t| sub_too_long(t))
            .filter(|t| Some(*t) != exception.as_ref())
            .collect();
        assert!(long.is_empty(), "subtexts over eight words: {long:?}");
    }

    #[test]
    fn every_label_is_plain() {
        let (labels, _) = settings_list_copy(ROWS);
        let bad: Vec<String> = labels
            .into_iter()
            .filter(|t| label_problem(t).is_some())
            .collect();
        assert!(bad.is_empty(), "labels with symbols: {bad:?}");
    }

    #[test]
    fn the_slint_string_tables_follow_the_rules() {
        let files = slint_settings_sources();
        assert!(!files.is_empty(), "no settings ui files found");
        let exception = see_through_sub(ROWS);
        let mut problems: Vec<String> = Vec::new();
        for (name, src) in &files {
            let (labels, subs) = settings_list_copy(src);
            for t in labels {
                if label_problem(&t).is_some() {
                    problems.push(format!("{name}: label {t}"));
                }
            }
            for t in subs.iter().chain(settings_descriptions(src).iter()) {
                if copy_problem(t).is_some() {
                    problems.push(format!("{name}: {t}"));
                }
            }
            for t in &subs {
                if sub_too_long(t) && Some(t) != exception.as_ref() {
                    problems.push(format!("{name}: over eight words: {t}"));
                }
            }
        }
        assert!(problems.is_empty(), "copy rule problems: {problems:#?}");
    }
}

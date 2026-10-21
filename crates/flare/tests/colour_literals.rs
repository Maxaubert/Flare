//! Scan: no theme colour is typed outside the theme (M4 Task 24).
//!
//! Every `#rrggbb` or `#rrggbbaa` literal in the non-test code of the app crates must be listed in
//! `colour_literals_allow.txt` (`path | literal | reason`). Anything else is a theme value that belongs in
//! `flare-theme` or comes from the look at run time. Test code is not scanned: it may use fixture values.
//! `flare-render` (M1's sample grid colours) and `flare-theme` (the data) are not scanned.

use std::fs;
use std::path::{Path, PathBuf};

const SCANNED: &[&str] = &[
    "flare/src",
    "flare-tabs/src",
    "flare-agent/src",
    "flare-win/src",
    "flare-settings/src",
];

/// The non-test text of a file: everything before its first `#[cfg(test)]` line at column 0.
fn non_test_text(src: &str) -> &str {
    let mut offset = 0;
    for line in src.split_inclusive('\n') {
        if line.trim_end() == "#[cfg(test)]" {
            return &src[..offset];
        }
        offset += line.len();
    }
    src
}

/// Every `#rrggbb` or `#rrggbbaa` literal on one line. The hash must not follow a word character, and the
/// digits must not run on into a letter (so `#abcdefg` and a 7 or 9 digit run are not colours).
fn hex_literals(line: &str) -> Vec<&str> {
    let b = line.as_bytes();
    let mut out = Vec::new();
    for (i, &c) in b.iter().enumerate() {
        if c != b'#' || (i > 0 && b[i - 1].is_ascii_alphanumeric()) {
            continue;
        }
        let digits = b[i + 1..]
            .iter()
            .take_while(|d| d.is_ascii_hexdigit())
            .count();
        let after = b.get(i + 1 + digits);
        let runs_on = after.is_some_and(|d| d.is_ascii_alphanumeric() || *d == b'_');
        if (digits == 6 || digits == 8) && !runs_on {
            out.push(&line[i..i + 1 + digits]);
        }
    }
    out
}

/// The allow list: `(path, literal)` pairs from `colour_literals_allow.txt`.
fn allowed(text: &str) -> Vec<(String, String)> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("//"))
        .filter_map(|l| {
            let mut parts = l.split('|').map(str::trim);
            let path = parts.next()?;
            let literal = parts.next()?;
            let reason = parts.next().unwrap_or("");
            assert!(
                !reason.is_empty(),
                "colour_literals_allow.txt line needs a reason: {l}"
            );
            Some((path.to_string(), literal.to_ascii_lowercase()))
        })
        .collect()
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().is_some_and(|e| e == "rs") {
            out.push(p);
        }
    }
}

/// Every literal in the scanned crates that the allow list does not name, as `path:line literal`.
fn unlisted(root: &Path, allow: &[(String, String)]) -> Vec<String> {
    let mut files = Vec::new();
    for dir in SCANNED {
        walk(&root.join("crates").join(dir), &mut files);
    }
    files.sort();
    let mut hits = Vec::new();
    for f in files {
        let Ok(src) = fs::read_to_string(&f) else {
            continue;
        };
        let rel = f
            .strip_prefix(root)
            .unwrap_or(&f)
            .to_string_lossy()
            .replace('\\', "/");
        for (i, line) in non_test_text(&src).lines().enumerate() {
            for lit in hex_literals(line) {
                let lower = lit.to_ascii_lowercase();
                let listed = allow.iter().any(|(p, l)| *p == rel && *l == lower);
                if !listed {
                    hits.push(format!("{rel}:{} {lit}", i + 1));
                }
            }
        }
    }
    hits
}

#[test]
fn hex_literal_matcher_takes_six_and_eight_digit_colours_only() {
    assert_eq!(hex_literals(r##"let a = "#dedacf";"##), vec!["#dedacf"]);
    assert_eq!(hex_literals("x(\"#ffffff0f\")"), vec!["#ffffff0f"]);
    assert!(hex_literals("a #fff b").is_empty());
    assert!(hex_literals("#abcdefg").is_empty());
    assert!(hex_literals("#abcdefab1").is_empty());
    assert!(hex_literals("#abcde").is_empty());
    assert!(hex_literals("x#abcdef").is_empty());
}

#[test]
fn non_test_text_stops_at_the_first_top_level_cfg_test() {
    let src = "fn a() {}\n#[cfg(test)]\nmod t { const X: &str = \"#123456\"; }\n";
    assert_eq!(non_test_text(src), "fn a() {}\n");
    let indented = "    #[cfg(test)]\n    fn helper() {}\nfn after() {}\n";
    assert_eq!(non_test_text(indented), indented);
}

#[test]
fn no_theme_colour_literal_in_source() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    let allow_text = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("colour_literals_allow.txt"),
    )
    .expect("read colour_literals_allow.txt");
    let hits = unlisted(&root, &allowed(&allow_text));
    assert!(
        hits.is_empty(),
        "theme colours typed outside the theme (take them from flare-theme or the look, or list them with a reason):\n{}",
        hits.join("\n")
    );
}

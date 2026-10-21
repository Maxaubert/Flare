//! The words of the help search (M7 Task 6): the oracle rows from Prism Terminal's own code and
//! the word-level cases of `core/shared/help/search.test.ts`.

use flare_help::words::{expand, meaningful_words, stem, tokenise, within_one_edit};
use serde_json::Value;
use std::collections::BTreeMap;

const WORDS: &str = include_str!("fixtures/words.json");

fn fixture() -> Value {
    serde_json::from_str(WORDS).unwrap()
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap().to_string())
        .collect()
}

fn s(list: &[&str]) -> Vec<String> {
    list.iter().map(|x| x.to_string()).collect()
}

#[test]
fn tokenise_matches_the_oracle() {
    let f = fixture();
    let rows = f["tokenise"].as_array().unwrap();
    assert!(rows.len() >= 60);
    for row in rows {
        let input = row[0].as_str().unwrap();
        assert_eq!(tokenise(input), strings(&row[1]), "tokenise({input:?})");
    }
}

#[test]
fn meaningful_words_match_the_oracle() {
    let f = fixture();
    let rows = f["meaningful"].as_array().unwrap();
    assert_eq!(rows.len(), 80);
    for row in rows {
        let q = row[0].as_str().unwrap();
        assert_eq!(
            meaningful_words(q),
            strings(&row[1]),
            "meaningful_words({q:?})"
        );
    }
}

#[test]
fn stems_match_the_oracle_for_every_catalogue_word() {
    let f = fixture();
    let map: BTreeMap<String, String> = f["stem"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
        .collect();
    assert!(map.len() > 1000, "only {} words", map.len());
    for (word, want) in &map {
        assert_eq!(&stem(word), want, "stem({word:?})");
    }
}

#[test]
fn expand_matches_the_oracle() {
    let f = fixture();
    let map = f["expand"].as_object().unwrap();
    assert!(map.len() > 60);
    for (word, want) in map {
        assert_eq!(expand(word), strings(want), "expand({word:?})");
    }
}

#[test]
fn within_one_edit_matches_the_oracle_pairs() {
    let f = fixture();
    let rows = f["edit"].as_array().unwrap();
    assert_eq!(rows.len(), 200);
    for row in rows {
        let (a, b) = (row[0].as_str().unwrap(), row[1].as_str().unwrap());
        assert_eq!(
            within_one_edit(a, b),
            row[2].as_bool().unwrap(),
            "{a:?} vs {b:?}"
        );
    }
}

#[test]
fn tokenise_lower_cases_strips_punctuation_and_folds_accents() {
    assert_eq!(
        tokenise("How do I find BIG files?!"),
        s(&["how", "do", "i", "find", "big", "files"])
    );
    assert_eq!(
        tokenise("what's using port 3000"),
        s(&["whats", "using", "port", "3000"])
    );
    assert_eq!(
        tokenise("Get-ChildItem | Sort-Object"),
        s(&["get", "childitem", "sort", "object"])
    );
    assert_eq!(tokenise("café"), s(&["cafe"]));
    assert!(tokenise("  ").is_empty());
    assert!(tokenise("🙂🙂 ???").is_empty());
}

#[test]
fn meaningful_words_drop_the_scaffolding_of_a_question() {
    assert_eq!(
        meaningful_words("how do I find big files"),
        s(&["find", "big", "files"])
    );
    assert_eq!(
        meaningful_words("please, what is the way to delete a folder in my shell"),
        s(&["delete", "folder", "shell"])
    );
}

#[test]
fn meaningful_words_keep_the_small_words_that_carry_meaning_here() {
    for w in ["all", "not", "up", "out", "back", "kill", "who", "where"] {
        assert!(
            meaningful_words(&format!("the {w} thing")).contains(&w.to_string()),
            "{w}"
        );
    }
}

#[test]
fn meaningful_words_fall_back_to_every_word_when_nothing_else_is_left() {
    assert_eq!(meaningful_words("how do i"), s(&["how", "do", "i"]));
}

#[test]
fn meaningful_words_say_each_word_once_and_are_bounded() {
    assert_eq!(meaningful_words("file file file"), s(&["file"]));
    let many: Vec<String> = (0..500).map(|i| format!("word{i}")).collect();
    assert!(meaningful_words(&many.join(" ")).len() <= 24);
}

#[test]
fn stem_folds_plurals_ing_ed_er_and_est() {
    let cases = [
        ("biggest", "big"),
        ("bigger", "big"),
        ("largest", "large"),
        ("files", "file"),
        ("running", "run"),
        ("deleted", "delete"),
        ("deleting", "delete"),
        ("directories", "directory"),
        ("processes", "process"),
        ("stopped", "stop"),
        ("killed", "kill"),
        ("renamed", "rename"),
        ("moving", "move"),
        ("copied", "copy"),
        ("branches", "branch"),
        ("using", "use"),
    ];
    for (word, want) in cases {
        assert_eq!(stem(word), want, "stem({word})");
    }
    // "newest files" is about what changed last, "new file" about making one.
    assert_ne!(stem("newest"), "new");
}

#[test]
fn stem_leaves_short_words_command_names_and_look_alikes_alone() {
    for w in [
        "ls", "ps", "cls", "less", "rm", "dir", "dns", "status", "this", "alias", "process",
        "address", "folder", "server", "user", "string", "test", "latest", "speed", "ping",
        "docker", "sha256",
    ] {
        assert_eq!(stem(w), w);
    }
}

#[test]
fn expand_knows_the_everyday_vocabulary_of_a_shell() {
    let has = |word: &str, wanted: &[&str]| {
        let got = expand(word);
        for w in wanted {
            assert!(
                got.iter().any(|g| g == w),
                "expand({word}) lacks {w}: {got:?}"
            );
        }
    };
    has("big", &["large", "huge", "size"]);
    has("remove", &["delete", "erase", "rm", "del"]);
    has("directories", &["folder", "dir"]);
    has("kill", &["stop", "end", "terminate", "quit"]);
    has("sudo", &["admin", "administrator", "elevated"]);
    has("unzip", &["extract", "decompress"]);
    assert!(!expand("unzip").contains(&"zip".to_string()));
}

#[test]
fn expand_never_lists_the_word_itself_and_has_nothing_for_a_stranger() {
    assert!(!expand("big").contains(&"big".to_string()));
    assert!(expand("kubernetes").is_empty());
    assert!(expand("").is_empty());
}

#[test]
fn expand_reads_running_as_a_process_without_making_run_one() {
    assert!(expand("running").contains(&"process".to_string()));
    assert!(expand("run").is_empty());
    assert!(!expand("process").contains(&"run".to_string()));
}

#[test]
fn within_one_edit_accepts_one_slip_and_no_more() {
    assert!(within_one_edit("proccess", "process"));
    assert!(within_one_edit("proces", "process"));
    assert!(within_one_edit("direcotry", "directory"));
    assert!(within_one_edit("directary", "directory"));
    assert!(!within_one_edit("dirctary", "directory"));
    assert!(within_one_edit("process", "process"));
}

// The JavaScript semantics Rust does not give for free.

#[test]
fn lengths_are_counted_in_utf16_units_like_the_original() {
    // An astral letter is two units in JavaScript, so swapping it with a neighbour is more than
    // one slip there, and a word of 20 of them is 40 units, the cap.
    assert!(!within_one_edit("\u{1D400}x", "x\u{1D400}"));
    assert!(!within_one_edit("ab\u{1D400}", "ab"));
    let forty = "a".repeat(40);
    let long = "a".repeat(60);
    assert_eq!(tokenise(&long), vec![forty]);
    let astral = "\u{10428}".repeat(30);
    assert_eq!(tokenise(&astral)[0].chars().count(), 20);
}

#[test]
fn tokenise_keeps_digits_and_other_scripts_as_words() {
    // The breve of the Cyrillic short i is a mark and folds away, as in the original.
    assert_eq!(tokenise("١٢٣ файлы 大きい"), s(&["١٢٣", "фаилы", "大きい"]));
    assert_eq!(tokenise("ﬁle"), s(&["file"]));
}

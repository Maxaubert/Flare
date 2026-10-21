//! The help search held to Prism Terminal's own answers (M7 Task 6): the golden vectors and the 39
//! cases of `core/shared/help/search.test.ts` on its fixture catalogue.

use flare_help::entries::all_help;
use flare_help::search::{HelpSearchHit, HelpSearchOptions, search_help};
use flare_help::types::{HelpCategory, HelpEntry, HelpShell};
use serde::Deserialize;
use std::collections::HashSet;
use std::sync::OnceLock;
use std::time::Instant;

const GOLDEN: &str = include_str!("fixtures/golden_search.json");
const FIXTURE_JSON: &str = include_str!("fixtures/search_fixture.json");

/// Vectors whose unrounded JavaScript score sits within 1e-9 of a half-cent boundary, so a last-bit
/// difference between the two maths libraries may move one cent. At most three, one comment each.
/// None needed so far.
const KNOWN_ULP: [&str; 0] = [];

#[derive(Deserialize)]
struct Vector {
    q: String,
    shell: Option<HelpShell>,
    limit: usize,
    ids: Vec<String>,
    scores: Vec<f64>,
}

#[derive(Deserialize)]
struct Golden {
    queries: Vec<Vector>,
}

#[test]
fn golden_search_ids_and_scores_equal_prism_terminals() {
    let golden: Golden = serde_json::from_str(GOLDEN).unwrap();
    assert!(
        golden.queries.len() > 2000,
        "only {} vectors",
        golden.queries.len()
    );
    let mut failures: Vec<String> = Vec::new();
    for v in &golden.queries {
        if KNOWN_ULP.contains(&v.q.as_str()) {
            continue;
        }
        let opts = HelpSearchOptions {
            shell: v.shell,
            category: None,
            limit: Some(v.limit),
        };
        let hits = search_help(all_help(), &v.q, &opts);
        let ids: Vec<&str> = hits.iter().map(|h| h.entry.id.as_str()).collect();
        let scores: Vec<f64> = hits.iter().map(|h| h.score).collect();
        let same = ids.len() == v.ids.len()
            && ids.iter().zip(&v.ids).all(|(a, b)| *a == b.as_str())
            && scores.iter().zip(&v.scores).all(|(a, b)| a == b);
        if same {
            continue;
        }
        let first = (0..ids.len().max(v.ids.len()))
            .find(|&i| {
                ids.get(i).map(|s| s.to_string()) != v.ids.get(i).cloned()
                    || scores.get(i) != v.scores.get(i)
            })
            .unwrap_or(0);
        failures.push(format!(
            "query {:?} shell {:?} limit {}: first difference at rank {first}\n  flare ids {:?}\n  flare scores {:?}\n  pt ids {:?}\n  pt scores {:?}",
            v.q, v.shell, v.limit, ids, scores, v.ids, v.scores
        ));
        if failures.len() >= 5 {
            break;
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn stable_for_the_same_question() {
    for q in [
        "how do I find big files",
        "delete a folder",
        "port 3000",
        "git undo",
        "zzz",
    ] {
        let a = search_help(all_help(), q, &HelpSearchOptions::default());
        let b = search_help(all_help(), q, &HelpSearchOptions::default());
        let ids = |h: &[HelpSearchHit]| {
            h.iter()
                .map(|x| (x.entry.id.clone(), x.score))
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(&a), ids(&b));
        assert!(a.windows(2).all(|w| w[0].score >= w[1].score));
        let unique: HashSet<&str> = a.iter().map(|h| h.entry.id.as_str()).collect();
        assert_eq!(unique.len(), a.len());
    }
}

// The fixture catalogue of search.test.ts.

fn fixture() -> &'static [HelpEntry] {
    static F: OnceLock<Vec<HelpEntry>> = OnceLock::new();
    F.get_or_init(|| serde_json::from_str(FIXTURE_JSON).unwrap())
}

fn opts(shell: Option<HelpShell>, limit: usize) -> HelpSearchOptions {
    HelpSearchOptions {
        shell,
        category: None,
        limit: Some(limit),
    }
}

fn ids(query: &str, shell: Option<HelpShell>, limit: usize) -> Vec<String> {
    search_help(fixture(), query, &opts(shell, limit))
        .iter()
        .map(|h| h.entry.id.clone())
        .collect()
}

fn top(query: &str, shell: Option<HelpShell>) -> Option<String> {
    ids(query, shell, 1).into_iter().next()
}

fn hit(query: &str, shell: Option<HelpShell>) -> (String, f64) {
    let h = search_help(fixture(), query, &opts(shell, 50));
    (h[0].entry.id.clone(), h[0].score)
}

const PS: Option<HelpShell> = Some(HelpShell::Powershell);
const CMD: Option<HelpShell> = Some(HelpShell::Cmd);
const BASH: Option<HelpShell> = Some(HelpShell::Bash);

fn is(got: Option<String>, want: &str) {
    assert_eq!(got.as_deref(), Some(want));
}

#[test]
fn how_do_i_find_big_files_lands_on_the_biggest_files_entry_of_the_shell_asked_for() {
    is(top("how do I find big files", PS), "ps-biggest-files");
    is(top("how do I find big files", BASH), "bash-biggest-files");
    is(
        top("what is taking up all the space", PS),
        "ps-biggest-files",
    );
    is(top("largest file", PS), "ps-biggest-files");
}

#[test]
fn whats_using_port_3000() {
    is(top("whats using port 3000", PS), "ps-port-in-use");
    is(top("what's using port 3000?", BASH), "bash-port");
    is(top("port already in use", CMD), "cmd-port");
    is(top("EADDRINUSE", PS), "ps-port-in-use");
}

#[test]
fn kill_a_process() {
    is(top("kill a process", PS), "ps-kill-process");
    is(top("kill a process", CMD), "cmd-kill");
    is(top("kill a process", BASH), "bash-kill");
    is(top("program is frozen", PS), "ps-kill-process");
}

#[test]
fn go_back_a_folder() {
    is(top("go back a folder", PS), "ps-up-one-folder");
    is(top("go back a folder", BASH), "bash-up");
    is(
        top("how do i get out of this directory", PS),
        "ps-up-one-folder",
    );
}

#[test]
fn delete_a_folder() {
    is(top("delete a folder", PS), "ps-delete-folder");
    is(top("remove directory", CMD), "cmd-delete-folder");
    is(top("erase a dir", BASH), "bash-delete-folder");
    // The file entry is about deleting too, and must come after.
    let both = ids("delete a folder", PS, 5);
    let at = |id: &str| both.iter().position(|x| x == id).unwrap_or(usize::MAX);
    assert!(at("ps-delete-folder") < at("ps-delete-file"));
}

#[test]
fn grep_finds_select_string_through_its_keywords_and_grep_itself_in_bash() {
    is(top("grep", PS), "ps-search-text");
    is(top("grep", BASH), "bash-grep");
    // With no shell named, the command called grep is the answer.
    is(top("grep", None), "bash-grep");
    assert!(ids("grep", BASH, 5).contains(&"bash-filter-output".to_string()));
}

#[test]
fn an_exact_command_name_puts_that_entry_first() {
    is(top("taskkill", None), "cmd-kill");
    is(top("Get-ChildItem", PS), "ps-list-files");
    is(top("git status", None), "git-status");
    is(top("less", None), "bash-less");
    is(top("pwd", None), "bash-pwd");
    is(top("/compact", None), "claude-compact");
    is(top("/compact", PS), "claude-compact");
    is(top("/comp", CMD), "claude-compact"); // still being typed
}

#[test]
fn undo_last_commit() {
    is(top("undo last commit", None), "git-undo-commit");
    is(top("how can I undo my last commit", PS), "git-undo-commit");
    is(top("discard my changes", BASH), "git-discard");
}

#[test]
fn unzip() {
    is(top("unzip", PS), "ps-unzip");
    is(top("unzip", BASH), "bash-unzip");
    is(top("extract a zip", PS), "ps-unzip");
    is(top("compress a folder", PS), "ps-zip");
}

#[test]
fn where_am_i() {
    is(top("where am i", PS), "ps-where-am-i");
    is(top("where am I?", BASH), "bash-pwd");
    is(top("who am i", PS), "ps-whoami");
}

#[test]
fn see_hidden_files() {
    is(top("see hidden files", PS), "ps-hidden-files");
    is(top("see hidden files", BASH), "bash-hidden");
    is(top("display invisible files", PS), "ps-hidden-files");
}

#[test]
fn other_everyday_phrasings() {
    is(top("what is my ip", PS), "ps-ip-address");
    is(top("create a directory", PS), "ps-make-folder");
    is(top("run as admin", PS), "ps-admin");
    is(top("clean the screen", CMD), "cmd-clear");
    is(top("previous commands", PS), "ps-history");
    is(top("duplicate a file", PS), "ps-copy-file");
    is(top("what is running", PS), "ps-list-processes");
}

#[test]
fn proccess_forgives_one_typo_and_scores_below_the_real_spelling() {
    is(top("kill proccess", PS), "ps-kill-process");
    is(top("make a direcotry", PS), "ps-make-folder");
    is(top("hiden files", BASH), "bash-hidden");
    let typo = hit("kill proccess", PS);
    let right = hit("kill process", PS);
    assert_eq!(typo.0, right.0);
    assert!(typo.1 < right.1);
}

#[test]
fn a_corrected_typo_brings_its_synonyms_along() {
    // Nothing about zipping says "directory": the slip has to be read as "directory" and then as
    // "folder" for every word to have matched.
    let (id, score) = hit("compress a direcotry", PS);
    assert_eq!(id, "ps-zip");
    assert!(score >= 1000.0);
}

#[test]
fn a_short_form_is_the_same_word_as_its_long_form() {
    is(top("run as admin", PS), "ps-admin");
    is(top("run as administrator", PS), "ps-admin");
    assert_eq!(hit("run as admin", PS).1, hit("run as administrator", PS).1);
    is(top("env variable", PS), "ps-env-var");
}

#[test]
fn finds_a_word_that_is_still_being_typed() {
    is(top("unz", PS), "ps-unzip");
    assert!(ids("proc", PS, 5).contains(&"ps-list-processes".to_string()));
}

#[test]
fn ranks_an_entry_matching_every_word_above_one_matching_only_some() {
    let hits = search_help(fixture(), "find big files", &opts(PS, 50));
    let names: Vec<&str> = hits.iter().map(|h| h.entry.id.as_str()).collect();
    assert_eq!(names[0], "ps-biggest-files");
    assert!(names.contains(&"ps-find-file-by-name")); // matches "find" and "files" only
    assert!(hits[0].score > hits[1].score);
}

fn tied(id: &str, task: &str) -> HelpEntry {
    serde_json::from_value(serde_json::json!({
        "id": id, "shell": "any", "category": "files", "task": task, "summary": "x",
        "command": "frob", "keywords": ["zzz"]
    }))
    .unwrap()
}

#[test]
fn breaks_a_tie_by_the_shorter_task_then_by_id() {
    let pair = vec![
        tied("b-two", "Frobnicate a widget now"),
        tied("a-two", "Frobnicate a widget now"),
        tied("c-one", "Frobnicate a widget"),
    ];
    let got: Vec<String> = search_help(&pair, "frobnicate widget", &HelpSearchOptions::default())
        .iter()
        .map(|h| h.entry.id.clone())
        .collect();
    assert_eq!(got, vec!["c-one", "a-two", "b-two"]);
}

#[test]
fn an_empty_query_is_the_catalogue_of_that_shell_in_catalogue_order() {
    let none = HelpSearchOptions::default();
    let all = search_help(fixture(), "", &none);
    let want: Vec<&str> = fixture().iter().map(|x| x.id.as_str()).collect();
    assert_eq!(
        all.iter().map(|h| h.entry.id.as_str()).collect::<Vec<_>>(),
        want
    );
    assert!(all.iter().all(|h| h.score == 0.0));

    let bash = search_help(
        fixture(),
        "   ",
        &HelpSearchOptions {
            shell: BASH,
            ..none
        },
    );
    let want: Vec<&str> = fixture()
        .iter()
        .filter(|x| x.shell == HelpShell::Bash || x.shell == HelpShell::Any)
        .map(|x| x.id.as_str())
        .collect();
    assert_eq!(
        bash.iter().map(|h| h.entry.id.as_str()).collect::<Vec<_>>(),
        want
    );

    let git = search_help(
        fixture(),
        "",
        &HelpSearchOptions {
            shell: BASH,
            category: Some(HelpCategory::Git),
            limit: None,
        },
    );
    assert_eq!(
        git.iter().map(|h| h.entry.id.as_str()).collect::<Vec<_>>(),
        vec!["git-undo-commit", "git-discard", "git-log", "git-status"]
    );
    assert_eq!(
        search_help(
            fixture(),
            "",
            &HelpSearchOptions {
                limit: Some(3),
                ..none
            }
        )
        .len(),
        3
    );
}

#[test]
fn the_shell_filter_keeps_that_shell_and_any_and_nothing_else() {
    for shell in [HelpShell::Powershell, HelpShell::Cmd, HelpShell::Bash] {
        for q in ["delete", "port", "files", "git", "kill", "grep", "ls", ""] {
            for h in search_help(fixture(), q, &opts(Some(shell), 100)) {
                assert!(
                    h.entry.shell == shell || h.entry.shell == HelpShell::Any,
                    "{q}"
                );
            }
        }
    }
    let any = search_help(fixture(), "commit", &opts(Some(HelpShell::Any), 100));
    assert!(!any.is_empty());
    assert!(any.iter().all(|h| h.entry.shell == HelpShell::Any));
}

#[test]
fn the_category_filter_and_the_limit() {
    let hits = search_help(
        fixture(),
        "folder",
        &HelpSearchOptions {
            shell: PS,
            category: Some(HelpCategory::Folders),
            limit: None,
        },
    );
    assert!(!hits.is_empty());
    assert!(
        hits.iter()
            .all(|h| h.entry.category == HelpCategory::Folders)
    );
    assert_eq!(search_help(fixture(), "file", &opts(None, 2)).len(), 2);
    // A limit that makes no sense is no limit.
    let zero = HelpSearchOptions {
        limit: Some(0),
        ..Default::default()
    };
    assert!(search_help(fixture(), "file", &zero).len() > 2);
    assert!(search_help(fixture(), "file", &HelpSearchOptions::default()).len() > 2);
}

#[test]
fn a_query_that_matches_nothing_is_an_empty_list_not_the_catalogue() {
    let none = HelpSearchOptions::default();
    assert!(search_help(fixture(), "xylophone quokka", &none).is_empty());
    assert!(search_help(fixture(), "???", &none).is_empty());
    assert!(search_help(fixture(), "🙂", &none).is_empty());
}

#[test]
fn never_throws_and_always_answers_with_unique_entries_in_score_order() {
    let long_a = "a".repeat(5000);
    let long_q = "find big files ".repeat(400);
    let junk: Vec<&str> = vec![
        "",
        " ",
        "\n\t",
        "🙂🔥💾",
        ".*+?^${}()|[]\\",
        "(((((",
        "[a-z]+$",
        "\\",
        "%PATH% && del /s /q C:\\*",
        "<script>alert(1)</script>",
        "'; DROP TABLE help; --",
        &long_a,
        &long_q,
        "١٢٣ файлы 大きい ファイル",
        "__proto__ constructor toString hasOwnProperty",
        "-",
        "/",
        "0",
        "ﬁle",
    ];
    for q in &junk {
        for shell in [None, PS, BASH, CMD, Some(HelpShell::Any)] {
            let hits = search_help(fixture(), q, &opts(shell, 100));
            let unique: HashSet<&str> = hits.iter().map(|h| h.entry.id.as_str()).collect();
            assert_eq!(unique.len(), hits.len());
            assert!(hits.windows(2).all(|w| w[0].score >= w[1].score));
            assert!(hits.iter().all(|h| h.score.is_finite()));
        }
    }
}

#[test]
fn survives_input_that_is_not_what_the_types_promise() {
    // Entries with every text field empty, as a broken catalogue file could ship.
    let bare: HelpEntry = serde_json::from_value(serde_json::json!({
        "id": "", "shell": "any", "category": "files", "task": "", "summary": "",
        "command": "", "keywords": [""], "variants": [{"label": "", "command": ""}]
    }))
    .unwrap();
    let odd = tied("y", "Find a thing");
    let bad = vec![bare, odd];
    let none = HelpSearchOptions::default();
    assert_eq!(search_help(&bad, "find thing", &none).len(), 1);
    assert!(search_help(&bad, "", &none).len() == 2);
    assert!(search_help(&[], "anything", &none).is_empty());
    assert!(
        search_help(
            fixture(),
            "x",
            &HelpSearchOptions {
                limit: Some(usize::MAX),
                ..none
            }
        )
        .len()
            <= fixture().len()
    );
}

#[test]
fn does_not_read_a_word_off_the_prototype_as_a_hit() {
    let none = HelpSearchOptions::default();
    assert!(search_help(fixture(), "constructor", &none).is_empty());
    assert!(search_help(fixture(), "__proto__", &none).is_empty());
}

#[test]
fn scores_are_sorted_and_results_unique_for_every_real_phrasing_too() {
    for q in [
        "how do I find big files",
        "kill",
        "git",
        "folder",
        "show",
        "a",
        "the",
    ] {
        let hits = search_help(fixture(), q, &HelpSearchOptions::default());
        let unique: HashSet<*const HelpEntry> = hits.iter().map(|h| h.entry as *const _).collect();
        assert_eq!(unique.len(), hits.len());
        assert!(hits.windows(2).all(|w| w[0].score >= w[1].score));
    }
}

#[test]
fn is_stable_the_same_question_gets_the_same_answer() {
    let a: Vec<String> = ids("delete files in a folder", None, 100);
    let mut reversed: Vec<HelpEntry> = fixture().to_vec();
    reversed.reverse();
    let b: Vec<String> = search_help(&reversed, "delete files in a folder", &opts(None, 100))
        .iter()
        .map(|h| h.entry.id.clone())
        .collect();
    assert_eq!(a, b);
}

#[test]
fn three_hundred_entries_and_a_long_question_stay_fast() {
    let big = || -> Vec<HelpEntry> {
        (0..300)
            .map(|i| {
                let mut e = fixture()[i % fixture().len()].clone();
                e.id = format!("{}-{i}", e.id);
                e.task = format!("{} number {i}", e.task);
                e.keywords.push(format!("variant{i}"));
                e.keywords.push(format!("extra word {}", i % 17));
                e
            })
            .collect()
    };
    let long = "how do I find the biggest and largest files in my folders and then delete the hidden ones that are not running as a process on port 3000 while I undo my last commit and unzip the archive please, also direcotry and proccess and whatever elze might be taking up all the space on this machine right now because the disk is completely full";
    let none = HelpSearchOptions::default();
    let median = |mut xs: Vec<f64>| {
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        xs[xs.len() / 2]
    };
    let mut cold = Vec::new();
    for _ in 0..5 {
        let entries = big();
        let t = Instant::now();
        let hits = search_help(&entries, long, &none);
        cold.push(t.elapsed().as_secs_f64() * 1000.0);
        assert!(!hits.is_empty());
    }
    let entries = big();
    search_help(&entries, long, &none);
    let mut warm = Vec::new();
    for i in 0..20 {
        let q = format!("{long} {i}");
        let t = Instant::now();
        search_help(&entries, &q, &none);
        warm.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    // Debug builds are several times slower than the release app; the bounds follow.
    let slack = if cfg!(debug_assertions) { 10.0 } else { 1.0 };
    let (w, c) = (median(warm), median(cold));
    println!("search_help 300 entries: cold median {c:.2} ms, warm median {w:.2} ms");
    assert!(w < 20.0 * slack, "warm {w} ms");
    assert!(c < 150.0 * slack, "cold {c} ms");
}

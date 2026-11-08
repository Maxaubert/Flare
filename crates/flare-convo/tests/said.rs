//! What a plugin with `sessions` hears (plugins phase 5): the entries a transcript line holds, their cleaning and
//! caps, the last entries read from a transcript's end, the tail started at the end, and the follower that hands over
//! only what is written after it started and stops when it is dropped.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use flare_convo::follow::{Follower, MAX_BATCH, Said};
use flare_convo::said::{Entry, READ_LAST, Role, TEXT_MAX, clean, entries_of, read_last};
use flare_convo::tail::Tail;
use flare_convo::track::Source;
use serde_json::json;

fn append(path: &Path, text: &str) {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).unwrap();
    }
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("open for append");
    f.write_all(text.as_bytes()).expect("append");
}

fn user(text: &str) -> String {
    json!({"type": "user", "timestamp": "2026-10-26T10:00:00Z", "message": {"role": "user", "content": text}})
        .to_string()
}

fn reply(text: &str) -> String {
    json!({"type": "assistant", "message": {"content": [{"type": "text", "text": text}]}})
        .to_string()
}

fn texts(es: &[Entry]) -> Vec<(&'static str, String, Option<String>)> {
    es.iter()
        .map(|e| (e.role.name(), e.text.clone(), e.tool.clone()))
        .collect()
}

#[test]
fn a_prompt_a_reply_and_a_tool_call_are_entries_in_order_and_the_rest_is_not() {
    let u = entries_of(&user("Look at the build"));
    assert_eq!(
        u,
        [Entry {
            role: Role::User,
            text: "Look at the build".into(),
            tool: None,
            at: Some("2026-10-26T10:00:00Z".into()),
        }]
    );
    let line = json!({"type": "assistant", "message": {"content": [
        {"type": "thinking", "thinking": "hidden"},
        {"type": "text", "text": "I will run it."},
        {"type": "text", "text": "Then read it."},
        {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "cargo build", "timeout": 5}},
        {"type": "tool_use", "id": "t2", "name": "Edit", "input": {"file_path": "C:\\a\\x.rs", "old_string": "a"}},
        {"type": "tool_use", "id": "t3", "name": "Custom", "input": {"n": 1}},
        {"type": "text", "text": "Done."}
    ]}})
    .to_string();
    assert_eq!(
        texts(&entries_of(&line)),
        [
            ("assistant", "I will run it.\nThen read it.".into(), None),
            ("tool", "cargo build".into(), Some("Bash".into())),
            ("tool", "C:\\a\\x.rs".into(), Some("Edit".into())),
            ("tool", r#"{"n":1}"#.into(), Some("Custom".into())),
            ("assistant", "Done.".into(), None),
        ]
    );
    // A prompt given as blocks; a tool result is not something anyone said.
    let blocks = json!({"type": "user", "message": {"content": [
        {"type": "tool_result", "tool_use_id": "t1", "content": "lots of output"},
        {"type": "text", "text": "and this"}
    ]}})
    .to_string();
    assert_eq!(
        texts(&entries_of(&blocks)),
        [("user", "and this".into(), None)]
    );
    for none in [
        json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "t", "content": "x"}]}})
            .to_string(),
        json!({"type": "user", "isSidechain": true, "message": {"content": "a subagent's prompt"}}).to_string(),
        json!({"type": "user", "isMeta": true, "message": {"content": "Caveat: ..."}}).to_string(),
        json!({"type": "summary", "summary": "x"}).to_string(),
        json!({"type": "user", "message": {"content": "   "}}).to_string(),
        "not json".to_string(),
        "[1,2]".to_string(),
    ] {
        assert!(entries_of(&none).is_empty(), "{none}");
    }
}

#[test]
fn text_is_cleaned_keeps_its_line_breaks_and_is_cut_at_the_cap() {
    assert_eq!(
        clean("  a\u{1b}[31mb\r\nc\rd\u{7}\te\u{9b}  ", 100),
        "a[31mb\nc\nd\te"
    );
    let long = "x".repeat(TEXT_MAX + 50);
    let e = &entries_of(&reply(&long))[0];
    assert_eq!(e.text.chars().count(), TEXT_MAX);
    assert!(e.text.ends_with("..."));
    // Exactly at the cap is not cut; one wide character counts as one.
    let exact = "\u{5b57}".repeat(TEXT_MAX);
    assert_eq!(entries_of(&reply(&exact))[0].text, exact);
    // A tool's name is short and clean too.
    let tool = json!({"type": "assistant", "message": {"content": [
        {"type": "tool_use", "name": format!("Na\u{1b}me{}", "n".repeat(100)), "input": {}}
    ]}})
    .to_string();
    let e = &entries_of(&tool)[0];
    assert_eq!(e.tool.as_deref().unwrap().chars().count(), 64);
    assert!(!e.tool.as_deref().unwrap().contains('\u{1b}'));
    assert_eq!(e.text, "");
}

#[test]
fn read_last_gives_the_last_two_hundred_entries_oldest_first() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.jsonl");
    assert!(
        read_last(&path, READ_LAST).unwrap().is_empty(),
        "no file, no entries"
    );
    let mut text = String::new();
    for i in 0..250 {
        text.push_str(&user(&format!("prompt {i}")));
        text.push('\n');
        text.push_str(&json!({"type": "user", "message": {"content": [{"type": "tool_result", "content": "x"}]}}).to_string());
        text.push('\n');
    }
    append(&path, &text);
    let got = read_last(&path, READ_LAST).unwrap();
    assert_eq!(got.len(), READ_LAST);
    assert_eq!(got[0].text, "prompt 50");
    assert_eq!(got[READ_LAST - 1].text, "prompt 249");
    assert_eq!(read_last(&path, 3).unwrap().len(), 3);
}

#[test]
fn read_last_reads_only_the_end_of_a_big_transcript_and_drops_the_line_it_cut() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.jsonl");
    // Five MB of one prompt per line, then the last few: only the end is read, and no half line becomes an entry.
    let filler = user(&"f".repeat(1000));
    let mut text = String::new();
    while text.len() < 5 * 1024 * 1024 {
        text.push_str(&filler);
        text.push('\n');
    }
    for i in 0..3 {
        text.push_str(&user(&format!("last {i}")));
        text.push('\n');
    }
    append(&path, &text);
    let got = read_last(&path, READ_LAST).unwrap();
    assert_eq!(got.len(), READ_LAST);
    assert_eq!(got[READ_LAST - 1].text, "last 2");
    assert!(got.iter().all(|e| e.role == Role::User));
}

#[test]
fn a_tail_started_at_the_end_reads_only_what_comes_next() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.jsonl");
    append(&path, "{\"a\":1}\n{\"a\":2}\n{\"half");
    let mut tail = Tail::new(&path);
    tail.skip_to_end().unwrap();
    assert!(tail.poll().unwrap().lines.is_empty());
    // The half line it started in is dropped up to its newline, and not counted as a long one.
    append(&path, "\":3}\n{\"a\":4}\n");
    let b = tail.poll().unwrap();
    assert_eq!(b.lines, ["{\"a\":4}"]);
    assert_eq!(b.skipped_long, 0);
    // A file that is not there yet: everything written to it is new.
    let mut fresh = Tail::new(dir.path().join("later.jsonl"));
    fresh.skip_to_end().unwrap();
    append(&dir.path().join("later.jsonl"), "{\"b\":1}\n");
    assert_eq!(fresh.poll().unwrap().lines, ["{\"b\":1}"]);
}

/// A follower whose batches land in a list.
fn follow(src: Source) -> (Follower, Arc<Mutex<Vec<Said>>>) {
    let got: Arc<Mutex<Vec<Said>>> = Arc::default();
    let sink = Arc::clone(&got);
    let f = Follower::start(src, Arc::new(move |s| sink.lock().unwrap().push(s)));
    (f, got)
}

fn wait_for(what: &str, mut cond: impl FnMut() -> bool) {
    let t = Instant::now();
    while !cond() {
        assert!(
            t.elapsed() < Duration::from_secs(8),
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Appends `<tag> N` lines until one of them is handed over: from then on the follower is reading
/// the file's live end, and every line written after must come through. Lines written before it got
/// there (the start, a rewrite) are rightly not news, so a probe may be dropped; the next one is not.
fn probe_through(path: &Path, got: &Mutex<Vec<Said>>, tag: &str) {
    let prefix = format!("{tag} ");
    let t = Instant::now();
    let mut n = 0;
    while !all_texts(got).iter().any(|x| x.starts_with(&prefix)) {
        assert!(
            t.elapsed() < Duration::from_secs(20),
            "no {tag} line came through"
        );
        append(path, &format!("{}\n", user(&format!("{tag} {n}"))));
        n += 1;
        std::thread::sleep(Duration::from_millis(300));
    }
    // The follower is live now, so the last probe arrives too: wait for it, so no probe trails behind.
    let last = format!("{tag} {}", n - 1);
    wait_for("the last probe", || all_texts(got).contains(&last));
}

fn all_texts(got: &Mutex<Vec<Said>>) -> Vec<String> {
    got.lock()
        .unwrap()
        .iter()
        .flat_map(|s| s.entries.iter().map(|e| e.text.clone()))
        .collect()
}

#[test]
fn the_follower_hands_over_only_what_is_written_after_it_started_and_stops_when_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("projects").join("p").join("s.jsonl");
    append(
        &path,
        &format!("{}\n{}\n", user("old one"), reply("old reply")),
    );
    let src = Source {
        transcript: path.clone(),
        claude_dir: dir.path().to_path_buf(),
        session: "s".into(),
    };
    let (f, got) = follow(src);
    // The worker settles at the file's end when its thread runs, which a busy runner can delay past any
    // fixed wait (a line written before that is rightly not news). Probe until a line comes through.
    probe_through(&path, &got, "start");
    let before = all_texts(&got).len();
    append(
        &path,
        &format!("{}\n{}\n", user("new one"), reply("new reply")),
    );
    wait_for("the new entries", || all_texts(&got).len() >= before + 2);
    let texts = all_texts(&got);
    assert_eq!(&texts[before..], ["new one", "new reply"]);
    assert!(
        texts[..before].iter().all(|x| x.starts_with("start ")),
        "only probes before: the old lines are never handed over, {texts:?}"
    );
    // A rewrite is not news: the old lines are not handed over again.
    std::fs::write(&path, format!("{}\n", user("rewritten history"))).unwrap();
    // A line written before the follower has seen the rewrite is part of the rewritten file, which is
    // not news, so it is rightly dropped. A fixed 600 ms wait was a guess at when the follower had
    // looked, and a busy runner's worker missed it. Instead, probe until one line comes through: from
    // then on the follower is past the rewrite and every new line must be handed over.
    let mid = all_texts(&got).len();
    probe_through(&path, &got, "probe");
    append(&path, &format!("{}\n", user("after the rewrite")));
    wait_for("the line after the rewrite", || {
        all_texts(&got).iter().any(|x| x == "after the rewrite")
    });
    let texts = all_texts(&got);
    assert_eq!(texts.last().map(String::as_str), Some("after the rewrite"));
    assert!(
        texts[mid..texts.len() - 1]
            .iter()
            .all(|x| x.starts_with("probe ")),
        "{texts:?}"
    );
    assert!(!texts.iter().any(|t| t == "rewritten history"));
    let exit = f.exit_flag();
    drop(f);
    wait_for("the worker to end", || exit.load(Ordering::SeqCst));
}

#[test]
fn a_transcript_that_appears_later_is_read_whole_and_a_big_batch_keeps_its_newest() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("projects").join("p").join("s.jsonl");
    let (f, got) = follow(Source {
        transcript: path.clone(),
        claude_dir: dir.path().to_path_buf(),
        session: "s".into(),
    });
    std::thread::sleep(Duration::from_millis(100));
    let mut text = String::new();
    for i in 0..(MAX_BATCH + 20) {
        text.push_str(&user(&format!("p{i}")));
        text.push('\n');
    }
    append(&path, &text);
    wait_for("the batch", || !got.lock().unwrap().is_empty());
    let first = got.lock().unwrap()[0].clone();
    assert_eq!(first.entries.len(), MAX_BATCH);
    assert_eq!(first.skipped, 20);
    assert_eq!(first.entries[0].text, "p20");
    drop(f);
}

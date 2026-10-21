//! Tail tests on real temporary files: growth, partial lines, truncation, replacement, long lines, the worker.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use flare_convo::tail::{MAX_LINE, MAX_READ, Tail, subscribe};
use flare_convo::{find_transcript, valid_session_id};

fn append(path: &Path, bytes: &[u8]) {
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("open for append");
    f.write_all(bytes).expect("append");
}

#[test]
fn a_missing_file_is_an_empty_batch() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut tail = Tail::new(dir.path().join("not-yet.jsonl"));
    let batch = tail.poll().expect("no error for a missing file");
    assert!(batch.lines.is_empty());
    assert!(!batch.restarted);
}

#[test]
fn reads_only_what_was_appended_since_the_last_poll() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("s.jsonl");
    append(&path, b"{\"a\":1}\n{\"a\":2}\n");
    let mut tail = Tail::new(&path);
    assert_eq!(tail.poll().unwrap().lines, vec!["{\"a\":1}", "{\"a\":2}"]);
    assert!(tail.poll().unwrap().lines.is_empty(), "nothing new");
    append(&path, b"{\"a\":3}\n");
    assert_eq!(tail.poll().unwrap().lines, vec!["{\"a\":3}"]);
}

#[test]
fn a_partial_last_line_waits_for_its_newline() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("s.jsonl");
    append(&path, b"{\"first\":true}\n{\"sec");
    let mut tail = Tail::new(&path);
    assert_eq!(tail.poll().unwrap().lines, vec!["{\"first\":true}"]);
    assert!(
        tail.poll().unwrap().lines.is_empty(),
        "the half line is not a line yet"
    );
    append(&path, b"ond\":true}\r\n");
    assert_eq!(
        tail.poll().unwrap().lines,
        vec!["{\"second\":true}"],
        "the carriage return is stripped and the two halves join"
    );
}

#[test]
fn a_bounded_read_says_there_is_more_and_the_rest_comes_next() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("big.jsonl");
    // Lines of about 1 KB, enough to pass one MAX_READ chunk.
    let line = format!("{{\"pad\":\"{}\"}}\n", "x".repeat(1000));
    let count = MAX_READ / line.len() + 50;
    let body = line.repeat(count);
    fs::write(&path, body.as_bytes()).expect("write");
    let mut tail = Tail::new(&path);
    let first = tail.poll().unwrap();
    assert!(first.more, "more bytes remain after one bounded read");
    assert!(
        first.lines.len() < count,
        "one poll does not read the whole file"
    );
    let mut total = first.lines.len();
    let mut batch = first;
    while batch.more {
        batch = tail.poll().unwrap();
        total += batch.lines.len();
    }
    assert_eq!(total, count, "every line arrives across the polls");
}

#[test]
fn a_truncated_file_restarts_from_zero() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("s.jsonl");
    append(&path, b"{\"old\":1}\n{\"old\":2}\n");
    let mut tail = Tail::new(&path);
    assert_eq!(tail.poll().unwrap().lines.len(), 2);
    fs::write(&path, b"{\"new\":1}\n").expect("truncate and rewrite");
    let batch = tail.poll().unwrap();
    assert!(batch.restarted, "shorter than the offset means a restart");
    assert_eq!(batch.lines, vec!["{\"new\":1}"]);
}

#[test]
fn a_replaced_file_restarts_even_when_it_is_longer() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("s.jsonl");
    append(&path, b"{\"v\":1}\n");
    let mut tail = Tail::new(&path);
    assert_eq!(tail.poll().unwrap().lines.len(), 1);
    // Delete and create anew: a new file id (creation time), so the offset is not trusted.
    fs::remove_file(&path).expect("remove");
    let longer = format!(
        "{}\n",
        "{\"v\":2,\"pad\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"}"
    );
    fs::write(&path, longer.as_bytes()).expect("write a longer file");
    let batch = tail.poll().unwrap();
    assert!(batch.restarted, "a new file is read from its start");
    assert_eq!(batch.lines.len(), 1);
    assert!(batch.lines[0].contains("\"v\":2"));
}

#[test]
fn a_line_longer_than_the_cap_is_skipped_and_counted_and_the_next_line_reads() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("s.jsonl");
    let mut huge = vec![b'h'; MAX_LINE + 1024];
    huge.push(b'\n');
    append(&path, b"{\"before\":1}\n");
    append(&path, &huge);
    append(&path, b"{\"after\":1}\n");
    let mut tail = Tail::new(&path);
    let mut lines = Vec::new();
    let last_skipped = loop {
        let batch = tail.poll().unwrap();
        lines.extend(batch.lines.clone());
        if !batch.more {
            break batch.skipped_long;
        }
    };
    assert_eq!(lines, vec!["{\"before\":1}", "{\"after\":1}"]);
    assert_eq!(last_skipped, 1);
    assert_eq!(tail.skipped_long(), 1);
}

#[test]
fn a_bad_utf8_byte_is_replaced_not_fatal() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("s.jsonl");
    append(&path, b"{\"t\":\"\xff\"}\n");
    let mut tail = Tail::new(&path);
    let lines = tail.poll().unwrap().lines;
    assert_eq!(lines.len(), 1);
    assert!(lines[0].contains('\u{fffd}'));
}

#[test]
fn the_worker_delivers_growth_and_stops_when_the_subscription_drops() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("live.jsonl");
    append(&path, b"{\"n\":1}\n");
    let sub = subscribe(&path);
    let first = sub
        .next_timeout(Duration::from_secs(5))
        .expect("the first line arrives");
    assert_eq!(first.lines, vec!["{\"n\":1}"]);
    append(&path, b"{\"n\":2}\n");
    let second = sub
        .next_timeout(Duration::from_secs(5))
        .expect("growth arrives within a poll");
    assert_eq!(second.lines, vec!["{\"n\":2}"]);
    drop(sub);
    // The worker sees the stop flag within a slice; nothing more is sent to a dropped subscription.
    std::thread::sleep(Duration::from_millis(100));
}

#[test]
fn a_subscription_with_nothing_new_sends_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("quiet.jsonl");
    append(&path, b"{\"q\":1}\n");
    let sub = subscribe(&path);
    assert!(sub.next_timeout(Duration::from_secs(5)).is_some());
    assert!(
        sub.next_timeout(Duration::from_millis(1200)).is_none(),
        "no news, no batch"
    );
}

#[test]
fn a_session_id_must_be_a_plain_name() {
    assert!(valid_session_id("3f2a9c1e-77aa-4c0d-9b1e-0a1b2c3d4e5f"));
    assert!(!valid_session_id("a/b"));
    assert!(!valid_session_id("a.b"));
    assert!(!valid_session_id(""));
}

#[test]
fn the_fallback_finds_the_session_under_any_project_folder() {
    let home = tempfile::tempdir().expect("tempdir");
    let folder = home
        .path()
        .join(".claude")
        .join("projects")
        .join("C--Work-elsewhere");
    fs::create_dir_all(&folder).expect("mkdir");
    let id = "9b1e0a1b-2c3d-4e5f-8a9b-0c1d2e3f4a5b";
    fs::write(folder.join(format!("{id}.jsonl")), b"{}\n").expect("write");
    let found = find_transcript(home.path(), id).expect("found under another folder");
    assert!(found.ends_with(format!("{id}.jsonl")));
    assert!(find_transcript(home.path(), "00000000-0000-0000-0000-000000000000").is_none());
    assert!(find_transcript(home.path(), "../x").is_none());
}

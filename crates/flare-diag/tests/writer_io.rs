//! M9 Task 18: the asynchronous log writer, against real files in a temp folder.

use flare_diag::writer::{DiagLog, DiagLogOptions, LineSink, DIAG_FILE, QUEUE_MAX};
use serde_json::{json, Map, Value};
use std::fs::{self, OpenOptions};
use std::os::windows::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

fn fields(pairs: &[(&str, Value)]) -> Map<String, Value> {
    let mut m = Map::new();
    for (k, v) in pairs {
        m.insert((*k).to_string(), v.clone());
    }
    m
}

fn opts(dir: &Path) -> DiagLogOptions {
    let mut o = DiagLogOptions::new(dir.join("logs"));
    o.flush_ms = 3_600_000; // the tests flush by hand
    o.now = Arc::new(|| 1_700_000_000_123);
    o
}

fn lines_of(path: &Path) -> Vec<Value> {
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .map(|l| serde_json::from_str(l).expect("a line is JSON"))
        .collect()
}

fn live(dir: &Path) -> std::path::PathBuf {
    dir.join("logs").join(DIAG_FILE)
}

fn kinds_of(dir: &Path) -> Vec<String> {
    lines_of(&live(dir))
        .iter()
        .map(|l| l["k"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn writes_one_flat_json_object_per_line_with_t_up_src_and_kind() {
    let tmp = tempfile::tempdir().unwrap();
    let mut o = opts(tmp.path());
    o.uptime = Arc::new(|| 5_000);
    let log = DiagLog::start(o);
    log.write(
        "main",
        "hello",
        fields(&[("a", json!(1)), ("b", json!("x"))]),
    );
    log.flush();
    let text = fs::read_to_string(live(tmp.path())).unwrap();
    assert!(text.ends_with('\n'));
    let line: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    let keys: Vec<&str> = line
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["t", "up", "src", "k", "a", "b"]);
    assert_eq!(line["t"], "2023-11-14T22:13:20.123Z");
    assert_eq!(line["up"], 5_000);
    assert_eq!(line["src"], "main");
    assert_eq!(line["k"], "hello");
}

#[test]
fn batches_the_appends_and_keeps_their_order() {
    let tmp = tempfile::tempdir().unwrap();
    let writes = Arc::new(AtomicU64::new(0));
    let mut o = opts(tmp.path());
    let w = writes.clone();
    o.on_file_write = Some(Arc::new(move || {
        w.fetch_add(1, Ordering::SeqCst);
    }));
    let log = DiagLog::start(o);
    let mut hs = Vec::new();
    for t in 0..4 {
        let log = log.clone();
        hs.push(std::thread::spawn(move || {
            for i in 0..25 {
                log.write("main", "n", fields(&[("th", json!(t)), ("i", json!(i))]));
            }
        }));
    }
    for h in hs {
        h.join().unwrap();
    }
    log.flush();
    let lines = lines_of(&live(tmp.path()));
    assert_eq!(lines.len(), 100);
    for t in 0..4 {
        let mine: Vec<i64> = lines
            .iter()
            .filter(|l| l["th"] == t)
            .map(|l| l["i"].as_i64().unwrap())
            .collect();
        assert_eq!(mine, (0..25).collect::<Vec<_>>());
    }
    assert_eq!(writes.load(Ordering::SeqCst), 1, "one file write per batch");
}

#[test]
fn flush_sync_at_quit_writes_the_last_line_before_a_crash() {
    let tmp = tempfile::tempdir().unwrap();
    let log = DiagLog::start(opts(tmp.path()));
    log.write("main", "last", Map::new());
    log.flush_sync();
    assert_eq!(kinds_of(tmp.path()), ["last"]);
}

#[test]
fn rotates_at_the_cap_to_1_through_4_so_there_are_never_more_than_five_files() {
    let tmp = tempfile::tempdir().unwrap();
    let mut o = opts(tmp.path());
    o.max_bytes = 2_000;
    let log = DiagLog::start(o);
    for i in 0..60 {
        log.write(
            "main",
            "fill",
            fields(&[("pad", json!("x".repeat(100))), ("i", json!(i))]),
        );
        log.flush();
    }
    let dir = tmp.path().join("logs");
    let names: Vec<String> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(names.contains(&"diag.jsonl".to_string()));
    for n in 1..=4 {
        assert!(names.contains(&format!("diag.jsonl.{n}")), "{names:?}");
    }
    assert!(!names.contains(&"diag.jsonl.5".to_string()));
    assert_eq!(names.len(), 5);
    assert!(fs::metadata(live(tmp.path())).unwrap().len() <= 2_000 + 200);
}

#[test]
fn keeps_writing_when_a_rotation_fails_says_so_once_and_rotates_again_later() {
    let tmp = tempfile::tempdir().unwrap();
    let up = Arc::new(AtomicU64::new(1_000));
    let mut o = opts(tmp.path());
    o.max_bytes = 600;
    let u = up.clone();
    o.uptime = Arc::new(move || u.load(Ordering::SeqCst));
    let log = DiagLog::start(o);
    let pad = "y".repeat(200);
    log.write("main", "a", fields(&[("pad", json!(pad.clone()))]));
    log.flush();
    // Hold the live file: others may read and write it, but not rename or delete it.
    let held = OpenOptions::new()
        .read(true)
        .share_mode(3)
        .open(live(tmp.path()))
        .unwrap();
    for k in ["b", "c", "d"] {
        log.write("main", k, fields(&[("pad", json!(pad.clone()))]));
        log.flush();
    }
    assert!(log.failed());
    log.flush();
    let kinds = kinds_of(tmp.path());
    // The log keeps writing while the rotation fails, up to twice the cap (past it lines are dropped and
    // counted: a held file must not grow without bound).
    for k in ["a", "b", "c"] {
        assert!(kinds.iter().any(|x| x == k), "{kinds:?}");
    }
    assert!(
        kinds.iter().any(|x| x == "d" || x == "logger-dropped"),
        "{kinds:?}"
    );
    assert_eq!(kinds.iter().filter(|k| *k == "logger-error").count(), 1);
    assert!(!tmp.path().join("logs").join("diag.jsonl.1").exists());
    drop(held);
    up.fetch_add(30_001, Ordering::SeqCst);
    log.write("main", "e", fields(&[("pad", json!(pad))]));
    log.flush();
    assert!(tmp.path().join("logs").join("diag.jsonl.1").exists());
}

#[test]
fn a_rotation_that_keeps_failing_never_lets_the_live_file_grow_past_twice_the_cap() {
    let tmp = tempfile::tempdir().unwrap();
    let up = Arc::new(AtomicU64::new(1_000));
    let mut o = opts(tmp.path());
    o.max_bytes = 600;
    let u = up.clone();
    o.uptime = Arc::new(move || u.load(Ordering::SeqCst));
    let log = DiagLog::start(o);
    let pad = "y".repeat(200);
    log.write("main", "a", fields(&[("pad", json!(pad.clone()))]));
    log.flush();
    let held = OpenOptions::new()
        .read(true)
        .share_mode(3)
        .open(live(tmp.path()))
        .unwrap();
    for _ in 0..30 {
        log.write("main", "b", fields(&[("pad", json!(pad.clone()))]));
        log.flush();
    }
    let len = fs::metadata(live(tmp.path())).unwrap().len();
    assert!(
        len <= 2 * 600 + 300,
        "the held file grew to {len} bytes: nothing bounded it"
    );
    drop(held);
    up.fetch_add(30_001, Ordering::SeqCst);
    log.write("main", "e", fields(&[("pad", json!(pad))]));
    log.flush();
    log.flush();
    let all = lines_of(&live(tmp.path()));
    let dropped: u64 = all
        .iter()
        .filter(|l| l["k"] == "logger-dropped")
        .map(|l| l["n"].as_u64().unwrap_or(0))
        .sum();
    assert!(
        dropped > 0,
        "what was dropped is said once the log recovers: {all:?}"
    );
}

#[test]
fn a_blocked_rotation_that_retries_never_costs_an_older_generation() {
    let tmp = tempfile::tempdir().unwrap();
    let up = Arc::new(AtomicU64::new(1_000));
    let mut o = opts(tmp.path());
    o.max_bytes = 600;
    let u = up.clone();
    o.uptime = Arc::new(move || u.load(Ordering::SeqCst));
    let log = DiagLog::start(o);
    let pad = "y".repeat(200);
    log.write("main", "a", fields(&[("pad", json!(pad.clone()))]));
    log.flush();
    // Four generations of history already exist.
    let dir = tmp.path().join("logs");
    for n in 1..=4 {
        fs::write(dir.join(format!("diag.jsonl.{n}")), format!("gen{n}")).unwrap();
    }
    // Something holds the live file: it cannot be renamed.
    let held = OpenOptions::new()
        .read(true)
        .share_mode(3)
        .open(live(tmp.path()))
        .unwrap();
    for round in 0..5u64 {
        up.fetch_add(30_001, Ordering::SeqCst);
        log.write("main", "b", fields(&[("pad", json!(pad.clone()))]));
        log.flush();
        for n in 1..=4 {
            assert_eq!(
                fs::read_to_string(dir.join(format!("diag.jsonl.{n}"))).unwrap(),
                format!("gen{n}"),
                "generation {n} was lost on retry {round}"
            );
        }
    }
    drop(held);
}

#[test]
fn a_rotation_that_fails_after_the_park_puts_the_live_file_back() {
    let tmp = tempfile::tempdir().unwrap();
    let up = Arc::new(AtomicU64::new(1_000));
    let mut o = opts(tmp.path());
    o.max_bytes = 600;
    let u = up.clone();
    o.uptime = Arc::new(move || u.load(Ordering::SeqCst));
    let log = DiagLog::start(o);
    let pad = "y".repeat(200);
    log.write("main", "a", fields(&[("pad", json!(pad.clone()))]));
    log.flush();
    let dir = tmp.path().join("logs");
    for n in 1..=4 {
        fs::write(dir.join(format!("diag.jsonl.{n}")), format!("gen{n}")).unwrap();
    }
    // Something holds the oldest generation without share-delete: the live file parks, then
    // removing .4 fails.
    let held = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(dir.join("diag.jsonl.4"))
        .unwrap();
    for round in 0..4u64 {
        up.fetch_add(30_001, Ordering::SeqCst);
        log.write("main", "b", fields(&[("pad", json!(pad.clone()))]));
        log.flush();
        assert!(
            !dir.join("diag.jsonl.rotating").exists(),
            "the parked file was left behind on retry {round}"
        );
        let kinds = kinds_of(tmp.path());
        assert!(
            kinds.iter().any(|k| k == "a"),
            "history lost on retry {round}: {kinds:?}"
        );
    }
    assert!(log.failed());
    drop(held);
    up.fetch_add(30_001, Ordering::SeqCst);
    log.write("main", "c", fields(&[("pad", json!(pad))]));
    log.flush();
    let first = fs::read_to_string(dir.join("diag.jsonl.1")).unwrap();
    assert!(
        first.contains("\"k\":\"a\""),
        "the newest history went to .1"
    );
}

#[test]
fn a_refused_verbose_write_leaves_the_switch_as_it_was() {
    let tmp = tempfile::tempdir().unwrap();
    // A folder where the state file should be: the atomic write is refused.
    fs::create_dir(tmp.path().join("diag.json")).unwrap();
    let log = DiagLog::start(opts(tmp.path()));
    assert!(!log.verbose());
    log.set_verbose(true);
    assert!(
        !log.verbose(),
        "the page must not claim a switch that was not stored"
    );
}

#[test]
fn a_write_never_does_io_on_the_calling_thread() {
    // Review fix: this was a 100 ms stopwatch over 10,000 writes, which failed under the full
    // parallel suite. The rule is checked directly now: every file append records its thread, and
    // none is the caller's. The stopwatch stays as a loose sanity bound.
    let tmp = tempfile::tempdir().unwrap();
    let mut o = opts(tmp.path());
    o.flush_ms = 10;
    let appenders = Arc::new(std::sync::Mutex::new(Vec::new()));
    let rec = appenders.clone();
    o.on_file_write = Some(Arc::new(move || {
        rec.lock().unwrap().push(std::thread::current().id());
    }));
    let log = DiagLog::start(o);
    log.write("main", "first", Map::new());
    log.flush();
    let me = std::thread::current().id();
    appenders.lock().unwrap().clear();
    let held = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(live(tmp.path()))
        .unwrap();
    let t0 = Instant::now();
    for _ in 0..10_000 {
        log.write("main", "x", fields(&[("a", json!(1))]));
    }
    assert!(t0.elapsed() < Duration::from_secs(2), "{:?}", t0.elapsed());
    drop(held);
    let t1 = Instant::now();
    while appenders.lock().unwrap().is_empty() && t1.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(10));
    }
    let seen = appenders.lock().unwrap().clone();
    assert!(!seen.is_empty(), "the writer thread never appended");
    assert!(
        seen.iter().all(|t| *t != me),
        "a write appended on the calling thread"
    );
}

#[test]
fn a_stuck_disk_only_grows_the_queue_to_its_cap() {
    let tmp = tempfile::tempdir().unwrap();
    let log = DiagLog::start(opts(tmp.path()));
    let hold = log.hold_io();
    for _ in 0..5_000 {
        log.write("main", "x", Map::new());
    }
    assert_eq!(log.queued(), QUEUE_MAX);
    assert_eq!(log.dropped(), 3_000);
    drop(hold);
}

#[test]
fn flush_sync_orders_a_batch_the_writer_took_before_the_quit_lines_never_after_or_twice() {
    let tmp = tempfile::tempdir().unwrap();
    let (took_tx, took_rx) = mpsc::channel::<()>();
    let (go_tx, go_rx) = mpsc::channel::<()>();
    let go_rx = std::sync::Mutex::new(go_rx);
    let took_tx = std::sync::Mutex::new(took_tx);
    let mut o = opts(tmp.path());
    o.flush_ms = 5;
    o.after_take = Some(Arc::new(move || {
        let _ = took_tx.lock().unwrap().send(());
        let _ = go_rx.lock().unwrap().recv();
    }));
    let log = DiagLog::start(o);
    log.write("main", "batch1", Map::new());
    log.write("main", "batch2", Map::new());
    took_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the writer took a batch");
    log.write("main", "quit1", Map::new());
    log.write("main", "quit2", Map::new());
    log.flush_sync();
    go_tx.send(()).unwrap();
    log.flush();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(kinds_of(tmp.path()), ["batch1", "batch2", "quit1", "quit2"]);
}

#[test]
fn drops_the_newest_lines_of_a_flood_not_the_first_and_counts_them() {
    let tmp = tempfile::tempdir().unwrap();
    let log = DiagLog::start(opts(tmp.path()));
    for i in 0..2_500 {
        log.write("main", "flood", fields(&[("i", json!(i))]));
    }
    log.flush();
    let lines = lines_of(&live(tmp.path()));
    let flood: Vec<&Value> = lines.iter().filter(|l| l["k"] == "flood").collect();
    assert_eq!(flood.len(), 2_000);
    assert_eq!(flood[0]["i"], 0);
    assert_eq!(flood[1_999]["i"], 1_999);
    let dropped: Vec<&Value> = lines
        .iter()
        .filter(|l| l["k"] == "logger-dropped")
        .collect();
    assert_eq!(dropped.len(), 1);
    assert_eq!(dropped[0]["n"], 500);
}

#[test]
fn picks_up_an_existing_file_size_so_a_relaunch_rotates_on_time() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("logs");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(DIAG_FILE), "z".repeat(1_900)).unwrap();
    let mut o = opts(tmp.path());
    o.max_bytes = 2_000;
    let log = DiagLog::start(o);
    log.write("main", "n", fields(&[("pad", json!("p".repeat(200)))]));
    log.flush();
    assert!(dir.join("diag.jsonl.1").exists());
    assert_eq!(lines_of(&live(tmp.path())).len(), 1);
}

#[test]
fn remembers_the_verbose_switch_across_a_relaunch_and_logs_the_change() {
    let tmp = tempfile::tempdir().unwrap();
    let log = DiagLog::start(opts(tmp.path()));
    assert!(!log.verbose());
    log.set_verbose(true);
    assert!(log.verbose());
    let state = fs::read_to_string(tmp.path().join("diag.json")).unwrap();
    assert_eq!(state, "{\"verbose\":true}");
    log.flush();
    let lines = lines_of(&live(tmp.path()));
    let v = lines.iter().find(|l| l["k"] == "verbose").unwrap();
    assert_eq!(v["on"], true);
    let again = DiagLog::start(opts(tmp.path()));
    assert!(again.verbose());
}

#[test]
fn reads_a_damaged_state_file_as_quiet() {
    for text in ["{nope", "{\"verbose\":\"yes\"}", "[]", ""] {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("diag.json"), text).unwrap();
        let log = DiagLog::start(opts(tmp.path()));
        assert!(!log.verbose(), "{text:?}");
    }
}

#[test]
fn never_throws_and_says_once_that_it_could_not_write() {
    let tmp = tempfile::tempdir().unwrap();
    let blocker = tmp.path().join("logs");
    fs::write(&blocker, "i am a file").unwrap();
    let log = DiagLog::start(opts(tmp.path()));
    for _ in 0..50 {
        log.write("main", "x", Map::new());
    }
    log.flush();
    assert!(log.failed());
    let errors = |log: &DiagLog| {
        log.queued_lines()
            .iter()
            .filter(|l| l.contains("logger-error"))
            .count()
    };
    assert_eq!(errors(&log), 1);
    log.write("main", "y", Map::new());
    log.flush();
    assert_eq!(errors(&log), 0, "no second logger-error");
}

#[test]
fn keeps_its_own_four_keys_whatever_the_fields_say() {
    let tmp = tempfile::tempdir().unwrap();
    let mut o = opts(tmp.path());
    o.uptime = Arc::new(|| 77);
    let log = DiagLog::start(o);
    log.write(
        "main",
        "real",
        fields(&[
            ("src", json!("evil")),
            ("k", json!("evil")),
            ("t", json!("evil")),
            ("up", json!(1)),
            ("keep", json!(true)),
        ]),
    );
    log.flush();
    let l = &lines_of(&live(tmp.path()))[0];
    assert_eq!(l["src"], "main");
    assert_eq!(l["k"], "real");
    assert_eq!(l["up"], 77);
    assert_eq!(l["t"], "2023-11-14T22:13:20.123Z");
    assert_eq!(l["keep"], true);
}

#[test]
fn caps_what_a_caller_hands_it() {
    let tmp = tempfile::tempdir().unwrap();
    let log = DiagLog::start(opts(tmp.path()));
    let mut f = Map::new();
    f.insert("s".into(), json!("a".repeat(5_000)));
    f.insert("stack".into(), json!("b".repeat(3_000)));
    for i in 0..45 {
        f.insert(format!("k{i}"), json!(i));
    }
    log.write("main", "big", f);
    log.flush();
    let l = &lines_of(&live(tmp.path()))[0];
    assert!(l["s"].as_str().unwrap().chars().count() < 320);
    let stack = l["stack"].as_str().unwrap().chars().count();
    assert!((2_000..2_020).contains(&stack), "{stack}");
    assert!(l.as_object().unwrap().len() <= 4 + 40);
}

#[test]
fn is_a_log_that_writes_nothing_until_a_host_starts_one() {
    let tmp = tempfile::tempdir().unwrap();
    let log = DiagLog::null();
    log.write("main", "x", Map::new());
    log.set_verbose(true);
    log.flush();
    log.flush_sync();
    assert!(!log.verbose());
    assert!(!log.failed());
    assert!(log.dir().as_os_str().is_empty());
    assert!(fs::read_dir(tmp.path()).unwrap().next().is_none());
    // a host that gave an empty folder gets the same
    let none = DiagLog::start(DiagLogOptions::new(std::path::PathBuf::new()));
    assert!(none.dir().as_os_str().is_empty());
}

#[test]
fn a_poisoned_lock_does_not_stop_the_log() {
    let tmp = tempfile::tempdir().unwrap();
    let log = DiagLog::start(opts(tmp.path()));
    log.poison_queue_for_test();
    let sink: &dyn LineSink = &log;
    sink.emit("main", "after", 0, Map::new());
    log.write("main", "after2", Map::new());
    log.flush();
    assert_eq!(kinds_of(tmp.path()), ["after", "after2"]);
}

#[test]
fn the_writer_has_no_raw_text_api() {
    let src =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/writer.rs")).unwrap();
    let body = src.split("#[cfg(test)]").next().unwrap();
    let mut sigs = Vec::new();
    let mut cur: Option<String> = None;
    for line in body.lines() {
        let t = line.trim();
        if cur.is_none() && (t.starts_with("pub fn ") || t.starts_with("fn emit(")) {
            cur = Some(String::new());
        }
        if let Some(c) = cur.as_mut() {
            c.push_str(t);
            c.push(' ');
            if t.contains('{') || t.ends_with(';') {
                sigs.push(cur.take().unwrap());
            }
        }
    }
    assert!(sigs.len() > 10);
    for s in &sigs {
        for bad in ["write_raw", "write_str", "write_text", "write_line"] {
            assert!(!s.contains(bad), "{s}");
        }
        let params = s.split_once('(').map_or("", |(_, r)| r);
        let params = params.split(')').next().unwrap_or("");
        for p in params.split(',') {
            let p = p.trim();
            if p.contains("&str") || p.contains("String") {
                let name = p.split(':').next().unwrap().trim();
                assert!(
                    ["src", "kind", "fields"].contains(&name),
                    "a public text parameter besides src and kind: {s}"
                );
            }
        }
    }
    assert!(body.contains("fields: Map<String, Value>"));
    assert!(body.contains("clean_fields"));
}

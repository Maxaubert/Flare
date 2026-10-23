//! Resuming what a tab hosted: the id rules and the session lookup.
//!
//! What a tab that hosted an agent comes back to. The resume id is taken from Claude's own store
//! (`~/.claude/projects/<encoded-cwd>/<session-id>.jsonl`) and only ever reaches a command line
//! after `valid_resume` has accepted it.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::SystemTime;

use regex::Regex;

use crate::kinds::CODEX_RESUME;

/// How much of a transcript is read to tell an interactive one: the marks sit in its first lines.
pub const HEAD_BYTES: usize = 4096;

/// How many interactive sessions a lookup answers with.
pub const WANT: usize = 32;

/// The stats and head reads run this many at a time.
pub const PARALLEL: usize = 16;

/// A session id (a UUID is 36 chars) or the Codex marker; anything else is no resume at all.
pub fn valid_resume(r: Option<&str>) -> Option<&str> {
    let r = r?;
    if r == CODEX_RESUME {
        return Some(r);
    }
    let b = r.as_bytes();
    if !(8..=64).contains(&b.len()) {
        return None;
    }
    let hex = |c: u8| c.is_ascii_hexdigit();
    if !hex(b[0]) || !hex(b[b.len() - 1]) {
        return None;
    }
    if b.iter().all(|&c| hex(c) || c == b'-') {
        Some(r)
    } else {
        None
    }
}

/// The first lines of a transcript tell an interactive conversation from one a tool started
/// through the Agent SDK.
pub fn is_interactive_head(head: &str) -> bool {
    let first = head.find('\n').map_or(head, |i| &head[..i]);
    if queue_operation_re().is_match(first) {
        return false;
    }
    match entrypoint_re().captures(head) {
        None => true,
        Some(c) => &c[1] == "cli",
    }
}

fn queue_operation_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#""type"\s*:\s*"queue-operation""#).expect("static pattern"))
}

fn entrypoint_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#""entrypoint"\s*:\s*"([^"]+)""#).expect("static pattern"))
}

/// Claude's own encoding of a folder: every UTF-16 unit that is not `[A-Za-z0-9]` becomes `-`.
pub fn encode_cwd(cwd: &str) -> String {
    cwd.encode_utf16()
        .map(|u| match u8::try_from(u) {
            Ok(c) if c.is_ascii_alphanumeric() => c as char,
            _ => '-',
        })
        .collect()
}

/// The first `HEAD_BYTES` of a transcript, lossy UTF-8; empty when it cannot be read.
fn head_of(path: &Path) -> String {
    let opened = std::fs::File::open(path); // sync-ok: runs on a worker (the restore thread)
    let Ok(file) = opened else {
        return String::new();
    };
    let mut buf = Vec::with_capacity(HEAD_BYTES);
    let _ = file.take(HEAD_BYTES as u64).read_to_end(&mut buf); // sync-ok: runs on a worker (the restore thread)
    String::from_utf8_lossy(&buf).into_owned()
}

/// The id and modified time of each transcript that can be stat-ed, PARALLEL at a time.
fn stat_all(dir: &Path, names: &[String]) -> Vec<(String, SystemTime)> {
    stat_all_with(names, &|name| {
        std::fs::metadata(dir.join(name)) // sync-ok: runs on a worker (the restore thread)
            .ok()?
            .modified()
            .ok()
    })
}

/// `stat_all` with the stat made explicit, so a test can count how many run at once.
fn stat_all_with(
    names: &[String],
    stat: &(dyn Fn(&str) -> Option<SystemTime> + Sync),
) -> Vec<(String, SystemTime)> {
    let next = AtomicUsize::new(0);
    let workers = PARALLEL.min(names.len());
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                s.spawn(|| {
                    let mut local = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(name) = names.get(i) else { break };
                        let Some(modified) = stat(name) else {
                            continue;
                        };
                        let id = name.strip_suffix(".jsonl").unwrap_or(name).to_string();
                        local.push((id, modified));
                    }
                    local
                })
            })
            .collect();
        handles
            .into_iter()
            // sync-ok: BLOCKING worker threads; the walk runs off the UI thread
            .flat_map(|h| h.join().unwrap_or_default())
            .collect()
    })
}

/// BLOCKING (a worker thread): the interactive session ids recorded for `cwd`, newest first, at
/// most `WANT`, from `<home>\.claude\projects\<encode_cwd>\*.jsonl`. Empty on any error.
pub fn claude_sessions(cwd: &str, home: &Path) -> Vec<String> {
    claude_sessions_in(&home.join(".claude"), cwd)
}

/// `claude_sessions` over a transcript store root (`<root>\projects\<encode_cwd>\*.jsonl`).
pub fn claude_sessions_in(root: &Path, cwd: &str) -> Vec<String> {
    let dir = root.join("projects").join(encode_cwd(cwd));
    let listed = std::fs::read_dir(&dir); // sync-ok: runs on a worker (the restore thread)
    let Ok(listing) = listed else {
        return Vec::new();
    };
    let names: Vec<String> = listing
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".jsonl"))
        .collect();
    let mut found = stat_all(&dir, &names);
    found.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| b.0.cmp(&a.0)));

    let mut out = Vec::new();
    for batch in found.chunks(PARALLEL) {
        if out.len() >= WANT {
            break;
        }
        let heads: Vec<String> = std::thread::scope(|s| {
            let handles: Vec<_> = batch
                .iter()
                .map(|(id, _)| {
                    let path = dir.join(format!("{id}.jsonl"));
                    s.spawn(move || head_of(&path))
                })
                .collect();
            handles
                .into_iter()
                // sync-ok: BLOCKING worker threads; the walk runs off the UI thread
                .map(|h| h.join().unwrap_or_default())
                .collect()
        });
        for ((id, _), head) in batch.iter().zip(&heads) {
            if out.len() >= WANT {
                break;
            }
            if is_interactive_head(head) {
                out.push(id.clone());
            }
        }
    }
    out
}

/// Where Claude keeps its store: `CLAUDE_CONFIG_DIR` when that is set and not empty, else
/// `<home>\.claude`. `config_dir` is the variable's value.
pub fn store_root(home: &Path, config_dir: Option<&std::ffi::OsStr>) -> PathBuf {
    match config_dir {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => home.join(".claude"),
    }
}

/// The home Claude uses: `USERPROFILE`, else None.
pub fn claude_home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use std::time::{Duration, UNIX_EPOCH};

    const CLI_HEAD: &str = "{\"type\":\"last-prompt\",\"lastPrompt\":\"the stable build was closed\"}\n{\"parentUuid\":null,\"isSidechain\":false,\"userType\":\"external\",\"entrypoint\":\"cli\",\"type\":\"user\"}\n";
    const SDK_HEAD: &str = "{\"type\":\"queue-operation\",\"operation\":\"enqueue\",\"timestamp\":\"2026-09-28T18:02:10.114Z\",\"content\":\"Review this change for security vulnerabilities.\"}\n{\"parentUuid\":null,\"isSidechain\":false,\"userType\":\"external\",\"entrypoint\":\"sdk-py\",\"type\":\"user\"}\n";

    const ID: &str = "3f2a9c1e-77aa-4c0d-9b1e-0a1b2c3d4e5f";

    #[test]
    fn claude_config_dir_moves_the_store() {
        let home = Path::new("C:/Users/me");
        assert_eq!(store_root(home, None), home.join(".claude"));
        assert_eq!(
            store_root(home, Some(std::ffi::OsStr::new(""))),
            home.join(".claude")
        );
        assert_eq!(
            store_root(home, Some(std::ffi::OsStr::new("D:/claude-config"))),
            PathBuf::from("D:/claude-config")
        );
        // A store somewhere else is read, and the one under home is not.
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join("cfg");
        let dir = cfg.join("projects").join(encode_cwd("C:\\w"));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(format!("{ID}.jsonl")), CLI_HEAD).unwrap();
        assert_eq!(claude_sessions_in(&cfg, "C:\\w"), vec![ID.to_string()]);
        assert!(claude_sessions("C:\\w", tmp.path()).is_empty());
    }

    #[test]
    fn accepts_a_session_id_and_the_codex_marker_refuses_a_command() {
        assert_eq!(valid_resume(Some(ID)), Some(ID));
        assert_eq!(valid_resume(Some("codex:last")), Some("codex:last"));
        assert_eq!(valid_resume(Some("x; rm -rf")), None);
        assert_eq!(valid_resume(Some("")), None);
        assert_eq!(valid_resume(None), None);
        assert_eq!(valid_resume(Some("abcdefg")), None);
        assert_eq!(valid_resume(Some("abcdef01")), Some("abcdef01"));
        let long64 = "a".repeat(64);
        assert_eq!(valid_resume(Some(&long64)), Some(long64.as_str()));
        let long65 = "a".repeat(65);
        assert_eq!(valid_resume(Some(&long65)), None);
        assert_eq!(valid_resume(Some("-abcdef01")), None);
        assert!(valid_resume(Some("3F2A9C1E-77AA-4C0D-9B1E-0A1B2C3D4E5F")).is_some());
        assert_eq!(valid_resume(Some(&format!("{ID}\n"))), None);
    }

    #[test]
    fn refuses_an_id_with_a_command_riding_behind_it() {
        assert_eq!(valid_resume(Some(&format!("{ID}; calc"))), None);
        assert_eq!(valid_resume(Some("`calc`")), None);
        assert_eq!(valid_resume(Some("codex:last; calc")), None);
    }

    fn write_session(dir: &Path, name: &str, body: &str, secs: u64) {
        let p = dir.join(name);
        fs::write(&p, body).unwrap();
        File::options()
            .write(true)
            .open(&p)
            .unwrap()
            .set_modified(UNIX_EPOCH + Duration::from_secs(secs))
            .unwrap();
    }

    #[test]
    fn reads_claudes_store_for_the_folder_newest_first() {
        let home = tempfile::tempdir().unwrap();
        let dir = home
            .path()
            .join(".claude")
            .join("projects")
            .join("C--Users-me-my-project");
        fs::create_dir_all(&dir).unwrap();
        write_session(&dir, "old.jsonl", "", 1_000_000);
        write_session(&dir, "new.jsonl", "", 2_000_000);
        write_session(&dir, "notes.txt", "", 3_000_000);
        assert_eq!(
            claude_sessions("C:\\Users\\me\\my project", home.path()),
            vec!["new".to_string(), "old".to_string()]
        );
    }

    #[test]
    fn answers_nothing_for_a_folder_claude_never_recorded() {
        let home = tempfile::tempdir().unwrap();
        assert!(claude_sessions("C:\\nowhere", home.path()).is_empty());
    }

    #[test]
    fn keeps_a_conversation_somebody_had_in_a_terminal() {
        assert!(is_interactive_head(CLI_HEAD));
    }

    #[test]
    fn refuses_a_session_a_tool_started_through_the_sdk() {
        assert!(!is_interactive_head(SDK_HEAD));
        assert!(!is_interactive_head(
            "{\"type\":\"user\",\"entrypoint\":\"sdk-cli\"}\n"
        ));
    }

    #[test]
    fn keeps_a_transcript_that_says_neither() {
        assert!(is_interactive_head(
            "{\"type\":\"user\",\"message\":{\"role\":\"user\"}}\n"
        ));
        assert!(is_interactive_head(""));
    }

    #[test]
    fn skips_a_newer_session_a_tool_started() {
        let home = tempfile::tempdir().unwrap();
        let dir = home
            .path()
            .join(".claude")
            .join("projects")
            .join("C--Users-me-repo");
        fs::create_dir_all(&dir).unwrap();
        write_session(&dir, "mine-11111111.jsonl", CLI_HEAD, 1_000_000);
        write_session(&dir, "review-2222222.jsonl", SDK_HEAD, 2_000_000);
        assert_eq!(
            claude_sessions("C:\\Users\\me\\repo", home.path()),
            vec!["mine-11111111".to_string()]
        );
    }

    #[test]
    fn sixty_files_give_the_first_32_interactive_newest_first() {
        let home = tempfile::tempdir().unwrap();
        let dir = home
            .path()
            .join(".claude")
            .join("projects")
            .join("C--Users-me-many");
        fs::create_dir_all(&dir).unwrap();
        let mut expected: Vec<(u64, usize)> = Vec::new();
        for i in 0..60usize {
            let secs = 1_000_000 + ((i as u64 * 37) % 60) * 10;
            write_session(&dir, &format!("s{i}.jsonl"), "", secs);
            expected.push((secs, i));
        }
        write_session(&dir, "notes.txt", "", 3_000_000);
        expected.sort_by(|a, b| b.cmp(a));
        let want: Vec<String> = expected
            .iter()
            .take(32)
            .map(|(_, i)| format!("s{i}"))
            .collect();
        let got = claude_sessions("C:\\Users\\me\\many", home.path());
        assert_eq!(got.len(), 32);
        assert_eq!(got, want);
    }

    #[test]
    fn sixteen_at_a_time_is_the_walk_width() {
        // The restore reads the stats and the heads sixteen at a time, on its worker thread.
        assert_eq!(PARALLEL, 16);
        // Over many more files than that, the stats really run in parallel and never beyond it.
        let names: Vec<String> = (0..200).map(|i| format!("{i}.jsonl")).collect();
        let now = AtomicUsize::new(0);
        let max = AtomicUsize::new(0);
        let found = stat_all_with(&names, &|_| {
            let c = now.fetch_add(1, Ordering::SeqCst) + 1;
            max.fetch_max(c, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(5));
            now.fetch_sub(1, Ordering::SeqCst);
            Some(UNIX_EPOCH)
        });
        assert_eq!(found.len(), 200);
        let m = max.load(Ordering::SeqCst);
        assert!(m > 1 && m <= PARALLEL, "{m} stats at once");
    }

    #[test]
    fn the_encoding_is_per_utf16_unit() {
        assert_eq!(
            encode_cwd("C:\\Users\\me\\my project"),
            "C--Users-me-my-project"
        );
        assert_eq!(encode_cwd("D:\\日本"), "D----");
        assert_eq!(encode_cwd("a😀b"), "a--b");
    }

    #[test]
    fn an_unreadable_folder_is_empty_not_a_panic() {
        let home = tempfile::tempdir().unwrap();
        // The folder claude would read is a FILE: the listing fails, and the answer is nothing.
        let parent = home.path().join(".claude").join("projects");
        fs::create_dir_all(&parent).unwrap();
        fs::write(parent.join("C--Users-me-blocked"), "not a folder").unwrap();
        assert!(claude_sessions("C:\\Users\\me\\blocked", home.path()).is_empty());
    }
}

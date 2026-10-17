//! The Claude Code plugin: its identity and where it lives (Task 3). The plugin is the bundled
//! `flare-status` folder; Claude Code runs its `hook.cmd` for the events in `hooks/hooks.json`
//! and that prints the OSC 777 sequence `signal.rs` reads. PT: `claudePlugin.ts`, `index.ts`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

/// The plugin's name in its manifest. Flare accepts no other name, and never claims a plugin
/// called `prism-terminal-status`.
pub const PLUGIN_NAME: &str = "flare-status";

/// Is this folder OUR Claude Code plugin? True when `<dir>\.claude-plugin\plugin.json` parses and
/// its `name` is [`PLUGIN_NAME`]. Cached per `dir.trim().to_lowercase()` for the life of the
/// process, as PT's `seen`.
pub fn is_our_plugin(dir: &str) -> bool {
    static SEEN: LazyLock<Mutex<HashMap<String, bool>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));
    let key = dir.trim().to_lowercase();
    let cached = SEEN.lock().ok().and_then(|seen| seen.get(&key).copied());
    if let Some(known) = cached {
        return known;
    }
    let answer = read_is_our_plugin(Path::new(dir.trim()));
    if let Ok(mut seen) = SEEN.lock() {
        seen.insert(key, answer);
    }
    answer
}

/// The same read without the cache: the body of [`is_our_plugin`].
pub fn read_is_our_plugin(dir: &Path) -> bool {
    let manifest = dir.join(".claude-plugin").join("plugin.json");
    let text = match std::fs::read_to_string(&manifest) {
        // sync-ok: a small manifest, read on the spawn worker
        Ok(text) => text,
        Err(_) => return false,
    };
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(value) => value.get("name").and_then(|name| name.as_str()) == Some(PLUGIN_NAME),
        Err(_) => false,
    }
}

/// `FLARE_CLAUDE_PLUGIN_DIR` (set and non-empty) is the ONLY candidate; else `<exe dir>\claude-plugin`,
/// and in a debug build or from a `deps` folder, `CARGO_MANIFEST_DIR\claude-plugin` as the last.
pub fn candidate_plugin_dirs(env_dir: Option<&str>, exe_dir: Option<&Path>) -> Vec<PathBuf> {
    if let Some(dir) = env_dir.map(str::trim).filter(|dir| !dir.is_empty()) {
        return vec![PathBuf::from(dir)];
    }
    let mut out = Vec::new();
    if let Some(exe) = exe_dir {
        out.push(exe.join("claude-plugin"));
    }
    let from_deps = exe_dir
        .and_then(Path::file_name)
        .is_some_and(|name| name == "deps");
    if cfg!(debug_assertions) || from_deps {
        out.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("claude-plugin"));
    }
    out
}

/// The first candidate that has `hooks\hooks.json`; None when none does (then no shell is handed
/// a plugin).
pub fn plugin_dir() -> Option<PathBuf> {
    let env_dir = std::env::var("FLARE_CLAUDE_PLUGIN_DIR").ok();
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    plugin_dir_in(&candidate_plugin_dirs(
        env_dir.as_deref(),
        exe_dir.as_deref(),
    ))
}

/// The first of `candidates` that holds a `hooks\hooks.json`.
pub fn plugin_dir_in(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates
        .iter()
        .find(|dir| dir.join("hooks").join("hooks.json").is_file()) // sync-ok: one stat per candidate, on the spawn worker
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::{AgentSignal, HookState, agent_hook_events, parse_agent_signal};

    fn plugin_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("claude-plugin")
    }

    fn read_plugin_file(rel: &str) -> String {
        std::fs::read_to_string(plugin_root().join(rel)).expect("plugin file") // sync-ok: test
    }

    fn hooks_value() -> serde_json::Value {
        serde_json::from_str(&read_plugin_file("hooks/hooks.json")).expect("hooks.json parses")
    }

    /// Every hook entry as (event, matcher, the args after the four fixed ones joined by a space).
    fn hook_entries(value: &serde_json::Value) -> Vec<(String, String, String, serde_json::Value)> {
        let mut out = Vec::new();
        for (event, groups) in value["hooks"].as_object().expect("hooks object") {
            for group in groups.as_array().expect("group list") {
                let matcher = group["matcher"].as_str().expect("matcher").to_string();
                for hook in group["hooks"].as_array().expect("hook list") {
                    let args: Vec<String> = hook["args"]
                        .as_array()
                        .expect("args")
                        .iter()
                        .map(|arg| arg.as_str().expect("arg").to_string())
                        .collect();
                    out.push((
                        event.clone(),
                        matcher.clone(),
                        args[4..].join(" "),
                        hook.clone(),
                    ));
                }
            }
        }
        out
    }

    fn write_manifest(dir: &Path, body: &str) {
        std::fs::create_dir_all(dir.join(".claude-plugin")).expect("dir"); // sync-ok: test
        std::fs::write(dir.join(".claude-plugin").join("plugin.json"), body).expect("write"); // sync-ok: test
    }

    #[test]
    fn has_a_manifest_claude_code_accepts() {
        let manifest: serde_json::Value =
            serde_json::from_str(&read_plugin_file(".claude-plugin/plugin.json")).expect("json");
        let name = manifest["name"].as_str().expect("name");
        assert_eq!(name, "flare-status");
        let shaped = name.len() > 1
            && name.as_bytes()[0].is_ascii_lowercase()
            && name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        assert!(shaped, "name {name} is not a plugin name");
        assert_eq!(manifest["version"].as_str(), Some("1.0.0"));
        let description = manifest["description"].as_str().expect("description");
        assert!(!description.contains('\u{2014}'));
    }

    #[test]
    fn hooks_json_matches_the_table() {
        let value = hooks_value();
        let mut got: Vec<(String, String, String)> = Vec::new();
        for (event, matcher, joined, hook) in hook_entries(&value) {
            assert_eq!(hook["type"].as_str(), Some("command"));
            assert_eq!(hook["command"].as_str(), Some("cmd.exe"));
            assert!(hook["timeout"].as_u64().expect("timeout") <= 10);
            let args: Vec<&str> = hook["args"]
                .as_array()
                .expect("args")
                .iter()
                .map(|arg| arg.as_str().expect("arg"))
                .collect();
            assert_eq!(
                &args[..4],
                &["/d", "/c", "call", "${CLAUDE_PLUGIN_ROOT}/hook.cmd"]
            );
            for word in &args[4..] {
                assert!(
                    word.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'),
                    "arg {word} is not a plain word"
                );
            }
            got.push((event, matcher, joined));
        }
        let mut want: Vec<(String, String, String)> = agent_hook_events()
            .into_iter()
            .map(|row| (row.event, row.matcher, row.args))
            .collect();
        got.sort();
        want.sort();
        assert_eq!(got, want);
    }

    #[test]
    fn session_start_and_end_are_not_hooked() {
        let value = hooks_value();
        let hooks = value["hooks"].as_object().expect("hooks object");
        assert!(!hooks.contains_key("SessionStart"));
        assert!(!hooks.contains_key("SessionEnd"));
    }

    #[test]
    fn hooks_json_has_25_entries() {
        assert_eq!(hook_entries(&hooks_value()).len(), 25);
    }

    #[test]
    fn no_prism_name_in_the_plugin() {
        for rel in [".claude-plugin/plugin.json", "hook.cmd", "hooks/hooks.json"] {
            let text = read_plugin_file(rel).to_lowercase();
            assert!(!text.contains("prism"), "{rel} names Prism");
        }
    }

    #[test]
    fn hook_cmd_is_crlf_and_static() {
        let bytes = std::fs::read(plugin_root().join("hook.cmd")).expect("hook.cmd"); // sync-ok: test
        let mut previous = 0u8;
        for &b in &bytes {
            if b == b'\n' {
                assert_eq!(previous, b'\r', "a bare LF in hook.cmd");
            }
            previous = b;
        }
        assert!(bytes.windows(2).any(|w| w == b"\r\n"));
        let text = String::from_utf8(bytes).expect("utf8");
        for banned in [
            "%*", "more", "set /p", "findstr", "type con", "typecon", "<",
        ] {
            assert!(!text.contains(banned), "hook.cmd contains {banned}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn every_hook_prints_what_the_parser_reads() {
        use std::os::windows::process::CommandExt;
        use std::process::{Command, Stdio};
        use std::time::Instant;

        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let started = Instant::now();
        let tmp = tempfile::tempdir().expect("tempdir");
        let folder = tmp.path().join("Flare [x] (T)");
        std::fs::create_dir_all(&folder).expect("folder"); // sync-ok: test
        std::fs::copy(plugin_root().join("hook.cmd"), folder.join("hook.cmd")).expect("copy"); // sync-ok: test
        let folder_text = folder.to_string_lossy().to_string();

        let value = hooks_value();
        let entries = hook_entries(&value);
        assert_eq!(entries.len(), 25);
        for (_event, _matcher, joined, hook) in entries {
            let args: Vec<String> = hook["args"]
                .as_array()
                .expect("args")
                .iter()
                .map(|arg| {
                    let text = arg.as_str().expect("arg");
                    text.replace("${CLAUDE_PLUGIN_ROOT}", &folder_text)
                })
                .collect();
            let output = Command::new("cmd.exe") // sync-ok: test
                .args(&args)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .creation_flags(CREATE_NO_WINDOW)
                .output()
                .expect("cmd.exe runs the hook");
            assert!(output.status.success(), "hook {joined} failed");
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let line: serde_json::Value =
                serde_json::from_str(stdout.trim()).expect("hook prints JSON");
            let sequence = line["terminalSequence"].as_str().expect("terminalSequence");
            assert!(sequence.starts_with("\u{1b}]777;"), "{sequence:?}");
            assert!(sequence.ends_with('\u{7}'), "{sequence:?}");
            let payload = &sequence[6..sequence.len() - 1];
            let mut words = joined.split(' ');
            let state = match words.next().expect("state") {
                "working" => HookState::Working,
                "question" => HookState::Question,
                "done" => HookState::Done,
                "failed" => HookState::Failed,
                other => panic!("unknown state {other}"),
            };
            let kind = words.next().map(str::to_string);
            let want = AgentSignal { state, kind };
            assert_eq!(parse_agent_signal(payload), Some(want), "payload {payload}");
        }
        assert!(started.elapsed().as_secs() < 20, "25 spawns took too long");
    }

    #[test]
    fn knows_our_plugin_by_its_manifest_name_and_nothing_else() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let ours = tmp.path().join("ours");
        write_manifest(&ours, r#"{"name":"flare-status"}"#);
        assert!(read_is_our_plugin(&ours));

        let other = tmp.path().join("other");
        write_manifest(&other, r#"{"name":"something-else"}"#);
        assert!(!read_is_our_plugin(&other));

        let broken = tmp.path().join("broken");
        write_manifest(&broken, "{");
        assert!(!read_is_our_plugin(&broken));

        let no_file = tmp.path().join("no-file");
        std::fs::create_dir_all(no_file.join(".claude-plugin")).expect("dir"); // sync-ok: test
        assert!(!read_is_our_plugin(&no_file));

        let missing = tmp.path().join("missing");
        assert!(!read_is_our_plugin(&missing));

        let prism = tmp.path().join("prism");
        write_manifest(&prism, r#"{"name":"prism-terminal-status"}"#);
        assert!(!read_is_our_plugin(&prism));
    }

    #[test]
    fn the_shipped_plugin_is_ours() {
        let dir = plugin_root();
        assert!(is_our_plugin(&dir.to_string_lossy()));
    }

    #[test]
    fn the_cache_remembers_per_lowercase_folder() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let folder = tmp.path().join("CacheCase");
        write_manifest(&folder, r#"{"name":"flare-status"}"#);
        let text = folder.to_string_lossy().to_string();
        assert!(is_our_plugin(&text));
        write_manifest(&folder, r#"{"name":"something-else"}"#);
        assert!(is_our_plugin(&text), "the first answer is kept");
        assert!(is_our_plugin(&format!("  {}  ", text.to_lowercase())));
    }

    #[test]
    fn env_dir_is_the_only_candidate() {
        let exe = Path::new("C:\\f");
        assert_eq!(
            candidate_plugin_dirs(Some("D:\\p"), Some(exe)),
            vec![PathBuf::from("D:\\p")]
        );
        assert_eq!(
            candidate_plugin_dirs(Some(""), Some(exe))[0],
            PathBuf::from("C:\\f").join("claude-plugin")
        );
    }

    #[test]
    fn the_exe_folder_comes_first_then_the_crate_copy_in_debug() {
        let candidates = candidate_plugin_dirs(None, Some(Path::new("C:\\f")));
        assert_eq!(candidates[0], PathBuf::from("C:\\f").join("claude-plugin"));
        if cfg!(debug_assertions) {
            assert_eq!(candidates.last(), Some(&plugin_root()));
        }
    }

    #[test]
    fn a_plugin_dir_needs_its_hooks_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("plug");
        std::fs::create_dir_all(&dir).expect("dir"); // sync-ok: test
        let candidates = vec![dir.clone()];
        assert_eq!(plugin_dir_in(&candidates), None);
        std::fs::create_dir_all(dir.join("hooks")).expect("dir"); // sync-ok: test
        std::fs::write(dir.join("hooks").join("hooks.json"), "{}").expect("write"); // sync-ok: test
        assert_eq!(plugin_dir_in(&candidates), Some(dir));
    }
}

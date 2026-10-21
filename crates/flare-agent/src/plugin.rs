//! The Claude Code plugin: its identity and where it lives (Task 3). The plugin is the bundled
//! `flare-status` folder; Claude Code runs its `hook.cmd` for the events in `hooks/hooks.json`
//! and that prints the OSC 777 sequence `signal.rs` reads. PT: `claudePlugin.ts`, `index.ts`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

/// The plugin's name in its manifest. Flare accepts no other name: Prism Terminal's plugin
/// (`"prism-terminal-status"`) is never claimed.
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

/// What the shipped `hooks.json` carries where the program goes. It is never a valid command:
/// [`install_plugin`] replaces it with the absolute System32 `cmd.exe`, and nothing hands the
/// template itself to Claude Code.
pub const SYSTEM_CMD_PLACEHOLDER: &str = "@@FLARE_SYSTEM_CMD@@";

/// The Windows system folder (`C:\Windows\System32`) from `GetSystemDirectoryW`, never from an
/// environment variable. None off Windows or when the call fails.
pub fn system_directory() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;
        let mut buf = [0u16; 520];
        // SAFETY: the buffer is valid for `buf.len()` u16 units and the call writes at most that many.
        let len = unsafe { GetSystemDirectoryW(buf.as_mut_ptr(), buf.len() as u32) } as usize;
        if len == 0 || len >= buf.len() {
            return None;
        }
        Some(PathBuf::from(std::ffi::OsString::from_wide(&buf[..len])))
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// `<system_dir>\cmd.exe` when `system_dir` is a drive-rooted folder named `System32` (any case);
/// else None.
pub fn system_cmd_path(system_dir: &Path) -> Option<String> {
    let named = system_dir
        .file_name()
        .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("system32"));
    if !named {
        return None;
    }
    let path = system_dir.join("cmd.exe").to_string_lossy().to_string();
    is_drive_path(&path).then_some(path)
}

/// `X:\...` only: a rooted path without a drive, or a relative one, is still found by search rules.
fn is_drive_path(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() > 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\'
}

/// The template with every placeholder replaced by `cmd_path` (JSON-escaped). None when the
/// template has no placeholder, `cmd_path` is not an absolute `...\System32\cmd.exe`, or a bare
/// program name would survive.
pub fn render_hooks(template: &str, cmd_path: &str) -> Option<String> {
    if !is_drive_path(cmd_path) || !cmd_path.to_lowercase().ends_with("\\system32\\cmd.exe") {
        return None;
    }
    if !template.contains(SYSTEM_CMD_PLACEHOLDER) {
        return None;
    }
    let quoted = serde_json::to_string(cmd_path).ok()?;
    let escaped = &quoted[1..quoted.len() - 1];
    let out = template.replace(SYSTEM_CMD_PLACEHOLDER, escaped);
    hooks_commands_are_absolute(&out).then_some(out)
}

/// True when `hooks.json` text parses, has at least one hook, and every `command` is an absolute
/// drive path: no program is ever found by name.
pub fn hooks_commands_are_absolute(text: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return false;
    };
    let Some(events) = value.get("hooks").and_then(|hooks| hooks.as_object()) else {
        return false;
    };
    let mut count = 0usize;
    for groups in events.values() {
        for group in groups.as_array().into_iter().flatten() {
            for hook in group
                .get("hooks")
                .and_then(|list| list.as_array())
                .into_iter()
                .flatten()
            {
                count += 1;
                let command = hook.get("command").and_then(|c| c.as_str()).unwrap_or("");
                if !is_drive_path(command) {
                    return false;
                }
            }
        }
    }
    count > 0
}

/// What the shipped `plugin.json` carries where the MCP server's program goes. Like the hooks'
/// placeholder it is never a valid command: [`install_plugin_with`] writes the absolute path of
/// `flare-mcp.exe` beside the running exe, or leaves the whole `mcpServers` block out.
pub const MCP_EXE_PLACEHOLDER: &str = "@@FLARE_MCP_EXE@@";

/// The manifest template with the MCP server's program set to `mcp_exe` (an absolute drive path),
/// or with no `mcpServers` at all when there is none. None when the template does not parse or a
/// path that is not absolute is given. Nothing depends on Claude Code expanding variables.
pub fn render_manifest(template: &str, mcp_exe: Option<&str>) -> Option<String> {
    let mut value: serde_json::Value = serde_json::from_str(template).ok()?;
    let object = value.as_object_mut()?;
    match mcp_exe {
        Some(exe) if is_drive_path(exe) => {
            let server = object
                .get_mut("mcpServers")?
                .get_mut("flare")?
                .as_object_mut()?;
            if server.get("command").and_then(|c| c.as_str()) != Some(MCP_EXE_PLACEHOLDER) {
                return None;
            }
            server.insert("command".into(), serde_json::Value::String(exe.to_string()));
        }
        Some(_) => return None,
        None => {
            object.remove("mcpServers");
        }
    }
    let mut text = serde_json::to_string_pretty(&value).ok()?;
    text.push('\n');
    Some(text)
}

/// The sibling a file is written to before it is renamed over its target.
fn temp_name(target: &Path) -> PathBuf {
    let mut name = target.file_name().unwrap_or_default().to_os_string();
    name.push(".flare-tmp");
    target.with_file_name(name)
}

/// Writes `bytes` to a temp file beside `target`, then renames it over `target` (audit L26): a reader
/// (Claude Code starting a hook) sees the old file or the new one, never half of either. The rename
/// replaces an existing file on Windows. A failed write removes the temp file.
fn write_atomic(target: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let temp = temp_name(target);
    // sync-ok: install on the spawn worker
    let written = std::fs::write(&temp, bytes).and_then(|()| std::fs::rename(&temp, target));
    if written.is_err() {
        let _ = std::fs::remove_file(&temp); // sync-ok: install on the spawn worker
    }
    written
}

/// Install (or refresh) the plugin from `src` into `dest`: every file is copied as is, except
/// `hooks\hooks.json`, which is written with the absolute `cmd.exe` of `system_dir`. A file whose
/// bytes already match is left alone, so the identity check covers the rewritten file too: an
/// installed `hooks.json` with a bare name (or a stale path) differs and is rewritten. Returns how
/// many files were written; an error when the template cannot be rendered (nothing is written).
pub fn install_plugin(src: &Path, dest: &Path, system_dir: &Path) -> std::io::Result<usize> {
    install_plugin_with(src, dest, system_dir, None)
}

/// [`install_plugin`] that also writes `plugin.json` with `mcp_exe` (the absolute `flare-mcp.exe`) as
/// the MCP server's program. None: the manifest has no MCP server, so a build without the file gives
/// Claude Code nothing to fail on.
pub fn install_plugin_with(
    src: &Path,
    dest: &Path,
    system_dir: &Path,
    mcp_exe: Option<&Path>,
) -> std::io::Result<usize> {
    let bad = |why: &str| std::io::Error::new(std::io::ErrorKind::InvalidInput, why.to_string());
    let cmd = system_cmd_path(system_dir).ok_or_else(|| bad("not an absolute System32 folder"))?;
    // sync-ok: install on the spawn worker
    let template = std::fs::read_to_string(src.join("hooks").join("hooks.json"))?;
    let hooks =
        render_hooks(&template, &cmd).ok_or_else(|| bad("hooks.json cannot be rendered"))?;
    let manifest_rel = Path::new(".claude-plugin").join("plugin.json");
    // sync-ok: install on the spawn worker
    let manifest_template = std::fs::read_to_string(src.join(&manifest_rel))?;
    let mcp_text = mcp_exe.map(|p| p.to_string_lossy().into_owned());
    let manifest = render_manifest(&manifest_template, mcp_text.as_deref())
        .ok_or_else(|| bad("plugin.json cannot be rendered"))?;
    let hooks_rel = Path::new("hooks").join("hooks.json");
    let mut written = 0usize;
    let mut pending = vec![PathBuf::new()];
    while let Some(rel) = pending.pop() {
        // sync-ok: install on the spawn worker
        for entry in std::fs::read_dir(src.join(&rel))? {
            let entry = entry?;
            let child = rel.join(entry.file_name());
            if entry.file_type()?.is_dir() {
                pending.push(child);
                continue;
            }
            let bytes = if child == hooks_rel {
                hooks.clone().into_bytes()
            } else if child == manifest_rel {
                manifest.clone().into_bytes()
            } else {
                std::fs::read(entry.path())? // sync-ok: install on the spawn worker
            };
            let target = dest.join(&child);
            // sync-ok: install on the spawn worker
            if std::fs::read(&target).is_ok_and(|have| have == bytes) {
                continue;
            }
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?; // sync-ok: install on the spawn worker
            }
            write_atomic(&target, &bytes)?; // sync-ok: install on the spawn worker
            written += 1;
        }
    }
    Ok(written)
}

/// Where the installed copy lives: `%LOCALAPPDATA%\Flare\claude-plugin`.
pub fn installed_plugin_path() -> Option<PathBuf> {
    let local = std::env::var("LOCALAPPDATA")
        .ok()
        .filter(|value| !value.is_empty())?;
    Some(PathBuf::from(local).join("Flare").join("claude-plugin"))
}

/// The plugin folder to hand a shell: the bundled template is installed (refreshed) into
/// `%LOCALAPPDATA%\Flare\claude-plugin` with the absolute System32 `cmd.exe`, and that copy is
/// returned. None when no template is found or the install fails (then no shell is handed a
/// plugin; the template itself is never handed out).
pub fn plugin_dir(env_dir: Option<&str>) -> Option<PathBuf> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    let src = plugin_dir_in(&candidate_plugin_dirs(env_dir, exe_dir.as_deref()))?;
    let dest = installed_plugin_path()?;
    let system = system_directory()?;
    // The MCP server is only offered when its program is really beside this exe.
    let mcp = exe_dir
        .as_deref()
        .map(|d| d.join("flare-mcp.exe"))
        .filter(|p| p.is_file()); // sync-ok: one stat, on the spawn worker
    match install_plugin_with(&src, &dest, &system, mcp.as_deref()) {
        Ok(_) => Some(dest),
        Err(err) => {
            log::warn!("claude plugin install failed: {err}");
            None
        }
    }
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
        std::fs::read_to_string(plugin_root().join(rel)).expect("plugin file") // sync-ok: test fixture on a temp folder
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
        std::fs::create_dir_all(dir.join(".claude-plugin")).expect("dir"); // sync-ok: test fixture on a temp folder
        std::fs::write(dir.join(".claude-plugin").join("plugin.json"), body).expect("write"); // sync-ok: test fixture on a temp folder
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
        assert_eq!(manifest["version"].as_str(), Some("1.1.0"));
        let description = manifest["description"].as_str().expect("description");
        assert!(!description.contains('\u{2014}'));
    }

    #[test]
    fn declares_the_flare_mcp_server_by_a_written_in_absolute_path_and_never_by_expansion() {
        // Surfaces stage 2: Claude in a Flare tab gets show_image, show_file, show_text and
        // close_panel with no setup. The shipped manifest is a template (like hooks.json): the program
        // is the absolute `flare-mcp.exe` beside the running exe, written in at install, never a bare
        // name PATH or the current folder could answer and never a variable Claude Code might not
        // expand. The tab's secret and the pipe reach the server through Claude's own environment,
        // not through the manifest or a command line.
        let template = read_plugin_file(".claude-plugin/plugin.json");
        let value: serde_json::Value = serde_json::from_str(&template).expect("json");
        assert_eq!(
            value["mcpServers"]["flare"]["command"].as_str(),
            Some(MCP_EXE_PLACEHOLDER)
        );
        assert!(
            !template.contains("${"),
            "no variable expansion is relied on"
        );
        assert!(!template.contains("pipe"), "no pipe name in the manifest");
        assert!(!template.contains("TOKEN"), "no secret in the manifest");

        let exe = "C:\\Program Files\\Flare\\flare-mcp.exe";
        let rendered = render_manifest(&template, Some(exe)).expect("renders");
        let out: serde_json::Value = serde_json::from_str(&rendered).expect("json");
        let servers = out["mcpServers"].as_object().expect("mcpServers");
        assert_eq!(servers.len(), 1);
        assert_eq!(servers["flare"]["command"].as_str(), Some(exe));
        assert_eq!(servers["flare"]["args"].as_array().map(Vec::len), Some(0));
        assert!(servers["flare"].get("env").is_none());
        assert!(!rendered.contains("@@"));
        assert_eq!(out["name"].as_str(), Some("flare-status"));

        // No program beside the exe: no server at all, and the plugin is still ours.
        let bare = render_manifest(&template, None).expect("renders");
        let bare_value: serde_json::Value = serde_json::from_str(&bare).expect("json");
        assert!(bare_value.get("mcpServers").is_none());
        assert_eq!(bare_value["name"].as_str(), Some("flare-status"));
        // A relative or bare program is never written.
        assert!(render_manifest(&template, Some("flare-mcp.exe")).is_none());
        assert!(render_manifest(&template, Some("..\\flare-mcp.exe")).is_none());
        assert!(render_manifest("not json", None).is_none());
    }

    #[test]
    fn install_writes_the_mcp_server_only_when_the_program_exists() {
        let tmp = tempfile::tempdir().expect("tmp");
        let system = fake_system32(tmp.path());
        let with = tmp.path().join("with");
        let exe = tmp.path().join("flare-mcp.exe");
        install_plugin_with(&plugin_root(), &with, &system, Some(&exe)).expect("install");
        let text = std::fs::read_to_string(with.join(".claude-plugin").join("plugin.json"))
            .expect("manifest"); // sync-ok: test fixture on a temp folder
        let value: serde_json::Value = serde_json::from_str(&text).expect("json");
        let command = value["mcpServers"]["flare"]["command"]
            .as_str()
            .expect("command");
        assert!(is_drive_path(command), "{command}");
        assert!(command.ends_with("flare-mcp.exe"));
        let without = tmp.path().join("without");
        install_plugin_with(&plugin_root(), &without, &system, None).expect("install");
        let text = std::fs::read_to_string(without.join(".claude-plugin").join("plugin.json"))
            .expect("manifest"); // sync-ok: test fixture on a temp folder
        assert!(!text.contains("mcpServers") && !text.contains("@@"));
        assert!(read_is_our_plugin(&without));
        // The installed copy follows the program: adding it later rewrites the manifest.
        assert_eq!(
            install_plugin_with(&plugin_root(), &without, &system, Some(&exe)).expect("again"),
            1
        );
    }

    #[test]
    fn hooks_json_matches_the_table() {
        let value = hooks_value();
        let mut got: Vec<(String, String, String)> = Vec::new();
        for (event, matcher, joined, hook) in hook_entries(&value) {
            assert_eq!(hook["type"].as_str(), Some("command"));
            assert_eq!(hook["command"].as_str(), Some(SYSTEM_CMD_PLACEHOLDER));
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
                    *word == "+sid" || word.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'),
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
    fn the_plugin_carries_no_refused_name() {
        for rel in [".claude-plugin/plugin.json", "hook.cmd", "hooks/hooks.json"] {
            let text = read_plugin_file(rel).to_lowercase();
            for name in ["\"prism-agent", "\"prism-terminal-status"] {
                assert!(!text.contains(name), "{rel} names a refused plugin");
            }
        }
    }

    #[test]
    fn hook_cmd_is_crlf_and_static() {
        let bytes = std::fs::read(plugin_root().join("hook.cmd")).expect("hook.cmd"); // sync-ok: test fixture on a temp folder
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
        std::fs::create_dir_all(&folder).expect("folder"); // sync-ok: test fixture on a temp folder
        let system = system_directory().expect("system directory");
        install_plugin(&plugin_root(), &folder, &system).expect("install"); // sync-ok: test fixture on a temp folder
        let folder_text = folder.to_string_lossy().to_string();

        let installed = std::fs::read_to_string(folder.join("hooks").join("hooks.json")) // sync-ok: test fixture on a temp folder
            .expect("installed hooks");
        let value: serde_json::Value = serde_json::from_str(&installed).expect("json");
        let entries = hook_entries(&value);
        assert_eq!(entries.len(), 25);
        for (_event, _matcher, joined, hook) in entries {
            let sid = joined.ends_with(" +sid");
            let args: Vec<String> = hook["args"]
                .as_array()
                .expect("args")
                .iter()
                .map(|arg| {
                    let text = arg.as_str().expect("arg");
                    text.replace("${CLAUDE_PLUGIN_ROOT}", &folder_text)
                })
                .collect();
            let output = Command::new(hook["command"].as_str().expect("command")) // sync-ok: test fixture on a temp folder
                .args(&args)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .env("CLAUDE_CODE_SESSION_ID", SESSION)
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
            let second = words.next();
            let (kind, session) = match second {
                Some("+sid") => (None, Some(SESSION.to_string())),
                other => (other.map(str::to_string), None),
            };
            assert_eq!(sid, session.is_some(), "{joined}");
            let want = AgentSignal {
                state,
                kind,
                session,
            };
            assert_eq!(parse_agent_signal(payload), Some(want), "payload {payload}");
        }
        assert!(started.elapsed().as_secs() < 20, "25 spawns took too long");
    }

    #[cfg(windows)]
    const SESSION: &str = "b44e954e-a9cf-4ba9-a3c0-99ec6795d1d9";

    /// The id is Claude's, but read with care: cmd expands %VAR% before it parses, so a ) & | < > ^
    /// or quote in the value would break the block or the JSON and lose the STATE with it.
    #[cfg(windows)]
    #[test]
    fn prints_the_state_as_valid_json_whatever_the_session_id_holds() {
        use std::os::windows::process::CommandExt;
        use std::process::{Command, Stdio};

        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let tmp = tempfile::tempdir().expect("tempdir");
        let system = system_directory().expect("system directory");
        install_plugin(&plugin_root(), tmp.path(), &system).expect("install"); // sync-ok: test fixture on a temp folder
        let hook = tmp.path().join("hook.cmd");
        let cmd = system_cmd_path(&system).expect("cmd path");
        let upper = SESSION.to_uppercase();
        let semicolon = format!(";{}", &SESSION[1..]);
        let trailing = format!("{SESSION}x");
        for value in [
            "a&calc",
            "x)\"",
            "a\\b",
            "%PATH%",
            "!PATH!",
            upper.as_str(),
            semicolon.as_str(),
            trailing.as_str(),
            "",
        ] {
            let output = Command::new(&cmd) // sync-ok: test fixture on a temp folder
                .args(["/d", "/c", "call"])
                .arg(&hook)
                .args(["done", "+sid"])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .env("CLAUDE_CODE_SESSION_ID", value)
                .creation_flags(CREATE_NO_WINDOW)
                .output()
                .expect("cmd.exe runs the hook");
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let line: serde_json::Value = serde_json::from_str(stdout.trim())
                .unwrap_or_else(|e| panic!("{value:?} broke the JSON: {e}: {stdout}"));
            let sequence = line["terminalSequence"].as_str().expect("terminalSequence");
            let payload = &sequence[6..sequence.len() - 1];
            assert_eq!(
                parse_agent_signal(payload),
                Some(AgentSignal {
                    state: HookState::Done,
                    kind: None,
                    session: None,
                }),
                "{value:?}"
            );
        }
        // And the one good value is carried.
        let output = Command::new(&cmd) // sync-ok: test fixture on a temp folder
            .args(["/d", "/c", "call"])
            .arg(&hook)
            .args(["done", "+sid"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .env("CLAUDE_CODE_SESSION_ID", SESSION)
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .expect("cmd.exe runs the hook");
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        assert!(stdout.contains(&format!("session={SESSION}")), "{stdout}");
    }

    fn fake_system32(tmp: &Path) -> PathBuf {
        let dir = tmp.join("Windows").join("System32");
        std::fs::create_dir_all(&dir).expect("dir"); // sync-ok: test fixture on a temp folder
        dir
    }

    fn installed_cmd_commands(dest: &Path) -> Vec<String> {
        let text = std::fs::read_to_string(dest.join("hooks").join("hooks.json")).expect("hooks"); // sync-ok: test fixture on a temp folder
        let value: serde_json::Value = serde_json::from_str(&text).expect("json");
        hook_entries(&value)
            .into_iter()
            .map(|(_, _, _, hook)| hook["command"].as_str().expect("command").to_string())
            .collect()
    }

    #[test]
    fn the_template_is_never_valid_as_is() {
        let text = read_plugin_file("hooks/hooks.json");
        assert!(text.contains(SYSTEM_CMD_PLACEHOLDER));
        assert!(!hooks_commands_are_absolute(&text));
    }

    #[test]
    fn install_leaves_no_bare_program_name() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let system = fake_system32(tmp.path());
        let dest = tmp.path().join("home").join(".claude-test").join("plug");
        let written = install_plugin(&plugin_root(), &dest, &system).expect("install");
        assert!(written >= 3);
        let commands = installed_cmd_commands(&dest);
        assert_eq!(commands.len(), 25);
        for command in &commands {
            assert!(
                Path::new(command).is_absolute(),
                "{command} is not absolute"
            );
            assert!(
                command.to_lowercase().ends_with("system32\\cmd.exe"),
                "{command}"
            );
            assert!(!command.contains(SYSTEM_CMD_PLACEHOLDER));
        }
        let text = std::fs::read_to_string(dest.join("hooks").join("hooks.json")).expect("read"); // sync-ok: test fixture on a temp folder
        assert!(hooks_commands_are_absolute(&text));
        assert!(dest.join("hook.cmd").is_file());
        assert!(is_our_plugin(&dest.to_string_lossy()));
    }

    #[test]
    fn a_second_install_writes_nothing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let system = fake_system32(tmp.path());
        let dest = tmp.path().join("plug");
        install_plugin(&plugin_root(), &dest, &system).expect("install");
        assert_eq!(
            install_plugin(&plugin_root(), &dest, &system).expect("again"),
            0
        );
    }

    #[test]
    fn an_installed_plugin_with_a_bare_name_is_rewritten() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let system = fake_system32(tmp.path());
        let dest = tmp.path().join("plug");
        install_plugin(&plugin_root(), &dest, &system).expect("install");
        let hooks = dest.join("hooks").join("hooks.json");
        let good = std::fs::read_to_string(&hooks).expect("read"); // sync-ok: test fixture on a temp folder
        let bare = good.replace(
            &system_cmd_path(&system).expect("cmd").replace('\\', "\\\\"),
            "cmd.exe",
        );
        assert_ne!(bare, good);
        assert!(!hooks_commands_are_absolute(&bare));
        std::fs::write(&hooks, &bare).expect("plant"); // sync-ok: test fixture on a temp folder
        assert_eq!(
            install_plugin(&plugin_root(), &dest, &system).expect("refresh"),
            1
        );
        assert_eq!(std::fs::read_to_string(&hooks).expect("read"), good); // sync-ok: test fixture on a temp folder
    }

    #[test]
    fn a_refresh_goes_through_a_temp_file_and_a_rename() {
        // A refresh must never leave a half-written file for Claude Code to read (audit L26). With the
        // temp name taken by a folder the write cannot even start, so the old file is untouched; an
        // in-place write would have gone ahead and changed it.
        let tmp = tempfile::tempdir().expect("tempdir");
        let one = fake_system32(tmp.path());
        let other = tmp.path().join("Other").join("System32");
        std::fs::create_dir_all(&other).expect("dir"); // sync-ok: test fixture on a temp folder
        let dest = tmp.path().join("plug");
        install_plugin(&plugin_root(), &dest, &one).expect("install");
        let hooks = dest.join("hooks").join("hooks.json");
        let before = std::fs::read(&hooks).expect("read"); // sync-ok: test fixture on a temp folder
        std::fs::create_dir_all(temp_name(&hooks)).expect("block the temp name"); // sync-ok: test fixture on a temp folder
        assert!(install_plugin(&plugin_root(), &dest, &other).is_err());
        assert_eq!(std::fs::read(&hooks).expect("read"), before); // sync-ok: test fixture on a temp folder
        // And a good refresh leaves no temp file behind.
        std::fs::remove_dir(temp_name(&hooks)).expect("unblock"); // sync-ok: test fixture on a temp folder
        assert_eq!(
            install_plugin(&plugin_root(), &dest, &other).expect("refresh"),
            1
        );
        assert!(!temp_name(&hooks).exists());
    }

    #[test]
    fn a_moved_system_folder_is_rewritten() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let one = fake_system32(tmp.path());
        let other = tmp.path().join("Other").join("System32");
        std::fs::create_dir_all(&other).expect("dir"); // sync-ok: test fixture on a temp folder
        let dest = tmp.path().join("plug");
        install_plugin(&plugin_root(), &dest, &one).expect("install");
        assert_eq!(
            install_plugin(&plugin_root(), &dest, &other).expect("refresh"),
            1
        );
    }

    #[test]
    fn a_relative_or_misnamed_system_folder_installs_nothing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dest = tmp.path().join("plug");
        for bad in [Path::new("System32"), Path::new(""), tmp.path()] {
            assert!(install_plugin(&plugin_root(), &dest, bad).is_err());
        }
        assert!(!dest.exists());
        assert_eq!(render_hooks("{}", "C:\\Windows\\System32\\cmd.exe"), None);
        assert_eq!(render_hooks(SYSTEM_CMD_PLACEHOLDER, "cmd.exe"), None);
    }

    #[cfg(windows)]
    #[test]
    fn the_real_system_directory_is_an_absolute_system32() {
        let system = system_directory().expect("GetSystemDirectoryW");
        let cmd = system_cmd_path(&system).expect("cmd path");
        assert!(Path::new(&cmd).is_file(), "{cmd}"); // sync-ok: test fixture on a temp folder
        assert!(cmd.to_lowercase().ends_with("system32\\cmd.exe"));
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
        std::fs::create_dir_all(no_file.join(".claude-plugin")).expect("dir"); // sync-ok: test fixture on a temp folder
        assert!(!read_is_our_plugin(&no_file));

        let missing = tmp.path().join("missing");
        assert!(!read_is_our_plugin(&missing));

        let refused = tmp.path().join("elsewhere");
        write_manifest(&refused, r#"{"name":"prism-terminal-status"}"#);
        assert!(!read_is_our_plugin(&refused));
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
        std::fs::create_dir_all(&dir).expect("dir"); // sync-ok: test fixture on a temp folder
        let candidates = vec![dir.clone()];
        assert_eq!(plugin_dir_in(&candidates), None);
        std::fs::create_dir_all(dir.join("hooks")).expect("dir"); // sync-ok: test fixture on a temp folder
        std::fs::write(dir.join("hooks").join("hooks.json"), "{}").expect("write"); // sync-ok: test fixture on a temp folder
        assert_eq!(plugin_dir_in(&candidates), Some(dir));
    }
}

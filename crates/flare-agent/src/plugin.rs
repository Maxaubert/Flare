//! The Claude Code plugin: its identity and where it lives (Task 3). The plugin is the bundled
//! `flare-status` folder; Claude Code runs its `hook.cmd` (Windows, through the System32 `cmd.exe`)
//! or its `hook.sh` (macOS, through `/bin/sh`) for the events in `hooks/hooks.json`, and that prints
//! the OSC 777 sequence `signal.rs` reads. PT: `claudePlugin.ts`, `index.ts`.

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

/// The hook script the state hooks run on this platform: `hook.cmd` on Windows, `hook.sh` on macOS. The
/// other platform's script is never installed.
#[cfg(windows)]
pub const HOOK_SCRIPT: &str = "hook.cmd";
#[cfg(not(windows))]
pub const HOOK_SCRIPT: &str = "hook.sh";
#[cfg(windows)]
const OTHER_HOOK_SCRIPT: &str = "hook.sh";
#[cfg(not(windows))]
const OTHER_HOOK_SCRIPT: &str = "hook.cmd";

/// The arguments every state hook of the template starts with: the Windows `cmd.exe` form. On macOS
/// the install swaps them for the one `${CLAUDE_PLUGIN_ROOT}/hook.sh` (`/bin/sh` runs it, no shell
/// string, no execute bit needed).
#[cfg_attr(windows, allow(dead_code))]
const CMD_HOOK_ARGS: [&str; 4] = ["/d", "/c", "call", "${CLAUDE_PLUGIN_ROOT}/hook.cmd"];
/// The script argument of a macOS state hook.
#[cfg_attr(windows, allow(dead_code))]
const SH_HOOK_ARG: &str = "${CLAUDE_PLUGIN_ROOT}/hook.sh";

/// The system folder the hooks' program comes from. Windows: `C:\Windows\System32` from
/// `GetSystemDirectoryW`, never from an environment variable (None when the call fails). macOS:
/// `/bin`, whose `sh` is part of the sealed system volume.
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
        Some(PathBuf::from("/bin"))
    }
}

/// The hooks' program in `system_dir`: on Windows `<system_dir>\cmd.exe` when `system_dir` is a
/// drive-rooted folder named `System32` (any case); on macOS `<system_dir>/sh` when `system_dir` is an
/// absolute folder named `bin` with no `.` or `..` in it. Else None.
pub fn system_cmd_path(system_dir: &Path) -> Option<String> {
    #[cfg(windows)]
    {
        windows_cmd_path(system_dir)
    }
    #[cfg(not(windows))]
    {
        posix_shell_path(system_dir)
    }
}

/// The Windows rule of [`system_cmd_path`].
#[cfg_attr(not(windows), allow(dead_code))]
fn windows_cmd_path(system_dir: &Path) -> Option<String> {
    let named = system_dir
        .file_name()
        .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("system32"));
    if !named {
        return None;
    }
    let path = system_dir.join("cmd.exe").to_string_lossy().to_string();
    is_drive_path(&path).then_some(path)
}

/// The macOS rule of [`system_cmd_path`].
#[cfg_attr(windows, allow(dead_code))]
fn posix_shell_path(system_dir: &Path) -> Option<String> {
    let text = system_dir.to_str()?;
    if !is_posix_absolute(text) || system_dir.file_name().and_then(|n| n.to_str()) != Some("bin") {
        return None;
    }
    Some(format!("{}/sh", text.trim_end_matches('/')))
}

/// `X:\...` only: a rooted path without a drive, or a relative one, is still found by search rules.
fn is_drive_path(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() > 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\'
}

/// An absolute POSIX path that names one file the same way from any folder: `/` first, more than the
/// root, no control character, and no `.` or `..` segment (so nothing is found by a search rule or
/// walks out of where it says).
fn is_posix_absolute(text: &str) -> bool {
    text.len() > 1
        && text.starts_with('/')
        && !text.chars().any(char::is_control)
        && !text.split('/').any(|seg| seg == "." || seg == "..")
}

/// A program path a hook or the manifest may name: a drive path everywhere, and on macOS also an
/// absolute POSIX path. Never a bare name.
fn is_absolute_program(text: &str) -> bool {
    is_drive_path(text) || (cfg!(not(windows)) && is_posix_absolute(text))
}

/// The template with every placeholder replaced by `cmd_path` (JSON-escaped), and the hooks that run
/// `flare-mcp` left out. None when the template has no placeholder, `cmd_path` is not an absolute
/// `...\System32\cmd.exe`, or a bare program name would survive.
pub fn render_hooks(template: &str, cmd_path: &str) -> Option<String> {
    render_hooks_with(template, cmd_path, None)
}

/// [`render_hooks`] that also writes `mcp_exe` (the absolute `flare-mcp.exe`) where a hook runs it: the
/// prompt hook that tells Claude which page the browser panel shows. None: those hooks are left out
/// (and a group or event they leave empty with them), so a build without the program gives Claude Code
/// nothing to fail on. None as the answer when `mcp_exe` is given but is not an absolute drive path.
pub fn render_hooks_with(template: &str, cmd_path: &str, mcp_exe: Option<&str>) -> Option<String> {
    #[cfg(not(windows))]
    if is_posix_absolute(cmd_path) && cmd_path.ends_with("/bin/sh") {
        return render_posix_hooks(template, cmd_path, mcp_exe);
    }
    if !is_drive_path(cmd_path) || !cmd_path.to_lowercase().ends_with("\\system32\\cmd.exe") {
        return None;
    }
    if !template.contains(SYSTEM_CMD_PLACEHOLDER) {
        return None;
    }
    let escape = |path: &str| -> Option<String> {
        let quoted = serde_json::to_string(path).ok()?;
        Some(quoted[1..quoted.len() - 1].to_string())
    };
    let mut out = template.replace(SYSTEM_CMD_PLACEHOLDER, &escape(cmd_path)?);
    match mcp_exe {
        Some(exe) if is_drive_path(exe) => {
            out = out.replace(MCP_EXE_PLACEHOLDER, &escape(exe)?);
        }
        Some(_) => return None,
        None if out.contains(MCP_EXE_PLACEHOLDER) => out = without_mcp_hooks(&out)?,
        None => {}
    }
    hooks_commands_are_absolute(&out).then_some(out)
}

/// The macOS form of [`render_hooks_with`]: every state hook runs `sh_path` (an absolute `.../bin/sh`) with
/// the one argument `${CLAUDE_PLUGIN_ROOT}/hook.sh` in place of the template's four `cmd.exe` ones, and the
/// state words after them as they are. A state hook whose arguments do not start with the template's four
/// fails the render, so nothing half-converted is ever written. `mcp_exe` must be an absolute POSIX path.
#[cfg(not(windows))]
fn render_posix_hooks(template: &str, sh_path: &str, mcp_exe: Option<&str>) -> Option<String> {
    if !template.contains(SYSTEM_CMD_PLACEHOLDER) {
        return None;
    }
    if mcp_exe.is_some_and(|exe| !is_posix_absolute(exe)) {
        return None;
    }
    let base = match mcp_exe {
        Some(_) => template.to_string(),
        None => without_mcp_hooks(template)?,
    };
    let mut value: serde_json::Value = serde_json::from_str(&base).ok()?;
    let events = value.get_mut("hooks")?.as_object_mut()?;
    for groups in events.values_mut() {
        for group in groups.as_array_mut()?.iter_mut() {
            for hook in group.get_mut("hooks")?.as_array_mut()?.iter_mut() {
                let command = hook.get("command").and_then(|c| c.as_str()).unwrap_or("");
                if command == SYSTEM_CMD_PLACEHOLDER {
                    let args = hook.get("args")?.as_array()?;
                    let head: Vec<&str> = args.iter().take(4).filter_map(|a| a.as_str()).collect();
                    if head != CMD_HOOK_ARGS {
                        return None;
                    }
                    let mut new_args = vec![serde_json::Value::String(SH_HOOK_ARG.to_string())];
                    new_args.extend(args.iter().skip(4).cloned());
                    hook["command"] = serde_json::Value::String(sh_path.to_string());
                    hook["args"] = serde_json::Value::Array(new_args);
                } else if command == MCP_EXE_PLACEHOLDER {
                    hook["command"] = serde_json::Value::String(mcp_exe?.to_string());
                }
            }
        }
    }
    let mut out = serde_json::to_string_pretty(&value).ok()?;
    out.push('\n');
    hooks_commands_are_absolute(&out).then_some(out)
}

/// `hooks.json` text without the hooks whose program is still [`MCP_EXE_PLACEHOLDER`], and without a
/// group or an event that has nothing left.
fn without_mcp_hooks(text: &str) -> Option<String> {
    let mut value: serde_json::Value = serde_json::from_str(text).ok()?;
    let events = value.get_mut("hooks")?.as_object_mut()?;
    for groups in events.values_mut() {
        let Some(groups) = groups.as_array_mut() else {
            continue;
        };
        for group in groups.iter_mut() {
            if let Some(hooks) = group.get_mut("hooks").and_then(|h| h.as_array_mut()) {
                hooks.retain(|hook| {
                    hook.get("command").and_then(|c| c.as_str()) != Some(MCP_EXE_PLACEHOLDER)
                });
            }
        }
        groups.retain(|group| {
            group
                .get("hooks")
                .and_then(|h| h.as_array())
                .is_some_and(|h| !h.is_empty())
        });
    }
    events.retain(|_, groups| groups.as_array().is_some_and(|g| !g.is_empty()));
    let mut out = serde_json::to_string_pretty(&value).ok()?;
    out.push('\n');
    Some(out)
}

/// True when `hooks.json` text parses, has at least one hook, and every `command` is an absolute
/// drive path (on macOS also an absolute POSIX path): no program is ever found by name.
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
                if !is_absolute_program(command) {
                    return false;
                }
            }
        }
    }
    count > 0
}

/// What the shipped `plugin.json` (and the prompt hook in `hooks.json`) carries where the MCP server's
/// program goes. Like the hooks' placeholder it is never a valid command: [`install_plugin_with`]
/// writes the absolute path of `flare-mcp.exe` beside the running exe, or leaves the whole
/// `mcpServers` block (and that hook) out.
pub const MCP_EXE_PLACEHOLDER: &str = "@@FLARE_MCP_EXE@@";

/// The manifest template with the MCP server's program set to `mcp_exe` (an absolute drive path),
/// or with no `mcpServers` at all when there is none. None when the template does not parse or a
/// path that is not absolute is given. Nothing depends on Claude Code expanding variables.
pub fn render_manifest(template: &str, mcp_exe: Option<&str>) -> Option<String> {
    let mut value: serde_json::Value = serde_json::from_str(template).ok()?;
    let object = value.as_object_mut()?;
    match mcp_exe {
        Some(exe) if is_absolute_program(exe) => {
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
/// the MCP server's program, and `hooks.json` with it as the prompt hook's. None: the manifest has no
/// MCP server and that hook is left out, so a build without the file gives Claude Code nothing to fail on.
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
    let mcp_text = mcp_exe.map(|p| p.to_string_lossy().into_owned());
    let hooks = render_hooks_with(&template, &cmd, mcp_text.as_deref())
        .ok_or_else(|| bad("hooks.json cannot be rendered"))?;
    let manifest_rel = Path::new(".claude-plugin").join("plugin.json");
    // sync-ok: install on the spawn worker
    let manifest_template = std::fs::read_to_string(src.join(&manifest_rel))?;
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
            // The other platform's hook script is never installed: nothing here runs it.
            if child == Path::new(OTHER_HOOK_SCRIPT) {
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

/// Where the installed copy for the Flare in `exe_dir` lives: `%LOCALAPPDATA%\Flare\claude-plugin-<tag>`, the
/// tag naming the folder the exe is in. The copy holds the absolute path of THIS Flare's `flare-mcp.exe`, so
/// two Flares (the installed one, a dev or worktree build) must not share one: the last started would decide
/// which program every Claude session of every Flare runs, and a removed worktree would break the others.
///
/// macOS: `~/Library/Application Support/Flare/claude-plugin-<tag>`.
pub fn installed_plugin_path(exe_dir: Option<&Path>) -> Option<PathBuf> {
    #[cfg(windows)]
    let local = PathBuf::from(
        std::env::var("LOCALAPPDATA")
            .ok()
            .filter(|value| !value.is_empty())?,
    );
    #[cfg(not(windows))]
    let local = flare_win::paths::local_app_data()?;
    let name = match exe_dir {
        Some(dir) => format!("claude-plugin-{}", dir_tag(dir)),
        None => "claude-plugin".to_string(),
    };
    Some(local.join("Flare").join(name))
}

/// Eight hex digits naming a folder (FNV-1a of its lowercased path, slashes unified).
fn dir_tag(dir: &Path) -> String {
    let text = dir.to_string_lossy().replace('/', "\\").to_lowercase();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.trim_end_matches(|c| c == char::from(92u8)).bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{:08x}", hash >> 32)
}

/// The plugin folder to hand a shell: the bundled template is installed (refreshed) into
/// [`installed_plugin_path`] with the absolute System32 `cmd.exe` (macOS: `/bin/sh`), and that copy is
/// returned. None when no template is found or the install fails (then no shell is handed a
/// plugin; the template itself is never handed out).
pub fn plugin_dir(env_dir: Option<&str>) -> Option<PathBuf> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    let src = plugin_dir_in(&candidate_plugin_dirs(env_dir, exe_dir.as_deref()))?;
    let dest = installed_plugin_path(exe_dir.as_deref())?;
    let system = system_directory()?;
    // The MCP server is only offered when its program is really beside this exe.
    let mcp = exe_dir
        .as_deref()
        .map(|d| d.join(format!("flare-mcp{}", std::env::consts::EXE_SUFFIX)))
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
    use crate::signal::agent_hook_events;
    use crate::signal::{AgentSignal, HookState, parse_agent_signal};

    #[test]
    fn two_flares_in_two_folders_never_share_an_installed_plugin() {
        let a = Path::new(r"C:\Program Files\Flare");
        let b = Path::new(r"C:\Dev\Flare\.claude\worktrees\s10\target\debug");
        let (pa, pb) = (
            installed_plugin_path(Some(a)),
            installed_plugin_path(Some(b)),
        );
        if let (Some(pa), Some(pb)) = (pa, pb) {
            assert_ne!(pa, pb, "one folder each");
            // The same folder always gets the same place, however it is spelled.
            assert_eq!(
                Some(pa),
                installed_plugin_path(Some(Path::new("c:/program files/flare/")))
            );
        }
    }

    fn plugin_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("claude-plugin")
    }

    fn read_plugin_file(rel: &str) -> String {
        std::fs::read_to_string(plugin_root().join(rel)).expect("plugin file") // sync-ok: test fixture on a temp folder
    }

    fn hooks_value() -> serde_json::Value {
        serde_json::from_str(&read_plugin_file("hooks/hooks.json")).expect("hooks.json parses")
    }

    /// Every `hook.cmd` entry as (event, matcher, the args after the four fixed ones joined by a
    /// space). The prompt hook that runs `flare-mcp` is not one: [`mcp_hook_entries`] has it.
    fn hook_entries(value: &serde_json::Value) -> Vec<(String, String, String, serde_json::Value)> {
        let mut out = Vec::new();
        for (event, groups) in value["hooks"].as_object().expect("hooks object") {
            for group in groups.as_array().expect("group list") {
                let matcher = group["matcher"].as_str().expect("matcher").to_string();
                for hook in group["hooks"].as_array().expect("hook list") {
                    if is_mcp_hook(hook) {
                        continue;
                    }
                    let args: Vec<String> = hook["args"]
                        .as_array()
                        .expect("args")
                        .iter()
                        .map(|arg| arg.as_str().expect("arg").to_string())
                        .collect();
                    // The template's and a Windows install's four `cmd.exe` arguments, or a macOS install's one script.
                    let fixed = if args.first().map(String::as_str) == Some(SH_HOOK_ARG) {
                        1
                    } else {
                        4
                    };
                    out.push((
                        event.clone(),
                        matcher.clone(),
                        args[fixed..].join(" "),
                        hook.clone(),
                    ));
                }
            }
        }
        out
    }

    /// The hook runs `flare-mcp` (the placeholder, or an installed `...\flare-mcp.exe`).
    fn is_mcp_hook(hook: &serde_json::Value) -> bool {
        let command = hook["command"].as_str().unwrap_or("");
        let lower = command.to_lowercase();
        command == MCP_EXE_PLACEHOLDER
            || lower.ends_with("flare-mcp.exe")
            || lower.ends_with("/flare-mcp")
    }

    /// Every `flare-mcp` hook entry as (event, matcher, the hook).
    fn mcp_hook_entries(value: &serde_json::Value) -> Vec<(String, String, serde_json::Value)> {
        let mut out = Vec::new();
        for (event, groups) in value["hooks"].as_object().expect("hooks object") {
            for group in groups.as_array().expect("group list") {
                let matcher = group["matcher"].as_str().expect("matcher").to_string();
                for hook in group["hooks"].as_array().expect("hook list") {
                    if is_mcp_hook(hook) {
                        out.push((event.clone(), matcher.clone(), hook.clone()));
                    }
                }
            }
        }
        out
    }

    #[test]
    fn the_prompt_hook_runs_flare_mcp_by_its_placeholder_with_one_flag() {
        let value = hooks_value();
        let entries = mcp_hook_entries(&value);
        assert_eq!(entries.len(), 1);
        let (event, matcher, hook) = &entries[0];
        assert_eq!(
            (event.as_str(), matcher.as_str()),
            ("UserPromptSubmit", "*")
        );
        assert_eq!(hook["type"].as_str(), Some("command"));
        assert_eq!(hook["command"].as_str(), Some(MCP_EXE_PLACEHOLDER));
        assert_eq!(hook["args"], serde_json::json!(["--hook-context"]));
        // Short: the hook itself gives up on Flare after a second, so a prompt is never held long.
        assert!(hook["timeout"].as_u64().expect("timeout") <= 5);
        // The state hook of the same event is still there, and still first.
        let first = &value["hooks"]["UserPromptSubmit"][0]["hooks"][0];
        assert_eq!(first["command"].as_str(), Some(SYSTEM_CMD_PLACEHOLDER));
    }

    #[test]
    fn the_prompt_hook_gets_the_absolute_program_or_is_left_out() {
        let template = read_plugin_file("hooks/hooks.json");
        let cmd = "C:\\Windows\\System32\\cmd.exe";
        let exe = "C:\\Program Files\\Flare [x]\\flare-mcp.exe";
        let with = render_hooks_with(&template, cmd, Some(exe)).expect("renders");
        assert!(!with.contains("@@"));
        assert!(hooks_commands_are_absolute(&with));
        let value: serde_json::Value = serde_json::from_str(&with).expect("json");
        let entries = mcp_hook_entries(&value);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].2["command"].as_str(), Some(exe));
        assert_eq!(
            hook_entries(&value).len(),
            25,
            "the state hooks are all kept"
        );
        // Without the program the hook goes, its empty group with it, and nothing else changes.
        let without = render_hooks_with(&template, cmd, None).expect("renders");
        assert_eq!(
            render_hooks(&template, cmd).as_deref(),
            Some(without.as_str())
        );
        assert!(!without.contains("@@") && !without.contains("\"--hook-context\""));
        let value: serde_json::Value = serde_json::from_str(&without).expect("json");
        assert!(mcp_hook_entries(&value).is_empty());
        assert_eq!(
            value["hooks"]["UserPromptSubmit"].as_array().map(Vec::len),
            Some(1)
        );
        assert_eq!(hook_entries(&value).len(), 25);
        // A relative or bare program is never written.
        assert!(render_hooks_with(&template, cmd, Some("flare-mcp.exe")).is_none());
        assert!(render_hooks_with(&template, cmd, Some("..\\flare-mcp.exe")).is_none());
    }

    #[test]
    fn install_writes_the_prompt_hook_only_when_the_program_exists_and_ships_the_browser_command() {
        let tmp = tempfile::tempdir().expect("tmp");
        let system = fake_system32(tmp.path());
        let exe = tmp.path().join(MCP_NAME);
        let with = tmp.path().join("with");
        install_plugin_with(&plugin_root(), &with, &system, Some(&exe)).expect("install");
        let text = std::fs::read_to_string(with.join("hooks").join("hooks.json")).expect("hooks"); // sync-ok: test fixture on a temp folder
        let value: serde_json::Value = serde_json::from_str(&text).expect("json");
        let entries = mcp_hook_entries(&value);
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].2["command"].as_str(),
            Some(exe.to_string_lossy().as_ref())
        );
        let command = std::fs::read_to_string(with.join("commands").join("browser.md")) // sync-ok: test fixture on a temp folder
            .expect("the /browser command is installed");
        assert!(command.contains("$ARGUMENTS"));
        let without = tmp.path().join("without");
        install_plugin(&plugin_root(), &without, &system).expect("install");
        let text =
            std::fs::read_to_string(without.join("hooks").join("hooks.json")).expect("hooks"); // sync-ok: test fixture on a temp folder
        assert!(!text.contains("\"--hook-context\"") && !text.contains("@@"));
        assert!(hooks_commands_are_absolute(&text));
        // Adding the program later rewrites the hooks and the manifest, and nothing else.
        assert_eq!(
            install_plugin_with(&plugin_root(), &without, &system, Some(&exe)).expect("again"),
            2
        );
    }

    #[test]
    fn the_browser_command_opens_a_page_looks_once_and_then_waits() {
        let text = read_plugin_file("commands/browser.md");
        assert!(text.starts_with("---\n") || text.starts_with("---\r\n"));
        for word in [
            "description:",
            "argument-hint:",
            "$ARGUMENTS",
            "open_page",
            "browser_look",
            "https://www.google.com/",
            "https://www.google.com/search?q=",
            "wait for the user",
        ] {
            assert!(text.contains(word), "browser.md lacks {word}");
        }
        // Only the two tools it needs are allowed without asking, and by their plugin names.
        let allowed = text
            .lines()
            .find_map(|l| l.strip_prefix("allowed-tools:"))
            .expect("allowed-tools");
        let tools: Vec<&str> = allowed.split(',').map(str::trim).collect();
        assert_eq!(
            tools,
            [
                "mcp__plugin_flare-status_flare__open_page",
                "mcp__plugin_flare-status_flare__browser_look"
            ]
        );
        assert!(!text.contains('\u{2014}'));
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
        assert_eq!(manifest["version"].as_str(), Some("1.2.0"));
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
        let exe = tmp.path().join(MCP_NAME);
        install_plugin_with(&plugin_root(), &with, &system, Some(&exe)).expect("install");
        let text = std::fs::read_to_string(with.join(".claude-plugin").join("plugin.json"))
            .expect("manifest"); // sync-ok: test fixture on a temp folder
        let value: serde_json::Value = serde_json::from_str(&text).expect("json");
        let command = value["mcpServers"]["flare"]["command"]
            .as_str()
            .expect("command");
        assert!(is_absolute_program(command), "{command}");
        assert!(command.ends_with(MCP_NAME));
        let without = tmp.path().join("without");
        install_plugin_with(&plugin_root(), &without, &system, None).expect("install");
        let text = std::fs::read_to_string(without.join(".claude-plugin").join("plugin.json"))
            .expect("manifest"); // sync-ok: test fixture on a temp folder
        assert!(!text.contains("mcpServers") && !text.contains("@@"));
        assert!(read_is_our_plugin(&without));
        // The installed copy follows the program: adding it later rewrites the manifest (and the
        // hooks, for the prompt hook).
        assert_eq!(
            install_plugin_with(&plugin_root(), &without, &system, Some(&exe)).expect("again"),
            2
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
        for rel in [
            ".claude-plugin/plugin.json",
            "hook.cmd",
            "hook.sh",
            "hooks/hooks.json",
            "commands/browser.md",
        ] {
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

    #[test]
    fn hook_sh_is_lf_and_static() {
        let bytes = std::fs::read(plugin_root().join("hook.sh")).expect("hook.sh"); // sync-ok: test fixture
        assert!(
            !bytes.contains(&b'\r'),
            "a CR in hook.sh: /bin/sh would read it as part of a word"
        );
        let text = String::from_utf8(bytes).expect("utf8");
        let code: String = text
            .lines()
            .filter(|l| !l.trim_start().starts_with('#'))
            .collect::<Vec<_>>()
            .join("\n");
        // No stdin, no evaluation, no program found by name beyond the shell's own builtins.
        for banned in [
            "read ", "eval", "$*", "$@", "`", "$(", "<", "exec", "source", ". /", "cat ", "env ",
        ] {
            assert!(!code.contains(banned), "hook.sh runs {banned}");
        }
        // The values are printf's arguments, never its format.
        for line in text
            .lines()
            .filter(|l| l.trim_start().starts_with("printf"))
        {
            assert!(line.trim_start().starts_with("printf '"), "{line}");
        }
        assert!(!text.contains('\u{2014}'));
    }

    #[test]
    fn a_platform_installs_its_own_hook_script_and_never_the_other() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let system = fake_system32(tmp.path());
        let dest = tmp.path().join("plug");
        install_plugin(&plugin_root(), &dest, &system).expect("install");
        assert!(dest.join(HOOK_SCRIPT).is_file());
        assert!(!dest.join(OTHER_HOOK_SCRIPT).exists());
        let value: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dest.join("hooks").join("hooks.json")).expect("hooks"), // sync-ok: test fixture
        )
        .expect("json");
        for (_, _, _, hook) in hook_entries(&value) {
            let script = hook["args"]
                .as_array()
                .expect("args")
                .iter()
                .filter_map(|a| a.as_str())
                .find(|a| a.starts_with("${CLAUDE_PLUGIN_ROOT}/"))
                .expect("the script argument")
                .to_string();
            assert_eq!(script, format!("${{CLAUDE_PLUGIN_ROOT}}/{HOOK_SCRIPT}"));
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn the_posix_render_keeps_every_state_word_and_refuses_what_it_cannot_convert() {
        let template = read_plugin_file("hooks/hooks.json");
        let exe = "/Applications/Flare [x].app/Contents/MacOS/flare-mcp";
        let out = render_hooks_with(&template, "/bin/sh", Some(exe)).expect("renders");
        assert!(!out.contains("@@") && !out.contains("/hook.cmd\"") && !out.contains("\"/d\""));
        assert!(hooks_commands_are_absolute(&out));
        let value: serde_json::Value = serde_json::from_str(&out).expect("json");
        let mut got: Vec<(String, String, String)> = hook_entries(&value)
            .into_iter()
            .map(|(e, m, a, hook)| {
                assert_eq!(hook["command"].as_str(), Some("/bin/sh"));
                assert_eq!(hook["args"][0].as_str(), Some(SH_HOOK_ARG));
                (e, m, a)
            })
            .collect();
        let mut want: Vec<(String, String, String)> = agent_hook_events()
            .into_iter()
            .map(|row| (row.event, row.matcher, row.args))
            .collect();
        got.sort();
        want.sort();
        assert_eq!(got, want);
        assert_eq!(mcp_hook_entries(&value)[0].2["command"].as_str(), Some(exe));
        // Without the program the prompt hook goes, as on Windows.
        let bare = render_hooks_with(&template, "/bin/sh", None).expect("renders");
        assert!(mcp_hook_entries(&serde_json::from_str(&bare).expect("json")).is_empty());
        // A relative program, a relative shell, a `..` shell and a hook of another shape write nothing.
        assert!(render_hooks_with(&template, "/bin/sh", Some("flare-mcp")).is_none());
        assert!(render_hooks_with(&template, "bin/sh", None).is_none());
        assert!(render_hooks_with(&template, "/x/../bin/sh", None).is_none());
        let odd = template.replacen("\"/d\"", "\"/q\"", 1);
        assert!(render_hooks_with(&odd, "/bin/sh", None).is_none());
        // The shell is `/bin/sh` from a folder named `bin`, and nothing else.
        assert_eq!(system_directory(), Some(PathBuf::from("/bin")));
        assert_eq!(
            system_cmd_path(Path::new("/bin")).as_deref(),
            Some("/bin/sh")
        );
        for bad in ["/usr/local", "bin", "/x/./bin", "/"] {
            assert_eq!(system_cmd_path(Path::new(bad)), None, "{bad}");
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn every_hook_prints_what_the_parser_reads_on_macos() {
        use std::process::{Command, Stdio};
        let tmp = tempfile::tempdir().expect("tempdir");
        let folder = tmp.path().join("Flare [x] (T) $HOME");
        std::fs::create_dir_all(&folder).expect("folder"); // sync-ok: test fixture on a temp folder
        let system = system_directory().expect("system directory");
        install_plugin(&plugin_root(), &folder, &system).expect("install");
        let folder_text = folder.to_string_lossy().to_string();
        let installed = std::fs::read_to_string(folder.join("hooks").join("hooks.json")) // sync-ok: test fixture
            .expect("installed hooks");
        let value: serde_json::Value = serde_json::from_str(&installed).expect("json");
        let entries = hook_entries(&value);
        assert_eq!(entries.len(), 25);
        for (_event, _matcher, joined, hook) in entries {
            let args: Vec<String> = hook["args"]
                .as_array()
                .expect("args")
                .iter()
                .map(|arg| {
                    arg.as_str()
                        .expect("arg")
                        .replace("${CLAUDE_PLUGIN_ROOT}", &folder_text)
                })
                .collect();
            let output = Command::new(hook["command"].as_str().expect("command")) // sync-ok: test fixture
                .args(&args)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .env("CLAUDE_CODE_SESSION_ID", SESSION)
                .output()
                .expect("/bin/sh runs the hook");
            assert!(output.status.success(), "hook {joined} failed");
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let line: serde_json::Value =
                serde_json::from_str(stdout.trim()).expect("hook prints JSON");
            let sequence = line["terminalSequence"].as_str().expect("terminalSequence");
            assert!(sequence.starts_with("\u{1b}]777;"), "{sequence:?}");
            assert!(sequence.ends_with('\u{7}'), "{sequence:?}");
            let payload = &sequence[6..sequence.len() - 1];
            assert_eq!(
                parse_agent_signal(payload),
                Some(expected_signal(&joined)),
                "payload {payload}"
            );
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn prints_the_state_as_valid_json_whatever_the_session_id_holds_on_macos() {
        use std::process::{Command, Stdio};
        let tmp = tempfile::tempdir().expect("tempdir");
        install_plugin(
            &plugin_root(),
            tmp.path(),
            &system_directory().expect("bin"),
        )
        .expect("install");
        let hook = tmp.path().join("hook.sh");
        let upper = SESSION.to_uppercase();
        let semicolon = format!(";{}", &SESSION[1..]);
        let trailing = format!("{SESSION}x");
        let quote = format!("\"{}", &SESSION[1..]);
        let run = |value: &str| {
            let output = Command::new("/bin/sh") // sync-ok: test fixture
                .arg(&hook)
                .args(["done", "+sid"])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .env("CLAUDE_CODE_SESSION_ID", value)
                .output()
                .expect("/bin/sh runs the hook");
            String::from_utf8_lossy(&output.stdout).to_string()
        };
        for value in [
            "a&calc",
            "x)\"",
            "a\\b",
            "$PATH",
            "%s%n%s",
            "$(id)",
            upper.as_str(),
            semicolon.as_str(),
            trailing.as_str(),
            quote.as_str(),
            "",
        ] {
            let stdout = run(value);
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
        assert!(run(SESSION).contains(&format!("session={SESSION}")));
    }

    /// The signal a hook with these state words (`done +sid`, `failed rate_limit`) must print.
    #[cfg(not(windows))]
    fn expected_signal(joined: &str) -> AgentSignal {
        let mut words = joined.split(' ');
        let state = match words.next().expect("state") {
            "working" => HookState::Working,
            "question" => HookState::Question,
            "done" => HookState::Done,
            "failed" => HookState::Failed,
            other => panic!("unknown state {other}"),
        };
        let (kind, session) = match words.next() {
            Some("+sid") => (None, Some(SESSION.to_string())),
            other => (other.map(str::to_string), None),
        };
        AgentSignal {
            state,
            kind,
            session,
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

    const SESSION: &str = "b44e954e-a9cf-4ba9-a3c0-99ec6795d1d9";

    /// The MCP server's file name on this platform.
    const MCP_NAME: &str = if cfg!(windows) {
        "flare-mcp.exe"
    } else {
        "flare-mcp"
    };

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

    /// A stand-in for the system folder the hooks' program comes from: `<tmp>\Windows\System32` on
    /// Windows, `<tmp>/usr/bin` on macOS (any absolute folder of that name is taken, as any drive's
    /// `System32` is on Windows).
    fn fake_system32(tmp: &Path) -> PathBuf {
        fake_system_dir(tmp, if cfg!(windows) { "Windows" } else { "usr" })
    }

    /// `<tmp>/<parent>/System32` (Windows) or `<tmp>/<parent>/bin` (macOS), made.
    fn fake_system_dir(tmp: &Path, parent: &str) -> PathBuf {
        let dir = tmp
            .join(parent)
            .join(if cfg!(windows) { "System32" } else { "bin" });
        std::fs::create_dir_all(&dir).expect("dir"); // sync-ok: test fixture on a temp folder
        dir
    }

    /// The end of the hooks' program path on this platform.
    const SYSTEM_CMD_TAIL: &str = if cfg!(windows) {
        "system32\\cmd.exe"
    } else {
        "/bin/sh"
    };

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
                command.to_lowercase().ends_with(SYSTEM_CMD_TAIL),
                "{command}"
            );
            assert!(!command.contains(SYSTEM_CMD_PLACEHOLDER));
        }
        let text = std::fs::read_to_string(dest.join("hooks").join("hooks.json")).expect("read"); // sync-ok: test fixture on a temp folder
        assert!(hooks_commands_are_absolute(&text));
        assert!(dest.join(HOOK_SCRIPT).is_file());
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
            if cfg!(windows) { "cmd.exe" } else { "sh" },
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
        let other = fake_system_dir(tmp.path(), "Other");
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
        let other = fake_system_dir(tmp.path(), "Other");
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

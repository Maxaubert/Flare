//! The throwaway world folder a scenario runs in (M9's sandbox, extended by Task 14), and the
//! shell and agent waits every scenario shares. The world holds the profile (`APPDATA\Flare`, where
//! the app keeps its logs), two folders a scenario opens as tabs (`alpha`, `beta`) and the M9
//! registry subtree. Dropping it removes the folder; nothing is left in %TEMP%.

use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};

use crate::ctx::App;
use crate::sandbox::Sandbox;
use crate::until::{wait_on, SystemClock};

/// How many times a removal is retried, and the wait between tries (PT `run.mjs`: 5 x 200 ms).
const REMOVE_RETRIES: u32 = 5;
const REMOVE_WAIT: Duration = Duration::from_millis(200);

/// The PowerShell line that stops PSReadLine taking Ctrl+V from the owner's real clipboard. Copied
/// exactly from PT `run.mjs` (`muteShellPaste`); a scenario that presses a paste key needs it.
pub const PSREADLINE_MUTE: &str =
    "Set-PSReadLineKeyHandler -Chord 'Ctrl+v','Ctrl+V','Shift+Insert' -ScriptBlock {}; Clear-Host";

/// Clear-Host as Windows has it, screen and scrollback: on macOS pwsh sends the terminfo `clear` alone, the
/// screen goes into the scrollback, and a question box "cleared" that way is still the last text of the
/// buffer, so the Mac line adds the scrollback clear (rule 1112; agentHooks and attention on the runner).
pub const CLEAR_HOST: &str = if cfg!(target_os = "macos") {
    "Clear-Host; [Console]::Write([string][char]27 + '[3J')"
} else {
    "Clear-Host"
};

/// `cat` as a scenario means it (PowerShell's Get-Content): on macOS pwsh has no `cat` alias and the
/// name is /bin/cat (rule 1112).
pub const GET_CONTENT: &str = if cfg!(target_os = "macos") {
    "Get-Content"
} else {
    "cat"
};

/// The world of one scenario.
pub struct World {
    sandbox: Sandbox,
    /// The world folder itself (its name starts `flare-e2e-`).
    pub root: PathBuf,
    /// `APPDATA\Flare`: the app's profile, with its `logs` folder.
    pub profile: PathBuf,
    /// A folder a scenario opens as its first tab.
    pub alpha: PathBuf,
    /// A folder a scenario opens as its second tab.
    pub beta: PathBuf,
}

impl World {
    /// Makes a fresh world: a sandbox folder and the `alpha` and `beta` folders beside its profile.
    pub fn new() -> World {
        let sandbox = Sandbox::new();
        let root = sandbox.root.clone();
        let profile = sandbox.profile.clone();
        let alpha = root.join("alpha");
        let beta = root.join("beta");
        for d in [&alpha, &beta] {
            fs::create_dir_all(d).expect("world folder");
        }
        World {
            sandbox,
            root,
            profile,
            alpha,
            beta,
        }
    }

    /// The environment a launched child gets: APPDATA, LOCALAPPDATA and the registry sandbox.
    pub fn env(&self) -> Vec<(String, String)> {
        self.sandbox.env()
    }

    /// The sandbox under this world (the registry subtree lives in it).
    pub fn sandbox(&self) -> &Sandbox {
        &self.sandbox
    }

    /// The last path segment of the world folder: what the reaper matches a process by.
    pub fn folder_name(&self) -> String {
        self.root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for World {
    fn drop(&mut self) {
        // The folder goes first, with retries (a file a process still holds is released soon). The
        // sandbox's own drop then removes the registry subtree and finds the folder gone.
        remove_with_retries(&self.root);
    }
}

/// Stores one key in the closed profile's `settings.json` (a JSON object of strings), keeping every
/// key already there. A file that is missing or not an object starts empty.
pub fn store_setting(profile: &Path, key: &str, value: &str) -> Result<(), String> {
    fs::create_dir_all(profile).map_err(|e| e.to_string())?;
    let file = profile.join("settings.json");
    let mut map = fs::read_to_string(&file)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    map.insert(key.to_string(), json!(value));
    fs::write(&file, Value::Object(map).to_string()).map_err(|e| e.to_string())
}

/// Puts the tabs along the top in the closed profile. Vertical is the default since 2026-11-11; a
/// scenario written for the strip says so with this before it launches.
pub fn tabs_along_the_top(profile: &Path) -> Result<(), String> {
    store_setting(profile, "flare.window.tabLayout", "horizontal")
}

/// Removes `root` recursively, retrying `REMOVE_RETRIES` times `REMOVE_WAIT` apart. True when it
/// is gone at the end. A missing folder counts as gone.
pub fn remove_with_retries(root: &Path) -> bool {
    for attempt in 0..=REMOVE_RETRIES {
        match fs::remove_dir_all(root) {
            Ok(()) => return true,
            Err(_) if !root.exists() => return true,
            Err(_) if attempt == REMOVE_RETRIES => return false,
            Err(_) => thread::sleep(REMOVE_WAIT), // sync-ok: test harness, off the app
        }
    }
    false
}

/// The shell a scenario's tab runs. A prompt is read differently for each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    PowerShell,
    Cmd,
}

/// How long a scenario waits for a shell's first prompt (45 s for PowerShell, 20 s for cmd).
pub fn prompt_timeout(shell: Shell) -> Duration {
    match shell {
        Shell::PowerShell => Duration::from_secs(45),
        Shell::Cmd => Duration::from_secs(20),
    }
}

/// True when `text`, ignoring trailing white space, ends at a prompt: PowerShell's `PS <path>>`
/// (PT `typeLine`: `PS [^>]*>\s*$`), or cmd's `<path>>`.
pub fn prompt_showing(text: &str, shell: Shell) -> bool {
    let t = text.trim_end();
    if !t.ends_with('>') {
        return false;
    }
    match shell {
        Shell::PowerShell => match t.rfind("PS ") {
            Some(at) => !t[at + 3..t.len() - 1].contains('>'),
            None => false,
        },
        Shell::Cmd => true,
    }
}

/// The diagnostics log folder of a launched app's profile, where the app writes it: `<profile>/logs`,
/// or on macOS `<home>/Library/Logs/<name>` when the profile is the default one under a HOME (the Mac
/// world's, rule 1112), as the app's `logs_dir_of` has it (Console.app looks there).
pub fn logs_dir(profile: &std::path::Path) -> std::path::PathBuf {
    let mac_default = || {
        let name = profile.file_name()?;
        let support = profile.parent()?;
        let library = support.parent()?;
        (support.file_name()? == "Application Support" && library.file_name()? == "Library")
            .then(|| library.join("Logs").join(name))
    };
    match mac_default() {
        Some(dir) if cfg!(target_os = "macos") => dir,
        _ => profile.join("logs"),
    }
}

/// The home folder the launched app sees: the owner's own on Windows (`USERPROFILE`, the app inherits
/// it), the world's `home/` on macOS (the sandbox sets HOME, rule 1104).
pub fn user_home(ctx: &mut crate::ctx::Ctx) -> String {
    #[cfg(windows)]
    {
        let _ = ctx;
        std::env::var("USERPROFILE").unwrap_or_default()
    }
    #[cfg(not(windows))]
    {
        ctx.ensure_world()
            .sandbox()
            .home
            .to_string_lossy()
            .into_owned()
    }
}

/// The text of a tab's grid, one row per line. Empty when the probe is not answered.
pub fn rows_of_value(value: &Value) -> String {
    value
        .get("rows")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .map(|r| r.as_str().unwrap_or_default())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// The text of one tab (`grid.text` on that tab). Empty when the probe is not answered.
pub fn rows_text(app: &mut App, tab: &str) -> String {
    match app.session.probe("grid.text", json!({ "tab": tab })) {
        Ok(v) => rows_of_value(&v),
        Err(_) => String::new(),
    }
}

/// Polls `cond` every 25 ms until it holds or `timeout` passes. True when it held.
fn poll(timeout: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let mut clock = SystemClock::new();
    wait_on(&mut clock, timeout.as_millis() as u64, &mut cond)
}

/// Waits until the tab shows a prompt for `shell`. True on success; the caller records the FAIL.
pub fn wait_for_prompt(app: &mut App, tab: &str, shell: Shell, timeout: Duration) -> bool {
    poll(timeout, || prompt_showing(&rows_text(app, tab), shell))
}

/// Types the PSReadLine mute line into the front tab and presses Enter, then gives the shell a
/// moment to take it (PT `muteShellPaste`, which waits 300 ms). The caller waits for the prompt.
pub fn mute_psreadline_paste(app: &mut App) {
    let _ = app.session.type_text(PSREADLINE_MUTE);
    let _ = app.session.key("Enter");
    thread::sleep(Duration::from_millis(300)); // sync-ok: test harness, off the app
}

/// True when the `agent.state` answer says the process poll has given its first verdict on the tab.
/// The field is `polled`; Task 26 answers it.
pub fn verdict_said(value: &Value) -> bool {
    value
        .get("polled")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// The wait behind `wait_for_agent_verdict`, over any probe: true once the probe reports a verdict.
/// A probe that errors (the probe is not there yet) counts as no verdict, so a missing probe times
/// out and is a FAIL, never a pass.
pub fn wait_for_agent_verdict_with(
    probe: &mut dyn FnMut() -> Result<Value, String>,
    timeout: Duration,
) -> bool {
    poll(timeout, || matches!(probe(), Ok(v) if verdict_said(&v)))
}

/// Waits until the process poll has said its first word on `tab` (PT `run.mjs:126-133`). A shell
/// stood in for an agent calls this before it types, so a Claude title is not cleared by the poll.
pub fn wait_for_agent_verdict(app: &mut App, tab: &str, timeout: Duration) -> bool {
    let mut probe = || app.session.probe("agent.state", json!({ "tab": tab }));
    wait_for_agent_verdict_with(&mut probe, timeout)
}

#[cfg(test)]
mod settings_tests {
    use super::*;

    #[test]
    fn tabs_along_the_top_keeps_the_keys_already_stored() {
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("profile");
        tabs_along_the_top(&profile).unwrap();
        let read = || -> Value {
            serde_json::from_str(&fs::read_to_string(profile.join("settings.json")).unwrap())
                .unwrap()
        };
        assert_eq!(read(), json!({ "flare.window.tabLayout": "horizontal" }));
        fs::write(
            profile.join("settings.json"),
            r#"{"flare.term.fontPct":"130","flare.window.tabLayout":"vertical"}"#,
        )
        .unwrap();
        tabs_along_the_top(&profile).unwrap();
        assert_eq!(
            read(),
            json!({ "flare.term.fontPct": "130", "flare.window.tabLayout": "horizontal" })
        );
        fs::write(profile.join("settings.json"), "not json").unwrap();
        store_setting(&profile, "flare.window.edges", "faint").unwrap();
        assert_eq!(read(), json!({ "flare.window.edges": "faint" }));
    }
}

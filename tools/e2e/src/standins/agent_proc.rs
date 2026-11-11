//! A REAL AGENT PROCESS for a tab (review #47, owner 2026-10-23, "one check per claim").
//!
//! A hook line or a Claude title is a CLAIM about a tab; the one look it asks of the process poll takes
//! it back when no agent runs there. A scenario that sends hooks or titles from a plain shell is
//! therefore claiming about a tab with no agent. It starts this stand-in first: a copy of Windows
//! PowerShell named `claude.exe`, which the poll's detector accepts by program name, run inside the tab's
//! shell. What the scenario types afterwards goes to that nested shell, as a genuine agent's hooks come
//! from inside the agent.

use std::fs;
use std::path::Path;

/// The stand-in's source: the Windows PowerShell, copied under the name the poll wants.
#[cfg(windows)]
const POWERSHELL: &str = r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe";

/// How long after the command the poll has had time to see the process (the next line waits for its
/// prompt anyway; this is the margin for the first look).
pub const SETTLE_MS: u64 = 1_500;

/// macOS (rule 1109): the stand-in is a script named `claude` run by its interpreter (`/bin/zsh
/// <dir>/claude`, which the poll reads as the agent by its exact name, as an npm launcher under node)
/// that starts pwsh as its child; what the scenario types goes to that pwsh, as on Windows. A copy of
/// pwsh would not run (its runtime is beside the real one).
#[cfg(not(windows))]
pub fn command(profile: &Path) -> Result<String, String> {
    use std::os::unix::fs::PermissionsExt;
    let dir = profile.join("stand-in");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let fake = dir.join("claude");
    let pwsh = crate::standins::powershell();
    fs::write(&fake, format!("#!/bin/zsh\n'{pwsh}' \"$@\"\n")).map_err(|e| e.to_string())?;
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    Ok(command_for(&fake))
}

/// The line that starts the stand-in in a PowerShell tab, after writing `claude.exe` under `profile`.
#[cfg(windows)]
pub fn command(profile: &Path) -> Result<String, String> {
    let dir = profile.join("stand-in");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let fake = dir.join("claude.exe");
    if !fake.is_file() {
        fs::copy(POWERSHELL, &fake).map_err(|e| e.to_string())?;
    }
    Ok(command_for(&fake))
}

/// The start line for a stand-in at `exe`.
fn command_for(exe: &Path) -> String {
    format!("& '{}' -NoLogo -NoProfile", exe.display())
}

/// How many lines `wait_ready` sends before it gives up.
const READY_TRIES: u32 = 4;
/// How long one of them may take to print its answer.
const READY_WAIT_MS: u64 = 4_000;

/// The line `wait_ready` types on try `n`. The answer is built in the shell, so the echo of the typed
/// line never reads as the answer.
fn ready_line(n: u32) -> String {
    format!("Write-Host ('stand-in' + '-ready-' + '{n}')")
}

/// Whether `rows` (a tab's text) shows try `n`'s answer on a row of its own.
fn ready_seen(rows: &str, n: u32) -> bool {
    let answer = format!("stand-in-ready-{n}");
    rows.lines().any(|l| l.trim() == answer)
}

/// macOS (rule 1136): returns once the stand-in's pwsh in the front tab has run a line, true when it
/// did. A line typed the moment the nested pwsh shows its first prompt can be lost on the Mac runner
/// (its Enter, or the whole line), so the hook line typed next never ran and the tab never worked
/// (colourPicker, markStrip, the reduced-motion marks). Each try first presses Enter, so a line left
/// half taken runs (and fails) before the next one is typed. Windows returns at once, as before.
pub fn wait_ready(app: &mut crate::ctx::App) -> bool {
    if !cfg!(target_os = "macos") {
        return true;
    }
    round_trip(app)
}

/// The tries behind `wait_ready`.
fn round_trip(app: &mut crate::ctx::App) -> bool {
    let tab = app
        .session
        .state()
        .ok()
        .and_then(|s| s.front)
        .unwrap_or_default();
    for n in 1..=READY_TRIES {
        let _ = app.session.key("Enter");
        let _ = app.session.type_text(&ready_line(n));
        let _ = app.session.key("Enter");
        let start = std::time::Instant::now();
        while start.elapsed() < std::time::Duration::from_millis(READY_WAIT_MS) {
            if ready_seen(&crate::world::rows_text(app, &tab), n) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(50)); // sync-ok: test harness, off the app
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stand_in_is_named_for_the_poll_and_started_with_no_profile() {
        let line = command_for(Path::new(r"C:\x\stand-in\claude.exe"));
        assert_eq!(line, r"& 'C:\x\stand-in\claude.exe' -NoLogo -NoProfile");
    }

    // Rule 1136: only the shell's answer counts, never the echo of the typed line or an older try's.
    #[test]
    fn the_ready_answer_is_the_printed_row_and_not_the_typed_line() {
        let typed = format!("PS /tmp/home> {}", ready_line(2));
        assert!(!ready_seen(&typed, 2));
        assert!(!ready_seen(
            &format!("{typed}\nstand-in-ready-1\nPS /tmp/home>"),
            2
        ));
        assert!(ready_seen(
            &format!("{typed}\nstand-in-ready-2  \nPS /tmp/home>"),
            2
        ));
    }
}

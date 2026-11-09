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

/// macOS (rule 336): the stand-in is a script named `claude` run by its interpreter (`/bin/zsh
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stand_in_is_named_for_the_poll_and_started_with_no_profile() {
        let line = command_for(Path::new(r"C:\x\stand-in\claude.exe"));
        assert_eq!(line, r"& 'C:\x\stand-in\claude.exe' -NoLogo -NoProfile");
    }
}

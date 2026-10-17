//! Which shells the command help knows (M7 Task 5). Kept apart from the catalogue so a host can
//! read the shell choice without loading every entry (`PT/core/shared/help/shells.ts`).

use crate::types::{HelpShell, HelpShellChoice, Variant};

/// The shells somebody can be in, with the names the panel shows.
pub const HELP_SHELLS: [(HelpShellChoice, &str); 3] = [
    (HelpShellChoice::Powershell, "PowerShell"),
    (HelpShellChoice::Cmd, "Command Prompt"),
    (HelpShellChoice::Bash, "Bash"),
];

fn entry_shell_of(shell: HelpShellChoice) -> HelpShell {
    match shell {
        HelpShellChoice::Powershell => HelpShell::Powershell,
        HelpShellChoice::Cmd => HelpShell::Cmd,
        HelpShellChoice::Bash => HelpShell::Bash,
    }
}

/// Whether an entry written for `entry` is shown to someone in `shell`: its own shell, every
/// `Any` entry, and a `Windows` entry outside Bash.
pub fn shown_in(entry: HelpShell, shell: HelpShellChoice) -> bool {
    entry == entry_shell_of(shell)
        || entry == HelpShell::Any
        || (entry == HelpShell::Windows && shell != HelpShellChoice::Bash)
}

/// The variants shown in `shell`: those with no `shells` list, or one that names it.
pub fn variants_in(variants: &[Variant], shell: HelpShellChoice) -> Vec<&Variant> {
    variants
        .iter()
        .filter(|v| v.shells.as_ref().is_none_or(|s| s.contains(&shell)))
        .collect()
}

/// The command language a detected shell speaks, from its id: `cmd`, `wsl-<distro>` and anything
/// with `bash` in it are Bash-or-cmd, and every other id (including none) reads as PowerShell.
pub fn shell_of_shell_id(id: Option<&str>) -> HelpShellChoice {
    let v = id.unwrap_or("").to_lowercase();
    if v == "cmd" {
        HelpShellChoice::Cmd
    } else if v.starts_with("wsl") || v.contains("bash") {
        HelpShellChoice::Bash
    } else {
        HelpShellChoice::Powershell
    }
}

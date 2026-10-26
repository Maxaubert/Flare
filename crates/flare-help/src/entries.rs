//! The help catalogue, loaded once from the generated data files (M7 Task 5). The seven files
//! come from Prism Terminal's own TypeScript (`tools/oracle-help`), with Flare's own entries from
//! its `flare.mjs` (agy, 2026-10-26); nothing here is typed by hand.

use crate::shells::shown_in;
use crate::types::{HelpCategory, HelpEntry, HelpShellChoice};
use std::sync::OnceLock;

static ALL: OnceLock<Vec<HelpEntry>> = OnceLock::new();

/// The whole catalogue, one shared list in the oracle's order: PowerShell, Command Prompt, Bash,
/// git, the agents, the package managers, WSL.
pub fn all_help() -> &'static [HelpEntry] {
    ALL.get_or_init(|| {
        let files = [
            include_str!("../data/powershell.json"),
            include_str!("../data/cmd.json"),
            include_str!("../data/bash.json"),
            include_str!("../data/git.json"),
            include_str!("../data/agents.json"),
            include_str!("../data/packages.json"),
            include_str!("../data/wsl.json"),
        ];
        let mut all = Vec::new();
        for text in files {
            let mut part: Vec<HelpEntry> =
                serde_json::from_str(text).expect("generated catalogue file parses");
            all.append(&mut part);
        }
        all
    })
}

/// The categories in the order the panel browses them, with the names it shows.
pub const HELP_CATEGORIES: [(HelpCategory, &str); 11] = [
    (HelpCategory::Folders, "Folders"),
    (HelpCategory::Files, "Files"),
    (HelpCategory::Text, "Text and search"),
    (HelpCategory::Processes, "Processes and ports"),
    (HelpCategory::Network, "Network"),
    (HelpCategory::System, "System"),
    (HelpCategory::Git, "Git"),
    (HelpCategory::Agents, "Claude Code, Codex and Antigravity"),
    (HelpCategory::Packages, "Installing things"),
    (HelpCategory::Wsl, "WSL (Linux on Windows)"),
    (HelpCategory::Shell, "The shell itself"),
];

/// The id prefix an entry of each shell wears.
pub fn id_prefix(shell: HelpShellChoice) -> &'static str {
    match shell {
        HelpShellChoice::Powershell => "ps-",
        HelpShellChoice::Cmd => "cmd-",
        HelpShellChoice::Bash => "sh-",
    }
}

/// What one shell's reader is offered: its own entries and every `Any` one, in catalogue order.
pub fn help_for(shell: HelpShellChoice) -> Vec<&'static HelpEntry> {
    all_help()
        .iter()
        .filter(|e| shown_in(e.shell, shell))
        .collect()
}

/// A command as the row draws and copies it: the placeholders at its end dropped (`Set-Location
/// FOLDER` reads `Set-Location `). Only names the entry declares, bare or wholly quoted, and only
/// trailing ones. The trailing space is kept.
pub fn blankless(command: &str, names: &[&str]) -> String {
    if names.is_empty() {
        return command.to_string();
    }
    let words: Vec<&str> = command.split(' ').collect();
    let blank = |w: &str| names.contains(&unquote(w));
    let mut end = words.len();
    while end > 1 && blank(words[end - 1]) {
        end -= 1;
    }
    if end == words.len() {
        command.to_string()
    } else {
        format!("{} ", words[..end].join(" "))
    }
}

/// One pair of matching outer quotes removed (`'X'` and `"X"` read as `X`).
fn unquote(w: &str) -> &str {
    let bytes = w.as_bytes();
    if bytes.len() >= 2
        && (bytes[0] == b'"' || bytes[0] == b'\'')
        && bytes[bytes.len() - 1] == bytes[0]
    {
        &w[1..w.len() - 1]
    } else {
        w
    }
}

/// A "command" that is a key to press (`Ctrl+C`, `Esc`, `F1`), not text to paste.
pub fn is_key_press(command: &str) -> bool {
    let mut rest = command;
    loop {
        let stripped = ["Ctrl+", "Alt+", "Shift+", "Win+"]
            .iter()
            .find_map(|m| rest.strip_prefix(m));
        match stripped {
            Some(r) => rest = r,
            None => break,
        }
    }
    is_key(rest)
}

fn is_key(k: &str) -> bool {
    if matches!(k, "Esc" | "Tab" | "Enter" | "Space" | "Backspace") {
        return true;
    }
    if k.len() == 1 && k.bytes().next().is_some_and(|b| b.is_ascii_uppercase()) {
        return true;
    }
    match k.strip_prefix('F') {
        Some(n) => (1..=2).contains(&n.len()) && n.bytes().all(|b| b.is_ascii_digit()),
        None => false,
    }
}

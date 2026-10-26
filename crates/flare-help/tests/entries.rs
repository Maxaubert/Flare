//! The command help types, shells and catalogue (M7 Task 5). Ported from Prism Terminal's
//! `core/shared/help/catalogue.test.ts` (the key, shell, blankless and catalogue-load rules).

use flare_help::entries::{
    HELP_CATEGORIES, all_help, blankless, help_for, id_prefix, is_key_press,
};
use flare_help::shells::{HELP_SHELLS, shell_of_shell_id, shown_in, variants_in};
use flare_help::types::{HelpCategory, HelpEntry, HelpShell, HelpShellChoice};
use regex::Regex;
use std::ptr;

const BLANKLESS_FIXTURE: &str = include_str!("fixtures/blankless.json");
const META_FIXTURE: &str = include_str!("fixtures/meta.json");
const POWERSHELL_DATA: &str = include_str!("../data/powershell.json");

fn commands_of(e: &HelpEntry) -> Vec<&str> {
    let mut out = vec![e.command.as_str()];
    out.extend(e.variants.iter().map(|v| v.command.as_str()));
    out
}

/// Whether `s` starts with `word` followed by a non-word character or the end (JS `^word\b`).
fn starts_with_word(s: &str, word: &str) -> bool {
    match s.strip_prefix(word) {
        Some(rest) => !rest
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_'),
        None => false,
    }
}

#[test]
fn a_key_is_told_from_a_command() {
    for k in [
        "Ctrl+C",
        "Esc",
        "Shift+Tab",
        "Ctrl+D",
        "Alt+V",
        "Shift+Enter",
        "Ctrl+J",
        "F1",
    ] {
        assert!(is_key_press(k), "{k}");
    }
    for c in [
        "cd ..",
        ":q!",
        "/compact",
        "git status",
        "Get-ChildItem",
        "ls",
        "Ctrl+C twice",
        "@PATH",
        "q",
    ] {
        assert!(!is_key_press(c), "{c}");
    }
}

#[test]
fn every_key_shaped_command_is_a_key() {
    let keys: Vec<&str> = all_help()
        .iter()
        .flat_map(commands_of)
        .filter(|c| {
            ["Ctrl", "Alt", "Shift", "Esc"]
                .iter()
                .any(|w| starts_with_word(c, w))
        })
        .collect();
    assert!(keys.len() > 5, "found {}", keys.len());
    let not_keys: Vec<&str> = keys.iter().copied().filter(|c| !is_key_press(c)).collect();
    assert!(not_keys.is_empty(), "{not_keys:?}");
}

#[test]
fn shell_ids_read_as_the_language_they_speak() {
    assert_eq!(shell_of_shell_id(Some("pwsh")), HelpShellChoice::Powershell);
    assert_eq!(
        shell_of_shell_id(Some("powershell")),
        HelpShellChoice::Powershell
    );
    assert_eq!(shell_of_shell_id(Some("cmd")), HelpShellChoice::Cmd);
    assert_eq!(shell_of_shell_id(Some("wsl-Ubuntu")), HelpShellChoice::Bash);
    assert_eq!(
        shell_of_shell_id(Some("wsl-Debian-12")),
        HelpShellChoice::Bash
    );
    assert_eq!(shell_of_shell_id(Some("git-bash")), HelpShellChoice::Bash);
    assert_eq!(shell_of_shell_id(Some("")), HelpShellChoice::Powershell);
    assert_eq!(shell_of_shell_id(None), HelpShellChoice::Powershell);
    assert_eq!(shell_of_shell_id(Some("CMD")), HelpShellChoice::Cmd);
}

#[test]
fn each_shell_gets_its_own_the_any_ones_and_the_windows_ones_outside_bash() {
    for (shell, _) in HELP_SHELLS {
        let list = help_for(shell);
        assert!(list.len() > 100, "{shell:?}: {}", list.len());
        assert!(list.iter().all(|e| shown_in(e.shell, shell)));
        let has_windows = list.iter().any(|e| e.shell == HelpShell::Windows);
        assert_eq!(has_windows, shell != HelpShellChoice::Bash, "{shell:?}");
        assert!(
            list.iter()
                .filter(|e| e.category == HelpCategory::Git)
                .count()
                > 30,
            "{shell:?} git"
        );
        assert!(
            list.iter()
                .filter(|e| e.category == HelpCategory::Agents)
                .count()
                > 30,
            "{shell:?} agents"
        );
    }
}

/// The WINDOWS_ONLY pattern of the TypeScript gate, written by hand.
fn windows_only() -> Regex {
    Regex::new(r"(?i)\bwinget\b|\.exe\b|\$env:|\birm\b|\w\\|\\\w|\.ps1\b|\.bat\b").unwrap()
}

#[test]
fn bash_sees_no_windows_only_command_from_a_shared_entry() {
    let re = windows_only();
    let mut bad = Vec::new();
    for e in help_for(HelpShellChoice::Bash)
        .into_iter()
        .filter(|e| e.shell != HelpShell::Bash)
    {
        let mut rows = vec![e.command.as_str()];
        rows.extend(
            variants_in(&e.variants, HelpShellChoice::Bash)
                .into_iter()
                .map(|v| v.command.as_str()),
        );
        for c in rows {
            if re.is_match(c) {
                bad.push(format!("{}: {c}", e.id));
            }
        }
    }
    assert!(bad.is_empty(), "{bad:?}");
}

#[test]
fn bash_sees_python3_never_a_bare_python() {
    let re = Regex::new(r"(^|\s)python\s").unwrap();
    let mut bad = Vec::new();
    for e in help_for(HelpShellChoice::Bash) {
        let mut rows = vec![e.command.as_str()];
        rows.extend(
            variants_in(&e.variants, HelpShellChoice::Bash)
                .into_iter()
                .map(|v| v.command.as_str()),
        );
        for c in rows {
            if re.is_match(c) {
                bad.push(format!("{}: {c}", e.id));
            }
        }
    }
    assert!(bad.is_empty(), "{bad:?}");
}

#[test]
fn blankless_drops_trailing_bare_placeholders() {
    assert_eq!(
        blankless("Set-Location FOLDER", &["FOLDER"]),
        "Set-Location "
    );
    assert_eq!(
        blankless("wsl --export DISTRO FILE", &["DISTRO", "FILE"]),
        "wsl --export "
    );
}

#[test]
fn blankless_drops_a_wholly_quoted_one_at_the_end() {
    assert_eq!(
        blankless("git commit -m \"MESSAGE\"", &["MESSAGE"]),
        "git commit -m "
    );
    assert_eq!(
        blankless("Add-Content FILE 'TEXT'", &["FILE", "TEXT"]),
        "Add-Content "
    );
}

#[test]
fn blankless_keeps_the_middle_a_partly_quoted_one_and_never_the_whole_command() {
    assert_eq!(
        blankless("Remove-Item FOLDER -Recurse -Force", &["FOLDER"]),
        "Remove-Item FOLDER -Recurse -Force"
    );
    assert_eq!(
        blankless("Get-ChildItem -Filter '*.EXT'", &["EXT"]),
        "Get-ChildItem -Filter '*.EXT'"
    );
    assert_eq!(blankless("git status", &[]), "git status");
    assert_eq!(blankless("FOLDER", &["FOLDER"]), "FOLDER");
}

#[test]
fn blankless_and_key_press_match_the_oracle_for_every_command() {
    let rows: Vec<serde_json::Value> = serde_json::from_str(BLANKLESS_FIXTURE).unwrap();
    assert!(rows.len() > 500, "{}", rows.len());
    let mut bad = Vec::new();
    for row in &rows {
        let command = row["command"].as_str().unwrap();
        let names: Vec<&str> = row["names"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n.as_str().unwrap())
            .collect();
        let out = row["out"].as_str().unwrap();
        let key = row["key"].as_bool().unwrap();
        if blankless(command, &names) != out {
            bad.push(format!(
                "blankless {command:?} names {names:?} got {:?} want {out:?}",
                blankless(command, &names)
            ));
        }
        if is_key_press(command) != key {
            bad.push(format!("key {command:?}"));
        }
    }
    assert!(
        bad.is_empty(),
        "{} mismatches, first {:?}",
        bad.len(),
        bad.first()
    );
}

#[test]
fn the_catalogue_loads_once_in_the_oracle_order() {
    let meta: serde_json::Value = serde_json::from_str(META_FIXTURE).unwrap();
    let total = meta["total"].as_u64().unwrap() as usize;
    assert_eq!(all_help().len(), total);
    // The first entry is the first of powershell.json, and the catalogue is one shared list.
    let data: Vec<serde_json::Value> = serde_json::from_str(POWERSHELL_DATA).unwrap();
    assert_eq!(all_help()[0].id, data[0]["id"].as_str().unwrap());
    assert!(ptr::eq(
        help_for(HelpShellChoice::Powershell)[0],
        &all_help()[0]
    ));
    assert!(ptr::eq(all_help(), all_help()));
}

#[test]
fn the_categories_are_in_the_panels_order_with_the_panels_names() {
    let want = vec![
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
    assert_eq!(HELP_CATEGORIES.to_vec(), want);
}

#[test]
fn id_prefixes_follow_the_shell() {
    assert_eq!(id_prefix(HelpShellChoice::Powershell), "ps-");
    assert_eq!(id_prefix(HelpShellChoice::Cmd), "cmd-");
    assert_eq!(id_prefix(HelpShellChoice::Bash), "sh-");
}

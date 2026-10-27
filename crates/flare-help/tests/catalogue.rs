//! The gate for the catalogue's content (M7 Task 27). Ported from Prism Terminal's
//! `core/shared/help/catalogue.test.ts`: the parts Task 5 did not port. The catalogue is curated text
//! written to be copied into a shell, so every rule here is held over the generated data, the
//! destructive commands most of all. Each check collects every offender and reports them together.

use fancy_regex::Regex as FancyRegex;
use flare_help::entries::{HELP_CATEGORIES, all_help, id_prefix};
use flare_help::search::{HelpSearchOptions, search_help};
use flare_help::shells::HELP_SHELLS;
use flare_help::types::{HelpCategory, HelpEntry, HelpShell, HelpShellChoice};
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::sync::OnceLock;

/// Built from its code point, so this file does not hold the character it forbids.
const EM_DASH: char = '\u{2014}';

/// Every command an entry offers: its own, then each variant's.
fn commands_of(e: &HelpEntry) -> Vec<&str> {
    let mut out = vec![e.command.as_str()];
    out.extend(e.variants.iter().map(|v| v.command.as_str()));
    out
}

/// Every string a person reads or copies, for the rules that cover all text.
fn texts_of(e: &HelpEntry) -> Vec<&str> {
    let mut out = vec![e.id.as_str(), e.task.as_str(), e.summary.as_str()];
    out.push(e.danger.as_deref().unwrap_or(""));
    out.extend(commands_of(e));
    for v in &e.variants {
        out.push(v.label.as_str());
    }
    for (k, v) in &e.placeholders {
        out.push(k.as_str());
        out.push(v.as_str());
    }
    out.extend(e.keywords.iter().map(String::as_str));
    out
}

/// JavaScript's string length: UTF-16 code units.
fn js_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// Words that are the command's own, not something to replace: git's HEAD, netstat's LISTENING, an
/// environment variable's real name. Everything else in capitals (three letters or more) must be a
/// declared placeholder, which keeps a reader from pasting the word FOLDER into a shell.
const LITERAL_CAPS: [&str; 40] = [
    "HEAD",
    "LISTENING",
    "PATH",
    "PATHEXT",
    "DATE",
    "TIME",
    "HOME",
    "USERPROFILE",
    "USERNAME",
    "APPDATA",
    "LOCALAPPDATA",
    "TEMP",
    "PROFILE",
    "PWD",
    "LASTEXITCODE",
    "ERRORLEVEL",
    "PSVERSIONTABLE",
    "MX",
    "TXT",
    "POST",
    "GET",
    "JSON",
    "CSV",
    "README",
    "CLAUDE",
    "AGENTS",
    "SHA256",
    "SHA1",
    "LTS",
    "FALSE",
    "GEQ",
    "MD5",
    "UTF8",
    "WSL",
    "PID",
    "CPU",
    "WS",
    "AND",
    "TCP",
    "UDP",
];

/// The capitalised words of a command that stand for something the reader supplies. A word right
/// after `$`, `%` or `:` is a variable's real name.
fn placeholders_in(command: &str) -> Vec<String> {
    let re = FancyRegex::new(r"(^|[^A-Za-z0-9_$%:])([A-Z][A-Z0-9_]{2,})(?![A-Za-z0-9_])")
        .expect("placeholder pattern compiles");
    let mut found = Vec::new();
    for caps in re.captures_iter(command) {
        let caps = caps.expect("placeholder scan");
        let word = &caps[2];
        if !LITERAL_CAPS.contains(&word) {
            found.push(word.to_string());
        }
    }
    found
}

/// The start of a pipeline stage, where a destructive word is a command and not an everyday word.
const STAGE: &str = r"(?:^|[|;&(]\s*|\bsudo\s+|\bxargs\s+(?:-\S+\s+)*)";

fn stage(rest: &str) -> String {
    format!("{STAGE}{rest}")
}

/// What counts as destructive: a pattern tested against each command (the main one and every
/// variant). A hit anywhere obliges the entry to carry a `danger` line. These are Prism Terminal's
/// 25 patterns, ported to fancy-regex for the two lookbehinds.
fn destructive() -> Vec<(&'static str, FancyRegex)> {
    let p = |name: &'static str, pattern: String| {
        (
            name,
            FancyRegex::new(&pattern).expect("destructive pattern compiles"),
        )
    };
    vec![
        p(
            "Remove-Item",
            r"(?i)(?<!Get-Help\s)(?<!Get-Command\s)\bRemove-Item\b(?!\s+(?:Env|Alias|Function|Variable):)(?!.*-WhatIf)"
                .to_string(),
        ),
        p("rm", stage(r"rm(?:\s|$)")),
        p("rmdir /s", r"(?i)\brmdir\s+/s\b".to_string()),
        p("rd /s", stage(r"(?i)rd\s+/s\b")),
        p("del", stage(r"(?i)(?:del|erase)(?:\s|$)")),
        p("format", stage(r"(?i)format\s+[A-Za-z]:")),
        p("Stop-Process", r"(?i)\bStop-Process\b(?!.*-WhatIf)".to_string()),
        p("taskkill", r"(?i)\btaskkill\b".to_string()),
        p("kill", stage(r"(?:kill|pkill|killall)(?:\s|$)")),
        p("git reset --hard", r"\bgit\s+reset\s+--hard\b".to_string()),
        p("git clean", r"\bgit\s+clean\b(?!\s+-\w*n)".to_string()),
        p("git push --force", r"\bgit\s+push\b.*(?:--force|\s-f\b)".to_string()),
        p("git checkout --", r"\bgit\s+checkout\s+(?:\S+\s+)?--\s".to_string()),
        p("git restore", r"\bgit\s+restore\s+(?!--staged)".to_string()),
        p("git branch -D", r"\bgit\s+branch\s+-D\b".to_string()),
        p("git stash drop / clear", r"\bgit\s+stash\s+(?:drop|clear)\b".to_string()),
        p("DROP", r"(?i)\bDROP\s+(?:TABLE|DATABASE)\b".to_string()),
        p("mkfs", r"\bmkfs\b".to_string()),
        p("dd", stage(r"dd\s+.*\bof=")),
        p("Set-Content / Out-File", r"(?i)\b(?:Set-Content|Out-File)\b".to_string()),
        p("Clear-Content", r"(?i)\bClear-Content\b".to_string()),
        p("npm cache clean", r"\bnpm\s+cache\s+clean\b".to_string()),
        p("sed -i", r"\bsed\s+-i(?!\.)".to_string()),
        p(
            "redirect over a file",
            r"(?i)(?<![>0-9&=-])>(?!>)\s*(?!nul\b|/dev/null|\$null|&)\S".to_string(),
        ),
        p("download over a file", r"\bcurl(?:\.exe)?\b.*\s-\w*[oO]\b|-OutFile\b".to_string()),
    ]
}

/// The patterns, compiled once for the whole run.
fn destructive_patterns() -> &'static [(&'static str, FancyRegex)] {
    static PATTERNS: OnceLock<Vec<(&'static str, FancyRegex)>> = OnceLock::new();
    PATTERNS.get_or_init(destructive)
}

fn destructive_hits(e: &HelpEntry) -> Vec<&'static str> {
    let commands = commands_of(e);
    destructive_patterns()
        .iter()
        .filter(|(_, re)| {
            commands
                .iter()
                .any(|c| re.is_match(c).expect("destructive scan"))
        })
        .map(|(name, _)| *name)
        .collect()
}

fn has_danger(e: &HelpEntry) -> bool {
    e.danger.as_deref().is_some_and(|d| !d.trim().is_empty())
}

fn shell_choice(shell: HelpShell) -> Option<HelpShellChoice> {
    match shell {
        HelpShell::Powershell => Some(HelpShellChoice::Powershell),
        HelpShell::Cmd => Some(HelpShellChoice::Cmd),
        HelpShell::Bash => Some(HelpShellChoice::Bash),
        HelpShell::Any | HelpShell::Windows => None,
    }
}

fn shell_search(shell: HelpShellChoice) -> HelpShell {
    match shell {
        HelpShellChoice::Powershell => HelpShell::Powershell,
        HelpShellChoice::Cmd => HelpShell::Cmd,
        HelpShellChoice::Bash => HelpShell::Bash,
    }
}

#[test]
fn a_few_hundred_entries_and_one_list() {
    let n = all_help().len();
    assert!(n > 250, "only {n} entries");
    assert!(n < 600, "{n} entries");
}

#[test]
fn ids_are_unique_and_kebab_case() {
    let kebab = Regex::new(r"^[a-z][a-z0-9]*(-[a-z0-9]+)+$").unwrap();
    let mut seen = BTreeMap::new();
    for e in all_help() {
        *seen.entry(e.id.as_str()).or_insert(0usize) += 1;
    }
    let dupes: Vec<&str> = seen
        .iter()
        .filter(|(_, n)| **n > 1)
        .map(|(id, _)| *id)
        .collect();
    assert!(dupes.is_empty(), "duplicate ids: {dupes:?}");
    let bad: Vec<&str> = all_help()
        .iter()
        .filter(|e| !kebab.is_match(&e.id))
        .map(|e| e.id.as_str())
        .collect();
    assert!(bad.is_empty(), "not kebab-case: {bad:?}");
}

#[test]
fn ids_carry_their_shell_or_tool_prefix() {
    let any_prefix = |c: HelpCategory| -> Option<&'static str> {
        match c {
            HelpCategory::Git => Some("git-"),
            HelpCategory::Agents => Some("claude-|codex-|agy-"),
            HelpCategory::Packages => Some("pkg-"),
            HelpCategory::Wsl => Some("wsl-"),
            _ => None,
        }
    };
    let matches_any = |id: &str, prefixes: &str| prefixes.split('|').any(|p| id.starts_with(p));
    let mut wrong = Vec::new();
    for e in all_help() {
        let ok = match e.shell {
            HelpShell::Any | HelpShell::Windows => {
                any_prefix(e.category).is_some_and(|p| matches_any(&e.id, p))
            }
            other => {
                let choice = shell_choice(other).expect("a shell entry");
                e.id.starts_with(id_prefix(choice))
            }
        };
        if !ok {
            wrong.push(format!("{} ({:?}/{:?})", e.id, e.shell, e.category));
        }
    }
    assert!(wrong.is_empty(), "wrong id prefix: {wrong:?}");
}

#[test]
fn every_category_is_listed_and_none_is_empty() {
    let listed: Vec<HelpCategory> = HELP_CATEGORIES.iter().map(|(c, _)| *c).collect();
    let unlisted: Vec<&str> = all_help()
        .iter()
        .filter(|e| !listed.contains(&e.category))
        .map(|e| e.id.as_str())
        .collect();
    assert!(unlisted.is_empty(), "unlisted categories: {unlisted:?}");
    for (c, name) in HELP_CATEGORIES {
        assert!(
            all_help().iter().any(|e| e.category == c),
            "category {name} has no entry"
        );
    }
}

#[test]
fn a_task_is_a_short_sentence_with_no_full_stop() {
    let bad: Vec<String> = all_help()
        .iter()
        .filter(|e| {
            let first = e.task.chars().next();
            let starts_ok = first == Some('"') || first.is_some_and(|c| c.is_ascii_uppercase());
            js_len(&e.task) < 8 || js_len(&e.task) > 62 || e.task.ends_with('.') || !starts_ok
        })
        .map(|e| format!("{}: {}", e.id, e.task))
        .collect();
    assert!(bad.is_empty(), "{bad:?}");
}

#[test]
fn summaries_and_variants_are_present() {
    let short: Vec<&str> = all_help()
        .iter()
        .filter(|e| js_len(e.summary.trim()) < 20)
        .map(|e| e.id.as_str())
        .collect();
    assert!(short.is_empty(), "short summaries: {short:?}");
    let blank: Vec<&str> = all_help()
        .iter()
        .filter(|e| {
            e.variants
                .iter()
                .any(|v| v.label.trim().is_empty() || v.command.trim().is_empty())
        })
        .map(|e| e.id.as_str())
        .collect();
    assert!(blank.is_empty(), "blank variants: {blank:?}");
}

#[test]
fn commands_are_one_line_of_plain_ascii_with_no_angle_placeholder() {
    let angle = Regex::new(r"<[A-Za-z][A-Za-z0-9_ -]*>").unwrap();
    let mut bad = Vec::new();
    for e in all_help() {
        for c in commands_of(e) {
            // A shell reads "<NAME>" as redirection from a file called NAME. A lone "<" or ">" is
            // real redirection and is allowed. Printable ASCII only: a curly quote or a
            // non-breaking space looks right on screen and is a different command to a shell.
            let printable = c.chars().all(|ch| (' '..='~').contains(&ch));
            if c.is_empty() || c != c.trim() || !printable || angle.is_match(c) {
                bad.push(format!("{}: {c}", e.id));
            }
        }
    }
    assert!(bad.is_empty(), "{bad:?}");
}

#[test]
fn no_em_dash_in_any_entry_or_in_the_files_that_hold_them() {
    let inside: Vec<&str> = all_help()
        .iter()
        .filter(|e| texts_of(e).iter().any(|t| t.contains(EM_DASH)))
        .map(|e| e.id.as_str())
        .collect();
    assert!(inside.is_empty(), "em dash in entries: {inside:?}");

    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut offending = Vec::new();
    for (dir, ext) in [(root.join("data"), "json"), (root.join("src"), "rs")] {
        for entry in fs::read_dir(&dir).expect("catalogue folder reads") {
            let path = entry.expect("dir entry").path();
            if path.extension().and_then(|x| x.to_str()) != Some(ext) {
                continue;
            }
            let text = fs::read_to_string(&path).expect("source file reads");
            if text.contains(EM_DASH) {
                offending.push(path.display().to_string());
            }
        }
    }
    assert!(offending.is_empty(), "em dash in: {offending:?}");
}

#[test]
fn six_to_sixteen_lower_case_keywords_none_twice() {
    let bad: Vec<String> = all_help()
        .iter()
        .filter(|e| {
            let set: BTreeSet<&str> = e.keywords.iter().map(String::as_str).collect();
            e.keywords.len() < 6
                || e.keywords.len() > 16
                || set.len() != e.keywords.len()
                || e.keywords
                    .iter()
                    .any(|k| k.trim().is_empty() || k != k.trim() || *k != k.to_lowercase())
        })
        .map(|e| format!("{} ({})", e.id, e.keywords.len()))
        .collect();
    assert!(bad.is_empty(), "{bad:?}");
}

#[test]
fn placeholders_are_declared_and_used_both_ways() {
    let upper = Regex::new(r"^[A-Z][A-Z0-9_]*$").unwrap();
    let mut bad = Vec::new();
    for e in all_help() {
        let declared: Vec<&str> = e.placeholders.keys().map(String::as_str).collect();
        let used: BTreeSet<String> = commands_of(e)
            .iter()
            .flat_map(|c| placeholders_in(c))
            .collect();
        for name in &declared {
            if !upper.is_match(name) {
                bad.push(format!("{}: \"{name}\" is not UPPER_SNAKE", e.id));
            } else {
                let name_re = FancyRegex::new(&format!(r"(^|[^A-Za-z0-9_]){name}(?![A-Za-z0-9_])"))
                    .expect("name pattern compiles");
                let used_here = commands_of(e)
                    .iter()
                    .any(|c| name_re.is_match(c).expect("name scan"));
                if !used_here {
                    bad.push(format!("{}: declares {name}, which no command uses", e.id));
                }
            }
            let explained = e
                .placeholders
                .get(*name)
                .is_some_and(|t| !t.trim().is_empty());
            if !explained {
                bad.push(format!("{}: {name} has no explanation", e.id));
            }
        }
        for name in &used {
            if !declared.contains(&name.as_str()) {
                bad.push(format!("{}: uses {name}, undeclared", e.id));
            }
        }
    }
    assert!(bad.is_empty(), "{bad:?}");
}

#[test]
fn destructive_commands_carry_a_danger_line() {
    let silent: Vec<String> = all_help()
        .iter()
        .filter(|e| !has_danger(e) && !destructive_hits(e).is_empty())
        .map(|e| format!("{}: {}", e.id, destructive_hits(e).join(", ")))
        .collect();
    assert!(silent.is_empty(), "{silent:?}");
}

#[test]
fn the_destructive_detector_knows_a_destructive_command_and_an_innocent_one() {
    let probe = |command: &str| -> Vec<&'static str> {
        let e = HelpEntry {
            id: "x-y".to_string(),
            shell: HelpShell::Any,
            category: HelpCategory::Files,
            task: String::new(),
            summary: String::new(),
            command: command.to_string(),
            variants: Vec::new(),
            placeholders: BTreeMap::new(),
            keywords: Vec::new(),
            danger: None,
        };
        destructive_hits(&e)
    };
    let hot = [
        "Remove-Item FILE",
        "rm -rf FOLDER",
        "sudo rm FILE",
        "find . -name \"*.tmp\" | xargs rm",
        "rmdir /s /q FOLDER",
        "del /q *.tmp",
        "format D:",
        "Stop-Process -Id 12",
        "taskkill /pid 12 /f",
        "kill -9 12",
        "git reset --hard HEAD~1",
        "git clean -fd",
        "git push --force-with-lease",
        "git checkout -- FILE",
        "git branch -D NAME",
        "dd if=/dev/zero of=/dev/sda",
        "mkfs.ext4 /dev/sdb1",
        "tree /f /a > tree.txt",
        "echo TEXT> NAME",
        "COMMAND *> FILE",
        "curl -L -o FILE URL",
        "curl -LO URL",
        "Invoke-WebRequest URL -OutFile FILE",
    ];
    for c in hot {
        assert!(!probe(c).is_empty(), "missed: {c}");
    }
    let cold = [
        "Get-ChildItem | Format-Table",
        "git log --format=%H",
        "git clean -nd",
        "Remove-Item FOLDER -Recurse -WhatIf",
        "Get-Help Remove-Item",
        "git branch -d NAME",
        "git restore --staged FILE",
        "sed -i.bak s/A/B/ FILE",
        "COMMAND >> FILE",
        "COMMAND 2>nul",
        "COMMAND *> $null",
        "COMMAND 2>/dev/null",
        "whoami /groups | findstr /c:\"S-1-16-12288\" >nul && echo elevated",
        "COMMAND | awk '{print $2}'",
        "curl -I URL",
        "curl -s https://ifconfig.me",
    ];
    for c in cold {
        assert!(probe(c).is_empty(), "false alarm: {c} -> {:?}", probe(c));
    }
}

#[test]
fn danger_lines_are_sentences() {
    let bad: Vec<&str> = all_help()
        .iter()
        .filter(|e| match e.danger.as_deref() {
            None => false,
            Some(d) => {
                let t = d.trim();
                js_len(t) < 15 || !(t.ends_with('.') || t.ends_with('!'))
            }
        })
        .map(|e| e.id.as_str())
        .collect();
    assert!(bad.is_empty(), "{bad:?}");
}

/// Real questions and the entries that would be a sensible first answer to each.
const POWERSHELL_QUESTIONS: &[(&str, &[&str])] = &[
    ("how do I find big files", &["ps-biggest-files"]),
    ("what is using port 3000", &["ps-port-in-use"]),
    ("undo last commit", &["git-undo-last-commit"]),
    ("where am i", &["ps-where-am-i"]),
    ("delete a folder", &["ps-delete-folder"]),
    ("grep", &["ps-search-in-files"]),
    ("go back a folder", &["ps-go-up-a-folder"]),
    ("kill a process", &["ps-stop-process"]),
    ("unzip a file", &["ps-unzip"]),
    ("see hidden files", &["ps-show-hidden-files"]),
    ("running scripts is disabled", &["ps-scripts-disabled"]),
    ("set an environment variable", &["ps-env-set-session"]),
    ("how much disk space is left", &["ps-disk-space"]),
    ("/compact", &["claude-compact"]),
    ("install a program", &["pkg-winget-install"]),
    ("how do i see whats in this folder", &["ps-list-files"]),
    ("what files are here", &["ps-list-files"]),
    ("ls -la", &["ps-list-files"]),
    ("newest files", &["ps-recent-files", "ps-list-files"]),
    ("search for a word in all files", &["ps-search-in-files"]),
    ("direcotry size", &["ps-folder-size"]),
    ("close a frozen program", &["ps-stop-process"]),
    ("install wsl", &["wsl-install"]),
    ("wsl is stuck", &["wsl-shutdown"]),
    ("install claude code", &["ps-install-claude"]),
];

const CMD_QUESTIONS: &[(&str, &[&str])] = &[
    ("list files", &["cmd-list-files"]),
    ("how do I find big files", &["cmd-biggest-files"]),
    ("unzip a file", &["cmd-unzip"]),
    ("what is using port 3000", &["cmd-port-in-use"]),
    ("delete a folder", &["cmd-delete-folder"]),
    ("where am i", &["cmd-where-am-i"]),
    ("kill a process", &["cmd-kill-process"]),
    ("search for text in files", &["cmd-find-text-in-files"]),
    ("go back a folder", &["cmd-go-up"]),
    ("copy a folder", &["cmd-copy-folder"]),
    ("what is my ip address", &["cmd-ip-address"]),
    ("rename a file", &["cmd-rename"]),
    ("undo last commit", &["git-undo-last-commit"]),
    ("add to path", &["cmd-add-to-path"]),
    ("how do i see whats in this folder", &["cmd-list-files"]),
    ("show me the files here", &["cmd-list-files"]),
    ("newest files", &["cmd-list-files"]),
    ("copy output to clipboard", &["cmd-pipe"]),
    ("install codex", &["cmd-install-codex"]),
    ("delete a linux distro", &["wsl-unregister"]),
];

const BASH_QUESTIONS: &[(&str, &[&str])] = &[
    ("how do I find big files", &["sh-biggest-files"]),
    ("what is using port 3000", &["sh-port-in-use"]),
    ("delete a folder", &["sh-delete-folder"]),
    (
        "permission denied when running a script",
        &["sh-make-executable"],
    ),
    ("where am i", &["sh-where-am-i"]),
    ("grep", &["sh-grep-recursive"]),
    ("kill a process", &["sh-kill-process"]),
    ("extract a tar.gz", &["sh-tar-extract"]),
    ("install a package with apt", &["sh-apt-install"]),
    ("how much disk space is left", &["sh-disk-free"]),
    ("undo last commit", &["git-undo-last-commit"]),
    (
        "resume my last claude conversation",
        &["claude-continue", "claude-resume"],
    ),
    ("how do i see whats in this folder", &["sh-list-files"]),
    ("what files are here", &["sh-list-files"]),
    ("how do i exit vim", &["sh-edit-file"]),
    ("where are my windows files", &["sh-windows-drives"]),
    ("script says bad interpreter ^M", &["sh-line-endings"]),
    ("install claude code", &["sh-install-claude"]),
    ("python virtual environment", &["sh-python-venv"]),
    ("pip install", &["sh-pip-install"]),
];

fn questions_for(shell: HelpShellChoice) -> &'static [(&'static str, &'static [&'static str])] {
    match shell {
        HelpShellChoice::Powershell => POWERSHELL_QUESTIONS,
        HelpShellChoice::Cmd => CMD_QUESTIONS,
        HelpShellChoice::Bash => BASH_QUESTIONS,
    }
}

#[test]
fn names_only_entries_that_exist() {
    let ids: BTreeSet<&str> = all_help().iter().map(|e| e.id.as_str()).collect();
    let mut missing = BTreeSet::new();
    for (shell, _) in HELP_SHELLS {
        for (_, want) in questions_for(shell) {
            for id in *want {
                if !ids.contains(id) {
                    missing.insert(*id);
                }
            }
        }
    }
    assert!(missing.is_empty(), "missing: {missing:?}");
}

#[test]
fn finds_a_sensible_first_answer_in_each_shell() {
    for (shell, name) in HELP_SHELLS {
        let opts = HelpSearchOptions {
            shell: Some(shell_search(shell)),
            category: None,
            limit: Some(3),
        };
        let mut wrong = Vec::new();
        for (question, want) in questions_for(shell) {
            let hits = search_help(all_help(), question, &opts);
            let first = hits.first().map(|h| h.entry.id.as_str());
            if !first.is_some_and(|f| want.contains(&f)) {
                wrong.push(format!(
                    "\"{question}\" -> {} (wanted {})",
                    first.unwrap_or("nothing"),
                    want.join(" or ")
                ));
            }
        }
        assert!(wrong.is_empty(), "{name}: {wrong:#?}");
    }
}

/// Every name the panel can draw: an entry's task, and each variant's label.
fn row_names() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for e in all_help() {
        out.push((e.id.clone(), e.task.clone()));
        for (i, v) in e.variants.iter().enumerate() {
            out.push((format!("{}#{}", e.id, i + 1), v.label.clone()));
        }
    }
    out
}

#[test]
fn a_row_name_is_at_most_eight_words_and_54_chars() {
    let bad: Vec<String> = row_names()
        .iter()
        .filter(|(_, n)| n.split_whitespace().count() > 8 || js_len(n) > 54)
        .map(|(id, n)| format!("{id}: {n}"))
        .collect();
    assert!(bad.is_empty(), "{bad:?}");
}

#[test]
fn never_opens_with_a_word_that_points_at_another_row() {
    // "Copy IT", "ALSO remove...", "ANOTHER folder": each needs the row above to mean anything, and
    // the panel does not promise there is one.
    let leans = Regex::new(
        r"(?i)^(it|its|this|that|these|those|the same|same|also|another|other|a second|and |or |but |plus |then |or else)\b",
    )
    .unwrap();
    let bad: Vec<String> = row_names()
        .iter()
        .filter(|(_, n)| leans.is_match(n.trim()))
        .map(|(id, n)| format!("{id}: {n}"))
        .collect();
    assert!(bad.is_empty(), "{bad:?}");
}

#[test]
fn never_says_it_or_the_same_before_naming_the_thing() {
    // Only where the pronoun comes FIRST, with nothing in the name for it to refer to.
    let early = Regex::new(r"(?i)^(?:\S+\s+){0,2}(?:it|them|the same)\b").unwrap();
    let bad: Vec<String> = row_names()
        .iter()
        .filter(|(_, n)| early.is_match(n.trim()))
        .map(|(id, n)| format!("{id}: {n}"))
        .collect();
    assert!(bad.is_empty(), "{bad:?}");
}

#[test]
fn is_a_name_not_a_sentence() {
    let bad: Vec<String> = row_names()
        .iter()
        .filter(|(_, n)| {
            let t = n.trim();
            t.ends_with(['.', ',', ';', ':']) || n.as_str() != t
        })
        .map(|(id, n)| format!("{id}: {n}"))
        .collect();
    assert!(bad.is_empty(), "{bad:?}");
}

#[test]
fn is_not_the_same_as_another_row_of_the_entry() {
    let mut bad = Vec::new();
    for e in all_help() {
        let mut names: Vec<String> = vec![e.task.to_lowercase()];
        names.extend(e.variants.iter().map(|v| v.label.to_lowercase()));
        let unique: BTreeSet<&String> = names.iter().collect();
        if unique.len() != names.len() {
            bad.push(e.id.clone());
        }
    }
    assert!(bad.is_empty(), "{bad:?}");
}

#[test]
fn every_variant_has_three_to_twelve_own_keywords() {
    let mut bad = Vec::new();
    for e in all_help() {
        for (i, v) in e.variants.iter().enumerate() {
            let set: BTreeSet<&str> = v.keywords.iter().map(String::as_str).collect();
            let padded = v
                .keywords
                .iter()
                .any(|w| w.trim().is_empty() || w != w.trim() || *w != w.to_lowercase());
            if v.keywords.len() < 3
                || v.keywords.len() > 12
                || set.len() != v.keywords.len()
                || padded
            {
                bad.push(format!("{}#{} ({})", e.id, i + 1, v.keywords.len()));
            }
        }
    }
    assert!(bad.is_empty(), "{bad:?}");
}

#[test]
fn a_hidden_word_adds_something() {
    // A keyword whose words are all already in the row's name is spent: the name is indexed too.
    let mut bad = Vec::new();
    for e in all_help() {
        for (i, v) in e.variants.iter().enumerate() {
            let in_name: BTreeSet<String> = v
                .label
                .to_lowercase()
                .split(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit()))
                .filter(|w| !w.is_empty())
                .map(str::to_string)
                .collect();
            let fresh = v
                .keywords
                .iter()
                .filter(|k| {
                    let every = !k.is_empty() && k.split_whitespace().all(|w| in_name.contains(w));
                    !every
                })
                .count();
            if fresh < 2 {
                bad.push(format!("{}#{}", e.id, i + 1));
            }
        }
    }
    assert!(bad.is_empty(), "{bad:?}");
}

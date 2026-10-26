//! The process tree rules for finding an agent under a shell (Task 8; Prism Terminal's agentDetect.ts).
//!
//! Detection walks the process tree under each shell and matches command lines against the agents'
//! PACKAGE signatures and program names, not bare words: this machine's dev folder is literally named
//! "Claude", and a tool touching files there must not light a tab. A bare name counts only as the
//! program itself, the FIRST token (a native `claude.exe` started as `claude --resume`).

use crate::kinds::DetectedAgent;
use regex::Regex;
use std::collections::{HashSet, VecDeque};
use std::sync::LazyLock;

/// One process: its id, its parent's id and its command line (empty when it could not be read).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcRow {
    pub pid: u32,
    pub ppid: u32,
    pub cmd: String,
}

/// The package signatures: where the agents' own code lives, wherever the command line shows it
/// (`node ...\node_modules\@anthropic-ai\claude-code\cli.js`). A folder that merely has such a word
/// in its name never matches: the word must be a package scope, or a folder followed by the
/// agent's entry file.
const PACKAGE_PATTERN: &str = r#"(?i)@anthropic-ai[\\/]claude-code|@openai[\\/]codex|@google[\\/]gemini-cli|(?:^|[\\/"\s])(claude-code|gemini-cli)[\\/](?:cli\.[cm]?js|bin[\\/]|dist[\\/])"#;
const PREFILTER_PATTERN: &str = r"(?i)claude|codex|aider|gemini|agy";

static PACKAGE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(PACKAGE_PATTERN).expect("PACKAGE_RE"));
static PREFILTER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(PREFILTER_PATTERN).expect("PREFILTER_RE"));

/// The agent a command line starts or runs, by its program (the FIRST token, bare name or path) or
/// by a package signature. Never by a word somewhere in the arguments: a tool run in a folder named
/// `claude` or `gemini`, or `git -C C:\src\claude status`, is not an agent.
pub fn classify(cmd: &str) -> Option<DetectedAgent> {
    let argv = split_command_line(cmd);
    let first = argv.first().map(|a| program_name(a));
    let by_program = first.and_then(agent_named);
    if by_program.is_some() {
        return by_program;
    }
    // A shim or exe in a later argument (`cmd /c C:\npm\claude.cmd`) counts only when it IS a
    // program file of that name; a folder or a plain word never does.
    if let Some(kind) = argv.iter().skip(1).find_map(|a| program_file(a)) {
        return Some(kind);
    }
    let m = PACKAGE_RE.captures(cmd)?;
    let text = m.get(0)?.as_str().to_ascii_lowercase();
    Some(if text.contains("claude-code") {
        DetectedAgent::Claude
    } else if text.contains("codex") {
        DetectedAgent::Codex
    } else {
        DetectedAgent::Other
    })
}

/// Whether a command line is an AI agent (Claude Code, codex and kin).
pub fn looks_like_agent(cmd: &str) -> bool {
    classify(cmd).is_some()
}

/// The arguments of a Windows command line, quotes removed (a program path may hold spaces).
fn split_command_line(cmd: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut any = false;
    for c in cmd.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => {
                cur.push(c);
                any = true;
            }
        }
    }
    if any {
        out.push(cur);
    }
    out
}

/// The file name of a path, lower case, without the executable suffix a shell may add.
fn program_name(arg: &str) -> String {
    let base = arg
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(arg)
        .to_ascii_lowercase();
    for ext in [".exe", ".cmd", ".ps1", ".bat"] {
        if let Some(stem) = base.strip_suffix(ext) {
            return stem.to_string();
        }
    }
    base
}

fn agent_named(name: impl AsRef<str>) -> Option<DetectedAgent> {
    match name.as_ref() {
        "claude" => Some(DetectedAgent::Claude),
        "codex" => Some(DetectedAgent::Codex),
        "agy" => Some(DetectedAgent::Agy),
        "aider" | "gemini" => Some(DetectedAgent::Other),
        _ => None,
    }
}

/// An argument that is an agent's program file: a path ending `.exe`, `.cmd` or `.ps1`.
fn program_file(arg: &str) -> Option<DetectedAgent> {
    let lower = arg.to_ascii_lowercase();
    if ![".exe", ".cmd", ".ps1"].iter().any(|e| lower.ends_with(e)) {
        return None;
    }
    agent_named(program_name(arg))
}

/// The broad word test the process query runs before the strict match above.
pub fn prefilter(cmd: &str) -> bool {
    PREFILTER_RE.is_match(cmd)
}

/// Which agent runs under `root_pid`, if any: Claude wins anywhere in the tree and returns at once;
/// else Codex; else agy; else Other (an agent with nothing to resume). A seen-set survives pid cycles.
pub fn tree_agent_kind(rows: &[ProcRow], root_pid: u32) -> Option<DetectedAgent> {
    let mut kind: Option<DetectedAgent> = None;
    for_each_descendant(rows, root_pid, |child| match classify(&child.cmd) {
        Some(DetectedAgent::Claude) => {
            kind = Some(DetectedAgent::Claude);
            true
        }
        Some(DetectedAgent::Codex) => {
            kind = Some(DetectedAgent::Codex);
            false
        }
        Some(DetectedAgent::Agy) => {
            if kind != Some(DetectedAgent::Codex) {
                kind = Some(DetectedAgent::Agy);
            }
            false
        }
        Some(other) => {
            if kind.is_none() {
                kind = Some(other);
            }
            false
        }
        None => false,
    });
    kind
}

/// Whether any process under `root_pid` looks like an agent.
pub fn tree_has_agent(rows: &[ProcRow], root_pid: u32) -> bool {
    let mut found = false;
    for_each_descendant(rows, root_pid, |child| {
        found = looks_like_agent(&child.cmd);
        found
    });
    found
}

/// Breadth-first walk of the tree under `root`; `visit` returns true to stop the walk early.
fn for_each_descendant(rows: &[ProcRow], root: u32, mut visit: impl FnMut(&ProcRow) -> bool) {
    let mut queue = VecDeque::from([root]);
    let mut seen = HashSet::new();
    while let Some(pid) = queue.pop_front() {
        if !seen.insert(pid) {
            continue; // a reused pid can fake a cycle
        }
        for child in rows.iter().filter(|r| r.ppid == pid) {
            if visit(child) {
                return;
            }
            queue.push_back(child.pid);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kinds::{AgentKind, RESUMABLE_AGENTS};

    const CLAUDE: &str = r#""C:\Program Files\nodejs\node.exe" "C:\Users\Admin\AppData\Roaming\npm\node_modules\@anthropic-ai\claude-code\cli.js""#;
    const CODEX: &str =
        r#"node "C:\Users\Admin\AppData\Roaming\npm\node_modules\@openai\codex\bin\codex.js""#;

    fn row(pid: u32, ppid: u32, cmd: &str) -> ProcRow {
        ProcRow {
            pid,
            ppid,
            cmd: cmd.to_string(),
        }
    }

    #[test]
    fn matches_claude_code_and_codex_by_their_package_paths() {
        assert!(looks_like_agent(CLAUDE));
        assert!(looks_like_agent(CODEX));
        assert!(looks_like_agent(r#""C:\.local\bin\claude.exe" --resume"#));
        assert!(looks_like_agent(r"C:\npm\claude.cmd"));
    }

    #[test]
    fn does_not_match_a_dev_folder_named_claude() {
        assert!(!looks_like_agent(
            r"node C:\Users\Admin\Documents\Claude\Github\Other\out\main\index.js"
        ));
        assert!(!looks_like_agent(
            r"vitest run C:\Users\Admin\Documents\Claude\Github\Other"
        ));
    }

    #[test]
    fn matches_an_agent_started_by_bare_name_as_the_first_token_only() {
        assert!(looks_like_agent("claude"));
        assert!(looks_like_agent("claude  --resume"));
        assert!(looks_like_agent(r#""claude.exe" -c"#));
        assert!(looks_like_agent("codex resume --last"));
        assert!(!looks_like_agent("git commit -m claude"));
        assert!(!looks_like_agent("notepad claude.txt"));
        assert!(!looks_like_agent("claudette.exe"));
    }

    #[test]
    fn a_path_or_word_in_the_arguments_is_not_an_agent() {
        for cmd in [
            r"node C:\work\claude-code-ui\node_modules\.bin\vite.js dev",
            r#"node "C:\Users\me\Claude Projects\site\node_modules\vite\bin\vite.js""#,
            r"git -C C:\src\claude status",
            r"git -C C:\src\gemini log",
            r"node C:\src\codex\build.js",
            r"python C:\tools\aider\notes.py",
            r"npm run dev --prefix C:\work\claude-code-ui",
        ] {
            assert!(!looks_like_agent(cmd), "{cmd}");
            assert_eq!(classify(cmd), None, "{cmd}");
        }
    }

    #[test]
    fn a_shim_or_exe_argument_is_still_an_agent() {
        assert_eq!(
            classify(r#"cmd.exe /c "C:\npm\claude.cmd" --resume"#),
            Some(DetectedAgent::Claude)
        );
        assert_eq!(
            classify(r"pwsh -File C:\npm\codex.ps1"),
            Some(DetectedAgent::Codex)
        );
    }

    #[test]
    fn agy_is_found_by_its_program_and_never_by_a_word() {
        const AGY: &str = r#""C:\Users\Admin\AppData\Local\agy\bin\agy.exe""#;
        for cmd in [
            "agy",
            "agy.exe",
            "agy -c",
            r#""agy.exe" --conversation 1b2c"#,
            AGY,
            r"C:/tools/agy.exe --model x",
            r#"cmd.exe /c "C:\bin\agy.exe""#,
        ] {
            assert_eq!(classify(cmd), Some(DetectedAgent::Agy), "{cmd}");
        }
        for cmd in [
            "legacy.exe",
            "strategy",
            r"node C:\src\legacy\agy\main.js",
            r"git -C C:\src\agy status",
            "notepad agy.txt",
            "agyle.exe",
        ] {
            assert_eq!(classify(cmd), None, "{cmd}");
        }
        assert!(prefilter(AGY));
        assert_eq!(
            tree_agent_kind(&[row(70, 1, "pwsh.exe"), row(71, 70, AGY)], 70),
            Some(DetectedAgent::Agy)
        );
    }

    #[test]
    fn claude_then_codex_win_over_agy_under_one_shell() {
        let codex_first = [
            row(80, 1, "pwsh.exe"),
            row(81, 80, CODEX),
            row(82, 80, "agy.exe"),
        ];
        assert_eq!(
            tree_agent_kind(&codex_first, 80),
            Some(DetectedAgent::Codex)
        );
        let agy_first = [
            row(80, 1, "pwsh.exe"),
            row(81, 80, "agy.exe"),
            row(82, 80, CODEX),
        ];
        assert_eq!(tree_agent_kind(&agy_first, 80), Some(DetectedAgent::Codex));
        let under_agy = [
            row(80, 1, "pwsh.exe"),
            row(81, 80, "agy.exe"),
            row(82, 81, "claude"),
        ];
        assert_eq!(tree_agent_kind(&under_agy, 80), Some(DetectedAgent::Claude));
        let beside_aider = [
            row(80, 1, "pwsh.exe"),
            row(81, 80, "aider"),
            row(82, 80, "agy.exe"),
        ];
        assert_eq!(tree_agent_kind(&beside_aider, 80), Some(DetectedAgent::Agy));
    }

    #[test]
    fn does_not_match_plain_shells_and_tools() {
        assert!(!looks_like_agent("pwsh.exe -NoLogo"));
        assert!(!looks_like_agent(
            r"C:\WINDOWS\system32\ping.exe -n 3 127.0.0.1"
        ));
    }

    #[test]
    fn finds_an_agent_anywhere_under_the_shell() {
        let rows = [
            row(10, 1, "pwsh.exe"),
            row(20, 10, "claude"),
            row(30, 20, "some-helper.exe"),
            row(40, 1, "pwsh.exe"),
        ];
        assert!(tree_has_agent(&rows, 10));
        assert!(!tree_has_agent(&rows, 40));
    }

    #[test]
    fn a_sibling_shell_stays_clean() {
        let rows = [
            row(10, 1, "pwsh.exe"),
            row(11, 1, "pwsh.exe"),
            row(12, 11, "claude"),
        ];
        assert!(!tree_has_agent(&rows, 10));
        assert!(tree_has_agent(&rows, 11));
    }

    #[test]
    fn survives_a_pid_cycle() {
        let rows = [row(5, 6, "pwsh.exe"), row(6, 5, "pwsh.exe")];
        assert!(!tree_has_agent(&rows, 5));
        assert_eq!(tree_agent_kind(&rows, 5), None);
    }

    #[test]
    fn names_the_agent_it_found() {
        assert_eq!(
            tree_agent_kind(&[row(10, 1, "pwsh.exe"), row(20, 10, CLAUDE)], 10),
            Some(DetectedAgent::Claude)
        );
        assert_eq!(
            tree_agent_kind(&[row(20, 1, "pwsh.exe"), row(21, 20, CODEX)], 20),
            Some(DetectedAgent::Codex)
        );
        assert_eq!(
            tree_agent_kind(
                &[row(30, 1, "pwsh.exe"), row(31, 30, r"C:\\bin\\codex.exe")],
                30
            ),
            Some(DetectedAgent::Codex)
        );
    }

    #[test]
    fn an_agent_with_nothing_to_resume_is_just_an_agent() {
        assert_eq!(
            tree_agent_kind(&[row(30, 1, "pwsh.exe"), row(31, 30, "C:/bin/aider")], 30),
            Some(DetectedAgent::Other)
        );
    }

    #[test]
    fn claude_wins_when_both_are_under_one_shell() {
        let rows = [
            row(50, 1, "pwsh.exe"),
            row(51, 50, CODEX),
            row(52, 50, CLAUDE),
        ];
        assert_eq!(tree_agent_kind(&rows, 50), Some(DetectedAgent::Claude));
        let rows = [
            row(50, 1, "pwsh.exe"),
            row(51, 50, CODEX),
            row(52, 51, CLAUDE),
        ];
        assert_eq!(tree_agent_kind(&rows, 50), Some(DetectedAgent::Claude));
    }

    #[test]
    fn a_plain_shell_hosts_nothing() {
        assert_eq!(tree_agent_kind(&[row(60, 1, "pwsh.exe")], 60), None);
    }

    #[test]
    fn a_four_deep_agent_is_found_and_a_foreign_root_is_none() {
        let rows = [
            row(100, 1, "pwsh.exe"),
            row(200, 100, "pwsh.exe"),
            row(300, 200, "pwsh.exe"),
            row(400, 300, "node claude-code/cli.js"),
        ];
        assert_eq!(tree_agent_kind(&rows, 100), Some(DetectedAgent::Claude));
        assert_eq!(tree_agent_kind(&rows, 300), Some(DetectedAgent::Claude));
        assert_eq!(tree_agent_kind(&rows, 999), None);
    }

    #[test]
    fn the_resumable_agents_are_each_detected_by_name() {
        for kind in RESUMABLE_AGENTS {
            let name = kind.as_str();
            assert!(looks_like_agent(name), "{name}");
            assert_eq!(
                tree_agent_kind(&[row(2, 1, &format!("{name} --x"))], 1),
                Some(DetectedAgent::from(kind)),
                "{name}"
            );
        }
    }

    #[test]
    fn the_resumable_agents_pass_the_prefilter() {
        for kind in RESUMABLE_AGENTS {
            assert!(prefilter(kind.as_str()), "{}", kind.as_str());
        }
        assert!(prefilter("aider"));
        assert!(prefilter("gemini"));
        assert!(!prefilter("pwsh.exe -NoLogo"));
        assert_eq!(AgentKind::parse("claude"), Some(AgentKind::Claude));
    }

    #[test]
    fn no_source_file_lists_the_resumable_agents_by_hand() {
        // Walk crates/*/src and look for a second copy of the agent list (the one list is RESUMABLE_AGENTS).
        let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut offenders = Vec::new();
        let mut stack = Vec::new();
        for entry in std::fs::read_dir(&crates).expect("crates dir") {
            // sync-ok: test-only scan of the source tree
            stack.push(entry.expect("entry").path().join("src"));
        }
        while let Some(dir) = stack.pop() {
            let Ok(read) = std::fs::read_dir(&dir) else {
                continue; // sync-ok: test-only scan of the source tree
            };
            for entry in read.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if !name.ends_with(".rs") || name == "kinds.rs" || name == "detect.rs" {
                    continue;
                }
                let text = std::fs::read_to_string(&path).unwrap_or_default(); // sync-ok: test-only
                // A comment that names the agents is prose, not a second list.
                let hand = text
                    .lines()
                    .filter(|l| !l.trim_start().starts_with("//"))
                    .any(|line| {
                        let kinds = ["AgentKind::Claude", "AgentKind::Codex", "AgentKind::Agy"];
                        let words = ["\"claude\"", "\"codex\"", "\"agy\""];
                        kinds.iter().filter(|k| line.contains(*k)).count() >= 2
                            || words.iter().filter(|w| line.contains(*w)).count() >= 2
                    });
                if hand {
                    offenders.push(path.display().to_string());
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "agent list written by hand in: {offenders:?}"
        );
    }
}

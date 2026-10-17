//! The process tree rules for finding an agent under a shell (Task 8; Prism Terminal's agentDetect.ts).
//!
//! Detection walks the process tree under each shell and matches command lines against the agents'
//! PACKAGE signatures, not bare words: this machine's dev folder is literally named "Claude", and a
//! tool touching files there must not light a tab. The only bare-name case is the program itself as
//! the FIRST token (a native `claude.exe` started as `claude --resume`).

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

const AGENT_PATTERN: &str = r#"(?i)claude-code|(?:^"?|[\\/])claude(\.exe|\.cmd|\.ps1)?(["\s]|$)|(?:^"?|[\\/])(codex|aider|gemini)([\\/.]|["\s]|$)|@openai[\\/]codex|gemini-cli"#;
const CLAUDE_PATTERN: &str = r#"(?i)claude-code|(?:^"?|[\\/])claude(\.exe|\.cmd|\.ps1)?(["\s]|$)"#;
const CODEX_PATTERN: &str =
    r#"(?i)(?:^"?|[\\/])codex(\.exe|\.cmd|\.ps1)?(["\s]|$)|@openai[\\/]codex"#;
const PREFILTER_PATTERN: &str = r"(?i)claude|codex|aider|gemini";

static AGENT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(AGENT_PATTERN).expect("AGENT_RE"));
static CLAUDE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(CLAUDE_PATTERN).expect("CLAUDE_RE"));
static CODEX_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(CODEX_PATTERN).expect("CODEX_RE"));
static PREFILTER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(PREFILTER_PATTERN).expect("PREFILTER_RE"));

/// Whether a command line is an AI agent (Claude Code, codex and kin), by its package signature.
pub fn looks_like_agent(cmd: &str) -> bool {
    AGENT_RE.is_match(cmd)
}

/// The broad word test the process query runs before the strict match above.
pub fn prefilter(cmd: &str) -> bool {
    PREFILTER_RE.is_match(cmd)
}

/// Which agent runs under `root_pid`, if any: Claude wins anywhere in the tree and returns at once;
/// else Codex; else Other (an agent with nothing to resume). A seen-set survives pid cycles.
pub fn tree_agent_kind(rows: &[ProcRow], root_pid: u32) -> Option<DetectedAgent> {
    let mut kind: Option<DetectedAgent> = None;
    for_each_descendant(rows, root_pid, |child| {
        if !looks_like_agent(&child.cmd) {
            return false;
        }
        if CLAUDE_RE.is_match(&child.cmd) {
            kind = Some(DetectedAgent::Claude);
            return true;
        }
        if CODEX_RE.is_match(&child.cmd) {
            kind = Some(DetectedAgent::Codex);
        } else if kind.is_none() {
            kind = Some(DetectedAgent::Other);
        }
        false
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
            r"node C:\Users\Admin\Documents\Claude\Github\Prism\out\main\index.js"
        ));
        assert!(!looks_like_agent(
            r"vitest run C:\Users\Admin\Documents\Claude\Github\Prism"
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
                let hand = text.lines().any(|line| {
                    (line.contains("AgentKind::Claude") && line.contains("AgentKind::Codex"))
                        || (line.contains("\"claude\"") && line.contains("\"codex\""))
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

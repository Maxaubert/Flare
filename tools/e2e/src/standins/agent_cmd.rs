//! FakeClaude: a claude.cmd stand-in with transcripts in a temp HOME (Task 15).
//!
//! `claude.cmd` runs a PowerShell script that writes the steps of a scripted agent (an OSC 0 title,
//! prompt text, the OSC 777 hook lines of `hooklines`) and then stays open, as an agent does. Its
//! HOME is a folder of its own, so a scenario never reads the owner's `~/.claude`. Transcripts are
//! written where Claude keeps them (`<home>\.claude\projects\<encoded cwd>\<id>.jsonl`) so the real
//! resume reader in `flare-agent` can find them.

use std::path::PathBuf;

use flare_agent::resume::encode_cwd;
use serde_json::Value;

use crate::standins::hooklines::hook_line;
use crate::world::World;

/// One step of a scripted agent.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    /// The console title (OSC 0), as Claude sets it: `claude` at start, `✳ Claude Code` when ready.
    Title(String),
    /// Text on its own line.
    Print(String),
    /// A hook line for `state` with the string fields of `payload` (`kind` for a failure).
    Hook { state: String, payload: Value },
    /// Wait this many milliseconds.
    Sleep(u64),
}

/// A `claude` stand-in in a folder of its own: `bin\claude.cmd` on PATH, `home\` as HOME.
#[derive(Debug)]
pub struct FakeClaude {
    pub dir: PathBuf,
}

impl FakeClaude {
    /// A fresh stand-in in a `flare-e2e-claude-<id>` folder under TEMP, removed on drop. The
    /// world's own folder is not used yet: Task 14 owns `World`'s fields.
    pub fn new(_world: &World) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "flare-e2e-claude-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(dir.join("bin")).expect("the claude folder");
        std::fs::create_dir_all(dir.join("home").join(".claude").join("projects"))
            .expect("the claude home");
        Self { dir }
    }

    /// The HOME the stand-in runs under.
    pub fn home(&self) -> PathBuf {
        self.dir.join("home")
    }

    /// The folder holding `claude.cmd`.
    pub fn bin(&self) -> PathBuf {
        self.dir.join("bin")
    }

    /// Writes a transcript for `cwd` with session `id` whose first line names `entrypoint` (`cli` is
    /// an interactive session, `sdk-cli` one a tool started). Returns the file.
    pub fn transcript(&self, cwd: &str, id: &str, entrypoint: &str) -> PathBuf {
        let dir = self
            .home()
            .join(".claude")
            .join("projects")
            .join(encode_cwd(cwd));
        std::fs::create_dir_all(&dir).expect("the project folder");
        let path = dir.join(format!("{id}.jsonl"));
        std::fs::write(
            &path,
            format!("{{\"type\":\"user\",\"entrypoint\":\"{entrypoint}\"}}\n"),
        )
        .expect("write the transcript");
        path
    }

    /// Writes the steps into the stand-in: `claude-steps.ps1` and the `claude.cmd` that runs it.
    /// The script ends by staying open for an hour, as a running agent does.
    pub fn script(&mut self, steps: &[Step]) {
        let mut body = String::from(
            "$ErrorActionPreference = 'Stop'\r\n$e = [string][char]27\r\n$b = [string][char]7\r\n",
        );
        for step in steps {
            match step {
                Step::Title(title) => body.push_str(&format!(
                    "[Console]::Write({})\r\n",
                    ps_text(&format!("\u{1b}]0;{title}\u{7}"))
                )),
                Step::Print(text) => body.push_str(&format!(
                    "[Console]::Write({})\r\n",
                    ps_text(&format!("{text}\r\n"))
                )),
                Step::Hook { state, payload } => body.push_str(&format!(
                    "[Console]::Write({})\r\n",
                    ps_text(&hook_line(state, payload))
                )),
                Step::Sleep(ms) => body.push_str(&format!("Start-Sleep -Milliseconds {ms}\r\n")),
            }
        }
        body.push_str("Start-Sleep -Seconds 3600\r\n");
        let bin = self.bin();
        // A UTF-8 byte-order mark, so Windows PowerShell reads the title's non-ASCII characters right.
        let bytes = [b"\xEF\xBB\xBF".as_slice(), body.as_bytes()].concat();
        std::fs::write(bin.join("claude-steps.ps1"), bytes).expect("write the steps");
        std::fs::write(
            bin.join("claude.cmd"),
            "@echo off\r\npowershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File \"%~dp0claude-steps.ps1\"\r\n",
        )
        .expect("write claude.cmd");
    }

    /// The variables a scenario sets so that `claude` resolves to this stand-in and its HOME is ours.
    pub fn env(&self) -> Vec<(String, String)> {
        let path = std::env::var("PATH").unwrap_or_default();
        let home = self.home();
        vec![
            ("PATH".into(), format!("{};{path}", self.bin().display())),
            ("HOME".into(), home.display().to_string()),
            ("USERPROFILE".into(), home.display().to_string()),
            (
                "CLAUDE_CONFIG_DIR".into(),
                home.join(".claude").display().to_string(),
            ),
        ]
    }
}

impl Drop for FakeClaude {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A PowerShell expression that writes `text`: quoted runs joined by `+`, with ESC and BEL as the
/// `$e` and `$b` variables the script defines (strings, so `+` concatenates).
fn ps_text(text: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut run = String::new();
    for c in text.chars() {
        if c == '\u{1b}' || c == '\u{7}' {
            if !run.is_empty() {
                parts.push(format!("'{}'", run.replace('\'', "''")));
                run.clear();
            }
            parts.push(if c == '\u{1b}' {
                "$e".into()
            } else {
                "$b".into()
            });
        } else {
            run.push(c);
        }
    }
    if !run.is_empty() || parts.is_empty() {
        parts.push(format!("'{}'", run.replace('\'', "''")));
    }
    parts.join(" + ")
}

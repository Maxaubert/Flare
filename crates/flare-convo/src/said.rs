//! What is said in a Claude session, as plugins hear it (plugins phase 5, design 6 and 7): each transcript line read
//! as the entries it holds, and the last entries of a transcript read from its end.
//!
//! An entry is a person's prompt (`user`), the assistant's words (`assistant`) or a tool call (`tool`, with the
//! tool's name and the one argument that says what it does). Left out: tool results (they are the tools' output, not
//! what anyone said, and can be huge), the assistant's thinking, Claude Code's own notes (`isMeta`) and the lines of
//! subagents (`isSidechain`): what a plugin with `sessions` reads is the conversation of the session itself.
//!
//! Text is cleaned here (control characters out, line breaks kept as `\n`) and cut to [`TEXT_MAX`] characters.
//! Nothing here reads the disk except [`read_last`], which a worker calls.

use std::collections::VecDeque;
use std::fs::File; // sync-ok: the imports of the transcript reader (no call here)
use std::io::{ErrorKind, Read, Seek, SeekFrom};
use std::path::Path;

use serde_json::Value;

/// The most characters of an entry's text (a cut ends in `...`).
pub const TEXT_MAX: usize = 2000;
/// The most characters of a tool's name.
pub const TOOL_MAX: usize = 64;
/// The most entries `session.read` gives (the last ones).
pub const READ_LAST: usize = 200;
/// The most bytes [`read_last`] reads from the end of a transcript.
pub const READ_BYTES: u64 = 4 * 1024 * 1024;
/// The longest timestamp kept.
const AT_MAX: usize = 40;

/// Who said it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    Tool,
}

impl Role {
    pub fn name(self) -> &'static str {
        match self {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        }
    }
}

/// One thing said in the session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub role: Role,
    /// Clean text, at most [`TEXT_MAX`] characters. For a tool call: what it does (its command, its file, ...).
    pub text: String,
    /// The tool's name, for a tool call.
    pub tool: Option<String>,
    /// When the transcript says it was written (its own ISO time), if it says.
    pub at: Option<String>,
}

/// `text` with every control character out but the line break and the tab (`\r\n` and a lone `\r` are a `\n`),
/// trimmed, and at most `max` characters (a cut ends in `...`).
pub fn clean(text: &str, max: usize) -> String {
    let mut out = String::new();
    let mut count = 0usize;
    let mut chars = text.trim().chars().peekable();
    while let Some(c) = chars.next() {
        let c = match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    continue;
                }
                '\n'
            }
            '\n' | '\t' => c,
            c if c.is_control() => continue,
            c => c,
        };
        if count == max {
            // One more character than fits: cut, leaving room for the dots.
            let keep: String = out.chars().take(max.saturating_sub(3)).collect();
            return keep + "...";
        }
        out.push(c);
        count += 1;
    }
    out
}

/// The one argument that says what a tool call does, else its input as compact JSON.
fn tool_text(input: Option<&Value>) -> String {
    const SAYS: [&str; 10] = [
        "command",
        "file_path",
        "notebook_path",
        "path",
        "pattern",
        "url",
        "query",
        "skill",
        "description",
        "prompt",
    ];
    let Some(input) = input else {
        return String::new();
    };
    for key in SAYS {
        if let Some(s) = input.get(key).and_then(Value::as_str) {
            return clean(s, TEXT_MAX);
        }
    }
    match input {
        Value::Object(o) if o.is_empty() => String::new(),
        v => clean(&v.to_string(), TEXT_MAX),
    }
}

/// The entries one transcript line holds, in order (none for a line that is not a message of the session itself).
pub fn entries_of(line: &str) -> Vec<Entry> {
    let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(line) else {
        return Vec::new();
    };
    let flag = |k: &str| obj.get(k).and_then(Value::as_bool).unwrap_or(false);
    if flag("isSidechain") || flag("isMeta") {
        return Vec::new();
    }
    let at = obj
        .get("timestamp")
        .and_then(Value::as_str)
        .map(|t| clean(t, AT_MAX))
        .filter(|t| !t.is_empty());
    let Some(content) = obj.get("message").and_then(|m| m.get("content")) else {
        return Vec::new();
    };
    let said = |role: Role, text: &str| Entry {
        role,
        text: clean(text, TEXT_MAX),
        tool: None,
        at: at.clone(),
    };
    let mut out = Vec::new();
    match obj.get("type").and_then(Value::as_str) {
        Some("user") => match content {
            Value::String(s) => out.push(said(Role::User, s)),
            Value::Array(blocks) => {
                let words: Vec<&str> = blocks
                    .iter()
                    .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                    .filter_map(|b| b.get("text").and_then(Value::as_str))
                    .collect();
                if !words.is_empty() {
                    out.push(said(Role::User, &words.join("\n")));
                }
            }
            _ => {}
        },
        Some("assistant") => match content {
            Value::String(s) => out.push(said(Role::Assistant, s)),
            Value::Array(blocks) => {
                // Text blocks in a row are one reply; a tool call between them ends it.
                let mut words: Vec<&str> = Vec::new();
                for b in blocks {
                    match b.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            if let Some(t) = b.get("text").and_then(Value::as_str) {
                                words.push(t);
                            }
                        }
                        Some("tool_use") => {
                            if !words.is_empty() {
                                out.push(said(Role::Assistant, &words.join("\n")));
                                words.clear();
                            }
                            let name = b
                                .get("name")
                                .and_then(Value::as_str)
                                .map(|n| clean(n, TOOL_MAX))
                                .filter(|n| !n.is_empty());
                            out.push(Entry {
                                role: Role::Tool,
                                text: tool_text(b.get("input")),
                                tool: name,
                                at: at.clone(),
                            });
                        }
                        _ => {}
                    }
                }
                if !words.is_empty() {
                    out.push(said(Role::Assistant, &words.join("\n")));
                }
            }
            _ => {}
        },
        _ => {}
    }
    out.retain(|e| e.role == Role::Tool || !e.text.is_empty());
    out
}

/// The last `max` entries of the transcript at `path`, oldest first, read from at most its last [`READ_BYTES`] (a line
/// cut by that start is dropped). A transcript that is not there has none. Runs on a worker.
pub fn read_last(path: &Path, max: usize) -> std::io::Result<Vec<Entry>> {
    let opened = File::open(path); // sync-ok: runs on the control channel's worker
    let mut file = match opened {
        Ok(f) => f,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let len = file.metadata()?.len(); // sync-ok: runs on the control channel's worker
    let start = len.saturating_sub(READ_BYTES);
    file.seek(SeekFrom::Start(start))?; // sync-ok: runs on the control channel's worker
    let mut bytes = Vec::new();
    (&mut file).take(len - start).read_to_end(&mut bytes)?; // sync-ok: runs on the control channel's worker
    let mut text = String::from_utf8_lossy(&bytes);
    if start > 0 {
        // The read began inside a line: it is not a whole one.
        let cut = text.find('\n').map_or(text.len(), |i| i + 1);
        text = std::borrow::Cow::Owned(text[cut..].to_string());
    }
    let mut last: VecDeque<Entry> = VecDeque::with_capacity(max.min(READ_LAST));
    for line in text.lines() {
        for e in entries_of(line) {
            if last.len() == max {
                last.pop_front();
            }
            if max > 0 {
                last.push_back(e);
            }
        }
    }
    Ok(last.into())
}

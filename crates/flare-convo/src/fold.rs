//! Folds transcript lines into the model Flare shows: the edits and the files the agent named.
//!
//! Parsing is lenient. A line that is not a JSON object is counted in `bad_lines` and skipped. An object of an unknown
//! `type`, or a tool the model does not know, is ignored. Nothing here panics on a field of the wrong shape: a missing
//! or wrongly typed field simply makes that one item absent. The shapes this relies on are in `docs/surfaces/convo.md`.

use std::collections::HashMap;
use std::sync::OnceLock;

use regex::Regex;
use serde_json::Value;

use crate::diff::{self, Diff};

/// The most edit calls kept (the oldest go first). A long session keeps its recent edits, not all of them.
pub const MAX_EDITS: usize = 2000;
/// The most text kept for all edits together; the oldest edits go first once it is passed.
pub const MAX_EDIT_BYTES: usize = 32 * 1024 * 1024;
/// The most bytes kept of one old or new text (or a whole file). A longer text is cut and the edit says so.
pub const MAX_TEXT: usize = 64 * 1024;

/// The file tools whose calls the Changes panel lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Edit,
    MultiEdit,
    Write,
    NotebookEdit,
}

/// Whether the matching `tool_result` reported success. `Pending` until a result is seen (or never, for a cut-off session).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Pending,
    Applied,
    Failed,
}

/// One change inside an edit call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditOp {
    /// Edit, or one entry of MultiEdit: `old` was replaced by `new`.
    Replace { old: String, new: String },
    /// Write, or a notebook cell: the whole new text. The text it replaced is not in the transcript.
    Full { content: String },
}

impl EditOp {
    /// The diff this one change shows: old to new for a replacement, nothing-to-new for a whole file.
    pub fn diff(&self) -> Diff {
        match self {
            EditOp::Replace { old, new } => diff::diff(old, new),
            EditOp::Full { content } => diff::diff("", content),
        }
    }
}

/// One file-changing tool call, in the order it was made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    /// The `tool_use` id.
    pub id: String,
    pub tool: Tool,
    pub path: String,
    /// The line's `timestamp`, as written (ISO 8601), when the line had one.
    pub time: Option<String>,
    /// Made by a subagent (a sidechain line, or a line of a subagent's own file).
    pub sidechain: bool,
    /// One entry for Edit, Write and NotebookEdit; one per entry for MultiEdit.
    pub ops: Vec<EditOp>,
    pub outcome: Outcome,
    /// Some of its text was cut to `MAX_TEXT`, so its diff is of the kept part only.
    pub truncated: bool,
}

/// A file the assistant named in its text, as an absolute Windows path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedFile {
    /// Backslashes, as the assistant wrote it (forward slashes are turned into backslashes).
    pub path: String,
    /// The extension is a picture type (png, jpg, jpeg, gif, bmp, webp, ico).
    pub image: bool,
    /// First named by a subagent.
    pub sidechain: bool,
}

/// What a transcript says so far. Feed it lines with `fold_line`, in order.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Convo {
    /// Every edit call, in order, main agent and subagents alike (see `Edit::sidechain`).
    pub edits: Vec<Edit>,
    /// Files and images the assistant named, first mention first, each path once.
    pub files: Vec<NamedFile>,
    /// Lines read (including bad ones).
    pub lines: u64,
    /// Lines that were not a JSON object.
    pub bad_lines: u64,
    edit_index: HashMap<String, usize>,
    /// Bytes of edit text currently kept (see `MAX_EDIT_BYTES`).
    edit_bytes: usize,
}

impl Convo {
    pub fn new() -> Self {
        Self::default()
    }

    /// Folds one line of the session file. Never fails: a bad line only bumps `bad_lines`.
    pub fn fold_line(&mut self, line: &str) {
        self.fold_from("", line);
    }

    /// Folds one line from `source`: `""` for the session file, or a subagent's transcript file name
    /// (`agent-<id>.jsonl`). Lines of a subagent file are always sidechain lines.
    pub fn fold_from(&mut self, source: &str, line: &str) {
        self.lines += 1;
        let value: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => {
                self.bad_lines += 1;
                return;
            }
        };
        let Some(obj) = value.as_object() else {
            self.bad_lines += 1;
            return;
        };
        let sidechain = !source.is_empty()
            || obj
                .get("isSidechain")
                .and_then(Value::as_bool)
                .unwrap_or(false);
        let time = obj
            .get("timestamp")
            .and_then(Value::as_str)
            .map(str::to_string);
        let Some(message) = obj.get("message") else {
            return;
        };
        let content = message.get("content");
        match obj.get("type").and_then(Value::as_str) {
            Some("assistant") => self.assistant(content, sidechain, time.as_deref()),
            Some("user") => self.user(content),
            _ => {}
        }
    }

    fn assistant(&mut self, content: Option<&Value>, sidechain: bool, time: Option<&str>) {
        match content {
            Some(Value::String(text)) => self.named_files(text, sidechain),
            Some(Value::Array(blocks)) => {
                for block in blocks {
                    match block.get("type").and_then(Value::as_str) {
                        Some("tool_use") => self.tool_use(block, sidechain, time),
                        Some("text") => {
                            if let Some(text) = block.get("text").and_then(Value::as_str) {
                                self.named_files(text, sidechain);
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    fn user(&mut self, content: Option<&Value>) {
        let Some(Value::Array(blocks)) = content else {
            return;
        };
        for block in blocks {
            if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                continue;
            }
            let Some(id) = block.get("tool_use_id").and_then(Value::as_str) else {
                continue;
            };
            let failed = block
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if let Some(&index) = self.edit_index.get(id) {
                self.edits[index].outcome = if failed {
                    Outcome::Failed
                } else {
                    Outcome::Applied
                };
            }
        }
    }

    fn tool_use(&mut self, block: &Value, sidechain: bool, time: Option<&str>) {
        let Some(id) = block.get("id").and_then(Value::as_str) else {
            return;
        };
        let Some(input) = block.get("input") else {
            return;
        };
        match block.get("name").and_then(Value::as_str) {
            Some("Edit") => self.edit_call(id, Tool::Edit, input, sidechain, time),
            Some("MultiEdit") => self.edit_call(id, Tool::MultiEdit, input, sidechain, time),
            Some("Write") => self.edit_call(id, Tool::Write, input, sidechain, time),
            Some("NotebookEdit") => self.edit_call(id, Tool::NotebookEdit, input, sidechain, time),
            _ => {}
        }
    }

    fn edit_call(
        &mut self,
        id: &str,
        tool: Tool,
        input: &Value,
        sidechain: bool,
        time: Option<&str>,
    ) {
        let path_key = if tool == Tool::NotebookEdit {
            "notebook_path"
        } else {
            "file_path"
        };
        let Some(path) = input.get(path_key).and_then(Value::as_str) else {
            return;
        };
        let text = |key: &str| input.get(key).and_then(Value::as_str).map(str::to_string);
        let ops: Vec<EditOp> = match tool {
            Tool::Edit => match (text("old_string"), text("new_string")) {
                (Some(old), Some(new)) => vec![EditOp::Replace { old, new }],
                _ => Vec::new(),
            },
            Tool::MultiEdit => input
                .get("edits")
                .and_then(Value::as_array)
                .map(|entries| {
                    entries
                        .iter()
                        .filter_map(|e| {
                            let old = e.get("old_string").and_then(Value::as_str)?;
                            let new = e.get("new_string").and_then(Value::as_str)?;
                            Some(EditOp::Replace {
                                old: old.to_string(),
                                new: new.to_string(),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default(),
            Tool::Write => text("content")
                .map(|content| vec![EditOp::Full { content }])
                .unwrap_or_default(),
            Tool::NotebookEdit => text("new_source")
                .map(|content| vec![EditOp::Full { content }])
                .unwrap_or_default(),
        };
        if ops.is_empty() {
            return;
        }
        let mut truncated = false;
        let ops: Vec<EditOp> = ops
            .into_iter()
            .map(|op| match op {
                EditOp::Replace { old, new } => EditOp::Replace {
                    old: cap_text(old, &mut truncated),
                    new: cap_text(new, &mut truncated),
                },
                EditOp::Full { content } => EditOp::Full {
                    content: cap_text(content, &mut truncated),
                },
            })
            .collect();
        let edit = Edit {
            id: id.to_string(),
            tool,
            path: path.to_string(),
            time: time.map(str::to_string),
            sidechain,
            ops,
            outcome: Outcome::Pending,
            truncated,
        };
        self.edit_bytes += edit_bytes(&edit);
        self.edit_index.insert(id.to_string(), self.edits.len());
        self.edits.push(edit);
        self.evict_old_edits();
    }

    /// Drops the oldest edits while there are too many or the kept text is too big. The newest edit is never dropped.
    fn evict_old_edits(&mut self) {
        let mut dropped = false;
        while self.edits.len() > 1
            && (self.edits.len() > MAX_EDITS || self.edit_bytes > MAX_EDIT_BYTES)
        {
            let gone = self.edits.remove(0);
            self.edit_bytes -= edit_bytes(&gone);
            dropped = true;
        }
        if dropped {
            self.edit_index.clear();
            for (i, e) in self.edits.iter().enumerate() {
                self.edit_index.insert(e.id.clone(), i);
            }
        }
    }

    fn named_files(&mut self, text: &str, sidechain: bool) {
        for caps in windows_path_re().captures_iter(text) {
            let Some(m) = caps.get(1) else {
                continue;
            };
            let raw = m.as_str().trim_end_matches(['.', ',', ';', ':', '!', '?']);
            let path = raw.replace('/', "\\");
            let Some(ext) = path
                .rsplit_once('\\')
                .and_then(|(_, name)| name.rsplit_once('.'))
                .map(|(_, e)| e.to_ascii_lowercase())
            else {
                continue;
            };
            if ext.is_empty() {
                continue;
            }
            let key = path.to_ascii_lowercase();
            if self
                .files
                .iter()
                .any(|f| f.path.to_ascii_lowercase() == key)
            {
                continue;
            }
            let image = matches!(
                ext.as_str(),
                "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "ico"
            );
            self.files.push(NamedFile {
                path,
                image,
                sidechain,
            });
        }
    }
}

/// An absolute Windows path: a drive, a colon, a slash, then characters that cannot end a path in prose. The drive
/// must not follow a letter, digit, `_`, `/`, a backslash or `:`, so a URL (`https://x.com/a.png`) and a file URL
/// (`file:///C:/a.png`) name no file. Group 1 is the path.
fn windows_path_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?:^|[^A-Za-z0-9_/\\:])([A-Za-z]:[\\/][^\s"'`<>|*?()\[\]{}]+)"#)
            .expect("static pattern")
    })
}

/// Cuts `s` to `MAX_TEXT` bytes at a character boundary, noting the cut.
fn cap_text(s: String, truncated: &mut bool) -> String {
    if s.len() <= MAX_TEXT {
        return s;
    }
    *truncated = true;
    let mut end = MAX_TEXT;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

fn edit_bytes(edit: &Edit) -> usize {
    edit.ops
        .iter()
        .map(|op| match op {
            EditOp::Replace { old, new } => old.len() + new.len(),
            EditOp::Full { content } => content.len(),
        })
        .sum()
}

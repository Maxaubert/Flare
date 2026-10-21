//! Folds transcript lines into the model Flare shows: the edits, the task list and the files the agent named.
//!
//! Parsing is lenient. A line that is not a JSON object is counted in `bad_lines` and skipped. An object of an unknown
//! `type`, or a tool the model does not know, is ignored. Nothing here panics on a field of the wrong shape: a missing
//! or wrongly typed field simply makes that one item absent. The shapes this relies on are in `docs/surfaces/convo.md`.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use regex::Regex;
use serde_json::Value;

use crate::diff::{self, Diff};

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
    /// Made by a subagent (a sidechain line).
    pub sidechain: bool,
    /// One entry for Edit, Write and NotebookEdit; one per entry for MultiEdit.
    pub ops: Vec<EditOp>,
    pub outcome: Outcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Pending,
    InProgress,
    Completed,
    /// A status word this reader does not know, kept as it is not lost.
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    /// A stable key inside its list: the `tool_use` id of the call that created it, or `todo:<index>` for a TodoWrite entry.
    pub key: String,
    /// The id the task tools use to refer to it (`TaskUpdate.taskId`), when the create result gave one.
    pub id: Option<String>,
    pub title: String,
    pub active_form: Option<String>,
    pub status: TaskStatus,
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
    /// The main agent's task list. TodoWrite replaces it; the task tools add to and change it.
    pub tasks: Vec<Task>,
    /// A subagent's task list, kept apart so it never replaces the main one.
    pub side_tasks: Vec<Task>,
    /// Files and images the assistant named, first mention first, each path once.
    pub files: Vec<NamedFile>,
    /// Lines read (including bad ones).
    pub lines: u64,
    /// Lines that were not a JSON object.
    pub bad_lines: u64,
    edit_index: HashMap<String, usize>,
    created: HashSet<String>,
}

impl Convo {
    pub fn new() -> Self {
        Self::default()
    }

    /// Folds one line. Never fails: a bad line only bumps `bad_lines`.
    pub fn fold_line(&mut self, line: &str) {
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
        let sidechain = obj
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
            } else if self.created.contains(id) && !failed {
                let text = result_text(block.get("content"));
                if let Some(task_id) = task_id_in(&text)
                    && let Some(task) = self.find_task_mut(id)
                {
                    task.id = Some(task_id);
                }
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
            Some("TodoWrite") => self.todo_write(input, sidechain),
            Some("TaskCreate") => self.task_create(id, input, sidechain),
            Some("TaskUpdate") => self.task_update(input, sidechain),
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
        self.edit_index.insert(id.to_string(), self.edits.len());
        self.edits.push(Edit {
            id: id.to_string(),
            tool,
            path: path.to_string(),
            time: time.map(str::to_string),
            sidechain,
            ops,
            outcome: Outcome::Pending,
        });
    }

    /// TodoWrite sends the whole list each time: it replaces the list it belongs to.
    fn todo_write(&mut self, input: &Value, sidechain: bool) {
        let Some(todos) = input.get("todos").and_then(Value::as_array) else {
            return;
        };
        let list: Vec<Task> = todos
            .iter()
            .enumerate()
            .filter_map(|(i, todo)| {
                let title = todo.get("content").and_then(Value::as_str)?;
                Some(Task {
                    key: format!("todo:{i}"),
                    id: None,
                    title: title.to_string(),
                    active_form: todo
                        .get("activeForm")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    status: status_of(todo.get("status").and_then(Value::as_str)),
                })
            })
            .collect();
        if sidechain {
            self.side_tasks = list;
        } else {
            self.tasks = list;
        }
    }

    /// The task tools (TaskCreate, TaskUpdate). Their shape is less certain than TodoWrite's; see `convo.md`.
    fn task_create(&mut self, id: &str, input: &Value, sidechain: bool) {
        let title = ["subject", "content", "description"]
            .iter()
            .find_map(|k| input.get(*k).and_then(Value::as_str));
        let Some(title) = title else {
            return;
        };
        let task = Task {
            key: id.to_string(),
            id: None,
            title: title.to_string(),
            active_form: input
                .get("activeForm")
                .and_then(Value::as_str)
                .map(str::to_string),
            status: TaskStatus::Pending,
        };
        self.created.insert(id.to_string());
        self.list_mut(sidechain).push(task);
    }

    fn task_update(&mut self, input: &Value, sidechain: bool) {
        let Some(task_id) = id_value(input.get("taskId")) else {
            return;
        };
        let status = input.get("status").and_then(Value::as_str);
        if status == Some("deleted") {
            self.list_mut(sidechain)
                .retain(|t| t.id.as_deref() != Some(task_id.as_str()) && t.key != task_id);
            return;
        }
        let title = input
            .get("subject")
            .or_else(|| input.get("content"))
            .and_then(Value::as_str);
        let active = input.get("activeForm").and_then(Value::as_str);
        let list = self.list_mut(sidechain);
        let Some(task) = list
            .iter_mut()
            .find(|t| t.id.as_deref() == Some(task_id.as_str()) || t.key == task_id)
        else {
            return;
        };
        if let Some(status) = status {
            task.status = status_of(Some(status));
        }
        if let Some(title) = title {
            task.title = title.to_string();
        }
        if let Some(active) = active {
            task.active_form = Some(active.to_string());
        }
    }

    fn list_mut(&mut self, sidechain: bool) -> &mut Vec<Task> {
        if sidechain {
            &mut self.side_tasks
        } else {
            &mut self.tasks
        }
    }

    /// A task created by a call with this key, in either list.
    fn find_task_mut(&mut self, key: &str) -> Option<&mut Task> {
        self.tasks
            .iter_mut()
            .chain(self.side_tasks.iter_mut())
            .find(|t| t.key == key)
    }

    fn named_files(&mut self, text: &str, sidechain: bool) {
        for m in windows_path_re().find_iter(text) {
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

/// An absolute Windows path: a drive, a colon, a slash, then characters that cannot end a path in prose.
fn windows_path_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"[A-Za-z]:[\\/][^\s"'`<>|*?()\[\]{}]+"#).expect("static pattern")
    })
}

fn status_of(word: Option<&str>) -> TaskStatus {
    match word {
        Some("pending") => TaskStatus::Pending,
        Some("in_progress") => TaskStatus::InProgress,
        Some("completed") => TaskStatus::Completed,
        _ => TaskStatus::Other,
    }
}

/// A `taskId` may be a string or a number.
fn id_value(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// The text of a tool result: a plain string, or the text blocks of an array, joined by newlines.
fn result_text(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// A task id from a create result: a JSON `id` (or `task.id`), else the first `#<digits>` in the text. A guess at a
/// format this reader has not seen written down; a miss only means `TaskUpdate` matches by key instead.
fn task_id_in(text: &str) -> Option<String> {
    if let Ok(v) = serde_json::from_str::<Value>(text)
        && let Some(id) = id_value(v.pointer("/task/id")).or_else(|| id_value(v.get("id")))
    {
        return Some(id);
    }
    let at = text.find('#')?;
    let digits: String = text[at + 1..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    (!digits.is_empty()).then_some(digits)
}

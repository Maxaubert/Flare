//! What the Changes panel shows of a [`Convo`]: the changed files, one file's diff as rows, and the compact summary
//! the control channel answers with. Plain data, no window; the app turns these into Slint rows.
//!
//! Like `fold` and `diff`, these are not cheap on a long session: call them on the tracker's worker, never on the UI
//! thread (rule 34).

use std::collections::HashMap;

use crate::diff::Kind;
use crate::fold::{Convo, Edit, EditOp, Outcome};

/// The most diff rows one file shows. A file edited a thousand times keeps its first rows and says so.
pub const MAX_ROWS: usize = 5000;
/// The most characters one diff row keeps. A longer line (a minified file) is cut and ends in an ellipsis, so the window
/// never shapes a 64 KB line (rule 34).
pub const LINE_CHARS: usize = 400;

/// One changed file, as the Changes panel lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSummary {
    /// The path as the agent wrote it (the newest spelling of it).
    pub path: String,
    /// The last segment.
    pub name: String,
    /// The folder part, for the second line; empty for a bare name.
    pub dir: String,
    /// Edit calls on the file (applied, failed and unanswered).
    pub edits: usize,
    /// Of those, the ones that came back as an error.
    pub failed: usize,
    /// Of those, a subagent made at least one.
    pub by_subagent: bool,
}

/// The key two spellings of one Windows path share: slashes one way, case folded.
fn key_of(path: &str) -> String {
    path.replace('/', "\\").to_lowercase()
}

/// The last segment and the folder part of a path.
fn split_path(path: &str) -> (String, String) {
    match path.rfind(['\\', '/']) {
        Some(i) => (path[i + 1..].to_string(), path[..i].to_string()),
        None => (path.to_string(), String::new()),
    }
}

/// The changed files, the file edited most recently first.
pub fn files(convo: &Convo) -> Vec<FileSummary> {
    let mut by_key: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<(usize, FileSummary)> = Vec::new();
    for (n, e) in convo.edits.iter().enumerate() {
        let k = key_of(&e.path);
        let slot = *by_key.entry(k).or_insert_with(|| {
            let (name, dir) = split_path(&e.path);
            out.push((
                n,
                FileSummary {
                    path: e.path.clone(),
                    name,
                    dir,
                    edits: 0,
                    failed: 0,
                    by_subagent: false,
                },
            ));
            out.len() - 1
        });
        let (last, f) = &mut out[slot];
        *last = n;
        f.edits += 1;
        if e.outcome == Outcome::Failed {
            f.failed += 1;
        }
        f.by_subagent |= e.sidechain;
        if f.path != e.path {
            let (name, dir) = split_path(&e.path);
            f.path = e.path.clone();
            f.name = name;
            f.dir = dir;
        }
    }
    out.sort_by_key(|b| std::cmp::Reverse(b.0));
    out.into_iter().map(|(_, f)| f).collect()
}

/// What a diff row is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    /// A line that names an edit ("Edit 2 of 3") or a hunk ("Lines 4 to 9").
    Header,
    Context,
    Removed,
    Added,
    /// A plain note: failed, cut, whole file written, more not shown.
    Note,
}

/// One row of a file's diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffRow {
    pub kind: RowKind,
    /// Line number in the text the edit carried (the snippet, not the file); 0 for none.
    pub old_no: u32,
    pub new_no: u32,
    pub text: String,
}

fn row(kind: RowKind, old_no: usize, new_no: usize, text: impl Into<String>) -> DiffRow {
    let mut text: String = text.into();
    if let Some((cut, _)) = text.char_indices().nth(LINE_CHARS) {
        // Keep LINE_CHARS - 1 characters and the marker, so the row is LINE_CHARS long.
        let keep = text
            .char_indices()
            .nth(LINE_CHARS - 1)
            .map_or(cut, |(i, _)| i);
        text.truncate(keep);
        text.push('…');
    }
    DiffRow {
        kind,
        old_no: u32::try_from(old_no).unwrap_or(u32::MAX),
        new_no: u32::try_from(new_no).unwrap_or(u32::MAX),
        text,
    }
}

/// One file's edits as rows, oldest edit first.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FileDiff {
    pub rows: Vec<DiffRow>,
    /// Rows were left out because the file passed `MAX_ROWS`.
    pub truncated: bool,
}

/// The edits of `path` as diff rows. An unknown path gives no rows.
pub fn file_diff(convo: &Convo, path: &str) -> FileDiff {
    let want = key_of(path);
    let edits: Vec<&Edit> = convo
        .edits
        .iter()
        .filter(|e| key_of(&e.path) == want)
        .collect();
    let mut out = FileDiff::default();
    let total = edits.len();
    'edits: for (n, e) in edits.iter().enumerate() {
        let mut head = if total == 1 {
            "Edit".to_string()
        } else {
            format!("Edit {} of {total}", n + 1)
        };
        // An edit with no answer yet is not marked: the answer is a line away, and a cut-off session never has one.
        if e.outcome == Outcome::Failed {
            head.push_str(", failed");
        }
        if e.sidechain {
            head.push_str(", by a subagent");
        }
        if !push(&mut out, row(RowKind::Header, 0, 0, head)) {
            break;
        }
        for op in &e.ops {
            if let EditOp::Full { .. } = op
                && !push(
                    &mut out,
                    row(
                        RowKind::Note,
                        0,
                        0,
                        "The whole file was written; the text it replaced is not in the conversation.",
                    ),
                )
            {
                break 'edits;
            }
            let diff = op.diff();
            for h in &diff.hunks {
                let from = h.new_start.max(h.old_start);
                if !push(
                    &mut out,
                    row(RowKind::Header, 0, 0, format!("Around line {from}")),
                ) {
                    break 'edits;
                }
                let (mut o, mut nw) = (h.old_start, h.new_start);
                for l in &h.lines {
                    let r = match l.kind {
                        Kind::Context => {
                            let r = row(RowKind::Context, o, nw, l.text.as_str());
                            o += 1;
                            nw += 1;
                            r
                        }
                        Kind::Removed => {
                            let r = row(RowKind::Removed, o, 0, l.text.as_str());
                            o += 1;
                            r
                        }
                        Kind::Added => {
                            let r = row(RowKind::Added, 0, nw, l.text.as_str());
                            nw += 1;
                            r
                        }
                    };
                    if !push(&mut out, r) {
                        break 'edits;
                    }
                }
            }
            if diff.truncated
                && !push(
                    &mut out,
                    row(
                        RowKind::Note,
                        0,
                        0,
                        "Only the first lines of this edit are shown.",
                    ),
                )
            {
                break 'edits;
            }
        }
        if e.truncated
            && !push(
                &mut out,
                row(
                    RowKind::Note,
                    0,
                    0,
                    "Only the first 64 KB of this edit is kept.",
                ),
            )
        {
            break;
        }
    }
    out
}

/// Pushes a row unless the file is full; the row that does not fit becomes the closing note. False once full.
fn push(out: &mut FileDiff, r: DiffRow) -> bool {
    if out.truncated {
        return false;
    }
    if out.rows.len() >= MAX_ROWS {
        out.truncated = true;
        out.rows.push(row(
            RowKind::Note,
            0,
            0,
            "More lines of this file are not shown.",
        ));
        return false;
    }
    out.rows.push(r);
    true
}

/// What `convo.state` answers: small, no file contents.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Summary {
    /// Different files the agent changed.
    pub files: usize,
    /// Edit calls on them.
    pub edits: usize,
    /// Of those, the ones that came back as an error.
    pub failed: usize,
}

pub fn summary(convo: &Convo) -> Summary {
    let list = files(convo);
    Summary {
        files: list.len(),
        edits: list.iter().map(|f| f.edits).sum(),
        failed: list.iter().map(|f| f.failed).sum(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn assistant(blocks: serde_json::Value) -> String {
        json!({"type": "assistant", "message": {"content": blocks}}).to_string()
    }

    fn edit_line(id: &str, path: &str, old: &str, new: &str) -> String {
        assistant(json!([{"type": "tool_use", "id": id, "name": "Edit",
            "input": {"file_path": path, "old_string": old, "new_string": new}}]))
    }

    fn result_line(id: &str, error: bool) -> String {
        json!({"type": "user", "message": {"content": [
            {"type": "tool_result", "tool_use_id": id, "is_error": error, "content": "x"}]}})
        .to_string()
    }

    fn convo(lines: &[String]) -> Convo {
        let mut c = Convo::new();
        for l in lines {
            c.fold_line(l);
        }
        c
    }

    #[test]
    fn files_list_the_newest_first_and_count_each_edit_once() {
        let c = convo(&[
            edit_line("1", r"C:\p\a.rs", "x", "y"),
            edit_line("2", r"C:\p\b.rs", "x", "y"),
            edit_line("3", "C:/p/A.rs", "y", "z"),
            result_line("3", true),
        ]);
        let f = files(&c);
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].name, "A.rs", "a.rs was edited last");
        assert_eq!(f[0].edits, 2);
        assert_eq!(f[0].failed, 1);
        assert_eq!(f[0].dir, "C:/p");
        assert_eq!(f[1].name, "b.rs");
        assert_eq!(f[1].edits, 1);
    }

    #[test]
    fn a_diff_marks_each_edit_and_numbers_its_lines() {
        let c = convo(&[
            edit_line("1", r"C:\p\a.rs", "one\ntwo\nthree", "one\n2\nthree"),
            result_line("1", false),
            edit_line("2", r"C:\p\a.rs", "q", "r"),
            result_line("2", true),
        ]);
        let d = file_diff(&c, r"c:\P\A.RS");
        let kinds: Vec<RowKind> = d.rows.iter().map(|r| r.kind).collect();
        assert_eq!(kinds[0], RowKind::Header);
        assert_eq!(d.rows[0].text, "Edit 1 of 2");
        assert!(d.rows.iter().any(|r| r.text == "Edit 2 of 2, failed"));
        let removed = d.rows.iter().find(|r| r.kind == RowKind::Removed).unwrap();
        assert_eq!(
            (removed.text.as_str(), removed.old_no, removed.new_no),
            ("two", 2, 0)
        );
        let added = d.rows.iter().find(|r| r.kind == RowKind::Added).unwrap();
        assert_eq!(
            (added.text.as_str(), added.old_no, added.new_no),
            ("2", 0, 2)
        );
        assert!(!d.truncated);
        assert!(file_diff(&c, r"C:\other.rs").rows.is_empty());
    }

    #[test]
    fn a_whole_file_write_says_the_old_text_is_unknown() {
        let c = convo(&[assistant(
            json!([{"type": "tool_use", "id": "w", "name": "Write",
            "input": {"file_path": r"C:\p\n.txt", "content": "a\nb"}}]),
        )]);
        let d = file_diff(&c, r"C:\p\n.txt");
        assert!(d.rows.iter().any(|r| r.kind == RowKind::Note));
        assert_eq!(
            d.rows.iter().filter(|r| r.kind == RowKind::Added).count(),
            2
        );
    }

    #[test]
    fn a_very_long_line_is_cut_to_the_row_width_with_an_ellipsis() {
        let minified = "x".repeat(60 * 1024);
        let c = convo(&[assistant(
            json!([{"type": "tool_use", "id": "w", "name": "Write",
            "input": {"file_path": r"C:\p\min.js", "content": minified}}]),
        )]);
        let d = file_diff(&c, r"C:\p\min.js");
        assert!(
            d.rows.iter().all(|r| r.text.chars().count() <= LINE_CHARS),
            "no row passes the width"
        );
        let added = d.rows.iter().find(|r| r.kind == RowKind::Added).unwrap();
        assert_eq!(added.text.chars().count(), LINE_CHARS);
        assert!(added.text.ends_with('…'));
    }

    #[test]
    fn a_huge_diff_is_cut_at_the_row_cap_with_a_note() {
        let big: String = (0..3000).map(|i| format!("line {i}\n")).collect();
        let lines: Vec<String> = (0..4)
            .map(|i| edit_line(&i.to_string(), r"C:\p\big.txt", "", &big))
            .collect();
        let c = convo(&lines);
        let d = file_diff(&c, r"C:\p\big.txt");
        assert!(d.truncated);
        assert_eq!(d.rows.len(), MAX_ROWS + 1);
        assert_eq!(d.rows.last().unwrap().kind, RowKind::Note);
    }

    #[test]
    fn the_summary_counts_files_edits_and_failures_and_ignores_the_task_tools() {
        let c = convo(&[
            edit_line("1", r"C:\p\a.rs", "x", "y"),
            edit_line("2", r"C:\p\a.rs", "y", "z"),
            result_line("2", true),
            edit_line("3", r"C:\p\b.rs", "x", "y"),
            assistant(json!([{"type": "tool_use", "id": "t", "name": "TodoWrite",
                "input": {"todos": [{"content": "Read", "status": "in_progress", "activeForm": "Reading"}]}}])),
        ]);
        assert_eq!(
            summary(&c),
            Summary {
                files: 2,
                edits: 3,
                failed: 1
            }
        );
    }

    #[test]
    fn an_empty_conversation_has_nothing_to_show() {
        let c = Convo::new();
        assert!(files(&c).is_empty());
        assert_eq!(summary(&c), Summary::default());
    }
}

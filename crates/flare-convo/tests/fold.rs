//! Fold tests on the synthetic transcript in `tests/fixtures/session.jsonl` (hand-written, no real content).

use flare_convo::fold::{Convo, EditOp, Outcome, TaskStatus, Tool};
use flare_convo::{Kind, transcript_path};

fn folded() -> Convo {
    let mut convo = Convo::new();
    for line in include_str!("fixtures/session.jsonl").lines() {
        convo.fold_line(line);
    }
    convo
}

fn edit<'a>(convo: &'a Convo, id: &str) -> &'a flare_convo::Edit {
    convo
        .edits
        .iter()
        .find(|e| e.id == id)
        .unwrap_or_else(|| panic!("no edit {id}"))
}

#[test]
fn counts_every_line_and_the_bad_ones() {
    let convo = folded();
    assert_eq!(convo.lines, 21);
    assert_eq!(convo.bad_lines, 2, "the broken object and the bare array");
}

#[test]
fn an_edit_is_listed_with_its_path_time_and_applied_result() {
    let convo = folded();
    let e = edit(&convo, "toolu_01");
    assert_eq!(e.tool, Tool::Edit);
    assert_eq!(e.path, "C:\\Users\\example\\project\\src\\parser.rs");
    assert_eq!(e.time.as_deref(), Some("2026-10-21T10:00:05.000Z"));
    assert_eq!(e.outcome, Outcome::Applied);
    assert!(!e.sidechain);
    assert_eq!(
        e.ops,
        vec![EditOp::Replace {
            old: "let x = 1;".into(),
            new: "let x = 2;".into()
        }]
    );
}

#[test]
fn multiedit_keeps_each_entry_as_its_own_change() {
    let convo = folded();
    let e = edit(&convo, "toolu_02");
    assert_eq!(e.tool, Tool::MultiEdit);
    assert_eq!(e.ops.len(), 2);
    assert_eq!(
        e.outcome,
        Outcome::Applied,
        "a result given as text blocks still counts"
    );
    assert_eq!(
        e.ops[1],
        EditOp::Replace {
            old: "fn b()".into(),
            new: "fn beta()".into()
        }
    );
}

#[test]
fn write_is_a_whole_file_and_diffs_as_all_added() {
    let convo = folded();
    let e = edit(&convo, "toolu_03");
    assert_eq!(e.tool, Tool::Write);
    assert_eq!(
        e.ops,
        vec![EditOp::Full {
            content: "pub fn hello() {}\n".into()
        }]
    );
    let d = e.ops[0].diff();
    assert_eq!(d.hunks.len(), 1);
    assert!(d.hunks[0].lines.iter().all(|l| l.kind == Kind::Added));
}

#[test]
fn a_failed_edit_is_kept_and_marked_failed() {
    let convo = folded();
    assert_eq!(edit(&convo, "toolu_04").outcome, Outcome::Failed);
}

#[test]
fn an_edit_with_no_result_is_pending_and_a_notebook_cell_is_a_whole_cell() {
    let convo = folded();
    let nb = edit(&convo, "toolu_30");
    assert_eq!(nb.tool, Tool::NotebookEdit);
    assert_eq!(nb.path, "C:\\Users\\example\\project\\analysis.ipynb");
    assert_eq!(nb.outcome, Outcome::Pending);
    assert_eq!(
        nb.ops,
        vec![EditOp::Full {
            content: "print(1)\n".into()
        }]
    );
}

#[test]
fn an_edit_missing_its_new_text_and_an_unknown_tool_are_ignored() {
    let convo = folded();
    assert!(
        convo.edits.iter().all(|e| e.id != "toolu_31"),
        "no new_string, no change"
    );
    assert!(
        convo.edits.iter().all(|e| e.id != "toolu_32"),
        "Bash is not a file edit"
    );
}

#[test]
fn the_sidechain_edit_is_tagged_and_kept_in_the_same_list() {
    let convo = folded();
    let side = edit(&convo, "toolu_20");
    assert!(side.sidechain);
    assert_eq!(convo.edits.len(), 6);
}

#[test]
fn todowrite_replaces_the_whole_list_each_time() {
    let convo = folded();
    // toolu_06 replaced the three-item list from toolu_05 with two items; then TaskCreate added one.
    assert_eq!(convo.tasks.len(), 3);
    assert_eq!(convo.tasks[0].title, "Fix the lexer");
    assert_eq!(convo.tasks[0].status, TaskStatus::Completed);
    assert_eq!(convo.tasks[1].title, "Run the tests");
    assert_eq!(convo.tasks[1].status, TaskStatus::InProgress);
    assert!(convo.tasks.iter().all(|t| t.title != "Read the parser"));
}

#[test]
fn a_task_tool_create_then_update_by_its_id() {
    let convo = folded();
    let task = &convo.tasks[2];
    assert_eq!(task.title, "Write the release notes");
    assert_eq!(
        task.active_form.as_deref(),
        Some("Writing the release notes")
    );
    assert_eq!(
        task.id.as_deref(),
        Some("7"),
        "the id came from the create result"
    );
    assert_eq!(
        task.status,
        TaskStatus::InProgress,
        "TaskUpdate found it by taskId"
    );
}

#[test]
fn a_subagent_todolist_never_replaces_the_main_one() {
    let convo = folded();
    assert_eq!(convo.side_tasks.len(), 1);
    assert_eq!(convo.side_tasks[0].title, "Side task");
    assert_eq!(convo.side_tasks[0].status, TaskStatus::Pending);
    assert_eq!(convo.tasks.len(), 3);
}

#[test]
fn named_files_are_absolute_windows_paths_with_an_extension_once_each() {
    let convo = folded();
    let paths: Vec<(&str, bool, bool)> = convo
        .files
        .iter()
        .map(|f| (f.path.as_str(), f.image, f.sidechain))
        .collect();
    assert_eq!(
        paths,
        vec![
            ("C:\\Users\\example\\project\\src\\parser.rs", false, false),
            (
                "C:\\Users\\example\\project\\docs\\diagram.png",
                true,
                false
            ),
            (
                "C:\\Users\\example\\project\\build\\preview.png",
                true,
                true
            ),
        ],
        "the folder path (no extension) and the trailing full stop are not files"
    );
}

#[test]
fn an_unknown_type_or_a_non_object_never_panics_or_changes_the_model() {
    let mut convo = Convo::new();
    convo.fold_line(r#"{"type":"something-new","message":{"content":[{"type":"tool_use","id":"x","name":"Edit","input":{"file_path":"C:\\a.rs","old_string":"a","new_string":"b"}}]}}"#);
    assert!(
        convo.edits.is_empty(),
        "an unknown type is ignored, even with an edit inside"
    );
    convo.fold_line("42");
    convo.fold_line("");
    assert_eq!(
        convo.bad_lines, 2,
        "a bare number and an empty string are both not objects"
    );
}

#[test]
fn a_user_text_message_names_no_files() {
    let mut convo = Convo::new();
    convo.fold_line(r#"{"type":"user","message":{"role":"user","content":"see C:\\Users\\example\\notes.txt"}}"#);
    assert!(convo.files.is_empty(), "only the assistant's words count");
}

#[test]
fn transcript_path_uses_the_slug_and_refuses_an_unsafe_id() {
    use std::path::Path;
    let home = Path::new("C:\\Users\\Admin");
    let p = transcript_path(
        home,
        "C:\\Users\\Admin\\my project",
        "3f2a9c1e-77aa-4c0d-9b1e-0a1b2c3d4e5f",
    )
    .expect("a UUID is safe");
    assert!(
        p.ends_with(
            Path::new(".claude")
                .join("projects")
                .join("C--Users-Admin-my-project")
                .join("3f2a9c1e-77aa-4c0d-9b1e-0a1b2c3d4e5f.jsonl")
        ),
        "got {}",
        p.display()
    );
    assert!(transcript_path(home, "C:\\x", "..\\..\\secret").is_none());
    assert!(transcript_path(home, "C:\\x", "").is_none());
}

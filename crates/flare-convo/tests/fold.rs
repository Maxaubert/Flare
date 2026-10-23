//! Fold tests on the synthetic transcript in `tests/fixtures/session.jsonl` (hand-written, no real content).

use flare_convo::fold::{
    Convo, EditOp, MAX_EDIT_BYTES, MAX_EDITS, MAX_TEXT, Outcome, TaskStatus, Tool,
};
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
fn a_subagent_updates_a_task_the_main_agent_made() {
    let mut convo = folded();
    let line = serde_json::json!({"type": "assistant", "message": {"content": [{
        "type": "tool_use", "id": "toolu_sub_upd", "name": "TaskUpdate",
        "input": {"taskId": "7", "status": "completed"}}]}})
    .to_string();
    convo.fold_from("agent-abc.jsonl", &line);
    assert_eq!(
        convo.tasks[2].status,
        TaskStatus::Completed,
        "the shared list is the main one's"
    );
    assert!(
        !convo.side_tasks.contains_key("agent-abc.jsonl")
            || convo.side_tasks["agent-abc.jsonl"].is_empty(),
        "no phantom task on the subagent's own list"
    );
    // And a subagent may delete it.
    let del = serde_json::json!({"type": "assistant", "message": {"content": [{
        "type": "tool_use", "id": "toolu_sub_del", "name": "TaskUpdate",
        "input": {"taskId": "7", "status": "deleted"}}]}})
    .to_string();
    convo.fold_from("agent-abc.jsonl", &del);
    assert_eq!(convo.tasks.len(), 2);
}

#[test]
fn a_subagent_todolist_never_replaces_the_main_one() {
    let convo = folded();
    assert_eq!(convo.side_tasks[""].len(), 1);
    assert_eq!(convo.side_tasks[""][0].title, "Side task");
    assert_eq!(convo.side_tasks[""][0].status, TaskStatus::Pending);
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

fn fold_agent(convo: &mut Convo, source: &str, fixture: &str) {
    for line in fixture.lines() {
        convo.fold_from(source, line);
    }
}

#[test]
fn a_subagent_file_folds_as_sidechain_and_each_list_keeps_its_own_name() {
    let mut convo = Convo::new();
    fold_agent(
        &mut convo,
        "agent-abc.jsonl",
        include_str!("fixtures/agent-abc.jsonl"),
    );
    fold_agent(
        &mut convo,
        "agent-def.jsonl",
        include_str!("fixtures/agent-def.jsonl"),
    );
    let e = edit(&convo, "toolu_sub_01");
    assert!(
        e.sidechain,
        "every line of a subagent file is a sidechain line"
    );
    assert_eq!(e.outcome, Outcome::Applied);
    assert_eq!(convo.side_tasks["agent-abc.jsonl"][0].title, "Side A");
    assert_eq!(
        convo.side_tasks["agent-def.jsonl"][0].title, "Side B",
        "two subagents' lists do not overwrite each other"
    );
    assert!(convo.tasks.is_empty(), "no subagent replaces the main list");
}

#[test]
fn a_url_or_a_file_url_in_the_assistants_words_names_no_file() {
    let mut convo = Convo::new();
    convo.fold_line(
        r#"{"type":"assistant","message":{"role":"assistant","content":"see https://x.com/a.png and file:///C:/a.png and C:\\real\\b.png"}}"#,
    );
    let paths: Vec<&str> = convo.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, vec!["C:\\real\\b.png"]);
}

#[test]
fn a_text_longer_than_the_cap_is_cut_and_the_edit_says_so() {
    let mut convo = Convo::new();
    let big = "x".repeat(MAX_TEXT + 10);
    let line = serde_json::json!({"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"big","name":"Write","input":{"file_path":"C:\\a.txt","content":big}}]}}).to_string();
    convo.fold_line(&line);
    let e = edit(&convo, "big");
    assert!(e.truncated);
    match &e.ops[0] {
        EditOp::Full { content } => assert_eq!(content.len(), MAX_TEXT),
        other => panic!("expected a whole file, got {other:?}"),
    }
}

#[test]
fn the_oldest_edits_go_once_there_are_too_many_and_a_late_result_is_ignored() {
    let mut convo = Convo::new();
    let total = MAX_EDITS + 100;
    for i in 0..total {
        let line = serde_json::json!({"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":format!("e{i}"),"name":"Edit","input":{"file_path":"C:\\a.rs","old_string":"a","new_string":"b"}}]}}).to_string();
        convo.fold_line(&line);
    }
    assert_eq!(convo.edits.len(), MAX_EDITS, "the cap holds");
    assert_eq!(
        convo.edits[0].id,
        format!("e{}", total - MAX_EDITS),
        "the oldest went first"
    );
    let late = serde_json::json!({"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"e0","content":"ok"}]}}).to_string();
    convo.fold_line(&late);
    let newest = serde_json::json!({"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":format!("e{}", total - 1),"content":"ok"}]}}).to_string();
    convo.fold_line(&newest);
    assert_eq!(
        edit(&convo, &format!("e{}", total - 1)).outcome,
        Outcome::Applied
    );
}

#[test]
fn the_kept_edit_text_is_bounded_in_bytes_too() {
    let mut convo = Convo::new();
    let content = "y".repeat(MAX_TEXT);
    let per_edit = content.len();
    let count = MAX_EDIT_BYTES / per_edit + 40;
    for i in 0..count {
        let line = serde_json::json!({"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":format!("w{i}"),"name":"Write","input":{"file_path":"C:\\a.txt","content":content}}]}}).to_string();
        convo.fold_line(&line);
    }
    let kept: usize = convo
        .edits
        .iter()
        .map(|e| match &e.ops[0] {
            EditOp::Full { content } => content.len(),
            _ => 0,
        })
        .sum();
    assert!(kept <= MAX_EDIT_BYTES, "kept {kept} bytes");
    assert_eq!(
        convo.edits.last().unwrap().id,
        format!("w{}", count - 1),
        "the newest is kept"
    );
}

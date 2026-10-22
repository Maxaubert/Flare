//! The conversation panels through the real window controller (Surfaces stage 6): a fake Claude folder with synthetic
//! transcripts, the hook line a Claude session sends (`OSC 777 ... session=`), and what the window shows.
//!
//! Held here: nothing runs for a tab with no Changes or Tasks panel open; an open panel starts one tracker on the tab's
//! session file and lists what it reads, and the list follows lines appended to the file; a new session switches the
//! file; closing the panel (or the tab) stops the tracker; `convo.state` answers the tab's own summary; a stored panel
//! reopens empty until the transcript loads; a clicked path shows the file in the panel.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use flare_surfaces::PanelKind;
use serde_json::{Value, json};

use super::*;
use crate::ctl_app::{CtlOp, CtlRequest, CtlRequestBox};

const SESSION_B: &str = "11111111-2222-4333-8444-555555555555";
const TAB: &str = "C:\\a";

fn hook(session: &str) -> String {
    format!("\x1b]777;flare-agent;state=working;session={session}\x07")
}

fn edit_line(id: &str, path: &str, old: &str, new: &str) -> String {
    json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": id, "name": "Edit",
        "input": {"file_path": path, "old_string": old, "new_string": new}}]}})
    .to_string()
}

fn todo_line(items: &[(&str, &str)]) -> String {
    let todos: Vec<Value> = items
        .iter()
        .map(|(c, s)| json!({"content": c, "status": s, "activeForm": format!("Doing {c}")}))
        .collect();
    json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "todo",
        "name": "TodoWrite", "input": {"todos": todos}}]}})
    .to_string()
}

/// A fake Claude folder the controller reads instead of the profile's.
struct Claude {
    dir: tempfile::TempDir,
}

impl Claude {
    fn new(f: &Fx) -> Claude {
        let dir = tempfile::tempdir().unwrap();
        f.ctl
            .borrow_mut()
            .set_convo_claude_dir(Some(dir.path().to_path_buf()));
        Claude { dir }
    }

    fn transcript(&self, folder: &str, session: &str) -> PathBuf {
        flare_convo::transcript_path_in(self.dir.path(), folder, session).expect("a path")
    }

    /// Adds lines to the session's transcript (made if it is not there).
    fn append(&self, folder: &str, session: &str, lines: &[String]) {
        use std::io::Write;
        let path = self.transcript(folder, session);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap(); // sync-ok: test code only
        let mut file = std::fs::OpenOptions::new() // sync-ok: test code only
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        for l in lines {
            writeln!(file, "{l}").unwrap();
        }
    }
}

/// A tab in `C:\a` whose agent has said its session.
fn claude_tab(f: &Fx, session: &str) -> String {
    let id = f.tabs(&[TAB]).remove(0);
    f.say(TAB, &hook(session));
    f.pump_until("the session", |f| {
        f.ctl.borrow().hub().claude_session(&id) == Some(session)
    });
    id
}

fn open(f: &Fx, id: &str, kind: PanelKind) {
    assert!(f.ctl.borrow_mut().open_convo_panel(id, kind));
    f.pump();
}

fn files(f: &Fx) -> Vec<String> {
    f.h.comp
        .get_panel_convo_files()
        .iter()
        .map(|r| format!("{}|{}", r.name, r.meta))
        .collect()
}

fn diff_text(f: &Fx) -> Vec<String> {
    f.h.comp
        .get_panel_convo_rows()
        .iter()
        .map(|r| r.text.to_string())
        .collect()
}

fn tasks(f: &Fx) -> Vec<(i32, i32, String, bool)> {
    f.h.comp
        .get_panel_convo_tasks()
        .iter()
        .map(|r| (r.kind, r.state, r.text.to_string(), r.active))
        .collect()
}

fn trackers(f: &Fx) -> usize {
    f.ctl.borrow().convo().tracker_count()
}

#[test]
fn a_tab_with_a_session_and_no_panel_runs_nothing() {
    let f = Fx::new();
    let claude = Claude::new(&f);
    let id = claude_tab(&f, SESSION_A);
    claude.append(TAB, SESSION_A, &[edit_line("e1", "C:\\a\\x.rs", "a", "b")]);
    for _ in 0..5 {
        f.step(100);
    }
    assert_eq!(trackers(&f), 0, "no panel, no tracker");
    assert!(!f.ctl.borrow().convo().active());
    // A panel of another kind does not start one either.
    let note = f._dir.path().join("note.txt");
    std::fs::write(&note, "hello").unwrap(); // sync-ok: test code only
    f.ctl
        .borrow_mut()
        .show_preview(&id, PanelKind::Text, &note.to_string_lossy());
    f.pump();
    assert_eq!(trackers(&f), 0);
}

#[test]
fn the_changes_panel_lists_the_edits_and_follows_the_file_as_it_grows() {
    for scale in [1.0f32, 2.25] {
        let f = Fx::with(
            Opts {
                scale,
                ..Opts::default()
            },
            |_| {},
        );
        let claude = Claude::new(&f);
        let id = claude_tab(&f, SESSION_A);
        claude.append(
            TAB,
            SESSION_A,
            &[edit_line("e1", "C:\\a\\x.rs", "one\ntwo", "one\n2")],
        );
        open(&f, &id, PanelKind::Changes);
        assert_eq!(trackers(&f), 1, "an open panel starts one tracker");
        f.pump_until("the first file", |f| files(f).len() == 1);
        assert_eq!(f.h.comp.get_panel_kind(), 4);
        assert!(f.h.comp.get_panel_shown());
        assert_eq!(f.h.comp.get_panel_title().as_str(), "Changes");
        assert_eq!(f.h.comp.get_panel_sub().as_str(), "1 file");
        assert_eq!(f.h.comp.get_panel_note().as_str(), "");
        assert_eq!(files(&f), ["x.rs|1 edit"]);
        // The agent edits again, and another file: the list follows.
        claude.append(
            TAB,
            SESSION_A,
            &[
                edit_line("e2", "C:\\a\\x.rs", "2", "3"),
                edit_line("e3", "C:\\a\\y.rs", "p", "q"),
            ],
        );
        f.pump_until("the second file", |f| files(f).len() == 2);
        assert_eq!(files(&f), ["y.rs|1 edit", "x.rs|2 edits"], "newest first");
        assert_eq!(f.h.comp.get_panel_sub().as_str(), "2 files");
        // A pick shows the file's edits as diff rows; the list has gone back under the diff.
        f.h.comp.invoke_panel_convo_select("C:\\a\\x.rs".into());
        f.pump_until("the diff", |f| {
            diff_text(f).iter().any(|t| t == "two") && diff_text(f).iter().any(|t| t == "3")
        });
        assert_eq!(f.h.comp.get_panel_convo_selected().as_str(), "C:\\a\\x.rs");
        let text = diff_text(&f);
        assert!(text.iter().any(|t| t == "Edit 1 of 2"), "{text:?}");
        // A new edit of the picked file extends its diff.
        let before = text.len();
        claude.append(TAB, SESSION_A, &[edit_line("e4", "C:\\a\\x.rs", "3", "4")]);
        f.pump_until("the longer diff", |f| diff_text(f).len() > before);
        // Back to the list.
        f.h.comp.invoke_panel_convo_select("".into());
        f.pump();
        assert_eq!(f.h.comp.get_panel_convo_selected().as_str(), "");
        assert_eq!(f.h.comp.get_panel_convo_rows().row_count(), 0);
    }
}

#[test]
fn a_failed_edit_is_marked_in_the_list() {
    let f = Fx::new();
    let claude = Claude::new(&f);
    let id = claude_tab(&f, SESSION_A);
    claude.append(
        TAB,
        SESSION_A,
        &[
            edit_line("e1", "C:\\a\\x.rs", "a", "b"),
            json!({"type": "user", "message": {"content": [{"type": "tool_result",
                "tool_use_id": "e1", "is_error": true, "content": "no match"}]}})
            .to_string(),
        ],
    );
    open(&f, &id, PanelKind::Changes);
    f.pump_until("the file", |f| {
        files(f).first().is_some_and(|r| r.contains("failed"))
    });
    assert_eq!(files(&f), ["x.rs|1 edit, 1 failed"]);
    assert!(f.h.comp.get_panel_convo_files().row_data(0).unwrap().failed);
}

#[test]
fn the_tasks_panel_shows_the_list_and_a_new_todowrite_updates_it() {
    let f = Fx::new();
    let claude = Claude::new(&f);
    let id = claude_tab(&f, SESSION_A);
    claude.append(
        TAB,
        SESSION_A,
        &[todo_line(&[
            ("Read", "completed"),
            ("Edit", "in_progress"),
            ("Test", "pending"),
        ])],
    );
    open(&f, &id, PanelKind::Tasks);
    f.pump_until("the tasks", |f| tasks(f).len() == 3);
    assert_eq!(f.h.comp.get_panel_kind(), 5);
    assert_eq!(f.h.comp.get_panel_title().as_str(), "Tasks");
    assert_eq!(f.h.comp.get_panel_sub().as_str(), "1 of 3 done");
    assert_eq!(
        tasks(&f),
        [
            (1, 2, "Read".to_string(), false),
            (1, 1, "Edit".to_string(), true),
            (1, 0, "Test".to_string(), false)
        ]
    );
    claude.append(
        TAB,
        SESSION_A,
        &[todo_line(&[
            ("Read", "completed"),
            ("Edit", "completed"),
            ("Test", "in_progress"),
        ])],
    );
    f.pump_until("the update", |f| {
        tasks(f).get(2).is_some_and(|t| t.1 == 1 && t.3)
    });
    assert_eq!(f.h.comp.get_panel_sub().as_str(), "2 of 3 done");
    assert!(!tasks(&f)[1].3, "the first task is no longer active");
}

#[test]
fn a_subagents_list_is_a_group_that_folds() {
    let f = Fx::new();
    let claude = Claude::new(&f);
    let id = claude_tab(&f, SESSION_A);
    claude.append(TAB, SESSION_A, &[todo_line(&[("Plan", "in_progress")])]);
    let sub = claude
        .transcript(TAB, SESSION_A)
        .with_extension("")
        .join("subagents");
    std::fs::create_dir_all(&sub).unwrap(); // sync-ok: test code only
    std::fs::write(
        sub.join("agent-abcdef01.jsonl"),
        format!(
            "{}\n",
            todo_line(&[("Look", "pending"), ("Think", "pending")])
        ),
    )
    .unwrap(); // sync-ok: test code only
    open(&f, &id, PanelKind::Tasks);
    f.pump_until("both lists", |f| tasks(f).len() == 2 + 1 + 2);
    let rows = tasks(&f);
    assert_eq!(rows[0].0, 0, "a header for the main list");
    assert_eq!(rows[2].0, 0, "a header for the subagent's list");
    // Fold the subagent's group: its two tasks go.
    f.h.comp
        .invoke_panel_convo_toggle("agent-abcdef01.jsonl".into());
    f.pump();
    assert_eq!(tasks(&f).len(), 3);
    assert!(
        f.h.comp
            .get_panel_convo_tasks()
            .row_data(2)
            .unwrap()
            .collapsed
    );
    f.h.comp
        .invoke_panel_convo_toggle("agent-abcdef01.jsonl".into());
    f.pump();
    assert_eq!(tasks(&f).len(), 5);
}

#[test]
fn a_new_session_switches_the_file_and_forgets_the_pick() {
    let f = Fx::new();
    let claude = Claude::new(&f);
    let id = claude_tab(&f, SESSION_A);
    claude.append(
        TAB,
        SESSION_A,
        &[edit_line("e1", "C:\\a\\old.rs", "a", "b")],
    );
    claude.append(
        TAB,
        SESSION_B,
        &[edit_line("e1", "C:\\a\\fresh.rs", "a", "b")],
    );
    open(&f, &id, PanelKind::Changes);
    f.pump_until("the first session", |f| files(f) == ["old.rs|1 edit"]);
    f.h.comp.invoke_panel_convo_select("C:\\a\\old.rs".into());
    f.pump();
    assert_eq!(
        f.h.comp.get_panel_convo_selected().as_str(),
        "C:\\a\\old.rs"
    );
    // A /clear: the hooks name another session.
    f.say(TAB, &hook(SESSION_B));
    f.pump_until("the second session", |f| files(f) == ["fresh.rs|1 edit"]);
    assert_eq!(f.ctl.borrow().convo().session_of(&id), Some(SESSION_B));
    assert_eq!(trackers(&f), 1, "one tracker, on the new file");
    assert_eq!(
        f.h.comp.get_panel_convo_selected().as_str(),
        "",
        "the pick is gone"
    );
    assert_eq!(f.h.comp.get_panel_convo_rows().row_count(), 0);
    // Appending to the old session's file changes nothing now.
    claude.append(
        TAB,
        SESSION_A,
        &[edit_line("e2", "C:\\a\\old.rs", "b", "c")],
    );
    for _ in 0..8 {
        f.step(100);
        std::thread::sleep(std::time::Duration::from_millis(60)); // sync-ok: test code only
    }
    assert_eq!(files(&f), ["fresh.rs|1 edit"]);
}

#[test]
fn closing_the_panel_stops_the_tracker_and_reopening_starts_it_again() {
    let f = Fx::new();
    let claude = Claude::new(&f);
    let id = claude_tab(&f, SESSION_A);
    claude.append(TAB, SESSION_A, &[edit_line("e1", "C:\\a\\x.rs", "a", "b")]);
    open(&f, &id, PanelKind::Changes);
    f.pump_until("the file", |f| files(f).len() == 1);
    assert_eq!(trackers(&f), 1);
    f.h.comp.invoke_panel_close();
    f.pump();
    assert_eq!(trackers(&f), 0, "no panel, no tracker");
    assert_eq!(
        f.h.comp.get_panel_convo_files().row_count(),
        0,
        "the rows are let go"
    );
    assert!(!f.h.comp.get_panel_open());
    assert_eq!(f.h.comp.get_panel_kind(), 0);
    // The chord brings it back; the worker starts afresh and reads the file again.
    assert!(f.ctl.borrow_mut().toggle_front_panel());
    f.pump_until("the file again", |f| files(f).len() == 1);
    assert_eq!(trackers(&f), 1);
    // Switching the panel to another kind keeps one tracker; to a file panel, none.
    open(&f, &id, PanelKind::Tasks);
    assert_eq!(trackers(&f), 1);
    let note = f._dir.path().join("note.txt");
    std::fs::write(&note, "hello").unwrap(); // sync-ok: test code only
    f.ctl
        .borrow_mut()
        .show_preview(&id, PanelKind::Text, &note.to_string_lossy());
    f.pump_until("the text", |f| f.h.comp.get_panel_kind() == 2);
    assert_eq!(trackers(&f), 0);
    assert_eq!(f.h.comp.get_panel_convo_files().row_count(), 0);
}

#[test]
fn closing_the_tab_stops_its_tracker() {
    let f = Fx::new();
    let claude = Claude::new(&f);
    let id = claude_tab(&f, SESSION_A);
    claude.append(TAB, SESSION_A, &[todo_line(&[("Read", "pending")])]);
    open(&f, &id, PanelKind::Tasks);
    f.pump_until("the task", |f| tasks(f).len() == 1);
    assert_eq!(trackers(&f), 1);
    f.ctl.borrow_mut().close_now(&id);
    f.pump();
    assert_eq!(trackers(&f), 0);
}

#[test]
fn a_panel_with_no_session_says_so_and_starts_nothing() {
    let f = Fx::new();
    let _claude = Claude::new(&f);
    let id = f.tabs(&[TAB]).remove(0);
    open(&f, &id, PanelKind::Changes);
    assert_eq!(trackers(&f), 0);
    assert_eq!(f.h.comp.get_panel_kind(), 4);
    assert!(
        f.h.comp
            .get_panel_note()
            .as_str()
            .starts_with("Claude is not running")
    );
    // The session arrives with the first prompt: the tracker starts, and the panel reads.
    f.say(TAB, &hook(SESSION_A));
    f.pump_until("the tracker", |f| trackers(f) == 1);
    f.pump_until("the reading note to change", |f| {
        let n = f.h.comp.get_panel_note();
        n.as_str() != "Reading the conversation\u{2026}"
            && !n.as_str().starts_with("Claude is not running")
    });
}

#[test]
fn an_empty_session_says_nothing_has_changed_yet() {
    let f = Fx::new();
    let claude = Claude::new(&f);
    let id = claude_tab(&f, SESSION_A);
    claude.append(
        TAB,
        SESSION_A,
        &[json!({"type": "queue-operation"}).to_string()],
    );
    open(&f, &id, PanelKind::Changes);
    f.pump_until("the empty words", |f| {
        f.h.comp.get_panel_note().as_str() == "Claude has not changed a file yet."
    });
    open(&f, &id, PanelKind::Tasks);
    f.pump_until("the empty tasks", |f| {
        f.h.comp.get_panel_note().as_str() == "Claude has no task list yet."
    });
}

// ---- the control channel ----

fn ask(f: &Fx, op: CtlOp) -> Receiver<Result<Value, flare_ctl::CtlError>> {
    let (tx, rx) = std::sync::mpsc::channel();
    f.ctl
        .borrow_mut()
        .on_msg(UiMsg::Ctl(CtlRequestBox(CtlRequest {
            op,
            deadline: Instant::now() + Duration::from_secs(5),
            reply: tx,
        })));
    rx
}

fn answer(f: &Fx, op: CtlOp) -> Value {
    ask(f, op)
        .recv_timeout(Duration::from_secs(5)) // sync-ok: test code only
        .expect("an answer")
        .expect("not an error")
}

#[test]
fn convo_state_summarises_the_tabs_own_conversation() {
    let f = Fx::new();
    let claude = Claude::new(&f);
    let id = claude_tab(&f, SESSION_A);
    // With no panel open the tab is not read: the answer says how to have it read.
    let v = answer(&f, CtlOp::ConvoState { tab: id.clone() });
    assert_eq!(v["tracking"], json!(false));
    assert!(v["note"].as_str().unwrap().contains("panel.open"));
    claude.append(
        TAB,
        SESSION_A,
        &[
            edit_line("e1", "C:\\a\\x.rs", "a", "b"),
            edit_line("e2", "C:\\a\\x.rs", "b", "c"),
            edit_line("e3", "C:\\a\\y.rs", "a", "b"),
            todo_line(&[
                ("Read", "completed"),
                ("Edit", "in_progress"),
                ("Test", "pending"),
            ]),
        ],
    );
    // `panel.open` kind changes (the agent's own way) opens the panel, and the tracker starts.
    let shown = answer(
        &f,
        CtlOp::ShowConvo {
            tab: id.clone(),
            kind: PanelKind::Changes,
        },
    );
    assert_eq!(shown["panel"]["kind"], json!("changes"));
    assert_eq!(shown["panel"]["title"], json!("Changes"));
    assert!(shown["panel"].get("path").is_none(), "no file behind it");
    f.pump_until("the summary", |f| {
        answer(f, CtlOp::ConvoState { tab: id.clone() })["files"] == json!(2)
    });
    let v = answer(&f, CtlOp::ConvoState { tab: id.clone() });
    assert_eq!(v["tracking"], json!(true));
    assert_eq!(v["session"], json!(SESSION_A));
    assert_eq!(v["edits"], json!(3));
    assert_eq!(v["failed"], json!(0));
    assert_eq!(
        v["tasks"],
        json!({"pending": 1, "in_progress": 1, "done": 1, "other": 0})
    );
    assert_eq!(v["active"], json!("Edit"));
    assert_eq!(v["changed"][0]["path"], json!("C:\\a\\y.rs"));
    assert_eq!(v["changed"][1]["edits"], json!(2));
    // panel.state reports the panel as a panel of its own kind.
    let st = answer(&f, CtlOp::PanelState { tab: id.clone() });
    assert_eq!(st["panel"]["kind"], json!("changes"));
    // A tab that is not there is refused.
    let gone = ask(&f, CtlOp::ConvoState { tab: "nope".into() })
        .recv_timeout(Duration::from_secs(5)) // sync-ok: test code only
        .unwrap();
    assert_eq!(gone.unwrap_err().code, flare_ctl::code::NO_TAB);
}

// ---- a stored panel ----

#[test]
fn a_stored_conversation_panel_reopens_empty_until_the_transcript_loads() {
    let f = Fx::with(
        Opts {
            saved: vec![(TAB, None)],
            ..Opts::default()
        },
        |_| {},
    );
    let file = f._dir.path().join("panels.json");
    let saved = [flare_surfaces::store::SavedTabPanel {
        cwd: TAB.into(),
        panel: Some(flare_surfaces::TabPanel {
            kind: PanelKind::Tasks,
            target: "tasks".into(),
            open: true,
            width: Some(0.35),
        }),
    }];
    std::fs::write(&file, flare_surfaces::store::panels_json(&saved)).unwrap(); // sync-ok: test code only
    f.ctl
        .borrow_mut()
        .set_panel_store(Arc::new(flare_surfaces::store::PanelStore::new(
            &file,
            Duration::from_millis(30),
        )));
    let claude = Claude::new(&f);
    AppCtl::start_restore(&f.ctl);
    f.pump_until("the restore", |f| f.ctl.borrow().model().restore_done());
    f.pump_until("the stored panel", |f| f.h.comp.get_panel_kind() == 5);
    assert_eq!(f.h.comp.get_panel_title().as_str(), "Tasks");
    assert!(
        f.h.comp
            .get_panel_note()
            .as_str()
            .starts_with("Claude is not running")
    );
    assert_eq!(
        f.h.comp.get_panel_convo_tasks().row_count(),
        0,
        "empty at first"
    );
    assert!((f.h.comp.get_panel_share() - 0.35).abs() < 1e-6);
    // The agent speaks, the session is known, and the tasks load.
    let id = f.ids()[0].clone();
    claude.append(TAB, SESSION_A, &[todo_line(&[("Read", "pending")])]);
    f.say(TAB, &hook(SESSION_A));
    f.pump_until("the tasks", |f| tasks(f).len() == 1);
    assert_eq!(trackers(&f), 1);
    let _ = id;
}

#[test]
fn a_stored_panel_of_a_kind_with_the_wrong_target_is_dropped() {
    let f = Fx::with(
        Opts {
            saved: vec![(TAB, None)],
            ..Opts::default()
        },
        |_| {},
    );
    let file = f._dir.path().join("panels.json");
    let saved = [flare_surfaces::store::SavedTabPanel {
        cwd: TAB.into(),
        panel: Some(flare_surfaces::TabPanel {
            kind: PanelKind::Changes,
            target: "C:\\somewhere.txt".into(),
            open: true,
            width: None,
        }),
    }];
    std::fs::write(&file, flare_surfaces::store::panels_json(&saved)).unwrap(); // sync-ok: test code only
    f.ctl
        .borrow_mut()
        .set_panel_store(Arc::new(flare_surfaces::store::PanelStore::new(
            &file,
            Duration::from_millis(30),
        )));
    AppCtl::start_restore(&f.ctl);
    f.pump_until("the restore", |f| f.ctl.borrow().model().restore_done());
    f.pump();
    assert_eq!(f.ctl.borrow().open_panels(), 0);
}

// ---- clicking a path ----

#[test]
fn a_click_on_the_path_in_a_diff_shows_the_file_and_a_program_shows_nothing() {
    let f = Fx::new();
    let claude = Claude::new(&f);
    let id = claude_tab(&f, SESSION_A);
    let work = f._dir.path().join("work");
    std::fs::create_dir_all(&work).unwrap(); // sync-ok: test code only
    let real: PathBuf = work.join("notes.txt");
    std::fs::write(&real, "kept\nlines\nhere").unwrap(); // sync-ok: test code only
    let prog = work.join("run.exe");
    std::fs::write(&prog, "MZ").unwrap(); // sync-ok: test code only
    let (real_s, prog_s) = (
        real.to_string_lossy().into_owned(),
        prog.to_string_lossy().into_owned(),
    );
    claude.append(
        TAB,
        SESSION_A,
        &[
            edit_line("e1", &real_s, "a", "b"),
            edit_line("e2", &prog_s, "a", "b"),
        ],
    );
    open(&f, &id, PanelKind::Changes);
    f.pump_until("both files", |f| files(f).len() == 2);
    // A program is never shown (rule 14): the click does nothing, and the Changes panel stays.
    f.h.comp.invoke_panel_convo_open(prog_s.as_str().into());
    for _ in 0..10 {
        f.pump();
        std::thread::sleep(std::time::Duration::from_millis(20)); // sync-ok: test code only
    }
    assert_eq!(f.h.comp.get_panel_kind(), 4);
    // The text file opens in the panel, in place of the list.
    f.h.comp.invoke_panel_convo_open(real_s.as_str().into());
    f.pump_until("the file", |f| f.h.comp.get_panel_kind() == 2);
    assert_eq!(f.h.comp.get_panel_title().as_str(), "notes.txt");
    assert_eq!(
        trackers(&f),
        0,
        "the Changes panel is gone, so is its worker"
    );
    assert_eq!(f.h.comp.get_panel_convo_files().row_count(), 0);
}

// ---- the menu ----

#[test]
fn the_terminal_menu_offers_the_two_panels_only_on_a_claude_tab() {
    let f = Fx::new();
    let _claude = Claude::new(&f);
    let plain = f.tabs(&["C:\\plain"]).remove(0);
    assert!(
        f.ctl.borrow().convo_menu_rows(&plain).is_empty(),
        "a tab with no agent gets no rows"
    );
    let id = claude_tab(&f, SESSION_A);
    let labels = |f: &Fx, id: &str| -> Vec<String> {
        f.ctl
            .borrow()
            .convo_menu_rows(id)
            .iter()
            .map(|r| r.0.label.clone())
            .collect()
    };
    assert_eq!(labels(&f, &id), ["Show changes", "Show tasks"]);
    open(&f, &id, PanelKind::Changes);
    assert_eq!(labels(&f, &id), ["Hide changes", "Show tasks"]);
    // Picking a row does what it says.
    assert!(f.ctl.borrow_mut().convo_menu_picked(&id, "Show tasks"));
    f.pump();
    assert_eq!(f.h.comp.get_panel_kind(), 5);
    assert!(f.ctl.borrow_mut().convo_menu_picked(&id, "Hide tasks"));
    f.pump();
    assert!(!f.h.comp.get_panel_open());
    assert!(!f.ctl.borrow_mut().convo_menu_picked(&id, "Paste"));
}

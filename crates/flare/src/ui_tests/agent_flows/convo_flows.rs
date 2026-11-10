//! The conversation panels through the real window controller (Surfaces stage 6): a fake Claude folder with synthetic
//! transcripts, the hook line a Claude session sends (`OSC 777 ... session=`), and what the window shows.
//!
//! Held here: nothing runs for a tab with no Changes panel open; an open panel starts one tracker on the tab's session
//! file and lists what it reads, and the list follows lines appended to the file; a new session switches the file;
//! closing the panel (or the tab) stops the tracker; `convo.state` answers the tab's own summary; a stored panel
//! reopens empty until the transcript loads, and a stored panel of the removed Tasks kind is dropped; a clicked path
//! shows the file in the panel.

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

/// A TodoWrite call: the Tasks panel that showed it was removed (2026-10-26), so the reader ignores it.
fn todo_line() -> String {
    json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "todo",
        "name": "TodoWrite", "input": {"todos": [
            {"content": "Read", "status": "in_progress", "activeForm": "Reading"}]}}]}})
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
    // Switching the panel to a file panel stops it.
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
    claude.append(TAB, SESSION_A, &[edit_line("e1", "C:\\a\\x.rs", "a", "b")]);
    open(&f, &id, PanelKind::Changes);
    f.pump_until("the file", |f| files(f).len() == 1);
    assert_eq!(trackers(&f), 1);
    let exited = f.ctl.borrow().convo().exit_flag(&id).expect("a tracker");
    assert!(!exited.load(std::sync::atomic::Ordering::SeqCst));
    f.ctl.borrow_mut().close_now(&id);
    f.pump();
    assert_eq!(trackers(&f), 0);
    // The map is empty, and the worker thread really returned.
    let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !exited.load(std::sync::atomic::Ordering::SeqCst) && std::time::Instant::now() < end {
        std::thread::sleep(std::time::Duration::from_millis(20)); // sync-ok: test code only
    }
    assert!(
        exited.load(std::sync::atomic::Ordering::SeqCst),
        "the worker of a closed tab stops"
    );
}

#[test]
fn an_edit_of_another_file_leaves_the_selected_diff_rows_alone() {
    let f = Fx::new();
    let claude = Claude::new(&f);
    let id = claude_tab(&f, SESSION_A);
    claude.append(
        TAB,
        SESSION_A,
        &[edit_line("e1", "C:\\a\\x.rs", "one", "two")],
    );
    open(&f, &id, PanelKind::Changes);
    f.pump_until("the file", |f| files(f).len() == 1);
    f.h.comp.invoke_panel_convo_select("C:\\a\\x.rs".into());
    f.pump_until("the diff", |f| diff_text(f).iter().any(|t| t == "two"));
    // Mark the model the window holds: a rebuild would replace it, and the mark with it.
    let rows = f.h.comp.get_panel_convo_rows();
    let mark = rows
        .as_any()
        .downcast_ref::<slint::VecModel<crate::ConvoDiffRow>>()
        .expect("the controller's own model");
    let before = mark.row_count();
    mark.push(crate::ConvoDiffRow {
        kind: 1,
        old_no: "".into(),
        new_no: "".into(),
        text: "mark".into(),
    });
    let (files_v, diff_v) = f.ctl.borrow().convo().piece_versions(&id).unwrap();
    claude.append(
        TAB,
        SESSION_A,
        &[edit_line("e2", "C:\\a\\y.rs", "p", "q"), todo_line()],
    );
    f.pump_until("the second file", |f| files(f).len() == 2);
    let (files_after, diff_after) = f.ctl.borrow().convo().piece_versions(&id).unwrap();
    assert_eq!(diff_v, diff_after, "the picked file's diff did not change");
    assert!(files_after > files_v, "the file list did");
    assert_eq!(
        f.h.comp.get_panel_convo_rows().row_count(),
        before + 1,
        "the rows model is the same instance, not a rebuilt one"
    );
    assert!(diff_text(&f).iter().any(|t| t == "mark"));
}

#[test]
fn a_path_answer_that_arrives_after_the_panel_changed_is_dropped() {
    let f = Fx::new();
    let claude = Claude::new(&f);
    let id = claude_tab(&f, SESSION_A);
    let work = f._dir.path().join("work");
    std::fs::create_dir_all(&work).unwrap(); // sync-ok: test code only
    let real: PathBuf = work.join("notes.txt");
    std::fs::write(&real, "kept").unwrap(); // sync-ok: test code only
    let real_s = real.to_string_lossy().into_owned();
    claude.append(TAB, SESSION_A, &[edit_line("e1", &real_s, "a", "b")]);
    open(&f, &id, PanelKind::Changes);
    f.pump_until("the file", |f| files(f).len() == 1);
    f.h.comp.invoke_panel_convo_select(real_s.as_str().into());
    f.pump();
    // The click starts the check on a worker; the person opens another file before the answer is read.
    let other = work.join("other.txt");
    std::fs::write(&other, "other").unwrap(); // sync-ok: test code only
    f.h.comp.invoke_panel_convo_open(real_s.as_str().into());
    f.ctl
        .borrow_mut()
        .show_preview(&id, PanelKind::Text, &other.to_string_lossy());
    for _ in 0..15 {
        f.pump();
        std::thread::sleep(std::time::Duration::from_millis(20)); // sync-ok: test code only
    }
    assert_eq!(f.h.comp.get_panel_kind(), 2);
    assert_eq!(
        f.h.comp.get_panel_title().as_str(),
        "other.txt",
        "the late answer changed nothing"
    );
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
            todo_line(),
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
    // The Tasks panel was removed (2026-10-26): the answer has no task counts.
    assert!(v.get("tasks").is_none() && v.get("active").is_none(), "{v}");
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
            kind: PanelKind::Changes,
            target: "changes".into(),
            open: true,
            width: Some(0.35),
            by_agent: false,
            view: None,
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
    f.pump_until("the stored panel", |f| f.h.comp.get_panel_kind() == 4);
    assert_eq!(f.h.comp.get_panel_title().as_str(), "Changes");
    assert!(
        f.h.comp
            .get_panel_note()
            .as_str()
            .starts_with("Claude is not running")
    );
    assert_eq!(
        f.h.comp.get_panel_convo_files().row_count(),
        0,
        "empty at first"
    );
    assert!((f.h.comp.get_panel_share() - 0.35).abs() < 1e-6);
    // The agent speaks, the session is known, and the changes load.
    claude.append(TAB, SESSION_A, &[edit_line("e1", "C:\\a\\x.rs", "a", "b")]);
    // The restored tab's shell is made on a worker, so it can still be missing here on a busy runner:
    // wait for it before it speaks (the restore being done does not mean the spawn has landed).
    f.pump_until("the restored shell", |f| {
        f.sp.made().iter().any(|m| m.spec().cwd == TAB)
    });
    f.say(TAB, &hook(SESSION_A));
    f.pump_until("the changes", |f| files(f).len() == 1);
    assert_eq!(trackers(&f), 1);
}

#[test]
fn a_stored_panel_of_the_removed_tasks_kind_is_dropped_and_the_tab_restores() {
    // The Tasks panel was removed (owner, 2026-10-26). A panels.json an older build wrote still names it: the restore
    // reads it without error, the tab comes back with no panel, and nothing reads its transcript.
    let f = Fx::with(
        Opts {
            saved: vec![(TAB, None)],
            ..Opts::default()
        },
        |_| {},
    );
    let file = f._dir.path().join("panels.json");
    let raw = r#"{"version":1,"tabs":[{"cwd":"C:\\a","panel":{"kind":"tasks","target":"tasks","open":true,"width":0.35}}]}"#;
    std::fs::write(&file, raw).unwrap(); // sync-ok: test code only
    f.ctl
        .borrow_mut()
        .set_panel_store(Arc::new(flare_surfaces::store::PanelStore::new(
            &file,
            Duration::from_millis(30),
        )));
    let _claude = Claude::new(&f);
    AppCtl::start_restore(&f.ctl);
    f.pump_until("the restore", |f| f.ctl.borrow().model().restore_done());
    f.pump();
    assert_eq!(f.ids().len(), 1, "the tab is restored");
    assert_eq!(f.ctl.borrow().open_panels(), 0, "with no panel");
    assert!(!f.h.comp.get_panel_open());
    assert_eq!(f.h.comp.get_panel_kind(), 0);
    // The restored tab's shell is made on a worker: wait for it before it speaks.
    f.pump_until("the restored shell", |f| {
        f.sp.made().iter().any(|m| m.spec().cwd == TAB)
    });
    f.say(TAB, &hook(SESSION_A));
    f.pump_until("the session", |f| {
        f.ctl.borrow().hub().claude_session(&f.ids()[0]) == Some(SESSION_A)
    });
    assert_eq!(trackers(&f), 0, "no panel, no tracker");
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
            by_agent: false,
            view: None,
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
fn the_terminal_menu_offers_the_changed_files_only_on_a_claude_tab() {
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
    // A tab whose rows show is Claude by definition (a session or the detection), so the wording never varies.
    let (show_c, hide_c) = ("Show files Claude changed", "Hide files Claude changed");
    assert_eq!(labels(&f, &id), [show_c]);
    // Its own group (a rule above it), with its own fitting glyph.
    let rows = f.ctl.borrow().convo_menu_rows(&id);
    assert!(rows[0].0.group);
    assert_eq!(rows[0].0.icon, crate::menus_ui::GLYPH_CHANGES);
    open(&f, &id, PanelKind::Changes);
    assert_eq!(labels(&f, &id), [hide_c]);
    // Picking the row does what it says.
    assert!(
        f.ctl
            .borrow_mut()
            .convo_menu_picked(&id, "Hide files Claude changed")
    );
    f.pump();
    assert!(!f.h.comp.get_panel_open());
    assert!(
        f.ctl
            .borrow_mut()
            .convo_menu_picked(&id, "Show files Claude changed")
    );
    f.pump();
    assert_eq!(f.h.comp.get_panel_kind(), 4);
    assert!(
        f.ctl
            .borrow_mut()
            .convo_menu_picked(&id, "Hide files Claude changed")
    );
    f.pump();
    assert!(!f.h.comp.get_panel_open());
    // The old words are gone (including the unnamed "changed files"), and no row names the agent board or the
    // task list (removed 2026-10-26).
    assert!(
        !f.ctl
            .borrow_mut()
            .convo_menu_picked(&id, "Show changed files")
    );
    assert!(!f.ctl.borrow_mut().convo_menu_picked(&id, "Show changes"));
    assert!(
        !f.ctl
            .borrow_mut()
            .convo_menu_picked(&id, "Show agent board")
    );
    assert!(!f.ctl.borrow_mut().convo_menu_picked(&id, "Show task list"));
    assert!(!f.ctl.borrow_mut().convo_menu_picked(&id, "Hide task list"));
    assert!(!f.ctl.borrow_mut().convo_menu_picked(&id, "Paste"));
}

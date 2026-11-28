//! The tab menu's "Claude sessions" rows (rules 1380 to 1382) in the real window controller: fake shells, the agent's
//! title and hook signals as Claude prints them, the menu picked through `menu_picked`, the busy question answered
//! through the dialog's own callbacks, and the keys read back from what each fake shell was sent.

use slint::Model;

use super::*;
use crate::claude_sessions::{END_TAB, ENTER_AFTER_MS, GROUP, INTERRUPT_SETTLE_MS, START_TAB};

const SID: &str = "5a1d0c2e-1111-4222-8333-94445555a666";

fn written(f: &Fx, cwd: &str) -> String {
    String::from_utf8_lossy(&f.made_with(cwd)[0].written()).into_owned()
}

fn id_of(f: &Fx, cwd: &str) -> String {
    let i = f.cwds().iter().position(|c| c == cwd).expect("a tab there");
    f.ids()[i].clone()
}

/// The shell of `cwd` printed its prompt (Flare's prompt hook reports the folder).
fn prompt(f: &Fx, cwd: &str) {
    let before = f.ctl.borrow().hub.prompts_seen(&id_of(f, cwd));
    f.say(cwd, &format!("\x1b]9;9;{cwd}\x07"));
    f.pump_until("the prompt report", |f| {
        f.ctl.borrow().hub.prompts_seen(&id_of(f, cwd)) > before
    });
}

/// A plain shell at its prompt the poll has looked at: no agent.
fn plain_shell(f: &Fx, cwd: &str) -> String {
    let id = id_of(f, cwd);
    f.ctl.borrow().hub.queue_poll_answer(&id, false, None);
    f.ctl.borrow_mut().host_wake();
    prompt(f, cwd);
    assert!(f.ctl.borrow().hub.polled(&id));
    id
}

/// Claude draws its prompt box holding `text`, as it does at rest and after each key.
fn boxed(f: &Fx, cwd: &str, text: &str) {
    let id = id_of(f, cwd);
    let rule = "\u{2500}".repeat(40);
    f.say(
        cwd,
        &format!("\r\n{rule}\r\n\u{276f} {text}\r\n{rule}\r\n  auto mode on"),
    );
    let want = text.to_string();
    f.pump_until("the box", |f| {
        let c = f.ctl.borrow();
        let h = c.handle_of(&id).expect("a handle");
        flare_agent::question::prompt_text(&h.typed_tail(flare_agent::indicator::SCREEN_TAIL_ROWS))
            == Some(want.clone())
    });
}

/// Claude idle in `cwd`, its session named by its hooks, its prompt box empty.
fn idle_claude(f: &Fx, cwd: &str) -> String {
    let id = f.claude_in(cwd);
    boxed(f, cwd, "");
    f.say(
        cwd,
        &format!("\x1b]777;flare-agent;state=done;session={SID}\x07"),
    );
    f.pump_until("the session", |f| {
        f.ctl.borrow().hub.last_claude_session(&id) == Some(SID)
    });
    f.pump_until("idle", |f| !f.ctl.borrow().hub.working(&id));
    id
}

fn working(f: &Fx, cwd: &str) {
    let id = id_of(f, cwd);
    f.say(cwd, OSC_WORKING);
    f.pump_until("working", |f| f.ctl.borrow().hub.working(&id));
}

fn rests(f: &Fx, cwd: &str) {
    let id = id_of(f, cwd);
    f.say(cwd, OSC_DONE);
    f.pump_until("at rest", |f| !f.ctl.borrow().hub.working(&id));
}

/// The labels of `tab`'s menu.
fn menu_labels(f: &Fx, tab: &str) -> Vec<String> {
    f.ctl.borrow_mut().open_tab_menu(tab, 10.0, 10.0);
    let rows = f.h.comp.get_menu_rows();
    let labels = (0..rows.row_count())
        .map(|i| rows.row_data(i).unwrap().label.to_string())
        .collect();
    f.ctl.borrow_mut().close_menu();
    labels
}

/// Opens `tab`'s menu and picks the row that starts with `label`.
fn pick(f: &Fx, tab: &str, label: &str) {
    f.ctl.borrow_mut().open_tab_menu(tab, 10.0, 10.0);
    let rows = f.h.comp.get_menu_rows();
    let i = (0..rows.row_count())
        .find(|&i| rows.row_data(i).unwrap().label.starts_with(label))
        .unwrap_or_else(|| panic!("no row {label}"));
    f.ctl.borrow_mut().menu_picked(i32::try_from(i).unwrap());
}

/// Lets the clock run `ms` and the controller's timer body take it.
fn later(f: &Fx, ms: u64) {
    f.clock.set(f.clock.get() + ms);
    f.ctl.borrow_mut().tick();
    f.pump();
}

/// Rule 1380: this tab's row and the flyout's counts follow the tabs; nothing shows with nothing to act on.
#[test]
fn the_rows_follow_the_tabs_and_show_their_counts() {
    let f = fx();
    f.open_tabs(&["C:\\a", "C:\\b", "C:\\c"]);
    let c = plain_shell(&f, "C:\\c");
    let a = idle_claude(&f, "C:\\a");
    let labels = menu_labels(&f, &a);
    assert!(labels.iter().any(|l| l == END_TAB), "{labels:?}");
    assert!(labels.iter().any(|l| l == GROUP));
    assert!(labels.iter().any(|l| l == "End all Claude sessions (1)"));
    assert!(
        !labels.iter().any(|l| l.starts_with("Restart")),
        "{labels:?}"
    );
    assert!(labels.iter().any(|l| l == "Start Claude in all tabs (1)"));
    let labels = menu_labels(&f, &c);
    assert!(labels.iter().any(|l| l == START_TAB), "{labels:?}");
    assert!(!labels.iter().any(|l| l == END_TAB));
    // b has no prompt report yet and the poll never looked: neither row, and it is not counted.
    let b = id_of(&f, "C:\\b");
    let labels = menu_labels(&f, &b);
    assert!(!labels.iter().any(|l| l == START_TAB || l == END_TAB));
    assert_eq!(labels[0], "Close tab", "the tab's own rows stay first");
}

/// Rule 1380: with no Claude and no free shell, the flyout is not there at all.
#[test]
fn nothing_to_act_on_shows_no_row() {
    let f = fx();
    f.open_tabs(&["C:\\a"]);
    let a = id_of(&f, "C:\\a");
    let labels = menu_labels(&f, &a);
    assert!(
        !labels
            .iter()
            .any(|l| l == GROUP || l == START_TAB || l == END_TAB),
        "{labels:?}"
    );
}

/// Rule 1381: End all types "/exit" in each idle Claude, the Enter 300 ms later, and every tab stays open.
#[test]
fn end_all_types_exit_then_enter_and_keeps_every_tab() {
    let f = fx();
    f.open_tabs(&["C:\\a", "C:\\b", "C:\\c"]);
    let c = plain_shell(&f, "C:\\c");
    let a = idle_claude(&f, "C:\\a");
    let b = idle_claude(&f, "C:\\b");
    pick(&f, &a, "End all Claude sessions");
    assert!(!f.h.comp.get_dialog_open(), "no question: nobody works");
    f.pump_until("the exits", |f| {
        written(f, "C:\\a") == "/exit" && written(f, "C:\\b") == "/exit"
    });
    boxed(&f, "C:\\a", "/exit");
    boxed(&f, "C:\\b", "/exit");
    later(&f, ENTER_AFTER_MS);
    f.pump_until("the Enters", |f| {
        written(f, "C:\\a") == "/exit\r" && written(f, "C:\\b") == "/exit\r"
    });
    assert_eq!(written(&f, "C:\\c"), "", "the plain shell gets nothing");
    prompt(&f, "C:\\a");
    prompt(&f, "C:\\b");
    assert!(f.ctl.borrow().claude_running().is_empty(), "both done");
    assert_eq!(f.ids(), [a, b, c], "no tab closed");
}

/// Rule 1382: a busy Claude asks first; Cancel does nothing at all, the idle tab included.
#[test]
fn a_busy_claude_asks_and_cancel_does_nothing() {
    let f = fx();
    f.open_tabs(&["C:\\a", "C:\\b"]);
    let a = idle_claude(&f, "C:\\a");
    idle_claude(&f, "C:\\b");
    working(&f, "C:\\b");
    pick(&f, &a, "End all Claude sessions");
    assert!(f.h.comp.get_dialog_open());
    assert_eq!(f.h.comp.get_dialog_title().as_str(), "1 session is working");
    assert_eq!(f.h.comp.get_dialog_alt_label().as_str(), "Wait");
    assert_eq!(f.h.comp.get_dialog_primary().as_str(), "End now");
    f.h.comp.invoke_dialog_cancel();
    assert!(!f.h.comp.get_dialog_open());
    assert_eq!(
        f.h.comp.get_dialog_alt_label().as_str(),
        "",
        "the next question has two buttons"
    );
    later(&f, 1000);
    assert_eq!(written(&f, "C:\\a"), "");
    assert_eq!(written(&f, "C:\\b"), "");
}

/// Rule 1382: Wait ends the idle one now and the busy one once it rests, with no key before that.
#[test]
fn wait_ends_the_busy_claude_once_it_rests() {
    let f = fx();
    f.open_tabs(&["C:\\a", "C:\\b"]);
    let a = idle_claude(&f, "C:\\a");
    idle_claude(&f, "C:\\b");
    working(&f, "C:\\b");
    pick(&f, &a, "End all Claude sessions");
    f.h.comp.invoke_dialog_alt();
    assert!(!f.h.comp.get_dialog_open());
    f.pump_until("a's exit", |f| written(f, "C:\\a") == "/exit");
    later(&f, 120_000);
    assert_eq!(written(&f, "C:\\b"), "", "nothing while it works");
    rests(&f, "C:\\b");
    f.pump_until("b's exit", |f| written(f, "C:\\b") == "/exit");
    // The exit came from the terminal's drain, not the timer: the Enter still needs the timer running.
    assert!(f.ctl.borrow().timer_on, "the Enter has a timer");
    boxed(&f, "C:\\b", "/exit");
    later(&f, ENTER_AFTER_MS);
    f.pump_until("b's Enter", |f| written(f, "C:\\b") == "/exit\r");
}

/// Rule 1382: End now sends Escape to the busy Claude first, then ends it once it rests.
#[test]
fn end_now_interrupts_then_ends() {
    let f = fx();
    f.open_tabs(&["C:\\a"]);
    let a = idle_claude(&f, "C:\\a");
    working(&f, "C:\\a");
    pick(&f, &a, END_TAB);
    assert_eq!(f.h.comp.get_dialog_title().as_str(), "1 session is working");
    f.h.comp.invoke_dialog_confirm();
    f.pump_until("the Escape", |f| written(f, "C:\\a") == "\x1b");
    rests(&f, "C:\\a");
    later(&f, INTERRUPT_SETTLE_MS);
    f.pump_until("the exit", |f| written(f, "C:\\a") == "\x1b/exit");
    boxed(&f, "C:\\a", "/exit");
    later(&f, ENTER_AFTER_MS);
    f.pump_until("the Enter", |f| written(f, "C:\\a") == "\x1b/exit\r");
}

/// Rule 1381: Start Claude types the line in the free shell, resuming the session it last had.
#[test]
fn start_types_the_resume_line_of_the_last_session() {
    let f = fx();
    f.open_tabs(&["C:\\a", "C:\\b"]);
    let a = idle_claude(&f, "C:\\a");
    let b = plain_shell(&f, "C:\\b");
    // a's Claude ends: the poll no longer finds it and its shell is back at the prompt.
    f.agent_exits(&a);
    f.say("C:\\a", "\x1b]0;\x07");
    prompt(&f, "C:\\a");
    f.pump_until("a free", |f| !f.ctl.borrow().hub.agent_here(&a));
    pick(&f, &a, "Start Claude in all tabs (2)");
    f.pump_until("the lines", |f| {
        written(f, "C:\\a") == format!("claude --resume {SID}\r")
            && written(f, "C:\\b") == "claude\r"
    });
    // Typed once: the prompt is no longer fresh, so a second pick finds nothing to start.
    f.ctl.borrow_mut().host_wake();
    let labels = menu_labels(&f, &b);
    assert!(
        !labels.iter().any(|l| l.starts_with("Start Claude")),
        "{labels:?}"
    );
}

/// Rule 1381: a draft in Claude's prompt box keeps the tab: nothing is typed into it.
#[test]
fn a_draft_is_never_sent() {
    let f = fx();
    f.open_tabs(&["C:\\a"]);
    let a = idle_claude(&f, "C:\\a");
    // Claude's prompt box with unsent words, as it draws it.
    boxed(&f, "C:\\a", "half a thought");
    assert!(f.ctl.borrow().claude_facts(&a).draft());
    pick(&f, &a, END_TAB);
    later(&f, ENTER_AFTER_MS);
    assert_eq!(written(&f, "C:\\a"), "");
    assert!(f.ctl.borrow().claude_running().is_empty());
}

/// Review: the shell reported its prompt after the last key (Claude quit, its mark not yet gone): "/exit" is never
/// typed into the shell.
#[test]
fn a_shell_back_at_its_prompt_gets_no_exit() {
    let f = fx();
    f.open_tabs(&["C:\\a"]);
    let a = idle_claude(&f, "C:\\a");
    prompt(&f, "C:\\a");
    assert!(
        f.ctl.borrow().claude_facts(&a).claude,
        "the mark is still there"
    );
    pick(&f, &a, END_TAB);
    later(&f, 1000);
    assert_eq!(written(&f, "C:\\a"), "");
    assert!(f.ctl.borrow().claude_running().is_empty());
}

/// Review: no prompt box on the screen (a picker or menu of Claude's): nothing is typed.
#[test]
fn no_prompt_box_gets_no_exit() {
    let f = fx();
    f.open_tabs(&["C:\\a"]);
    let a = idle_claude(&f, "C:\\a");
    // A picker fills the screen's tail: the box is no longer among the rows read.
    let picker: String = (1..=20).map(|i| format!("\r\n  {i}. a model")).collect();
    f.say("C:\\a", &format!("\r\n  Select model{picker}"));
    f.pump_until("the box gone", |f| {
        f.ctl.borrow().claude_facts(&a).prompt_box.is_none()
    });
    pick(&f, &a, END_TAB);
    later(&f, 1000);
    assert_eq!(written(&f, "C:\\a"), "");
}

/// Review: the Enter waits for "/exit" to show in the box; with other words there it is never pressed.
#[test]
fn the_enter_waits_for_the_echo() {
    let f = fx();
    f.open_tabs(&["C:\\a"]);
    let a = idle_claude(&f, "C:\\a");
    pick(&f, &a, END_TAB);
    f.pump_until("the exit", |f| written(f, "C:\\a") == "/exit");
    later(&f, ENTER_AFTER_MS);
    assert_eq!(written(&f, "C:\\a"), "/exit", "no echo yet: no Enter");
    boxed(&f, "C:\\a", "hi/exit");
    later(&f, crate::claude_sessions::ECHO_WAIT_MS);
    assert_eq!(written(&f, "C:\\a"), "/exit", "never sends other words");
    assert!(f.ctl.borrow().claude_running().is_empty(), "given up");
}

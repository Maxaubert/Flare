//! "Ask agent" (#33) inside the window controller, with a fake shell and a fake browser: what reaches the prompt, that
//! nothing ever submits it, and every way it is refused.

use flare_web::WebEvent;
use flare_web::fake::{Call, FakeWeb};

use super::*;
use crate::ask_agent::{NOTE_NO_AGENT, NOTE_NO_SELECTION, NOTE_PAGE_FAILED, NOTE_QUESTION_UP};
use crate::ctl_app::{CtlOp, CtlRequest, CtlRequestBox};

const PAGE: &str = "https://example.com/article";
const FRAMED: &str = "About this from example.com:\n> line one\n> line two";

type Slot = Rc<RefCell<Option<FakeWeb>>>;

fn with_fake(f: &Fx) -> Slot {
    let slot: Slot = Rc::default();
    let keep = slot.clone();
    f.ctl.borrow_mut().set_web_factory(Box::new(move |sink| {
        let fake = FakeWeb::new(sink);
        *keep.borrow_mut() = Some(fake.clone());
        Box::new(fake)
    }));
    slot
}

fn fake(slot: &Slot) -> FakeWeb {
    slot.borrow().clone().expect("the browser was made")
}

fn load(web: &FakeWeb, tab: &str, url: &str, nav: u64) {
    web.say(WebEvent::Loading {
        tab: tab.into(),
        url: url.into(),
        nav,
    });
    web.say(WebEvent::Loaded {
        tab: tab.into(),
        url: url.into(),
        ok: true,
        status: "0".into(),
        nav,
    });
}

/// A Claude tab in front.
fn claude(f: &Fx) -> String {
    let id = f.ctl.borrow().front_id().unwrap();
    let cwd = f.cwds()[0].clone();
    f.say(&cwd, TITLE_IDLE);
    f.pump_until("the agent to be here", |f| f.agent_here(&id));
    id
}

/// A web panel on `tab` with its page loaded.
fn page(f: &Fx, slot: &Slot, tab: &str) {
    let (tx, _rx) = std::sync::mpsc::channel();
    f.ctl
        .borrow_mut()
        .on_msg(UiMsg::Ctl(CtlRequestBox(CtlRequest {
            op: CtlOp::BrowserOpen {
                tab: tab.into(),
                url: PAGE.into(),
            },
            deadline: Instant::now() + Duration::from_secs(5),
            reply: tx,
        })));
    load(&fake(slot), tab, PAGE, 1);
    f.pump();
    f.ctl.borrow_mut().refresh();
    f.pump();
}

/// The id of the last script the page was asked to run, and the script.
fn last_script(slot: &Slot) -> Option<(u64, String)> {
    fake(slot).calls().into_iter().rev().find_map(|c| match c {
        Call::Script(_, id, js) => Some((id, js)),
        _ => None,
    })
}

fn scripts(slot: &Slot) -> usize {
    fake(slot)
        .calls()
        .into_iter()
        .filter(|c| matches!(c, Call::Script(..)))
        .count()
}

fn ask(f: &Fx) {
    f.ctl.borrow_mut().panel_ask_agent();
    f.pump();
}

/// The loaded page answers the script `id` with `selection`, in the script's own shape.
fn answer(f: &Fx, slot: &Slot, id: u64, selection: &str) {
    fake(slot).say(WebEvent::Script {
        id,
        result: Ok(serde_json::to_string(&format!("{PAGE}\n{selection}")).unwrap()),
    });
    f.pump();
}

fn note(f: &Fx) -> String {
    f.h.comp.get_ask_note().to_string()
}

#[test]
fn a_page_selection_is_pasted_bracketed_and_nothing_follows_it() {
    let (f, h, shell) = live(Opts::default());
    let slot = with_fake(&f);
    let tab = claude(&f);
    feed(&h, "\x1b[?2004h");
    page(&f, &slot, &tab);
    ask(&f);
    let (id, js) = last_script(&slot).expect("the page was asked");
    assert!(js.contains("window.getSelection().toString") || js.contains("getSelection()"));
    assert!(shell.written().is_empty(), "nothing before the answer");
    answer(&f, &slot, id, "line one\nline two");
    let want = format!("\x1b[200~{}\x1b[201~", FRAMED.replace('\n', "\r"));
    until_written(&shell, |w| w == want.as_bytes());
    std::thread::sleep(Duration::from_millis(60)); // sync-ok: test code only
    f.pump();
    assert_eq!(
        shell.written(),
        want.as_bytes(),
        "no Enter, no second paste"
    );
    assert_eq!(note(&f), "");
}

#[test]
fn without_bracketed_paste_the_framing_goes_as_one_line_with_no_cr_at_all() {
    let (f, _h, shell) = live(Opts::default());
    let slot = with_fake(&f);
    let tab = claude(&f);
    page(&f, &slot, &tab);
    ask(&f);
    let (id, _) = last_script(&slot).unwrap();
    answer(&f, &slot, id, "line one\nline two");
    until_written(&shell, |w| !w.is_empty());
    let written = shell.written();
    assert!(
        !written.contains(&b'\r') && !written.contains(&b'\n'),
        "{written:?}"
    );
    assert_eq!(
        written,
        b"About this from example.com: > line one > line two"
    );
}

#[test]
fn a_hostile_selection_cannot_end_the_paste_or_press_enter() {
    let (f, h, shell) = live(Opts::default());
    let slot = with_fake(&f);
    let tab = claude(&f);
    feed(&h, "\x1b[?2004h");
    page(&f, &slot, &tab);
    ask(&f);
    let (id, _) = last_script(&slot).unwrap();
    answer(&f, &slot, id, "a\x1b[201~\r\x03b\r\n");
    until_written(&shell, |w| !w.is_empty());
    assert_eq!(
        shell.written(),
        b"\x1b[200~About this from example.com:\r> a[201~\r> b\x1b[201~"
    );
}

#[test]
fn no_selection_on_the_page_says_so_and_types_nothing() {
    let (f, _h, shell) = live(Opts::default());
    let slot = with_fake(&f);
    let tab = claude(&f);
    page(&f, &slot, &tab);
    ask(&f);
    let (id, _) = last_script(&slot).unwrap();
    answer(&f, &slot, id, "");
    assert_eq!(note(&f), NOTE_NO_SELECTION);
    assert!(shell.written().is_empty());
}

#[test]
fn a_page_that_answers_with_anything_but_a_string_counts_as_no_selection() {
    let (f, _h, shell) = live(Opts::default());
    let slot = with_fake(&f);
    let tab = claude(&f);
    page(&f, &slot, &tab);
    ask(&f);
    let (id, _) = last_script(&slot).unwrap();
    fake(&slot).say(WebEvent::Script {
        id,
        result: Ok("null".into()),
    });
    f.pump();
    assert_eq!(note(&f), NOTE_NO_SELECTION);
    assert!(shell.written().is_empty());
}

#[test]
fn a_script_that_failed_says_so_and_types_nothing() {
    let (f, _h, shell) = live(Opts::default());
    let slot = with_fake(&f);
    let tab = claude(&f);
    page(&f, &slot, &tab);
    ask(&f);
    let (id, _) = last_script(&slot).unwrap();
    fake(&slot).say(WebEvent::Script {
        id,
        result: Err("gone".into()),
    });
    f.pump();
    assert_eq!(note(&f), NOTE_PAGE_FAILED);
    assert!(shell.written().is_empty());
}

#[test]
fn an_answer_from_before_a_navigation_is_dropped() {
    let (f, _h, shell) = live(Opts::default());
    let slot = with_fake(&f);
    let tab = claude(&f);
    page(&f, &slot, &tab);
    ask(&f);
    let (id, _) = last_script(&slot).unwrap();
    load(&fake(&slot), &tab, "https://other.example/", 2);
    f.pump();
    answer(&f, &slot, id, "from the old page");
    std::thread::sleep(Duration::from_millis(40)); // sync-ok: test code only
    f.pump();
    assert!(shell.written().is_empty());
    assert_eq!(note(&f), "", "dropped without a word");
}

#[test]
fn a_click_while_the_next_page_loads_names_the_page_the_text_came_from() {
    let (f, _h, shell) = live(Opts::default());
    let slot = with_fake(&f);
    let tab = claude(&f);
    page(&f, &slot, &tab);
    // The navigation has started and not committed: the old page is what shows, and what the script runs in.
    fake(&slot).say(WebEvent::Loading {
        tab: tab.clone(),
        url: "https://bank.example/".into(),
        nav: 2,
    });
    f.pump();
    ask(&f);
    let (id, _) = last_script(&slot).unwrap();
    answer(&f, &slot, id, "the old page's words");
    until_written(&shell, |w| !w.is_empty());
    assert_eq!(
        shell.written(),
        b"About this from example.com: > the old page's words"
    );
}

#[test]
fn an_answer_longer_than_the_script_could_give_is_refused_and_types_nothing() {
    let (f, _h, shell) = live(Opts::default());
    let slot = with_fake(&f);
    let tab = claude(&f);
    page(&f, &slot, &tab);
    ask(&f);
    let (id, _) = last_script(&slot).unwrap();
    // A page that replaced `String.prototype.slice` is not held by the script's own cut.
    answer(&f, &slot, id, &"x".repeat(crate::ask_agent::ANSWER_MAX + 1));
    assert_eq!(note(&f), NOTE_PAGE_FAILED);
    assert!(shell.written().is_empty());
}

#[test]
fn an_answer_for_a_panel_closed_since_the_click_is_dropped() {
    let (f, _h, shell) = live(Opts::default());
    let slot = with_fake(&f);
    let tab = claude(&f);
    page(&f, &slot, &tab);
    ask(&f);
    let (id, _) = last_script(&slot).unwrap();
    f.ctl.borrow_mut().web.close_tab(&tab);
    answer(&f, &slot, id, "late");
    std::thread::sleep(Duration::from_millis(40)); // sync-ok: test code only
    f.pump();
    assert!(shell.written().is_empty());
}

#[test]
fn two_clicks_before_the_answer_ask_once_and_type_once() {
    let (f, h, shell) = live(Opts::default());
    let slot = with_fake(&f);
    let tab = claude(&f);
    feed(&h, "\x1b[?2004h");
    page(&f, &slot, &tab);
    ask(&f);
    ask(&f);
    assert_eq!(scripts(&slot), 1);
    let (id, _) = last_script(&slot).unwrap();
    answer(&f, &slot, id, "once");
    until_written(&shell, |w| !w.is_empty());
    std::thread::sleep(Duration::from_millis(40)); // sync-ok: test code only
    f.pump();
    assert_eq!(shell.written().iter().filter(|b| **b == 0x1b).count(), 2);
}

#[test]
fn a_tab_with_no_agent_refuses_before_the_page_is_asked() {
    let (f, _h, shell) = live(Opts::default());
    let slot = with_fake(&f);
    let tab = f.ctl.borrow().front_id().unwrap();
    page(&f, &slot, &tab);
    ask(&f);
    assert_eq!(scripts(&slot), 0);
    assert_eq!(note(&f), NOTE_NO_AGENT);
    assert!(shell.written().is_empty());
}

#[test]
fn an_agent_gone_by_the_time_the_page_answers_is_refused() {
    let (f, _h, shell) = live(Opts::default());
    let slot = with_fake(&f);
    let tab = claude(&f);
    page(&f, &slot, &tab);
    ask(&f);
    f.agent_exits(&tab);
    f.pump_until("the agent to leave", |f| !f.agent_here(&tab));
    // The agent's own panel closes with it, so the browser's answer is dropped there; the gate at delivery stands
    // behind that for a panel the person opened.
    f.ctl
        .borrow_mut()
        .ask_selection_done(&tab, "example.com", Ok("too late".to_string()));
    assert_eq!(note(&f), NOTE_NO_AGENT);
    assert!(shell.written().is_empty());
}

#[test]
fn a_question_box_on_screen_refuses_so_a_paste_cannot_answer_it() {
    let (f, h, shell) = live(Opts::default());
    let slot = with_fake(&f);
    let tab = claude(&f);
    page(&f, &slot, &tab);
    feed(
        &h,
        "Do you want to proceed?\r\n\u{276f} 1. Yes\r\n  2. No\r\nEsc to cancel",
    );
    ask(&f);
    assert_eq!(note(&f), NOTE_QUESTION_UP);
    assert_eq!(scripts(&slot), 0, "the page is not even asked");
    assert!(shell.written().is_empty());
}

#[test]
fn a_question_that_comes_up_before_the_page_answers_is_refused_too() {
    let (f, h, shell) = live(Opts::default());
    let slot = with_fake(&f);
    let tab = claude(&f);
    page(&f, &slot, &tab);
    ask(&f);
    let (id, _) = last_script(&slot).unwrap();
    feed(
        &h,
        "Do you want to proceed?\r\n\u{276f} 1. Yes\r\n  2. No\r\nEsc to cancel",
    );
    answer(&f, &slot, id, "text");
    assert_eq!(note(&f), NOTE_QUESTION_UP);
    assert!(shell.written().is_empty());
}

#[test]
fn an_answer_after_the_person_moved_to_another_tab_is_dropped() {
    let (f, _h, shell) = live(Opts::default());
    let slot = with_fake(&f);
    let tab = claude(&f);
    page(&f, &slot, &tab);
    ask(&f);
    let (id, _) = last_script(&slot).unwrap();
    f.ctl.borrow_mut().open_folder("C:\\other");
    f.pump();
    assert_ne!(f.ctl.borrow().front_id().unwrap(), tab);
    answer(&f, &slot, id, "unseen");
    std::thread::sleep(Duration::from_millis(40)); // sync-ok: test code only
    f.pump();
    assert!(shell.written().is_empty());
}

#[test]
fn a_file_panel_sends_its_path_quoted_for_the_shell_and_no_enter() {
    let (f, h, shell) = live(Opts::default());
    let tab = claude(&f);
    feed(&h, "\x1b[?2004h");
    let file = f.dir.path().join("note book.txt");
    std::fs::write(&file, "words").unwrap(); // sync-ok: test code only
    let abs = file.to_string_lossy().into_owned();
    assert!(
        f.ctl
            .borrow_mut()
            .panels
            .show(&tab, flare_surfaces::PanelKind::Text, &abs)
    );
    f.ctl.borrow_mut().refresh();
    f.pump();
    ask(&f);
    let want = format!("\x1b[200~'{abs}'\x1b[201~");
    until_written(&shell, |w| w == want.as_bytes());
    std::thread::sleep(Duration::from_millis(40)); // sync-ok: test code only
    f.pump();
    assert_eq!(shell.written(), want.as_bytes());
}

#[test]
fn handed_over_text_has_no_file_and_types_nothing() {
    let (f, _h, shell) = live(Opts::default());
    let tab = claude(&f);
    assert!(f.ctl.borrow_mut().panels.show_text(&tab, "report", None));
    f.ctl.borrow_mut().refresh();
    f.pump();
    ask(&f);
    assert_eq!(note(&f), crate::ask_agent::NOTE_NO_FILE);
    assert!(shell.written().is_empty());
}

#[test]
fn the_button_is_offered_only_while_the_tab_has_an_agent() {
    let (f, _h, _shell) = live(Opts::default());
    let tab = f.ctl.borrow().front_id().unwrap();
    assert!(f.ctl.borrow_mut().panels.show_text(&tab, "report", None));
    f.ctl.borrow_mut().refresh();
    f.pump();
    assert!(f.h.comp.get_panel_open());
    assert!(!f.h.comp.get_panel_ask_on());
    claude(&f);
    f.ctl.borrow_mut().refresh();
    f.pump();
    assert!(f.h.comp.get_panel_ask_on());
    f.agent_exits(&tab);
    f.pump_until("the agent to leave", |f| !f.agent_here(&tab));
    f.ctl.borrow_mut().refresh();
    f.pump();
    assert!(!f.h.comp.get_panel_ask_on());
}

/// Opens the terminal's menu over the selection, as a right click's release does.
fn grid_menu(f: &Fx, tab: &str) -> Vec<String> {
    let front = f.ctl.borrow().front_tab().unwrap();
    let selection = front.handle.with_emu_quiet(|e| e.sel_text());
    {
        let mut c = f.ctl.borrow_mut();
        c.term.test_set_menu(tab, selection);
        c.open_term_menu(crate::term_menu::TermMenuAt {
            tab: tab.to_string(),
            x: 10.0,
            y: 10.0,
            selection: String::new(),
            link: None,
            path: None,
        });
    }
    f.ctl
        .borrow()
        .menu
        .as_ref()
        .expect("the menu")
        .rows
        .iter()
        .map(|(r, _)| r.label.clone())
        .collect()
}

fn pick(f: &Fx, labels: &[String], label: &str) {
    let i = labels
        .iter()
        .position(|l| l == label)
        .unwrap_or_else(|| panic!("no row {label}: {labels:?}"));
    f.ctl.borrow_mut().menu_picked(i32::try_from(i).unwrap());
    f.pump();
}

#[test]
fn the_grid_row_sends_the_selection_and_lets_it_go() {
    let (f, h, shell) = live(Opts::default());
    let tab = claude(&f);
    feed(&h, "\x1b[?2004h");
    feed(&h, "\x1b[2J\x1b[Hhello world");
    f.drag_cols(0, 0, 5);
    assert!(h.with_emu_quiet(|e| e.has_selection()));
    let rows = grid_menu(&f, &tab);
    pick(&f, &rows, "Ask agent about this selection");
    let want = "\x1b[200~About this from the terminal:\r> hello\x1b[201~";
    until_written(&shell, |w| w == want.as_bytes());
    std::thread::sleep(Duration::from_millis(40)); // sync-ok: test code only
    f.pump();
    assert_eq!(shell.written(), want.as_bytes());
    assert!(!h.with_emu_quiet(|e| e.has_selection()), "let go");
}

#[test]
fn the_grid_row_is_only_there_over_a_selection_on_a_tab_with_an_agent() {
    let (f, h, _shell) = live(Opts::default());
    let tab = f.ctl.borrow().front_id().unwrap();
    feed(&h, "hello world");
    f.drag_cols(0, 0, 5);
    let rows = grid_menu(&f, &tab);
    assert!(
        !rows.iter().any(|l| l == "Ask agent about this selection"),
        "no agent: {rows:?}"
    );
    f.ctl.borrow_mut().close_menu();
    claude(&f);
    f.drag_cols(0, 0, 5);
    let rows = grid_menu(&f, &tab);
    assert!(rows.iter().any(|l| l == "Ask agent about this selection"));
    f.ctl.borrow_mut().close_menu();
    h.with_emu(|e| e.sel_clear());
    let rows = grid_menu(&f, &tab);
    assert!(
        !rows.iter().any(|l| l == "Ask agent about this selection"),
        "no selection: {rows:?}"
    );
}

//! What the person marks in the side panel reaches the agent in their tab, inside the window controller with a fake
//! shell and a fake browser: the page's selection in the quiet context the prompt hook asks for (rule 1268), never
//! typed, and the terminal menu's "Ask agent about this selection", typed unsent (#33). The panel has no "Ask agent"
//! button any more (rule 1267).

use flare_web::WebEvent;
use flare_web::fake::{Call, FakeWeb};
use serde_json::{Value, json};

use super::*;
use crate::ctl_app::{CtlOp, CtlRequest, CtlRequestBox};

const PAGE: &str = "https://example.com/article";
/// The title the page's own document gives in its answer.
const TITLE: &str = "An article";

type Slot = Rc<RefCell<Option<FakeWeb>>>;
type Answer = std::sync::mpsc::Receiver<Result<Value, flare_ctl::CtlError>>;

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

/// The person sends a prompt, and its hook asks `panel.state` with its context (`CtlOp::PanelContext`).
fn ask(f: &Fx, tab: &str) -> Answer {
    f.ctl.borrow_mut().quiet_prompt_seen(tab);
    ask_unprompted(f, tab)
}

/// A call asks `panel.state` with `context`, with no prompt of its own.
fn ask_unprompted(f: &Fx, tab: &str) -> Answer {
    let (tx, rx) = std::sync::mpsc::channel();
    f.ctl
        .borrow_mut()
        .on_msg(UiMsg::Ctl(CtlRequestBox(CtlRequest {
            op: CtlOp::PanelContext {
                tab: tab.into(),
                cwd: None,
            },
            deadline: Instant::now() + Duration::from_secs(5),
            reply: tx,
        })));
    f.pump();
    rx
}

/// The answer's context, once it is in.
fn context(rx: &Answer) -> Value {
    rx.try_recv().expect("answered").expect("the state")["context"].clone()
}

/// The loaded page answers the script `id` with `selection`, in the script's own shape.
fn answer(f: &Fx, slot: &Slot, id: u64, selection: &str) {
    answer_as(f, slot, id, PAGE, TITLE, selection);
}

/// A document at `address` with this `title` answers the script `id` with `selection`.
fn answer_as(f: &Fx, slot: &Slot, id: u64, address: &str, title: &str, selection: &str) {
    fake(slot).say(WebEvent::Script {
        id,
        result: Ok(serde_json::to_string(&format!("{address}\n{title}\n{selection}")).unwrap()),
    });
    f.pump();
}

/// A mini-app on `tab`, loaded: a page Flare made, whose address means nothing to an agent.
fn mini_app(f: &Fx, slot: &Slot, tab: &str) {
    use flare_surfaces::app;
    let root = f.dir.path().join("apps");
    f.ctl.borrow_mut().set_apps_root(root.clone());
    let html = app::wrap_html("<p>hello</p>", false);
    let spec = app::write_app(&root, 1, &[(app::PAGE, html.as_bytes())], false).unwrap();
    let (tx, _rx) = std::sync::mpsc::channel();
    f.ctl
        .borrow_mut()
        .on_msg(UiMsg::Ctl(CtlRequestBox(CtlRequest {
            op: CtlOp::ShowApp {
                tab: tab.into(),
                spec: spec.clone(),
                title: "Chart".into(),
                diagram: false,
            },
            deadline: Instant::now() + Duration::from_secs(5),
            reply: tx,
        })));
    load(&fake(slot), tab, &spec.url, 1);
    f.pump();
    f.ctl.borrow_mut().refresh();
    f.pump();
}

fn note(f: &Fx) -> String {
    f.h.comp.get_ask_note().to_string()
}

#[test]
fn a_page_selection_reaches_the_quiet_context_with_its_page_and_nothing_is_typed() {
    let (f, h, shell) = live(Opts::default());
    let slot = with_fake(&f);
    let tab = claude(&f);
    feed(&h, "\x1b[?2004h");
    page(&f, &slot, &tab);
    let rx = ask(&f, &tab);
    let (id, js) = last_script(&slot).expect("the page was asked");
    assert!(js.contains("getSelection()"), "{js}");
    assert!(rx.try_recv().is_err(), "it waits for the page");
    answer(&f, &slot, id, "line one\nline two");
    assert_eq!(
        context(&rx),
        json!({"selection": {
            "text": "line one\nline two",
            "cut": false,
            "page": {"title": TITLE, "url": PAGE},
        }})
    );
    std::thread::sleep(Duration::from_millis(60)); // sync-ok: test code only
    f.pump();
    assert!(shell.written().is_empty(), "nothing is typed");
    assert_eq!(note(&f), "");
}

#[test]
fn each_prompt_reads_the_selection_again_so_a_cleared_one_is_gone() {
    let (f, _h, _shell) = live(Opts::default());
    let slot = with_fake(&f);
    let tab = claude(&f);
    page(&f, &slot, &tab);
    let rx = ask(&f, &tab);
    answer(&f, &slot, last_script(&slot).unwrap().0, "kept");
    assert_eq!(context(&rx)["selection"]["text"], "kept");
    let rx = ask(&f, &tab);
    assert_eq!(scripts(&slot), 2, "asked again");
    answer(&f, &slot, last_script(&slot).unwrap().0, "kept");
    assert_eq!(context(&rx)["selection"]["text"], "kept");
    let rx = ask(&f, &tab);
    answer(&f, &slot, last_script(&slot).unwrap().0, " \n\u{200b}");
    assert_eq!(
        context(&rx),
        json!({}),
        "nothing that prints is no selection"
    );
}

#[test]
fn a_hostile_selection_keeps_only_its_line_breaks() {
    let (f, _h, _shell) = live(Opts::default());
    let slot = with_fake(&f);
    let tab = claude(&f);
    page(&f, &slot, &tab);
    let rx = ask(&f, &tab);
    let (id, _) = last_script(&slot).unwrap();
    answer_as(
        &f,
        &slot,
        id,
        "https://me:pw@example.com/article",
        "t\x1b[201~\u{2028}x\u{200b}",
        "a\x1b[201~\r\x03b\r\n",
    );
    let sel = context(&rx)["selection"].clone();
    assert_eq!(sel["text"], "a[201~\nb");
    assert_eq!(sel["page"]["title"], "t[201~ x");
    assert_eq!(
        sel["page"]["url"], "https://example.com/article",
        "no user name or password"
    );
}

#[test]
fn a_long_selection_is_cut_to_2000_characters_and_says_so() {
    let (f, _h, _shell) = live(Opts::default());
    let slot = with_fake(&f);
    let tab = claude(&f);
    page(&f, &slot, &tab);
    let rx = ask(&f, &tab);
    answer(&f, &slot, last_script(&slot).unwrap().0, &"x".repeat(5000));
    let sel = context(&rx)["selection"].clone();
    assert_eq!(sel["cut"], true);
    assert_eq!(
        sel["text"].as_str().unwrap().len(),
        crate::quiet_context::SELECTION_MAX
    );
}

#[test]
fn the_page_named_is_the_document_that_answered_not_the_tabs_next_address() {
    let (f, _h, _shell) = live(Opts::default());
    let slot = with_fake(&f);
    let tab = claude(&f);
    page(&f, &slot, &tab);
    fake(&slot).say(WebEvent::Title {
        tab: tab.clone(),
        title: "The tab's title".into(),
    });
    // The navigation has started and not committed: the old page is what shows, and what the script runs in.
    fake(&slot).say(WebEvent::Loading {
        tab: tab.clone(),
        url: "https://bank.example/".into(),
        nav: 2,
    });
    f.pump();
    let rx = ask(&f, &tab);
    answer(&f, &slot, last_script(&slot).unwrap().0, "words");
    let sel = context(&rx)["selection"].clone();
    assert_eq!(sel["page"], json!({"title": TITLE, "url": PAGE}));
}

#[test]
fn a_failed_read_answers_without_a_selection() {
    let (f, _h, _shell) = live(Opts::default());
    let slot = with_fake(&f);
    let tab = claude(&f);
    page(&f, &slot, &tab);
    for result in [
        Err("gone".to_string()),
        Ok("null".to_string()),
        Ok(serde_json::to_string("\nt\nwords").unwrap()),
        Ok(format!(
            "\"{}\"",
            "x".repeat(crate::ask_agent::ANSWER_MAX + 1)
        )),
    ] {
        let rx = ask(&f, &tab);
        let (id, _) = last_script(&slot).unwrap();
        fake(&slot).say(WebEvent::Script { id, result });
        f.pump();
        assert_eq!(context(&rx), json!({}));
    }
    assert_eq!(f.ctl.borrow().quiet_reads_waiting(), 0);
}

#[test]
fn an_answer_from_before_a_navigation_is_dropped_and_the_wait_ends_without_it() {
    let (f, _h, _shell) = live(Opts::default());
    let slot = with_fake(&f);
    let tab = claude(&f);
    page(&f, &slot, &tab);
    let rx = ask(&f, &tab);
    let (id, _) = last_script(&slot).unwrap();
    load(&fake(&slot), &tab, "https://other.example/", 2);
    f.pump();
    answer(&f, &slot, id, "from the old page");
    assert!(
        rx.try_recv().is_err(),
        "the old page's answer is not believed"
    );
    f.ctl.borrow_mut().quiet_reads_expire();
    assert_eq!(context(&rx), json!({}));
}

#[test]
fn a_mini_apps_selection_is_never_read_its_script_could_fake_it() {
    let (f, _h, _shell) = live(Opts::default());
    let slot = with_fake(&f);
    let tab = claude(&f);
    mini_app(&f, &slot, &tab);
    let before = scripts(&slot);
    let rx = ask(&f, &tab);
    assert_eq!(scripts(&slot), before, "the page is not asked");
    assert_eq!(
        context(&rx),
        json!({}),
        "answered at once, with no selection"
    );
    assert_eq!(f.ctl.borrow().quiet_reads_waiting(), 0);
}

/// The person's own Enter in the terminal: what sends a prompt.
fn enter(f: &Fx, shift: bool) {
    let mut ev = flare_term::keys::KeyEvent::new("Enter", "Enter");
    ev.shift = shift;
    let _ = f.ctl.borrow_mut().on_key(&ev);
    f.pump();
}

#[test]
fn the_context_is_given_once_per_prompt_the_person_sent_and_repeats_get_the_plain_state() {
    let (f, _h, _shell) = live(Opts::default());
    let slot = with_fake(&f);
    let tab = claude(&f);
    page(&f, &slot, &tab);
    // No prompt sent: the agent's own tool asking with `context` gets the plain state, and the page is not asked.
    let before = scripts(&slot);
    let rx = ask_unprompted(&f, &tab);
    let v = rx.try_recv().expect("answered at once").expect("the state");
    assert!(v.get("context").is_none(), "{v}");
    assert_eq!(v["open"], true);
    assert_eq!(scripts(&slot), before);
    // A Shift+Enter is a new line, not a prompt.
    enter(&f, true);
    let v = ask_unprompted(&f, &tab).try_recv().unwrap().unwrap();
    assert!(v.get("context").is_none(), "{v}");
    // The person presses Enter: that prompt's call has the context.
    enter(&f, false);
    let rx = ask_unprompted(&f, &tab);
    answer(&f, &slot, last_script(&slot).unwrap().0, "chosen");
    assert_eq!(context(&rx)["selection"]["text"], "chosen");
    // The same prompt asked again: the plain state.
    let v = ask_unprompted(&f, &tab).try_recv().unwrap().unwrap();
    assert!(v.get("context").is_none(), "{v}");
}

#[test]
fn a_prompt_counts_only_for_its_tab_while_it_is_in_front() {
    let (f, _h, _shell) = live(Opts::default());
    let tab = claude(&f);
    assert!(f.ctl.borrow_mut().panels.show_text(&tab, "report", None));
    f.ctl.borrow_mut().refresh();
    f.pump();
    let has_context = |f: &Fx| {
        let v = f.ctl.borrow_mut().panel_context_once(&tab, None).unwrap();
        assert_eq!(v["open"], true, "{v}");
        v.get("context").is_some()
    };
    // A prompt for another tab does not open this one.
    f.ctl.borrow_mut().quiet_prompt_seen("tab-elsewhere");
    assert!(!has_context(&f));
    // This tab's prompt while another tab is in front: the plain state, and the prompt is used up.
    f.ctl.borrow_mut().quiet_prompt_seen(&tab);
    f.ctl.borrow_mut().model.state.active_id = Some("tab-elsewhere".into());
    let v = f.ctl.borrow_mut().panel_context_once(&tab, None).unwrap();
    assert!(v.get("context").is_none(), "{v}");
    f.ctl.borrow_mut().model.state.active_id = Some(tab.clone());
    assert!(!has_context(&f), "used up");
    // In front, with its prompt: the context.
    f.ctl.borrow_mut().quiet_prompt_seen(&tab);
    assert!(has_context(&f));
    assert!(!has_context(&f), "once");
}

#[test]
fn a_closed_panel_or_one_with_nothing_to_read_answers_at_once() {
    let (f, _h, _shell) = live(Opts::default());
    let tab = claude(&f);
    let rx = ask(&f, &tab);
    let v = rx.try_recv().expect("answered at once").expect("the state");
    assert_eq!(v, json!({"open": false}), "no panel: no context");
    assert!(f.ctl.borrow_mut().panels.show_text(&tab, "report", None));
    f.ctl.borrow_mut().refresh();
    f.pump();
    let rx = ask(&f, &tab);
    assert_eq!(context(&rx), json!({}), "handed text, nothing selected");
}

#[test]
fn the_panel_has_no_ask_agent_button_and_draw_arrow_stays() {
    let panel = include_str!("../../../ui/panel.slint");
    let app = include_str!("../../../ui/app.slint");
    for (name, text) in [("panel.slint", panel), ("app.slint", app)] {
        assert!(!text.contains("AskAgentButton"), "{name}");
        assert!(!text.contains("text: \"Ask agent\""), "{name}");
        assert!(!text.contains("panel-ask-agent"), "{name}");
    }
    assert!(panel.contains("draw-btn := WebButton"));
    assert!(panel.contains("head-draw-btn := WebButton"));
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

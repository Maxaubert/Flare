//! Markdown views (2026-11-10) inside the window controller, with a fake browser: a `.md` file opens compiled as a
//! page Flare writes, the header's control switches between Compiled, Markdown and Code, the pick stays with the panel
//! (and across a restart), a page that cannot be shown falls back to Code, and Ask agent names the file.

use flare_surfaces::app::AppSpec;
use flare_surfaces::mdview::MdView;
use flare_web::WebEvent;
use flare_web::fake::{Call, FakeWeb};

use super::*;

/// Every browser the controller made, in order: a browser with no page left is let go, and the next page makes another.
type Slot = Rc<RefCell<Vec<FakeWeb>>>;

fn with_fake(f: &Fx) -> Slot {
    let slot: Slot = Rc::default();
    let keep = slot.clone();
    f.ctl.borrow_mut().set_web_factory(Box::new(move |sink| {
        let fake = FakeWeb::new(sink);
        keep.borrow_mut().push(fake.clone());
        Box::new(fake)
    }));
    slot
}

/// The browser made last.
fn fake(slot: &Slot) -> FakeWeb {
    slot.borrow().last().cloned().expect("the browser was made")
}

fn calls(slot: &Slot) -> Vec<Call> {
    slot.borrow().iter().flat_map(FakeWeb::calls).collect()
}

/// The pages the browsers were asked to show, in order.
fn opened_pages(slot: &Slot) -> Vec<AppSpec> {
    calls(slot)
        .into_iter()
        .filter_map(|c| match c {
            Call::OpenApp { app, .. } => Some(app),
            _ => None,
        })
        .collect()
}

/// The apps folder of this test, handed to the controller.
fn apps_root(f: &Fx) -> std::path::PathBuf {
    let root = f.dir.path().join("apps");
    f.ctl.borrow_mut().set_apps_root(root.clone());
    root
}

const DOC: &str = "# Read me\n\nSome *text* and <script>alert(1)</script> [away](https://e.example/).\n\n- [x] done\n";

/// Writes `name` with `body` and shows it in the front tab's panel, as a click does.
fn show_md(f: &Fx, name: &str, body: &str) -> String {
    let path = f.dir.path().join(name);
    std::fs::write(&path, body).unwrap(); // sync-ok: test code only
    let abs = path.to_string_lossy().into_owned();
    let id = f.ctl.borrow().front_id().unwrap();
    f.ctl
        .borrow_mut()
        .show_preview(&id, flare_surfaces::PanelKind::Text, &abs);
    abs
}

fn page_html(spec: &AppSpec) -> String {
    std::fs::read_to_string(spec.folder.join(flare_surfaces::app::PAGE)).unwrap() // sync-ok: test code only
}

#[test]
fn a_markdown_file_opens_compiled_as_a_page_flare_wrote() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let root = apps_root(&f);
    let abs = show_md(&f, "r.md", DOC);
    f.pump_until("the page", |_| !opened_pages(&slot).is_empty());
    let spec = opened_pages(&slot).remove(0);
    assert_eq!(
        spec.source.as_deref(),
        Some(abs.as_str()),
        "the page names its file"
    );
    assert!(spec.folder.starts_with(&root));
    assert!(!spec.allow_network);
    let html = page_html(&spec);
    assert!(
        html.contains("default-src 'none'"),
        "the policy is in the page"
    );
    assert!(!html.contains("<script"), "the file's script shows as text");
    assert!(html.contains("&lt;script&gt;") && html.contains("alert(1)"));
    assert!(
        !html.contains("href=\"https"),
        "a link that leaves the page has no address"
    );
    let a = &f.h.comp;
    assert_eq!(a.get_panel_kind(), 3, "the browser draws it");
    assert!(a.get_panel_web_app(), "no address bar");
    assert!(a.get_panel_md());
    assert_eq!(a.get_panel_md_view(), MdView::Compiled.index());
    assert_eq!(a.get_panel_title().as_str(), "r.md");
    // The channel still names it a text file by its path.
    let id = f.ctl.borrow().front_id().unwrap();
    let snap = f.ctl.borrow().panels.snap(&id).unwrap();
    assert_eq!(
        (snap.kind.as_str(), snap.path.as_deref()),
        ("text", Some(abs.as_str()))
    );
    // The browser tools see no web page in the tab.
    assert!(f.ctl.borrow().panels.web_target(&id).is_none());
}

#[test]
fn a_text_file_that_is_not_markdown_has_no_view_control() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    apps_root(&f);
    show_md(&f, "notes.txt", "# not markdown\n");
    f.pump_until("the text", |f| f.h.comp.get_panel_kind() == 2);
    assert!(!f.h.comp.get_panel_md());
    assert!(opened_pages(&slot).is_empty());
}

#[test]
fn the_header_control_switches_between_the_three_views() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    apps_root(&f);
    show_md(&f, "r.md", DOC);
    f.pump_until("the compiled page", |_| opened_pages(&slot).len() == 1);
    let compiled = opened_pages(&slot).remove(0);
    // Code: the native text view, with the wrap button, and the page's browser goes.
    f.h.comp.invoke_panel_md_view_picked(MdView::Code.index());
    f.pump_until("the text", |f| f.h.comp.get_panel_kind() == 2);
    assert_eq!(f.h.comp.get_panel_md_view(), MdView::Code.index());
    assert!(f.h.comp.get_panel_md(), "the control stays over the text");
    assert!(
        calls(&slot).iter().any(|c| matches!(c, Call::Close(_))),
        "the page's browser was closed"
    );
    // Markdown: a page of its own, the source coloured.
    f.h.comp
        .invoke_panel_md_view_picked(MdView::Markdown.index());
    f.pump_until("the markdown page", |_| opened_pages(&slot).len() == 2);
    let source = opened_pages(&slot).remove(1);
    assert_ne!(source.host, compiled.host, "every page has its own origin");
    let html = page_html(&source);
    assert!(html.contains("<pre class=\"src\">"), "{html}");
    assert!(html.contains("<span class=\"h\"># Read me</span>"));
    assert_eq!(f.h.comp.get_panel_kind(), 3);
    assert_eq!(f.h.comp.get_panel_md_view(), MdView::Markdown.index());
    // A view that is not one of the three changes nothing.
    f.h.comp.invoke_panel_md_view_picked(7);
    f.pump();
    assert_eq!(f.h.comp.get_panel_md_view(), MdView::Markdown.index());
}

#[test]
fn a_file_shown_again_opens_compiled_again() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    apps_root(&f);
    show_md(&f, "r.md", DOC);
    f.pump_until("the page", |_| opened_pages(&slot).len() == 1);
    f.h.comp.invoke_panel_md_view_picked(MdView::Code.index());
    f.pump_until("the text", |f| f.h.comp.get_panel_kind() == 2);
    // The same file clicked again: compiled, every time.
    show_md(&f, "r.md", DOC);
    f.ctl.borrow_mut().refresh();
    f.pump_until("compiled again", |_| opened_pages(&slot).len() == 2);
    assert_eq!(f.h.comp.get_panel_md_view(), MdView::Compiled.index());
}

#[test]
fn a_page_that_cannot_be_shown_falls_back_to_code() {
    // No browser in this run: the page's view fails at once and the file shows as text.
    let (f, _h, _) = live(Opts::default());
    apps_root(&f);
    show_md(&f, "r.md", DOC);
    f.pump_until("the text", |f| f.h.comp.get_panel_kind() == 2);
    assert_eq!(f.h.comp.get_panel_md_view(), MdView::Code.index());
    assert!(rows_in_panel(&f) >= 5);
    assert_eq!(
        f.h.comp.get_panel_note().as_str(),
        "",
        "no failure note over the text"
    );
}

#[test]
fn a_page_whose_browser_fails_later_falls_back_to_code() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    apps_root(&f);
    show_md(&f, "r.md", DOC);
    f.pump_until("the page", |_| opened_pages(&slot).len() == 1);
    let id = f.ctl.borrow().front_id().unwrap();
    fake(&slot).say(WebEvent::Failed {
        tab: id.clone(),
        why: "the renderer stopped".into(),
        missing_runtime: true,
    });
    f.pump_until("the text", |f| f.h.comp.get_panel_kind() == 2);
    assert_eq!(f.h.comp.get_panel_md_view(), MdView::Code.index());
    // The person asks for the page again: it is tried again.
    f.h.comp
        .invoke_panel_md_view_picked(MdView::Compiled.index());
    f.pump_until("a new page", |_| opened_pages(&slot).len() == 2);
}

#[test]
fn without_an_apps_folder_a_markdown_file_shows_as_code() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    show_md(&f, "r.md", DOC);
    f.pump_until("the text", |f| f.h.comp.get_panel_kind() == 2);
    assert!(f.h.comp.get_panel_md());
    assert_eq!(f.h.comp.get_panel_md_view(), MdView::Code.index());
    assert!(opened_pages(&slot).is_empty());
}

#[test]
fn a_render_that_fails_falls_back_to_code_and_says_why_there() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    apps_root(&f);
    let abs = show_md(&f, "r.md", DOC);
    std::fs::remove_file(&abs).unwrap(); // sync-ok: test code only
    f.ctl.borrow_mut().refresh();
    f.pump_until("the note", |f| {
        f.h.comp.get_panel_note().as_str() == "The file is gone."
    });
    assert_eq!(f.h.comp.get_panel_md_view(), MdView::Code.index());
    assert!(opened_pages(&slot).is_empty());
}

#[test]
fn the_picked_view_is_written_and_comes_back_after_a_restart() {
    let f = fx_with(Opts {
        saved: vec![("C:\\a", None)],
        ..Opts::default()
    });
    let slot = with_fake(&f);
    apps_root(&f);
    let doc = f.dir.path().join("r.md");
    std::fs::write(&doc, DOC).unwrap(); // sync-ok: test code only
    let target = doc.to_string_lossy().into_owned();
    let file = f.dir.path().join("panels.json");
    let saved = [flare_surfaces::store::SavedTabPanel {
        cwd: "C:\\a".into(),
        panel: Some(flare_surfaces::TabPanel {
            kind: flare_surfaces::PanelKind::Text,
            target: target.clone(),
            open: true,
            width: None,
            by_agent: false,
            view: Some(MdView::Markdown),
        }),
    }];
    std::fs::write(&file, flare_surfaces::store::panels_json(&saved)).unwrap(); // sync-ok: test code only
    let store = Arc::new(flare_surfaces::store::PanelStore::new(
        &file,
        Duration::from_millis(30),
    ));
    f.ctl.borrow_mut().set_panel_store(store.clone());
    f.start_restore_and_wait();
    f.pump_until("the restored page", |_| !opened_pages(&slot).is_empty());
    assert_eq!(f.h.comp.get_panel_md_view(), MdView::Markdown.index());
    assert!(page_html(&opened_pages(&slot)[0]).contains("<pre class=\"src\">"));
    // A new pick is written.
    f.h.comp.invoke_panel_md_view_picked(MdView::Code.index());
    f.pump_until("the text", |f| f.h.comp.get_panel_kind() == 2);
    store.flush();
    let back = flare_surfaces::store::PanelStore::new(&file, Duration::from_millis(30)).load();
    assert_eq!(
        back[0].panel.as_ref().and_then(|p| p.view),
        Some(MdView::Code)
    );
}

#[test]
fn ask_agent_on_a_markdown_page_names_the_file_and_sends_its_path_with_nothing_selected() {
    let (f, h, shell) = live(Opts::default());
    let slot = with_fake(&f);
    apps_root(&f);
    let id = f.ctl.borrow().front_id().unwrap();
    let cwd = f.cwds()[0].clone();
    f.say(&cwd, TITLE_IDLE);
    f.pump_until("the agent to be here", |f| f.agent_here(&id));
    feed(&h, "\x1b[?2004h");
    let abs = show_md(&f, "r.md", DOC);
    f.pump_until("the page", |_| !opened_pages(&slot).is_empty());
    let url = opened_pages(&slot)[0].url.clone();
    let script = |slot: &Slot| {
        calls(slot).into_iter().rev().find_map(|c| match c {
            Call::Script(_, sid, _) => Some(sid),
            _ => None,
        })
    };
    f.ctl.borrow_mut().panel_ask_agent();
    f.pump();
    let sid = script(&slot).expect("the page was asked for its selection");
    fake(&slot).say(WebEvent::Script {
        id: sid,
        result: Ok(serde_json::to_string(&format!("{url}\nSome text")).unwrap()),
    });
    f.pump();
    let typed = String::from_utf8_lossy(&shell.written()).into_owned();
    assert!(typed.contains("About this from r.md:"), "{typed:?}");
    assert!(typed.contains("> Some text"), "{typed:?}");
    assert!(
        typed.ends_with("\x1b[201~"),
        "nothing follows the paste: {typed:?}"
    );
    // Nothing selected: the file's path goes, as from the Code view.
    f.ctl.borrow_mut().panel_ask_agent();
    f.pump();
    let sid = script(&slot).unwrap();
    fake(&slot).say(WebEvent::Script {
        id: sid,
        result: Ok(serde_json::to_string(&format!("{url}\n")).unwrap()),
    });
    f.pump();
    let typed = String::from_utf8_lossy(&shell.written()).into_owned();
    assert!(typed.contains(&abs), "{typed:?}");
}

//! The control channel end to end on macOS (port phase 4, rule 1012): the agent's own side (`flare_mcp`'s
//! tools over `FlareChannel`), the real Unix socket, the app's real server (`CtlServer`) and dispatcher, and the
//! real window controller, headless. The browser is the fake one, played here: a page asked for is loaded, and a
//! diagram's page says it drew.
//!
//! Held: every `show_*` tool, `open_page`, `close_panel`, `set_tab_badge`, `clear_tab_badge`, `set_tab_progress`,
//! `clear_tab_progress` and `convo.state` reach the window from another thread over the socket and are answered,
//! and what each did is on the window's panel or the tab's row; a caller with no tab token, or one that names a
//! process other than the server, gets nothing.

use std::sync::atomic::AtomicU32;
use std::sync::mpsc::{Receiver, Sender};

use flare_ctl::{ClientError, HelloParams, Method};
use flare_mcp::{FlareChannel, handle_message};
use flare_web::WebEvent;
use flare_web::fake::{Call, FakeWeb};
use serde_json::{Value, json};

use super::*;
use crate::ctl_app::{AppsCfg, CtlServer, MERMAID};

const TOKEN: &str = "secret-socket";
const WAIT: Duration = Duration::from_secs(10);

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

/// The browser loads `url` in `tab` (its navigation numbers are its own).
fn finish(fake: &FakeWeb, tab: &str, url: &str) {
    thread_local! {
        static NAV: std::cell::Cell<u64> = const { std::cell::Cell::new(9000) };
    }
    let nav = NAV.with(|n| {
        n.set(n.get() + 1);
        n.get()
    });
    fake.say(WebEvent::Loading {
        tab: tab.into(),
        url: url.into(),
        nav,
    });
    fake.say(WebEvent::Loaded {
        tab: tab.into(),
        url: url.into(),
        ok: true,
        status: "0".into(),
        nav,
    });
}

/// Plays the browser for every call it got since the last turn: pages load, diagrams draw.
fn play_browser(slot: &Slot, seen: &mut usize) {
    let Some(fake) = slot.borrow().clone() else {
        return;
    };
    let calls = fake.calls();
    for c in calls.iter().skip(*seen) {
        match c {
            Call::Open { tab, url, .. } | Call::Navigate(tab, url) => finish(&fake, tab, url),
            Call::OpenApp { tab, app, .. } => finish(&fake, tab, &app.url),
            Call::Cdp(tab, id, m, p) if m == "Runtime.evaluate" && p.contains("__flareDiagram") => {
                fake.say(WebEvent::Cdp {
                    tab: tab.clone(),
                    id: *id,
                    result: Ok(r#"{"result":{"type":"object","value":{"ok":true}}}"#.into()),
                });
            }
            _ => {}
        }
    }
    *seen = calls.len();
}

fn unique_pipe() -> String {
    static N: AtomicU32 = AtomicU32::new(0);
    flare_ctl::pipe_name(
        &format!(
            "flare-app-socket-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ),
        None,
    )
}

/// The app's own server on a fresh socket, its calls posted to the test window's queue, `TOKEN` naming `tab`.
fn serve(f: &Fx, tab: &str, apps_root: &Path) -> (CtlServer, String) {
    let pipe = unique_pipe();
    let post: Post = {
        let q = f.q.clone();
        Arc::new(move |m| q.lock().unwrap().push_back(m))
    };
    let tab = tab.to_string();
    let server = CtlServer::start(
        &pipe,
        post,
        Arc::new(move |t: &str| (t == TOKEN).then(|| tab.clone())),
        f.dir.path().to_string_lossy().into_owned(),
        Arc::new(|| false),
        AppsCfg::new(
            apps_root.to_path_buf(),
            Arc::new(flare_term::path_open::resolve_html_file),
            Arc::new(|| false),
            MERMAID,
        ),
        None,
    )
    .expect("the app's server starts on the socket");
    f.ctl.borrow_mut().set_ctl(
        server.events(),
        crate::appctl::CtlInfo {
            pipe: pipe.clone(),
            mcp_exe: None,
        },
    );
    (server, pipe)
}

enum Ask {
    Tool(&'static str, Value),
    Convo,
}

/// The agent's side on its own thread: one tool call (or `convo.state`) per request, over the socket.
fn agent(pipe: String, token: Option<&str>, pid: u32) -> (Sender<Ask>, Receiver<Value>) {
    let (ask_tx, ask_rx) = std::sync::mpsc::channel::<Ask>();
    let (reply_tx, reply_rx) = std::sync::mpsc::channel();
    let token = token.map(str::to_string);
    std::thread::spawn(move || {
        let mut ch =
            FlareChannel::new(token.clone(), Some(pipe.clone())).with_server_pid(Some(pid));
        let mut n = 0;
        for ask in ask_rx {
            n += 1;
            let reply = match ask {
                Ask::Tool(name, args) => handle_message(
                    &json!({"jsonrpc": "2.0", "id": n, "method": "tools/call",
                        "params": {"name": name, "arguments": args}}),
                    &mut ch,
                )
                .unwrap_or(Value::Null),
                Ask::Convo => {
                    let r = flare_ctl::connect_to_server(&pipe, WAIT, pid).and_then(|mut c| {
                        c.hello(
                            &HelloParams {
                                client: "socket-test".into(),
                                version: "1".into(),
                                token: None,
                                tab_token: token.clone(),
                                no_events: true,
                            },
                            WAIT,
                        )?;
                        c.call(Method::ConvoState, json!({}), WAIT)
                    });
                    match r {
                        Ok(v) => json!({"ok": v}),
                        Err(ClientError::Remote(e)) => json!({"err": e.code}),
                        Err(e) => json!({"err": e.to_string()}),
                    }
                }
            };
            if reply_tx.send(reply).is_err() {
                return;
            }
        }
    });
    (ask_tx, reply_rx)
}

/// Sends one request and runs the window (and the browser) until its answer is back.
fn call(
    f: &Fx,
    slot: &Slot,
    seen: &mut usize,
    tx: &Sender<Ask>,
    rx: &Receiver<Value>,
    ask: Ask,
) -> Value {
    tx.send(ask).unwrap();
    let t = Instant::now();
    while t.elapsed() < WAIT {
        f.pump();
        play_browser(slot, seen);
        f.pump();
        if let Ok(v) = rx.try_recv() {
            return v;
        }
        std::thread::sleep(Duration::from_millis(3)); // sync-ok: test code only
    }
    panic!("the call was not answered");
}

fn ok_text(r: &Value) -> String {
    assert_eq!(r["result"]["isError"], false, "{r}");
    r["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// A one-pixel PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8, 0xCF, 0xC0, 0xF0,
    0x1F, 0x00, 0x05, 0x00, 0x01, 0xFF, 0x89, 0x99, 0x3D, 0x1D, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

/// A Claude tab in a real folder of the test's own, with files to show.
fn claude_tab(f: &Fx) -> (String, PathBuf) {
    let work = f.dir.path().join("work");
    std::fs::create_dir_all(&work).unwrap(); // sync-ok: test code only, never the app
    // The window's own folder name for it (the temp folder sits under /var, a link to /private/var).
    let work = work.canonicalize().unwrap();
    std::fs::write(work.join("cat.png"), PNG).unwrap(); // sync-ok: test code only, never the app
    std::fs::write(work.join("notes.txt"), "line one\nline two\n").unwrap(); // sync-ok: test code only, never the app
    std::fs::write(work.join("chart.html"), "<p id=c>chart</p>").unwrap(); // sync-ok: test code only, never the app
    std::fs::write(work.join("reply.mp3"), b"ID3 fake").unwrap(); // sync-ok: test code only, never the app
    let cwd = work.to_string_lossy().into_owned();
    f.open_tabs(&[&cwd]);
    let id = f.claude_in(&cwd);
    (id, work)
}

/// What the tab's panel shows (kind and title), empty when it has no panel open.
fn panel(f: &Fx, tab: &str) -> (String, String) {
    let state = f.ctl.borrow().panels.snap(tab);
    let Some(snap) = state.filter(|_| f.ctl.borrow().panels.is_open(tab)) else {
        return (String::new(), String::new());
    };
    let info = AppCtl::panel_info(&snap);
    (
        info["kind"].as_str().unwrap_or_default().to_string(),
        info["title"].as_str().unwrap_or_default().to_string(),
    )
}

#[test]
fn every_show_set_tab_and_convo_tool_reaches_the_window_over_the_socket() {
    let f = fx_with(Opts {
        real_fs: true,
        ..Opts::default()
    });
    let slot = with_fake(&f);
    let (tab, work) = claude_tab(&f);
    let apps_root = f.dir.path().join("apps");
    f.ctl.borrow_mut().set_apps_root(apps_root.clone());
    let (server, pipe) = serve(&f, &tab, &apps_root);
    let (tx, rx) = agent(pipe, Some(TOKEN), std::process::id());
    let mut seen = 0;
    let mut ask = |a: Ask| call(&f, &slot, &mut seen, &tx, &rx, a);
    let file = |name: &str| work.join(name).to_string_lossy().into_owned();

    // The panels: a picture, a text file, text, a page, a diagram, a sound and a web page.
    ok_text(&ask(Ask::Tool(
        "show_image",
        json!({"path": file("cat.png")}),
    )));
    assert_eq!(panel(&f, &tab).0, "image");
    ok_text(&ask(Ask::Tool(
        "show_file",
        json!({"path": file("notes.txt")}),
    )));
    assert_eq!(panel(&f, &tab).0, "text");
    ok_text(&ask(Ask::Tool(
        "show_text",
        json!({"text": "hello", "title": "Said"}),
    )));
    assert_eq!(panel(&f, &tab), ("text".into(), "Said".into()));
    ok_text(&ask(Ask::Tool(
        "show_html",
        json!({"html": "<p>hi</p>", "title": "Hi"}),
    )));
    assert_eq!(panel(&f, &tab), ("app".into(), "Hi".into()));
    ok_text(&ask(Ask::Tool(
        "show_html",
        json!({"path": file("chart.html"), "title": "Chart"}),
    )));
    assert_eq!(panel(&f, &tab), ("app".into(), "Chart".into()));
    ok_text(&ask(Ask::Tool(
        "show_diagram",
        json!({"mermaid": "graph TD\n  A --> B", "title": "Flow"}),
    )));
    assert_eq!(panel(&f, &tab), ("app".into(), "Flow".into()));
    ok_text(&ask(Ask::Tool(
        "show_audio",
        json!({"path": file("reply.mp3"), "title": "Reply"}),
    )));
    assert_eq!(panel(&f, &tab), ("app".into(), "Reply".into()));
    ok_text(&ask(Ask::Tool(
        "open_page",
        json!({"url": "https://example.com/"}),
    )));
    assert_eq!(panel(&f, &tab).0, "web");
    assert_eq!(
        ok_text(&ask(Ask::Tool("close_panel", json!({})))),
        "Closed the panel."
    );
    assert_eq!(panel(&f, &tab).0, "");

    // The tab's row: a badge and a progress, then both cleared.
    let row = |f: &Fx| {
        let i = f.ids().iter().position(|t| *t == tab).unwrap();
        f.row(i)
    };
    ok_text(&ask(Ask::Tool(
        "set_tab_badge",
        json!({"key": "call-me", "icon": "phone", "tip": "Call-me is on"}),
    )));
    assert_eq!(row(&f).chip_count, 1);
    assert!(
        row(&f).tip.as_str().contains("Call-me is on"),
        "{}",
        row(&f).tip
    );
    ok_text(&ask(Ask::Tool(
        "set_tab_progress",
        json!({"value": 0.5, "label": "Half"}),
    )));
    assert_eq!(row(&f).prog, 0.5);
    ok_text(&ask(Ask::Tool("clear_tab_progress", json!({}))));
    assert_eq!(row(&f).prog, -1.0);
    ok_text(&ask(Ask::Tool(
        "clear_tab_badge",
        json!({"key": "call-me"}),
    )));
    assert_eq!(row(&f).chip_count, 0);

    // convo.state: the tab's own summary, over a connection of its own.
    let v = ask(Ask::Convo);
    assert!(v["ok"].is_object(), "{v}");

    drop(ask);
    drop(tx);
    assert_eq!(server.stop(), 0);
}

#[test]
fn a_caller_without_the_tabs_token_or_told_another_process_gets_nothing() {
    let f = fx_with(Opts {
        real_fs: true,
        ..Opts::default()
    });
    let slot = with_fake(&f);
    let (tab, _work) = claude_tab(&f);
    let apps_root = f.dir.path().join("apps");
    let (server, pipe) = serve(&f, &tab, &apps_root);
    let mut seen = 0;

    // A guessed token: Flare refuses, nothing is shown.
    let (tx, rx) = agent(pipe.clone(), Some("guess"), std::process::id());
    let r = call(
        &f,
        &slot,
        &mut seen,
        &tx,
        &rx,
        Ask::Tool("show_text", json!({"text": "x"})),
    );
    assert_eq!(r["result"]["isError"], true, "{r}");
    assert_eq!(panel(&f, &tab).0, "");
    let v = call(&f, &slot, &mut seen, &tx, &rx, Ask::Convo);
    assert_eq!(v["err"], flare_ctl::code::UNAUTHORIZED, "{v}");

    // The right token, but the shell was told another process serves the channel: the client leaves before it
    // says a word, so the token never reaches the socket.
    let (tx, rx) = agent(pipe, Some(TOKEN), std::process::id().wrapping_add(4));
    let r = call(
        &f,
        &slot,
        &mut seen,
        &tx,
        &rx,
        Ask::Tool("show_text", json!({"text": "x"})),
    );
    assert_eq!(r["result"]["isError"], true, "{r}");
    let said = r["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default();
    assert!(said.contains("Flare is not running this session"), "{said}");
    assert_eq!(panel(&f, &tab).0, "");
    assert_eq!(server.stop(), 0);
}

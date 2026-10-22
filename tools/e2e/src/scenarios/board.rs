//! Live preview, the agent board and question notifications (Surfaces stage 7): `devPreview`, `agentBoard` and
//! `questionToast`.
//!
//! Each starts the built release exe in its own world and profile, offscreen and unfocusable like every scenario. Nothing
//! reaches the desktop: the listeners are the scenario's own (a thread that answers HTTP on `127.0.0.1`), the agents are
//! command lines in a real shell that print the title and the hook lines a Claude session sends, and the notifications go
//! to the app's recording seam (`notify.log`), never to Windows. The person's click on a notification is the
//! `ToastClick` request.
//!
//! `devPreview` (100 and 225 percent): an address nothing listens on gets no chip; a listener makes the chip; it sits
//! whole in the terminal's top right; a click opens the web panel at that address and the page the listener serves loads
//! in it; a second server's chip goes when its listener is dropped and comes back when a server is started again on that port; a cleared screen takes a chip at once; a remote
//! address never gets one; the setting off keeps all of it away.
//! `agentBoard` (100 and 225 percent): the board lists the two tabs with an agent, not the plain shell, each with its state
//! and time; a click on a row goes to that tab; the chord and the control channel open it; closing it stops its clock.
//! `questionToast`: a question in a tab out of sight records one notification with the tab's name; none while that tab is
//! in front; a click goes to the tab; the setting off records none.

use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use flare_ctl::{connect, Client, ClientError, HelloParams, Method, PipeEnd};
use flare_e2e_proto::Request;
use serde_json::{json, Value};

use super::input_b::{css, front_tab, probe, prompt, type_line};
use super::web::script;
use crate::ctx::{App, Ctx, Launch};

const WAIT: Duration = Duration::from_secs(15);

/// A local server of the scenario's own: answers every request with a small page, until it is dropped (then its port
/// closes, as a stopped dev server's does).
struct Site {
    port: u16,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Site {
    fn start(title: &str) -> Result<Site, String> {
        Site::start_on(title, 0)
    }

    /// Like `start`, on a given port (0 for any): a server started again where the last one was.
    fn start_on(title: &str, port: u16) -> Result<Site, String> {
        let listener = TcpListener::bind(("127.0.0.1", port)).map_err(|e| format!("bind: {e}"))?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let body = format!(
            "<!doctype html><html><head><meta charset=\"utf-8\"><title>{title}</title></head><body><h1 id=\"h\">{title}</h1></body></html>"
        );
        let thread = std::thread::spawn(move || {
            while !flag.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut s, _)) => {
                        let _ = s.set_nonblocking(false);
                        let _ = s.set_read_timeout(Some(Duration::from_millis(500)));
                        let mut buf = [0u8; 2048];
                        let _ = s.read(&mut buf);
                        let reply = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = s.write_all(reply.as_bytes());
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(15)),
                }
            }
            // The listener is dropped here: the port stops listening.
        });
        Ok(Site {
            port,
            stop,
            thread: Some(thread),
        })
    }

    /// Stops the server: its port stops listening before this returns.
    fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Site {
    fn drop(&mut self) {
        self.stop();
    }
}

/// A port nothing listens on (bound and dropped a moment ago).
fn free_port() -> Result<u16, String> {
    let l = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| e.to_string())?;
    Ok(l.local_addr().map_err(|e| e.to_string())?.port())
}

fn state(app: &mut App) -> Value {
    probe(app, "surf7.state", Value::Null)
}

/// Lets `ms` of real time go by (a wait that holds nothing, to see that nothing happens).
fn pass(ctx: &mut Ctx, ms: u64) {
    let t = std::time::Instant::now();
    ctx.until("time to pass", ms + 5_000, &mut || {
        t.elapsed() >= Duration::from_millis(ms)
    });
}

fn chip_port(app: &mut App) -> Option<i64> {
    state(app)["chip"]["port"].as_i64()
}

fn size_to_1000(ctx: &mut Ctx, app: &mut App, dpi: u32, at: &str) -> Result<(), String> {
    let scale = f64::from(dpi) / 96.0;
    app.session.call(&Request::Resize {
        w: 1000.0,
        h: 640.0,
    })?;
    let sized = ctx.until("the window to be 1000 css px wide", 5_000, &mut || {
        probe(app, "dpi.state", Value::Null)["client_w_px"].as_f64()
            == Some((1000.0 * scale).round())
    });
    ctx.ok(sized, format!("{at}: the window is 1000 css px wide"));
    Ok(())
}

// ---- devPreview ----------------------------------------------------------------------------------------------

pub fn dev_preview(ctx: &mut Ctx) -> Result<(), String> {
    let alpha = ctx.ensure_world().alpha.clone();
    let profile = ctx.ensure_world().profile.clone();
    for (dpi, label) in [(96u32, "100%"), (216, "225%")] {
        if std::env::var("WEB_DPI_ONLY").is_ok_and(|o| o != label) {
            continue;
        }
        let mut app = ctx.launch(Launch {
            renderer: Some("gpu"),
            folders: vec![alpha.clone()],
            scale_dpi: Some(dpi),
            ..Launch::default()
        })?;
        let r = preview_on(ctx, &mut app, dpi, label);
        let left = app.close();
        ctx.ok(left, format!("{label}: the app quits by itself"));
        r?;
    }
    // The setting off, stored in the profile: nothing is offered however much listens.
    fs::create_dir_all(&profile).map_err(|e| e.to_string())?;
    fs::write(
        profile.join("settings.json"),
        json!({"flare.window.devPreview": "0"}).to_string(),
    )
    .map_err(|e| e.to_string())?;
    let mut app = ctx.launch(Launch {
        renderer: Some("gpu"),
        folders: vec![alpha],
        scale_dpi: Some(96),
        ..Launch::default()
    })?;
    let r = preview_off(ctx, &mut app);
    let left = app.close();
    ctx.ok(left, "setting off: the app quits by itself");
    r
}

fn preview_on(ctx: &mut Ctx, app: &mut App, dpi: u32, at: &str) -> Result<(), String> {
    size_to_1000(ctx, app, dpi, at)?;
    prompt(app)?;
    let s = state(app);
    ctx.ok(
        s["chip"].is_null() && s["tracked"] == json!(0) && s["chip_label"] == json!(""),
        format!("{at}: no chip and nothing tracked at the start"),
    );

    // ---- an address nothing listens on gets no chip ----
    let dead = free_port()?;
    type_line(app, &format!("echo http://localhost:{dead}/idle"))?;
    let looked = ctx.until("the address to be looked at", 5_000, &mut || {
        state(app)["tracked"] == json!(1)
    });
    ctx.ok(looked, format!("{at}: the printed address is looked up"));
    pass(ctx, 800);
    ctx.ok(
        state(app)["chip"].is_null(),
        format!("{at}: nothing listens on port {dead}, so no chip"),
    );

    // ---- a listener makes the chip ----
    let site = Site::start("dev one")?;
    let p1 = site.port;
    prompt(app)?;
    type_line(app, &format!("echo http://localhost:{p1}/hello?x=1"))?;
    let up = ctx.until("the chip", 20_000, &mut || {
        chip_port(app) == Some(i64::from(p1))
    });
    ctx.ok(up, format!("{at}: a listening local address gets a chip"));
    let s = state(app);
    ctx.ok(
        s["chip_label"] == json!(format!("localhost:{p1}")),
        format!("{at}: it reads localhost:{p1} ({})", s["chip_label"]),
    );
    ctx.ok(
        s["chip"]["url"] == json!(format!("http://localhost:{p1}/hello?x=1")),
        format!("{at}: and carries the whole address ({})", s["chip"]["url"]),
    );

    // ---- it sits whole in the terminal's top right ----
    let grid = css(&probe(app, "ui.rect", json!({"name": "grid"}))).unwrap_or_default();
    let chip_rect = probe(app, "ui.rect", json!({"name": "preview-chip"}));
    let chip = css(&chip_rect).unwrap_or_default();
    ctx.ok(
        chip.2 > 90.0
            && chip.0 >= grid.0
            && chip.0 + chip.2 <= grid.0 + grid.2
            && chip.1 >= grid.1
            && chip.1 + chip.3 <= grid.1 + grid.3
            && chip.0 + chip.2 > grid.0 + grid.2 * 0.6,
        format!("{at}: the chip is inside the terminal's top right ({chip:?} in {grid:?})"),
    );

    // ---- a click opens the web panel at that address ----
    let (cx, cy) = (chip.0 + chip.2 / 2.0, chip.1 + chip.3 / 2.0);
    app.session.click(cx as f32, cy as f32)?;
    let shown = ctx.until("the web panel at the address", 40_000, &mut || {
        let w = probe(app, "web.state", Value::Null);
        w["shown"] == json!(true)
            && w["kind"] == json!(3)
            && w["bar"]["url"]
                .as_str()
                .is_some_and(|u| u == format!("http://localhost:{p1}/hello?x=1"))
    });
    ctx.ok(
        shown,
        format!("{at}: the click opens the web panel at the printed address"),
    );
    let loaded = ctx.until("the page to load", 40_000, &mut || {
        let w = probe(app, "web.state", Value::Null);
        w["controllers"] == json!(1) && w["title"] == json!("dev one")
    });
    ctx.ok(
        loaded,
        format!("{at}: the header names the page the listener serves"),
    );
    let title = script(ctx, app, "document.title");
    ctx.ok(
        title.as_deref() == Some("\"dev one\""),
        format!("{at}: the page the listener serves loaded in it ({title:?})"),
    );
    let s = state(app);
    ctx.ok(
        s["chip_label"] == json!(""),
        format!("{at}: the panel shows it, so the chip is gone"),
    );
    app.session.call(&Request::Focus { on: true })?;

    // ---- a second server: its chip goes when its listener is dropped ----
    let mut site2 = Site::start("dev two")?;
    let p2 = site2.port;
    app.session.key("Ctrl+Shift+O")?; // the keys back to the terminal if the page had them
    pass(ctx, 200);
    prompt(app)?;
    type_line(app, &format!("echo http://127.0.0.1:{p2}/"))?;
    let up2 = ctx.until("the second chip", 20_000, &mut || {
        chip_port(app) == Some(i64::from(p2))
    });
    ctx.ok(
        up2,
        format!("{at}: another address replaces the chip: one chip for the newest"),
    );
    ctx.ok(
        state(app)["chip_label"] == json!(format!("127.0.0.1:{p2}")),
        format!("{at}: the chip names the newest address"),
    );
    site2.stop();
    let gone = ctx.until("the chip to go with its listener", 25_000, &mut || {
        state(app)["chip"].is_null()
    });
    ctx.ok(
        gone,
        format!("{at}: the chip goes when the process that printed it is gone"),
    );

    // ---- the same server started again on its port, the same address printed, the screen not cleared ----
    let mut site2b = None;
    for _ in 0..40 {
        if let Ok(x) = Site::start_on("dev two again", p2) {
            site2b = Some(x);
            break;
        }
        pass(ctx, 100);
    }
    let site2b = site2b.ok_or("could not listen on the same port again")?;
    prompt(app)?;
    type_line(app, &format!("echo http://127.0.0.1:{p2}/"))?;
    let back = ctx.until("the chip to come back", 25_000, &mut || {
        chip_port(app) == Some(i64::from(p2))
    });
    ctx.ok(
        back,
        format!("{at}: a server started again on the same port gets its chip back"),
    );
    drop(site2b);
    let gone2 = ctx.until("the chip to go again", 25_000, &mut || {
        state(app)["chip"].is_null()
    });
    ctx.ok(gone2, format!("{at}: and goes again with its listener"));

    // ---- a cleared screen takes the chip at once ----
    let site3 = Site::start("dev three")?;
    let p3 = site3.port;
    prompt(app)?;
    type_line(app, &format!("echo http://localhost:{p3}/three"))?;
    let up3 = ctx.until("the third chip", 20_000, &mut || {
        chip_port(app) == Some(i64::from(p3))
    });
    ctx.ok(up3, format!("{at}: a third server gets its chip"));
    prompt(app)?;
    type_line(app, "Clear-Host")?;
    let cleared = ctx.until("the clear to take the chip", 5_000, &mut || {
        state(app)["chip"].is_null()
    });
    ctx.ok(
        cleared,
        format!("{at}: clearing the screen takes the chip while the server still listens"),
    );
    pass(ctx, 1_500);
    ctx.ok(
        state(app)["chip"].is_null() && site3.port == p3,
        format!("{at}: and it does not come back by itself"),
    );

    // ---- a remote address never gets one ----
    let tracked_before = state(app)["tracked"].clone();
    prompt(app)?;
    type_line(
        app,
        "echo http://example.com:3000/ http://localhost.evil.com:3000/ https://evil.test/localhost:3000 http://localhost:3000@evil.test/",
    )?;
    let printed = ctx.until("the line to be printed", 5_000, &mut || {
        super::input_b::rows(app).matches("evil.test").count() >= 2
    });
    ctx.ok(printed, format!("{at}: the remote addresses were printed"));
    pass(ctx, 1_000);
    let s = state(app);
    ctx.ok(
        s["chip"].is_null() && s["tracked"] == tracked_before,
        format!(
            "{at}: no remote address gets a chip or is even looked at ({} tracked, {} before: {})",
            s["tracked"], tracked_before, s["tracked_urls"]
        ),
    );
    drop(site);
    Ok(())
}

fn preview_off(ctx: &mut Ctx, app: &mut App) -> Result<(), String> {
    prompt(app)?;
    let site = Site::start("dev off")?;
    type_line(app, &format!("echo http://localhost:{}/off", site.port))?;
    let printed = ctx.until("the line to be printed", 5_000, &mut || {
        super::input_b::rows(app).contains("/off")
    });
    ctx.ok(printed, "setting off: the address was printed");
    pass(ctx, 1_500);
    let s = state(app);
    ctx.ok(
        s["chip"].is_null() && s["tracked"] == json!(0) && s["chip_label"] == json!(""),
        "setting off: nothing listens for it and no chip is offered",
    );
    Ok(())
}

// ---- stand-in agents -------------------------------------------------------------------------------------------

/// `[Console]::Write` of an OSC with the payload.
fn osc(payload: &str) -> String {
    let p = payload.replace('\'', "''");
    format!("[Console]::Write(([string][char]27)+']{p}'+([string][char]7))")
}

/// The console title Claude sets when it is ready.
fn claude_title() -> String {
    "[Console]::Write(([string][char]27)+']0;'+[char]0x2733+' Claude Code'+([string][char]7))"
        .to_string()
}

fn hook(state: &str) -> String {
    osc(&format!("777;flare-agent;state={state}"))
}

fn wait_for_file(flag: &Path) -> String {
    let flag = flag.to_string_lossy().replace('\'', "''");
    format!("while (-not (Test-Path -LiteralPath '{flag}')) {{ Start-Sleep -Milliseconds 100 }}")
}

/// A command line that stands in for a Claude session that stays in `state` for the whole scenario. The shell never
/// returns to its prompt, which would take the agent back.
fn agent_in(state: &str) -> String {
    format!(
        "{}; {}; Start-Sleep -Seconds 900",
        claude_title(),
        hook(state)
    )
}

fn ids_of(app: &mut App) -> Vec<(String, bool)> {
    probe(app, "ctl.info", Value::Null)["tabs"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|t| {
                    (
                        t["id"].as_str().unwrap_or_default().to_string(),
                        t["active"] == json!(true),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Brings tab number `n` (1 based) to the front with Ctrl+n and waits for it.
fn go_to(ctx: &mut Ctx, app: &mut App, n: usize, at: &str) -> Result<String, String> {
    let ids = ids_of(app);
    let id = ids
        .get(n - 1)
        .map(|t| t.0.clone())
        .ok_or_else(|| format!("{at}: no tab {n}"))?;
    app.session.key(&format!("Ctrl+{n}"))?;
    let there = ctx.until(&format!("tab {n} in front"), 5_000, &mut || {
        front_tab(app) == id
    });
    ctx.ok(there, format!("{at}: tab {n} is in front"));
    Ok(id)
}

fn hello(pipe: &str, token: &str) -> Result<Client<PipeEnd>, ClientError> {
    let mut c = connect(pipe, WAIT)?;
    c.hello(
        &HelloParams {
            client: "flare-e2e".into(),
            version: "1".into(),
            token: None,
            tab_token: Some(token.to_string()),
        },
        WAIT,
    )?;
    Ok(c)
}

fn rows_of(v: &Value) -> Vec<(String, String, String)> {
    v["board_rows"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|r| {
                    (
                        r["name"].as_str().unwrap_or_default().to_string(),
                        r["state"].as_str().unwrap_or_default().to_string(),
                        r["status"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

// ---- agentBoard ------------------------------------------------------------------------------------------------

pub fn agent_board(ctx: &mut Ctx) -> Result<(), String> {
    for (dpi, label) in [(96u32, "100%"), (216, "225%")] {
        // A world of its own for each leg: the first leg's three tabs are saved and would come back as three more.
        let w = ctx.new_world();
        let mut app = ctx.launch_in(
            &w,
            Launch {
                renderer: Some("gpu"),
                folders: vec![w.alpha.clone(), w.beta.clone(), w.root.clone()],
                scale_dpi: Some(dpi),
                ..Launch::default()
            },
        )?;
        let r = board_on(ctx, &mut app, dpi, label);
        // An agent that works makes a close ask its question; the scenario is over, so the quit is forced.
        let _ = app.session.call(&Request::Quit { force: true });
        let left = app.close();
        ctx.ok(left, format!("{label}: the app quits by itself"));
        r?;
    }
    Ok(())
}

fn board_on(ctx: &mut Ctx, app: &mut App, dpi: u32, at: &str) -> Result<(), String> {
    size_to_1000(ctx, app, dpi, at)?;
    let ids = ids_of(app);
    ctx.ok(ids.len() == 3, format!("{at}: three tabs ({})", ids.len()));

    // Tab 1 works, tab 2 asks, tab 3 stays a plain shell.
    let a = go_to(ctx, app, 1, at)?;
    prompt(app)?;
    type_line(app, &agent_in("working"))?;
    let b = go_to(ctx, app, 2, at)?;
    prompt(app)?;
    type_line(app, &agent_in("question"))?;
    let c = go_to(ctx, app, 3, at)?;
    prompt(app)?;
    let s = state(app);
    ctx.ok(
        s["board_open"] == json!(false)
            && s["board_timer"] == json!(false)
            && s["board_ui_rows"] == json!(0),
        format!("{at}: a closed board has no rows and no timer"),
    );

    // ---- the chord opens it ----
    let seen = ctx.until("both agents to be known", 20_000, &mut || {
        let rows = rows_of(&state(app));
        rows.len() == 2
    });
    ctx.ok(
        seen,
        format!("{at}: two tabs have an agent, the plain shell does not"),
    );
    app.session.key("Ctrl+Shift+A")?;
    let open = ctx.until("the board to open", 5_000, &mut || {
        let s = state(app);
        s["board_open"] == json!(true) && s["panel_kind"] == json!(6)
    });
    ctx.ok(
        open,
        format!("{at}: Ctrl+Shift+A opens the board in the front tab's panel"),
    );
    let s = state(app);
    let rows = rows_of(&s);
    ctx.ok(
        rows.iter().map(|r| r.1.as_str()).collect::<Vec<_>>() == ["working", "question"],
        format!("{at}: the rows are in the strip's order with their states ({rows:?})"),
    );
    let said = |i: usize| rows.get(i).map(|r| r.2.clone()).unwrap_or_default();
    ctx.ok(
        said(0).starts_with("Working for ") && said(1).starts_with("Waiting"),
        format!("{at}: and say how long ({} / {})", said(0), said(1)),
    );
    ctx.ok(
        s["board_ui_rows"] == json!(2) && s["board_timer"] == json!(true),
        format!("{at}: the window holds the two rows and the clock runs"),
    );
    let p = probe(app, "panel.state", Value::Null);
    ctx.ok(
        p["kind"] == json!(6) && p["shown"] == json!(true) && p["title"] == json!("Agents"),
        format!("{at}: the panel is the board, titled Agents"),
    );

    // ---- the geometry ----
    let body = css(&probe(app, "ui.rect", json!({"name": "panel-body"}))).unwrap_or_default();
    let r0 = css(&probe(app, "ui.rect", json!({"name": "board-row-0"}))).unwrap_or_default();
    let r1 = css(&probe(app, "ui.rect", json!({"name": "board-row-1"}))).unwrap_or_default();
    ctx.ok(
        (r0.0 - body.0).abs() < 0.6
            && (r0.1 - body.1).abs() < 0.6
            && (r0.2 - body.2).abs() < 0.6
            && (r1.1 - (r0.1 + r0.3)).abs() < 0.6
            && r1.1 + r1.3 <= body.1 + body.3 + 0.6,
        format!("{at}: the rows stack from the top of the panel body ({r0:?} {r1:?} in {body:?})"),
    );

    // ---- the clock moves ----
    let first = rows_of(&state(app))
        .first()
        .map(|r| r.2.clone())
        .unwrap_or_default();
    let moved = ctx.until("the clock to move", 8_000, &mut || {
        rows_of(&state(app)).first().map(|r| r.2.clone()) != Some(first.clone())
    });
    ctx.ok(
        moved,
        format!("{at}: the time on a row moves while the board is open"),
    );

    // ---- a click goes to that tab ----
    let shot = ctx.shot(app, &format!("board-{dpi}"));
    let _ = shot;
    app.session
        .click((r0.0 + r0.2 / 2.0) as f32, (r0.1 + r0.3 / 2.0) as f32)?;
    let went = ctx.until("tab 1 to come to the front", 5_000, &mut || {
        front_tab(app) == a
    });
    ctx.ok(
        went,
        format!("{at}: a click on the first row goes to that tab"),
    );

    // ---- tab 3's board stays where it was opened ----
    let _ = go_to(ctx, app, 3, at)?;
    let back = state(app)["board_open"] == json!(true);
    ctx.ok(
        back,
        format!("{at}: the board is still tab 3's panel when it comes back"),
    );

    // ---- the control channel opens it too ----
    let info = probe(app, "ctl.info", Value::Null);
    let pipe = info["pipe"].as_str().unwrap_or_default().to_string();
    let token_b = info["tabs"][1]["token"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let mut chan = hello(&pipe, &token_b).map_err(|e| format!("hello: {e}"))?;
    let r = chan.call(Method::PanelOpen, json!({"kind": "board"}), WAIT);
    ctx.ok(
        r.as_ref().is_ok_and(|v| {
            v["panel"]["kind"] == json!("board") && v["panel"]["title"] == json!("Agents")
        }),
        format!("{at}: panel.open kind board opens it for an agent's own tab ({r:?})"),
    );
    let r = chan.call(
        Method::PanelOpen,
        json!({"kind": "board", "path": "x"}),
        WAIT,
    );
    ctx.ok(
        matches!(&r, Err(ClientError::Remote(e)) if e.code == "bad_params"),
        format!("{at}: and refuses a path that does not go with it ({r:?})"),
    );
    drop(chan);

    // ---- closing it stops the clock ----
    app.session.key("Ctrl+Shift+A")?;
    let closed = ctx.until("the board to close", 5_000, &mut || {
        let s = state(app);
        s["board_open"] == json!(false)
            && s["board_timer"] == json!(false)
            && s["board_ui_rows"] == json!(0)
    });
    ctx.ok(
        closed,
        format!("{at}: closing the board drops its rows and its clock"),
    );
    let _ = (b, c);
    Ok(())
}

// ---- questionToast ---------------------------------------------------------------------------------------------

pub fn question_toast(ctx: &mut Ctx) -> Result<(), String> {
    let w = ctx.new_world();
    let mut app = ctx.launch_in(
        &w,
        Launch {
            renderer: Some("gpu"),
            folders: vec![w.alpha.clone(), w.beta.clone()],
            scale_dpi: Some(96),
            ..Launch::default()
        },
    )?;
    let r = toast_on(ctx, &mut app, &w.root, &w.alpha);
    let _ = app.session.call(&Request::Quit { force: true });
    let left = app.close();
    ctx.ok(left, "the app quits by itself");
    r?;
    // The setting off, stored in the profile of a world of its own.
    let off = ctx.new_world();
    fs::create_dir_all(&off.profile).map_err(|e| e.to_string())?;
    fs::write(
        off.profile.join("settings.json"),
        json!({"flare.window.notifyAsk": "0"}).to_string(),
    )
    .map_err(|e| e.to_string())?;
    let mut app = ctx.launch_in(
        &off,
        Launch {
            renderer: Some("gpu"),
            folders: vec![off.alpha.clone(), off.beta.clone()],
            scale_dpi: Some(96),
            ..Launch::default()
        },
    )?;
    let r = toast_off(ctx, &mut app, &off.root);
    let _ = app.session.call(&Request::Quit { force: true });
    let left = app.close();
    ctx.ok(left, "setting off: the app quits by itself");
    r
}

fn toasts(app: &mut App) -> Vec<Value> {
    probe(app, "notify.log", Value::Null)["toasts"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// A stand-in agent in the tab in front: ready, then working until `go1` exists, then asking, then (after `go2`) working
/// and asking again.
fn asker(go1: &Path, go2: &Path) -> String {
    format!(
        "{}; {}; {}; {}; {}; {}; {}; {}; Start-Sleep -Seconds 900",
        claude_title(),
        hook("working"),
        wait_for_file(go1),
        hook("question"),
        wait_for_file(go2),
        hook("working"),
        "Start-Sleep -Milliseconds 400",
        hook("question"),
    )
}

fn toast_on(ctx: &mut Ctx, app: &mut App, root: &Path, alpha: &Path) -> Result<(), String> {
    let at = "toast";
    let go1 = root.join("go1.flag");
    let go2 = root.join("go2.flag");
    let _ = fs::remove_file(&go1);
    let _ = fs::remove_file(&go2);
    ctx.ok(toasts(app).is_empty(), "nothing is recorded at the start");
    // The window has the keyboard, as far as the app can tell (the suite's window never really has it).
    app.session.call(&Request::Focus { on: true })?;
    // Tab 1 holds the agent; tab 2 is where the person is.
    let a = go_to(ctx, app, 1, at)?;
    prompt(app)?;
    type_line(app, &asker(&go1, &go2))?;
    let working = ctx.until("the agent to be working", 20_000, &mut || {
        rows_of(&state(app)).first().map(|r| r.1.clone()) == Some("working".into())
    });
    ctx.ok(working, "the agent in tab 1 is working");
    let b = go_to(ctx, app, 2, at)?;

    // ---- a question out of sight records one notification ----
    fs::write(&go1, "go").map_err(|e| e.to_string())?;
    let got = ctx.until("the notification", 15_000, &mut || !toasts(app).is_empty());
    ctx.ok(
        got,
        "a question in a tab out of sight records a notification",
    );
    pass(ctx, 1_500);
    let all = toasts(app);
    ctx.ok(
        all.len() == 1,
        format!("and only one for one question ({})", all.len()),
    );
    let name = alpha
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    ctx.ok(
        all.first()
            .is_some_and(|t| t["tab"] == json!(a) && t["title"] == json!(name)),
        format!("it names the tab ({:?}, wanted {a} / {name})", all.first()),
    );
    ctx.ok(
        all.first()
            .is_some_and(|t| t["body"] == json!("Claude has a question for you.")),
        format!(
            "and says who asks when the text is not known ({:?})",
            all.first().map(|t| &t["body"])
        ),
    );

    // ---- a click on it brings the tab up ----
    ctx.ok(front_tab(app) == b, "the person is still in tab 2");
    app.session.call(&Request::ToastClick { tab: a.clone() })?;
    let there = ctx.until("tab 1 to come to the front", 5_000, &mut || {
        front_tab(app) == a
    });
    ctx.ok(
        there,
        "a click on the notification brings that tab to the front",
    );

    // ---- none while that tab is in view ----
    fs::write(&go2, "go").map_err(|e| e.to_string())?;
    pass(ctx, 900);
    let again = ctx.until("the second question", 10_000, &mut || {
        rows_of(&state(app)).first().map(|r| r.1.clone()) == Some("question".into())
    });
    ctx.ok(again, "the agent asks again, with its tab in front");
    pass(ctx, 2_000);
    ctx.ok(
        toasts(app).len() == 1,
        format!(
            "and the person looking at it is told nothing ({})",
            toasts(app).len()
        ),
    );
    let _ = b;
    Ok(())
}

fn toast_off(ctx: &mut Ctx, app: &mut App, root: &Path) -> Result<(), String> {
    let at = "setting off";
    app.session.call(&Request::Focus { on: true })?;
    let go1 = root.join("go1-off.flag");
    let go2 = root.join("go2-off.flag");
    let _ = fs::remove_file(&go1);
    let _ = fs::write(&go2, "go");
    let _a = go_to(ctx, app, 1, at)?;
    prompt(app)?;
    type_line(app, &asker(&go1, &go2))?;
    let working = ctx.until("the agent to be working", 20_000, &mut || {
        rows_of(&state(app)).first().map(|r| r.1.clone()) == Some("working".into())
    });
    ctx.ok(working, "setting off: the agent in tab 1 is working");
    let _b = go_to(ctx, app, 2, at)?;
    fs::write(&go1, "go").map_err(|e| e.to_string())?;
    let asked = ctx.until("the question", 15_000, &mut || {
        rows_of(&state(app)).first().map(|r| r.1.clone()) == Some("question".into())
    });
    ctx.ok(asked, "setting off: the agent asks");
    pass(ctx, 2_000);
    ctx.ok(
        toasts(app).is_empty(),
        "setting off: and no notification is recorded",
    );
    Ok(())
}

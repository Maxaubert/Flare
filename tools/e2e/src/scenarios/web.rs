//! The web panel (Surfaces stage 3): `webPanel`, `webVideo` and `webNoRuntime`.
//!
//! Each starts the built release exe in its own world and profile, so the browser's folder is the
//! world's, never the owner's Edge profile. Local pages are served by a WebView2 virtual host: the app
//! maps `https://flare-test.local/` to a folder of the world (the `WEB_ROOT` seam, which only exists
//! under `--e2e`), so no page is a `file:` address and nothing is fetched from the network. The channel
//! is spoken to with the `flare-ctl` client and a tab's secret read through the `ctl.info` probe, as
//! `ctlChannel` does.
//!
//! `webPanel` holds where the browser window sits (the controller's own bounds, the child windows of
//! Flare's window, what Slint drew and the layout maths, all four equal) at 100 and 225 percent, that
//! an overlay hides it with a still in its place, that the channel opens, navigates and refuses
//! addresses, what a page may open (a pop-up over the page after a trusted click, nothing after a scripted one,
//! rule 249), that it gets no permission, and that closing the panel
//! frees the browser and its process. `webVideo` plays a local clip and reads its clock and frame
//! counter. `webNoRuntime` pretends the runtime is missing.
//!
//! The window is parked off the screen and never takes the foreground, like every scenario. WebView2
//! navigates, runs scripts and answers there; what it paints is not asserted pixel by pixel.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use flare_ctl::{connect, Client, ClientError, HelloParams, Method, PipeEnd};
use flare_e2e_proto::{Button, Request};
use serde_json::{json, Value};

use super::input_b::{front_tab, probe, prompt};
use crate::ctx::{App, Ctx, Launch};

/// A page may take a while the first time the browser starts.
pub(super) const LOAD: Duration = Duration::from_secs(45);
pub(super) const HOST: &str = "https://flare-test.local";

const INDEX: &str = r##"<!doctype html><html><head><meta charset="utf-8"><title>Flare test page</title></head>
<body style="margin:0;background:#2a6;color:#fff;font:20px sans-serif">
<h1 id="h">hello flare</h1>
<a id="two" href="two.html">two</a>
<a id="blank" href="three.html" target="_blank">blank</a>
<a id="pop" href="#" onclick="var w=window.open('four.html'); document.title='pop:'+(w?'opened':'blocked'); return false;">pop</a>
<a id="dl" href="note.txt" download="flare-e2e-download-test.txt">save</a>
<button id="geo" onclick="navigator.geolocation.getCurrentPosition(function(){document.title='geo:ok'},function(e){document.title='geo:err'+e.code})">geo</button>
</body></html>"##;
const NOTE: &str = "a note the e2e offers to download\n";
const TWO: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>Page two</title></head><body><h1>two</h1></body></html>"#;
const THREE: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>Page three</title></head><body><h1>three</h1></body></html>"#;
const FOUR: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>Page four</title></head><body><h1>four</h1></body></html>"#;
/// A page with two targets at fixed places for the browser tools' trusted clicks (rule 249): a plain new-tab link and a
/// `window.open` with a size and a name, the way a shop opens its bank's 3-D Secure check.
const POP: &str = r##"<!doctype html><html><head><meta charset="utf-8"><title>Pop page</title></head>
<body style="margin:0;background:#246;color:#fff;font:16px sans-serif">
<a id="plain" href="three.html" target="_blank" style="position:absolute;left:0;top:0;width:240px;height:60px;display:block;background:#444">plain</a>
<a id="win" href="#" onclick="window.w=window.open('popup.html','threeDS','width=400,height=500'); document.title='opened:'+(window.w?'yes':'no'); return false;" style="position:absolute;left:0;top:100px;width:240px;height:60px;display:block;background:#464">win</a>
</body></html>"##;
/// A page that opens a sized window as it loads, with no click of anyone's: the blocker's case.
const AUTO: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>Auto page</title></head>
<body><h1>auto</h1><script>window.open('four.html','auto','width=300,height=300');</script></body></html>"#;
/// The pop-up: it reaches back to the page that opened it (`window.opener`), and closes itself when that page asks.
const POPUP: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>Pop-up page</title></head>
<body><h1>pop-up</h1><script>if(window.opener){window.opener.document.title='opener:'+window.name;}
window.addEventListener('message',function(e){if(e.data==='close'){window.close();}});</script></body></html>"#;
const VIDEO: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>Video page</title></head>
<body style="margin:0;background:#000"><video id="v" muted autoplay loop playsinline style="width:100%"></video>
<script>var v=document.getElementById('v');v.src=new URLSearchParams(location.search).get('clip')||'clip.webm';setInterval(function(){var q=v.getVideoPlaybackQuality();document.title=JSON.stringify({t:v.currentTime,paused:v.paused,frames:q.totalVideoFrames,dropped:q.droppedVideoFrames,rs:v.readyState});},100);</script></body></html>"#;

/// A folder of pages inside the world, served as `https://flare-test.local/`.
pub(super) fn web_root(ctx: &mut Ctx) -> Result<PathBuf, String> {
    let root = ctx.ensure_world().root.join("web");
    fs::create_dir_all(&root).map_err(|e| format!("make the web folder: {e}"))?;
    for (name, text) in [
        ("index.html", INDEX),
        ("two.html", TWO),
        ("three.html", THREE),
        ("four.html", FOUR),
        ("pop.html", POP),
        ("popup.html", POPUP),
        ("auto.html", AUTO),
        ("video.html", VIDEO),
        ("note.txt", NOTE),
    ] {
        fs::write(root.join(name), text).map_err(|e| format!("write {name}: {e}"))?;
    }
    Ok(root)
}

pub(super) fn launch_with(
    ctx: &mut Ctx,
    root: &Path,
    dpi: u32,
    extra: &[(&str, &str)],
) -> Result<App, String> {
    let alpha = ctx.ensure_world().alpha.clone();
    let mut env = vec![(
        "FLARE_E2E_WEB_ROOT".to_string(),
        root.to_string_lossy().into_owned(),
    )];
    env.extend(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
    ctx.launch(Launch {
        renderer: Some("gpu"),
        folders: vec![alpha],
        scale_dpi: Some(dpi),
        env,
        ..Launch::default()
    })
}

/// The pipe and the front tab's secret.
pub(super) fn channel_of(app: &mut App) -> Result<(String, String, String), String> {
    let v = probe(app, "ctl.info", Value::Null);
    let pipe = v["pipe"]
        .as_str()
        .ok_or("ctl.info has no pipe")?
        .to_string();
    let tabs = v["tabs"].as_array().ok_or("ctl.info has no tabs")?;
    let t = tabs
        .iter()
        .find(|t| t["active"] == json!(true))
        .ok_or("no tab in front")?;
    Ok((
        pipe,
        t["id"].as_str().unwrap_or_default().to_string(),
        t["token"].as_str().unwrap_or_default().to_string(),
    ))
}

pub(super) fn hello(pipe: &str, tab_token: &str) -> Result<Client<PipeEnd>, ClientError> {
    let mut c = connect(pipe, Duration::from_secs(10))?;
    c.hello(
        &HelloParams {
            client: "flare-e2e".into(),
            version: "1".into(),
            token: None,
            tab_token: Some(tab_token.to_string()),
            no_events: false,
        },
        Duration::from_secs(10),
    )?;
    Ok(c)
}

pub(super) fn code_of(r: &Result<Value, ClientError>) -> String {
    match r {
        Err(ClientError::Remote(e)) => e.code.clone(),
        Err(e) => format!("other: {e}"),
        Ok(_) => "ok".into(),
    }
}

pub(super) fn state(app: &mut App) -> Value {
    probe(app, "web.state", Value::Null)
}

pub(super) fn int(v: &Value) -> i64 {
    v.as_i64().unwrap_or(i64::MIN)
}

fn same_rect(a: &Value, b: &Value, slack: i64) -> bool {
    ["x", "y", "w", "h"]
        .iter()
        .all(|k| (int(&a[*k]) - int(&b[*k])).abs() <= slack)
}

/// Runs a script in the front tab's page and gives its JSON answer.
pub(super) fn script(ctx: &mut Ctx, app: &mut App, js: &str) -> Option<String> {
    app.session
        .call(&Request::WebScript { js: js.to_string() })
        .ok()?;
    let mut out = None;
    let done = ctx.until("the script to answer", 15_000, &mut || {
        let r = probe(app, "web.script.result", Value::Null);
        if r["done"] == json!(true) {
            out = Some(if r["ok"] == json!(true) {
                r["value"].as_str().unwrap_or_default().to_string()
            } else {
                format!("ERROR {}", r["error"])
            });
            true
        } else {
            false
        }
    });
    if done {
        out
    } else {
        None
    }
}

pub fn web_panel(ctx: &mut Ctx) -> Result<(), String> {
    let root = web_root(ctx)?;
    for (dpi, label) in [(96u32, "100%"), (216, "225%")] {
        if std::env::var("WEB_DPI_ONLY").is_ok_and(|o| o != label) {
            continue;
        }
        // The browser's popup blocker would stop a window.open that has no user gesture before Flare
        // hears of it; the test turns it off so the handler path is the one that runs.
        let mut app = launch_with(
            ctx,
            &root,
            dpi,
            &[(
                "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
                "--disable-popup-blocking",
            )],
        )?;
        let r = panel_on(ctx, &mut app, dpi, label);
        let left = app.close();
        ctx.ok(left, format!("{label}: the app quits by itself"));
        let pid = r?;
        // Quitting closes the web views itself (rule 28): the browser's process ends with the window.
        ctx.ok(
            pid != 0,
            format!("{label}: the browser had a process id to watch"),
        );
        if pid != 0 {
            let gone = ctx.until(
                "the browser process to end with the window",
                30_000,
                &mut || !crate::proc::pid_alive(pid),
            );
            ctx.ok(
                gone,
                format!("{label}: and quitting the window ends the browser's process {pid}"),
            );
        }
    }
    Ok(())
}

/// The browser process id while the page is open (0 when it is not known).
fn panel_on(ctx: &mut Ctx, app: &mut App, dpi: u32, at: &str) -> Result<u32, String> {
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
    prompt(app)?;
    let (pipe, front_id, token) = channel_of(app)?;

    // ---- nothing exists while no web panel is open ----
    let s0 = state(app);
    ctx.ok(
        s0["controllers"] == json!(0) && s0["backend"] == json!(false),
        format!(
            "{at}: no browser exists before a web panel opens ({} controllers)",
            s0["controllers"]
        ),
    );

    let mut c = hello(&pipe, &token).map_err(|e| format!("hello: {e}"))?;

    // ---- the channel opens a page ----
    let url = format!("{HOST}/index.html");
    let t0 = Instant::now();
    let r = c.call(Method::BrowserOpen, json!({ "url": url }), LOAD);
    let took = t0.elapsed().as_millis();
    ctx.ok(
        r.as_ref().is_ok_and(|v| {
            v["panel"]["kind"] == json!("web")
                && v["url"]
                    .as_str()
                    .is_some_and(|u| u.ends_with("/index.html"))
                && v["title"] == json!("Flare test page")
                && v["loaded"] == json!(true)
        }),
        format!("{at}: browser.open answers once the page has loaded ({r:?}, {took} ms)"),
    );
    let up = ctx.until(
        "the web panel to be shown with a controller",
        10_000,
        &mut || {
            let s = state(app);
            s["shown"] == json!(true) && s["kind"] == json!(3) && s["controllers"] == json!(1)
        },
    );
    ctx.ok(
        up,
        format!("{at}: the panel shows a web page and one controller exists"),
    );
    let s = state(app);
    ctx.ok(
        s["title"] == json!("Flare test page")
            && s["bar"]["url"]
                .as_str()
                .is_some_and(|u| u.ends_with("/index.html")),
        format!(
            "{at}: the panel is named for the page and the bar holds its address ({} / {})",
            s["title"], s["bar"]["url"]
        ),
    );

    // ---- where the browser window is: the browser, Slint and the maths agree ----
    let ctl = &s["controller"];
    let bounds = &ctl["bounds"];
    ctx.ok(
        ctl["visible"] == json!(true) && int(&bounds["w"]) > 0 && int(&bounds["h"]) > 0,
        format!("{at}: the controller is visible with bounds {bounds}"),
    );
    ctx.ok(
        ctl["autofill"] == json!(true),
        format!(
            "{at}: it offers to save passwords and fills in forms ({})",
            ctl["autofill"]
        ),
    );
    ctx.ok(
        same_rect(bounds, &s["drawn"], 1),
        format!(
            "{at}: its bounds equal the box Slint laid out for it ({} vs {})",
            bounds, s["drawn"]
        ),
    );
    ctx.ok(
        !s["maths"].is_null() && same_rect(bounds, &s["maths"], 2),
        format!(
            "{at}: and the layout maths of flare-surfaces says the same ({} vs {})",
            bounds, s["maths"]
        ),
    );
    let children = ctl["children"].as_array().cloned().unwrap_or_default();
    let child = children.iter().find(|c| same_rect(c, bounds, 1));
    ctx.ok(
        child.is_some(),
        format!(
            "{at}: a child window of Flare's window sits exactly there ({:?} among {})",
            child.map(|c| c["class"].clone()),
            children.len()
        ),
    );
    let client_w = (1000.0 * scale).round() as i64;
    ctx.ok(
        int(&bounds["x"]) + int(&bounds["w"]) == client_w,
        format!("{at}: the page ends at the window's right edge"),
    );

    // ---- the page is alive, and the script sees it ----
    let title = script(ctx, app, "document.title");
    ctx.ok(
        title.as_deref() == Some("\"Flare test page\""),
        format!("{at}: a script reads the page's title ({title:?})"),
    );

    // ---- an overlay hides the browser window and a still takes its place ----
    if let Some(g) = probe(app, "ui.rect", json!({"name": "grid"}))["dx"].as_f64() {
        let gy = probe(app, "ui.rect", json!({"name": "grid"}))["dy"]
            .as_f64()
            .unwrap_or(0.0);
        app.session
            .right_click(((g + 60.0) / scale) as f32, ((gy + 60.0) / scale) as f32)?;
        let hidden = ctx.until(
            "the browser window to hide behind the menu",
            8_000,
            &mut || {
                let s = state(app);
                s["overlay_open"] == json!(true)
                    && s["phase"] == json!("still")
                    && s["controller"]["visible"] == json!(false)
            },
        );
        ctx.ok(
            hidden,
            format!("{at}: a menu over the terminal hides the browser window"),
        );
        let s = state(app);
        ctx.ok(
            s["still"]["shown"] == json!(true) && int(&s["still"]["w"]) > 0,
            format!(
                "{at}: and a still of its last frame is drawn in its place ({} x {})",
                s["still"]["w"], s["still"]["h"]
            ),
        );
        app.session.key("Escape")?;
        let back = ctx.until("the browser window to come back", 8_000, &mut || {
            let s = state(app);
            s["overlay_open"] == json!(false)
                && s["phase"] == json!("live")
                && s["controller"]["visible"] == json!(true)
        });
        ctx.ok(
            back,
            format!("{at}: closing the menu shows the browser window again"),
        );
        ctx.ok(
            state(app)["still"]["shown"] == json!(false),
            format!("{at}: and the still is let go"),
        );
    }

    // ---- navigation through the channel ----
    let two = c.call(
        Method::BrowserNavigate,
        json!({ "url": format!("{HOST}/two.html") }),
        LOAD,
    );
    ctx.ok(
        two.as_ref().is_ok_and(|v| {
            v["title"] == json!("Page two")
                && v["url"].as_str().is_some_and(|u| u.ends_with("/two.html"))
        }),
        format!("{at}: browser.navigate to a url ({two:?})"),
    );
    let back = c.call(Method::BrowserNavigate, json!({ "action": "back" }), LOAD);
    ctx.ok(
        back.as_ref()
            .is_ok_and(|v| v["title"] == json!("Flare test page")),
        format!("{at}: browser.navigate back ({back:?})"),
    );
    let fwd = c.call(
        Method::BrowserNavigate,
        json!({ "action": "forward" }),
        LOAD,
    );
    ctx.ok(
        fwd.as_ref().is_ok_and(|v| v["title"] == json!("Page two")),
        format!("{at}: browser.navigate forward ({fwd:?})"),
    );
    let rel = c.call(Method::BrowserNavigate, json!({ "action": "reload" }), LOAD);
    ctx.ok(
        rel.as_ref().is_ok_and(|v| v["title"] == json!("Page two")),
        format!("{at}: browser.navigate reload ({rel:?})"),
    );
    let bar_ok = ctx.until("the bar to follow the page", 5_000, &mut || {
        let s = state(app);
        s["bar"]["url"]
            .as_str()
            .is_some_and(|u| u.ends_with("/two.html"))
            && s["bar"]["can_back"] == json!(true)
    });
    ctx.ok(
        bar_ok,
        format!("{at}: the bar shows the address and back is live"),
    );

    // ---- refused addresses and wrong calls ----
    for (bad, why) in [
        ("file:///C:/Windows/win.ini", "a file: address"),
        ("javascript:alert(1)", "a javascript: address"),
        ("data:text/html,x", "a data: address"),
        ("https://user:pw@example.com/", "an address with a login"),
    ] {
        let r = c.call(Method::BrowserOpen, json!({ "url": bad }), LOAD);
        ctx.ok(
            code_of(&r) == "bad_params",
            format!("{at}: browser.open refuses {why} ({})", code_of(&r)),
        );
    }
    let both = c.call(
        Method::BrowserNavigate,
        json!({ "url": format!("{HOST}/two.html"), "action": "back" }),
        LOAD,
    );
    ctx.ok(
        code_of(&both) == "bad_params",
        format!(
            "{at}: browser.navigate wants a url or an action, not both ({})",
            code_of(&both)
        ),
    );
    ctx.ok(
        state(app)["url"]
            .as_str()
            .is_some_and(|u| u.ends_with("/two.html")),
        format!("{at}: the refusals changed nothing in the page"),
    );

    // ---- what a page may open (rule 249), and it gets no permission ----
    script(ctx, app, &format!("location.href = '{HOST}/index.html'; 1"));
    let on_index = ctx.until("the index page", 15_000, &mut || {
        state(app)["url"]
            .as_str()
            .is_some_and(|u| u.ends_with("/index.html"))
            && state(app)["controller"]["title"] == json!("Flare test page")
    });
    ctx.ok(on_index, format!("{at}: back on the first page"));
    let browser_pid = u32::try_from(
        state(app)["controller"]["browser_pid"]
            .as_u64()
            .unwrap_or(0),
    )
    .unwrap_or(0);
    ctx.ok(
        browser_pid != 0,
        format!("{at}: the browser has a process id"),
    );
    let owners = [app.child.pid(), browser_pid];
    let windows_before = crate::proc::visible_top_windows_of(&owners);
    // A page that opens a window as it loads, with no click at all: the run turns the browser's own popup blocker
    // off, so the browser asks Flare, and Flare opens nothing, neither in the panel nor as a pop-up, and says so in
    // the bar. (A click the test hook's script makes is not this case: WebView2 counts a script it runs as the
    // person's gesture, lessons.md.)
    let auto = c.call(
        Method::BrowserNavigate,
        json!({ "url": format!("{HOST}/auto.html") }),
        LOAD,
    );
    let said = auto.is_ok()
        && ctx.until("the bar to say the pop-up was blocked", 15_000, &mut || {
            state(app)["sub"] == json!("Pop-up blocked")
        });
    let s = state(app);
    ctx.ok(
        said
            && s["url"].as_str().is_some_and(|u| u.ends_with("/auto.html"))
            && s["controllers"] == json!(1)
            && s["controller"]["popups"]
                .as_array()
                .is_some_and(Vec::is_empty)
            && crate::proc::visible_top_windows_of(&owners) <= windows_before,
        format!(
            "{at}: a window.open no click asked for opens nothing and the bar says \"Pop-up blocked\" ({} / {})",
            s["sub"], s["url"]
        ),
    ); // The browser tools' click is a trusted one (DevTools input events): it counts as the person's.
    let pop = c.call(
        Method::BrowserNavigate,
        json!({ "url": format!("{HOST}/pop.html") }),
        LOAD,
    );
    ctx.ok(
        pop.as_ref().is_ok_and(|v| v["title"] == json!("Pop page")),
        format!("{at}: to the pop-up page ({})", code_of(&pop)),
    );
    let r = c.call(Method::BrowserClick, json!({ "x": 120, "y": 130 }), LOAD);
    ctx.ok(
        r.is_ok(),
        format!("{at}: a trusted click on the window.open ({r:?})"),
    );
    let opened = ctx.until("the pop-up to open over the page", 15_000, &mut || {
        let s = state(app);
        s["controller"]["popups"].as_array().is_some_and(|p| {
            p.len() == 1
                && p[0]["url"]
                    .as_str()
                    .is_some_and(|u| u.ends_with("/popup.html"))
        }) && s["strip"]["h"].as_f64().is_some_and(|h| h > 0.0)
    });
    let s = state(app);
    let p = s["controller"]["popups"][0].clone();
    ctx.ok(
        opened
            && s["strip"]["text"] == json!("Pop-up from flare-test.local")
            && s["popup_open"] == json!(true),
        format!(
            "{at}: a trusted click on a sized, named window.open opens one pop-up under Flare's strip ({})",
            s["strip"]
        ),
    );
    ctx.ok(
        p["visible"] == json!(true)
            && s["controller"]["visible"] == json!(false)
            && same_rect(&p["bounds"], &s["drawn"], 1)
            && p["autofill"] == json!(true),
        format!(
            "{at}: the pop-up fills the page's box under the strip and the page waits hidden ({} vs {}, page visible {})",
            p["bounds"], s["drawn"], s["controller"]["visible"]
        ),
    );
    ctx.ok(
        same_rect(&s["drawn"], &s["maths"], 2),
        format!(
            "{at}: and the layout maths with the strip say the same ({} vs {})",
            s["drawn"], s["maths"]
        ),
    );
    ctx.ok(
        crate::proc::visible_top_windows_of(&owners) <= windows_before,
        format!(
            "{at}: and no window of its own appears ({windows_before} top-level windows before)"
        ),
    );
    // The pop-up reached back to its opener: `window.opener` and the window's name are the page's own.
    let linked = ctx.until("the opener to hear from its pop-up", 15_000, &mut || {
        state(app)["title"] == json!("opener:threeDS")
    });
    let read = script(ctx, app, "document.title");
    ctx.ok(
        linked && read.as_deref() == Some("\"opener:threeDS\""),
        format!(
            "{at}: the pop-up keeps its link to the page that opened it, and its name ({read:?})"
        ),
    );
    let refused = c.call(Method::BrowserClick, json!({ "x": 120, "y": 30 }), LOAD);
    ctx.ok(
        code_of(&refused) == "unavailable" && format!("{refused:?}").contains("a pop-up is open"),
        format!(
            "{at}: the agent's click waits for the person while the pop-up is open ({refused:?})"
        ),
    );
    // The pop-up's own window.close() closes it and the page shows again.
    script(ctx, app, "window.w.postMessage('close', '*'); 1");
    let closed = ctx.until("the pop-up to close itself", 15_000, &mut || {
        let s = state(app);
        s["controller"]["popups"]
            .as_array()
            .is_some_and(Vec::is_empty)
            && s["controller"]["visible"] == json!(true)
    });
    let s = state(app);
    ctx.ok(
        closed && s["strip"]["h"] == json!(0.0) && s["popup_open"] == json!(false),
        format!(
            "{at}: window.close() in the pop-up closes it and the page shows again ({})",
            s["strip"]
        ),
    );
    ctx.ok(
        same_rect(&s["controller"]["bounds"], &s["drawn"], 1),
        format!(
            "{at}: and the page has its whole box back ({} vs {})",
            s["controller"]["bounds"], s["drawn"]
        ),
    );
    // Again, and the strip's Close (Flare's own chrome) closes it.
    let r = c.call(Method::BrowserClick, json!({ "x": 120, "y": 130 }), LOAD);
    let again = r.is_ok()
        && ctx.until("the pop-up to open again", 15_000, &mut || {
            state(app)["controller"]["popups"]
                .as_array()
                .is_some_and(|p| p.len() == 1)
                && state(app)["strip"]["h"].as_f64().is_some_and(|h| h > 0.0)
        });
    ctx.ok(again, format!("{at}: a second click opens it again"));
    if again {
        let s = state(app);
        let strip_h = s["strip"]["h"].as_f64().unwrap_or(0.0);
        let right = int(&s["drawn"]["right"]) as f64 / scale;
        let top = int(&s["drawn"]["y"]) as f64 / scale;
        app.session
            .click((right - 8.0 - 12.0) as f32, (top - strip_h / 2.0) as f32)?;
        let shut = ctx.until("the strip's Close to close the pop-up", 15_000, &mut || {
            let s = state(app);
            s["controller"]["popups"]
                .as_array()
                .is_some_and(Vec::is_empty)
                && s["controller"]["visible"] == json!(true)
        });
        ctx.ok(
            shut,
            format!("{at}: the strip's Close closes the pop-up and shows the page"),
        );
    }
    // A plain new-tab link from a trusted click opens in the panel, as it always did.
    let r = c.call(Method::BrowserClick, json!({ "x": 120, "y": 30 }), LOAD);
    let plain = r.is_ok()
        && ctx.until(
            "target=_blank to open in the same panel",
            15_000,
            &mut || {
                state(app)["url"]
                    .as_str()
                    .is_some_and(|u| u.ends_with("/three.html"))
            },
        );
    let s = state(app);
    ctx.ok(
        plain
            && s["controllers"] == json!(1)
            && s["controller"]["popups"]
                .as_array()
                .is_some_and(Vec::is_empty),
        format!(
            "{at}: a trusted click on a target=_blank link opens it in the same panel ({})",
            s["url"]
        ),
    );
    // Whatever it did, the next step starts from the first page.
    let home = c.call(Method::BrowserNavigate, json!({ "url": url }), LOAD);
    ctx.ok(
        home.is_ok(),
        format!("{at}: and back to the first page ({})", code_of(&home)),
    );
    script(ctx, app, "document.getElementById('geo').click(); 1");
    let denied = ctx.until("the location request to be answered", 15_000, &mut || {
        state(app)["title"]
            .as_str()
            .is_some_and(|t| t.starts_with("geo:"))
    });
    ctx.ok(
        denied && state(app)["title"] == json!("geo:err1"),
        format!(
            "{at}: a location request is denied ({})",
            state(app)["title"]
        ),
    );

    // ---- a download is held until the person answers (S6) ----
    let downloads = downloads_folder();
    let saved_before = downloaded(&downloads);
    script(ctx, app, "document.getElementById('dl').click(); 1");
    let asked = ctx.until("the download question", 15_000, &mut || {
        probe(app, "app.state", Value::Null)["dialog"]["open"] == json!(true)
    });
    let q = probe(app, "app.state", Value::Null)["dialog"].clone();
    ctx.ok(
        asked
            && q["title"] == json!("Save this file?")
            && q["body"].as_str().is_some_and(|b| {
                b.contains("flare-e2e-download-test") && b.contains("flare-test.local")
            }),
        format!("{at}: a download asks first, naming the file and the host ({q})"),
    );
    // Nothing is saved while the question is up (a few seconds is longer than a small file takes).
    quiet_downloads(ctx, &downloads, &saved_before);
    ctx.ok(
        downloaded(&downloads) == saved_before,
        format!("{at}: and nothing is saved while the question is open"),
    );
    app.session.key("Escape")?;
    let closed_q = ctx.until("the question to close on Escape", 6_000, &mut || {
        probe(app, "app.state", Value::Null)["dialog"]["open"] == json!(false)
    });
    quiet_downloads(ctx, &downloads, &saved_before);
    ctx.ok(
        closed_q && downloaded(&downloads) == saved_before,
        format!("{at}: a no cancels it and no file reaches the Downloads folder"),
    );

    // ---- the splitter moves the browser window with it ----
    let w_before = int(&state(app)["controller"]["bounds"]["w"]);
    if let Some(sp) = probe(app, "ui.rect", json!({"name": "splitter"}))["dx"].as_i64() {
        let spw = probe(app, "ui.rect", json!({"name": "splitter"}))["dw"]
            .as_i64()
            .unwrap_or(0);
        let from = (
            (sp + spw / 2) as f64 / scale,
            (probe(app, "ui.rect", json!({"name": "splitter"}))["dy"]
                .as_i64()
                .unwrap_or(0)
                + 200) as f64
                / scale,
        );
        app.session.call(&Request::Drag {
            from: (from.0 as f32, from.1 as f32),
            to: ((from.0 - 100.0) as f32, from.1 as f32),
            steps: 8,
            button: Button::Left,
        })?;
        let wider = ctx.until(
            "the browser window to widen with the slot",
            6_000,
            &mut || {
                let s = state(app);
                int(&s["controller"]["bounds"]["w"]) > w_before + (90.0 * scale) as i64
                    && same_rect(&s["controller"]["bounds"], &s["drawn"], 1)
            },
        );
        let s = state(app);
        ctx.ok(
            wider && !s["maths"].is_null() && same_rect(&s["controller"]["bounds"], &s["maths"], 2),
            format!(
                "{at}: dragging the splitter 100 css px left widens the browser window and it still equals the maths ({} vs {})",
                s["controller"]["bounds"], s["maths"]
            ),
        );
    } else {
        ctx.ok(false, format!("{at}: the splitter has a rectangle"));
    }

    // ---- a change of display scale (a monitor of another scale) with the page open ----
    let other_dpi = if dpi == 96 { 144 } else { 96 };
    let other = f64::from(other_dpi) / 96.0;
    app.session.call(&Request::DpiChange {
        dpi: other_dpi,
        x: None,
        y: None,
    })?;
    let followed = ctx.until("the page to follow the new scale", 10_000, &mut || {
        let s = state(app);
        s["scale"]
            .as_f64()
            .is_some_and(|k| (k - other).abs() < 0.01)
            && same_rect(&s["controller"]["bounds"], &s["drawn"], 1)
            && !s["maths"].is_null()
            && same_rect(&s["controller"]["bounds"], &s["maths"], 2)
    });
    let s = state(app);
    ctx.ok(
        followed,
        format!(
            "{at}: a change to {other}x puts the page where the new layout says ({} vs {})",
            s["controller"]["bounds"], s["maths"]
        ),
    );
    app.session.call(&Request::DpiChange {
        dpi,
        x: None,
        y: None,
    })?;
    let home = ctx.until("the page to follow the scale back", 10_000, &mut || {
        let s = state(app);
        s["scale"]
            .as_f64()
            .is_some_and(|k| (k - scale).abs() < 0.01)
            && same_rect(&s["controller"]["bounds"], &s["drawn"], 1)
            && !s["maths"].is_null()
            && same_rect(&s["controller"]["bounds"], &s["maths"], 2)
    });
    ctx.ok(home, format!("{at}: and back again"));

    // ---- another tab: the page is hidden, then back ----
    // The second leg of a world restores the first leg's tabs, so count rather than assume.
    let tab_ids = |app: &mut App| -> Vec<String> {
        probe(app, "ctl.info", Value::Null)["tabs"]
            .as_array()
            .map(|t| {
                t.iter()
                    .filter_map(|t| t["id"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    let before = tab_ids(app);
    let place = before
        .iter()
        .position(|t| *t == front_id)
        .map_or(1, |i| i + 1);
    app.session.key("Ctrl+T")?;
    let one_more = ctx.until("one more tab", 15_000, &mut || {
        tab_ids(app).len() == before.len() + 1
    });
    ctx.ok(
        one_more,
        format!(
            "{at}: Ctrl+T opened one more tab ({} to {})",
            before.len(),
            tab_ids(app).len()
        ),
    );
    let hidden = ctx.until("the page to hide with its tab behind", 8_000, &mut || {
        let s = state(app);
        s["phase"] == json!("hidden")
            && s["views"]
                .as_array()
                .is_some_and(|v| v.len() == 1 && v[0]["visible"] == json!(false))
    });
    ctx.ok(
        hidden && state(app)["controllers"] == json!(1),
        format!("{at}: with its tab behind the page is hidden and still there"),
    );
    app.session.key(&format!("Ctrl+{place}"))?;
    let shown = ctx.until("the page to show again", 8_000, &mut || {
        let s = state(app);
        s["phase"] == json!("live") && s["controller"]["visible"] == json!(true)
    });
    ctx.ok(shown, format!("{at}: back on its tab the page shows again"));
    let _ = front_tab(app);

    // ---- closing the panel frees the browser ----
    let pid = u32::try_from(
        state(app)["controller"]["browser_pid"]
            .as_u64()
            .unwrap_or(0),
    )
    .unwrap_or(0);
    ctx.ok(pid != 0, format!("{at}: the browser's process id is known"));
    let closed = c.call(Method::PanelClose, json!({}), LOAD);
    ctx.ok(
        closed.as_ref().is_ok_and(|v| v["closed"] == json!(true)),
        format!("{at}: panel.close closes the web panel ({closed:?})"),
    );
    let freed = ctx.until("the controller to be freed", 8_000, &mut || {
        let s = state(app);
        s["controllers"] == json!(0) && s["backend"] == json!(false)
    });
    ctx.ok(
        freed,
        format!("{at}: closing the panel frees the controller and the browser object"),
    );
    if pid != 0 {
        let gone = ctx.until("the browser process to end", 30_000, &mut || {
            !crate::proc::pid_alive(pid)
        });
        ctx.ok(
            gone,
            format!("{at}: and the browser's own process {pid} ends"),
        );
    }

    // ---- the chord brings the page back ----
    app.session.key("Ctrl+Shift+P")?;
    let again = ctx.until("the page to come back", 40_000, &mut || {
        let s = state(app);
        s["controllers"] == json!(1) && s["controller"]["visible"] == json!(true)
    });
    ctx.ok(again, format!("{at}: Ctrl+Shift+P brings the page back"));
    let s = state(app);
    ctx.ok(
        s["url"]
            .as_str()
            .is_some_and(|u| u.ends_with("/index.html"))
            && same_rect(&s["controller"]["bounds"], &s["drawn"], 1),
        format!(
            "{at}: at the page it was on, in the right place ({})",
            s["url"]
        ),
    );
    let _ = c.call(Method::PanelClose, json!({}), LOAD);
    let freed = ctx.until("the controller to be freed again", 8_000, &mut || {
        state(app)["controllers"] == json!(0)
    });
    ctx.ok(
        freed,
        format!("{at}: and closing it once more frees it again"),
    );
    // One more time, and the page stays open: the window quits with it up, and the caller watches the
    // browser's process end.
    app.session.key("Ctrl+Shift+P")?;
    let last = ctx.until("the page to open for the quit", 40_000, &mut || {
        let s = state(app);
        s["controllers"] == json!(1) && s["controller"]["browser_pid"].as_u64().unwrap_or(0) != 0
    });
    ctx.ok(last, format!("{at}: opened once more to quit with it up"));
    Ok(u32::try_from(
        state(app)["controller"]["browser_pid"]
            .as_u64()
            .unwrap_or(0),
    )
    .unwrap_or(0))
}

/// The Downloads folder the browser saves to by default: the profile's own `Downloads`.
fn downloads_folder() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_default()
        .join("Downloads")
}

/// Waits two seconds (longer than a small file takes) or until a copy appears, whichever is first.
fn quiet_downloads(ctx: &mut Ctx, dir: &Path, before: &[String]) {
    let from = Instant::now();
    ctx.until("two quiet seconds in Downloads", 6_000, &mut || {
        downloaded(dir) != before || from.elapsed() >= Duration::from_secs(2)
    });
}

/// The names in the folder that start with the test's file name (a saved copy, or a numbered one).
fn downloaded(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.starts_with("flare-e2e-download-test"))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// Makes a 6 second clip with ffmpeg (the machine has it; the gate machine is this one). `codec` is the
/// ffmpeg video encoder: `libvpx` for the webm (no codec the Windows edition might lack), `libx264`
/// for the mp4 (H.264, what most local files are).
fn make_clip(path: &Path, codec: &str) -> Result<(), String> {
    let out = Command::new("ffmpeg")
        .args([
            "-y",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=6:size=320x240:rate=30",
            "-c:v",
            codec,
            "-b:v",
            "300k",
            "-pix_fmt",
            "yuv420p",
        ])
        .arg(path)
        .output()
        .map_err(|e| format!("ffmpeg is needed to make the test clip (install it, e.g. winget install Gyan.FFmpeg): {e}"))?;
    if out.status.success() && path.is_file() {
        Ok(())
    } else {
        Err(format!(
            "ffmpeg failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ))
    }
}

pub fn web_video(ctx: &mut Ctx) -> Result<(), String> {
    let root = web_root(ctx)?;
    make_clip(&root.join("clip.webm"), "libvpx")?;
    make_clip(&root.join("clip.mp4"), "libx264")?;
    let mut app = launch_with(ctx, &root, 96, &[])?;
    let mut r = video_on(ctx, &mut app, "clip.webm");
    if r.is_ok() {
        r = video_on(ctx, &mut app, "clip.mp4");
    }
    let left = app.close();
    ctx.ok(left, "the app quits by itself");
    r
}

fn video_on(ctx: &mut Ctx, app: &mut App, clip: &str) -> Result<(), String> {
    app.session.call(&Request::Resize {
        w: 1000.0,
        h: 640.0,
    })?;
    prompt(app)?;
    let (pipe, _, token) = channel_of(app)?;
    let mut c = hello(&pipe, &token).map_err(|e| format!("hello: {e}"))?;
    let r = c.call(
        Method::BrowserOpen,
        json!({ "url": format!("{HOST}/video.html?clip={clip}") }),
        LOAD,
    );
    ctx.ok(
        r.as_ref().is_ok_and(|v| {
            v["loaded"] == json!(true)
                && v["url"].as_str().is_some_and(|u| u.contains("/video.html"))
        }),
        format!("{clip}: the video page opens ({r:?})"),
    );
    let ready = script(ctx, app, "document.getElementById('v').readyState");
    ctx.ok(
        ready.is_some(),
        format!("{clip}: the page answers a script ({ready:?})"),
    );
    // The page publishes the clip's clock and frame counter as its title every 100 ms. The clock must
    // start and then advance at the wall clock's rate; the frame counter at the clip's 30 frames a second.
    let stats = |app: &mut App| -> Option<Value> {
        let t = state(app)["controller"]["title"].as_str()?.to_string();
        serde_json::from_str(&t).ok()
    };
    let mut first: Option<(Instant, Value)> = None;
    let started = ctx.until(
        "the video's clock to start",
        30_000,
        &mut || match stats(app) {
            Some(v) if v["t"].as_f64().unwrap_or(0.0) > 0.3 && v["paused"] == json!(false) => {
                first = Some((Instant::now(), v));
                true
            }
            _ => false,
        },
    );
    ctx.ok(
        started,
        format!(
            "{clip}: the video's clock starts ({:?})",
            first.as_ref().map(|f| &f.1)
        ),
    );
    if let Some((t0, v0)) = first {
        // Two seconds of the clip's own clock (a loop of the 6 second clip cannot wrap inside them).
        let want = v0["t"].as_f64().unwrap_or(0.0) + 2.0;
        let mut last = None;
        let advanced = ctx.until("two seconds of video", 15_000, &mut || match stats(app) {
            Some(v) if v["t"].as_f64().unwrap_or(0.0) >= want => {
                last = Some((t0.elapsed().as_secs_f64(), v));
                true
            }
            _ => false,
        });
        ctx.ok(
            advanced,
            format!("{clip}: the clock reaches two seconds on from where it began"),
        );
        if let Some((wall, v1)) = last {
            let media = v1["t"].as_f64().unwrap_or(0.0) - v0["t"].as_f64().unwrap_or(0.0);
            let rate = media / wall;
            ctx.ok(
                (0.85..=1.15).contains(&rate),
                format!("{clip}: the clock advances at the wall clock's rate ({media:.2} s of video in {wall:.2} s, {rate:.2}x)"),
            );
            let frames =
                v1["frames"].as_f64().unwrap_or(0.0) - v0["frames"].as_f64().unwrap_or(0.0);
            let per_sec = frames / wall;
            ctx.ok(
                per_sec >= 24.0,
                format!(
                    "{clip}: frames are decoded at the clip's rate ({per_sec:.1} per second; {} dropped)",
                    v1["dropped"]
                ),
            );
        }
    }
    let _ = c.call(Method::PanelClose, json!({}), LOAD);
    Ok(())
}

pub fn web_no_runtime(ctx: &mut Ctx) -> Result<(), String> {
    let root = web_root(ctx)?;
    let mut app = launch_with(ctx, &root, 96, &[("FLARE_E2E_WEBVIEW_RUNTIME", "missing")])?;
    let r = no_runtime_on(ctx, &mut app);
    let left = app.close();
    ctx.ok(left, "the app quits by itself");
    r
}

fn no_runtime_on(ctx: &mut Ctx, app: &mut App) -> Result<(), String> {
    app.session.call(&Request::Resize {
        w: 1000.0,
        h: 640.0,
    })?;
    prompt(app)?;
    let (pipe, _, token) = channel_of(app)?;
    let mut c = hello(&pipe, &token).map_err(|e| format!("hello: {e}"))?;
    let r = c.call(
        Method::BrowserOpen,
        json!({ "url": format!("{HOST}/index.html") }),
        LOAD,
    );
    ctx.ok(
        code_of(&r) == "unavailable",
        format!(
            "browser.open answers unavailable without the runtime ({})",
            code_of(&r)
        ),
    );
    let msg = match &r {
        Err(ClientError::Remote(e)) => e.message.clone(),
        _ => String::new(),
    };
    ctx.ok(
        msg.contains("go.microsoft.com"),
        format!("and the answer carries the download link ({msg})"),
    );
    let shown = ctx.until("the panel to say so", 8_000, &mut || {
        let s = state(app);
        s["shown"] == json!(true) && s["bar"]["missing_runtime"] == json!(true)
    });
    ctx.ok(
        shown,
        "the panel shows the message with the download button",
    );
    let s = state(app);
    ctx.ok(
        s["note"]
            .as_str()
            .is_some_and(|n| n.contains("https://go.microsoft.com/fwlink/p/?LinkId=2124703"))
            && s["controllers"] == json!(0),
        format!(
            "the note holds the link and no controller exists ({})",
            s["note"]
        ),
    );
    // Nothing else breaks: the terminal still answers, and another panel still opens.
    let alive = probe(app, "dpi.state", Value::Null)["grid_cols"]
        .as_u64()
        .unwrap_or(0)
        > 0;
    ctx.ok(alive, "the terminal is untouched");
    let t = c.call(
        Method::ViewText,
        json!({ "text": "still fine", "title": "Text" }),
        LOAD,
    );
    ctx.ok(
        t.is_ok(),
        format!("and a text panel still opens ({})", code_of(&t)),
    );
    Ok(())
}

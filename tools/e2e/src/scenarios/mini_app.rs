//! Mini-apps and diagrams (Surfaces stage 5): `miniApp`, `diagram` and `mcpDiagram`.
//!
//! Each starts the built release exe in its own world and profile and speaks to it over the control
//! channel with the `flare-ctl` client (or `flare-mcp.exe` over stdio). Nothing is fetched from the
//! network: the page a mini-app is asked to load from "outside" points at `http://127.0.0.1:9/` (the
//! discard port, nothing listens there), so a blocked request is told apart from a refused connection by
//! the page's own `securitypolicyviolation` events, never by a failed connection. The window is parked
//! off the screen and never takes the foreground, like every scenario.
//!
//! `miniApp` holds: the page renders and its script runs; the mini-app's view sits in the slot like any
//! web page, in a private profile of its own (`flare-app`, apart from the web panel's); the policy
//! blocks a fetch, an image and a script from outside, and a script that removes the policy tag does not
//! lift it; a navigation away, a link and a new window all go nowhere; a page named by path is vetted;
//! the size cap; the network switch (a second launch with it on); a second app is another origin with
//! its own folder and the first folder goes; the agent's browser tools work on the page; closing ends
//! it and its folder; and the folders of an earlier run are gone at start. The second wall (every
//! request that is not the app's own is answered with a refusal) is checked with the policy switched
//! off by the e2e seam `FLARE_E2E_APP_NO_POLICY`, which exists only under `--e2e`.
//!
//! `diagram` draws Mermaid text with the bundled build: an `svg` is in the page, a second kind of
//! diagram draws, and a diagram that does not parse fails the call with Mermaid's own error text while
//! the panel shows it. `mcpDiagram` does the same through `flare-mcp.exe`'s `show_diagram` and `show_html`.

use std::fs;
use std::path::{Path, PathBuf};

use flare_ctl::{Client, ClientError, Method, PipeEnd};
use flare_e2e_proto::Request;
use serde_json::{json, Value};

use super::agent_web::{is_error, text_of, Server};
use super::input_b::{probe, prompt};
use super::web::{channel_of, code_of, hello, launch_with, script, state, LOAD};
use crate::ctx::{App, Ctx};

const PAGE: &str = r##"<!doctype html><html><head><meta charset="utf-8"><title>Chart page</title></head>
<body style="margin:0;background:#2a6;color:#fff;font:20px sans-serif">
<h1 id="h">mini app</h1><button id="b">Press</button><div id="out" role="status">pending</div>
<script>
document.getElementById('out').textContent = 'script ran';
window.__v = [];
document.addEventListener('securitypolicyviolation', function (e) {
  window.__v.push(e.effectiveDirective + '|' + e.blockedURI);
});
document.getElementById('b').onclick = function () { document.getElementById('out').textContent = 'clicked'; };
</script></body></html>"##;

/// A file page for `path`.
/// Makes a srcdoc frame and an about:blank frame and records what `RTCPeerConnection` is in each, at once
/// and a moment later. A frame the policy blocks reports no window or a window without the API.
const FRAME_PROBE: &str = r##"(function(){
  window.__fr = {};
  ['srcdoc', 'blank'].forEach(function (k) {
    var f = document.createElement('iframe');
    if (k === 'srcdoc') { f.srcdoc = '<p>x</p>'; }
    document.body.appendChild(f);
    var w = null;
    try { w = f.contentWindow; } catch (e) { window.__fr[k] = 'threw'; }
    if (!(k in window.__fr)) { window.__fr[k] = w ? typeof w.RTCPeerConnection : 'no-window'; }
    setTimeout(function () {
      var w2 = null;
      try { w2 = f.contentWindow; } catch (e) {}
      window.__fr[k + '-late'] = w2 ? typeof w2.RTCPeerConnection : 'no-window';
    }, 600);
  });
  setTimeout(function () { window.__fr.late = true; }, 1000);
  return 'started';
})()"##;

/// Starts a peer connection (in `MODE`: this window, or a srcdoc or about:blank frame's) with a STUN
/// server at 127.0.0.1:PORT and records the candidates it gathers; `window.__ice.done` is true when
/// the gathering ended or 8 seconds went by.
const ICE_PROBE: &str = r##"(function(){
  var R = window;
  if ('MODE' !== 'top') {
    var f = document.createElement('iframe');
    if ('MODE' === 'srcdoc') { f.srcdoc = '<p>x</p>'; }
    document.body.appendChild(f);
    R = f.contentWindow;
  }
  window.__ice = { done: false, cands: [], api: R ? typeof R.RTCPeerConnection : 'no-window', err: '' };
  if (!R || typeof R.RTCPeerConnection !== 'function') { window.__ice.done = true; return 'none'; }
  try {
    var pc = new R.RTCPeerConnection({ iceServers: [{ urls: 'stun:127.0.0.1:PORT' }] });
    pc.createDataChannel('x');
    pc.onicecandidate = function (e) { if (e.candidate) { window.__ice.cands.push(e.candidate.candidate); } else { window.__ice.done = true; } };
    pc.createOffer().then(function (o) { return pc.setLocalDescription(o); }).catch(function (e) { window.__ice.err = String(e); window.__ice.done = true; });
    setTimeout(function () { window.__ice.done = true; }, 8000);
  } catch (e) { window.__ice.err = 'threw ' + e; window.__ice.done = true; }
  return 'started';
})()"##;

/// A UDP socket on loopback that counts what is sent to it: the STUN server a peer connection would
/// have asked. Nothing leaves the PC.
struct Spy {
    sock: std::net::UdpSocket,
    port: u16,
    seen: usize,
}

impl Spy {
    fn new() -> Result<Self, String> {
        let sock = std::net::UdpSocket::bind("127.0.0.1:0").map_err(|e| format!("spy: {e}"))?;
        sock.set_nonblocking(true).map_err(|e| e.to_string())?;
        let port = sock.local_addr().map_err(|e| e.to_string())?.port();
        Ok(Self {
            sock,
            port,
            seen: 0,
        })
    }

    /// Packets that arrived since the last call.
    fn fresh(&mut self) -> usize {
        let mut buf = [0u8; 2048];
        let mut n = 0;
        while self.sock.recv_from(&mut buf).is_ok() {
            n += 1;
        }
        self.seen += n;
        n
    }
}

/// Runs the ICE probe in `mode` (top, srcdoc, blank) and gives the page's record as JSON text.
fn gather(ctx: &mut Ctx, app: &mut App, spy: &Spy, mode: &str) -> (bool, String) {
    let js = ICE_PROBE
        .replace("MODE", mode)
        .replace("PORT", &spy.port.to_string());
    say(ctx, app, &js);
    until_says(
        ctx,
        "the peer connection to finish gathering",
        25_000,
        app,
        "JSON.stringify(window.__ice)",
        &|s| s.contains("\"done\":true"),
    )
}

const FILE_PAGE: &str = r##"<!doctype html><html><head><meta charset="utf-8"><title>From a file</title></head>
<body><p id="p">from a file</p><script>document.getElementById('p').textContent = 'file script ran';</script></body></html>"##;

fn world_root(ctx: &mut Ctx) -> Result<PathBuf, String> {
    let root = ctx.ensure_world().root.join("web");
    fs::create_dir_all(&root).map_err(|e| format!("make the web folder: {e}"))?;
    Ok(root)
}

/// What a script says, as the text (the page's JSON string, decoded).
pub(super) fn say(ctx: &mut Ctx, app: &mut App, js: &str) -> String {
    script(ctx, app, js)
        .map(|s| decoded(&s))
        .unwrap_or_default()
}

/// A script's JSON answer as text: a JSON string is decoded, anything else is kept as it is.
fn decoded(json_text: &str) -> String {
    serde_json::from_str::<String>(json_text).unwrap_or_else(|_| json_text.to_string())
}

/// Runs `js` in the page over and over until its answer satisfies `ok`: each turn starts the script and
/// the next one reads its answer (the wait is `ctx.until`, the one wait primitive). Gives whether it
/// held and the last answer, with the quotes of a JSON string taken off.
pub(super) fn until_says(
    ctx: &mut Ctx,
    what: &str,
    ms: u64,
    app: &mut App,
    js: &str,
    ok: &dyn Fn(&str) -> bool,
) -> (bool, String) {
    let mut started = false;
    let mut last = String::new();
    let held = ctx.until(what, ms, &mut || {
        if !started {
            started = app
                .session
                .call(&Request::WebScript { js: js.to_string() })
                .is_ok();
            return false;
        }
        let r = probe(app, "web.script.result", Value::Null);
        if r["done"] != json!(true) {
            return false;
        }
        started = false;
        last = if r["ok"] == json!(true) {
            decoded(r["value"].as_str().unwrap_or_default())
        } else {
            String::new()
        };
        ok(&last)
    });
    (held, last)
}

/// Waits for the panel to be shown and its browser window to settle on the slot.
fn settle(ctx: &mut Ctx, app: &mut App, what: &str) {
    let mut last = Value::Null;
    ctx.until(what, 15_000, &mut || {
        let s = state(app);
        let now = s["controller"]["bounds"].clone();
        let steady = now == last;
        last = now;
        s["shown"] == json!(true) && s["controllers"] == json!(1) && !s["maths"].is_null() && steady
    });
}

fn same_rect(a: &Value, b: &Value) -> bool {
    ["x", "y", "w", "h"].iter().all(|k| a[*k] == b[*k])
}

fn app_folders(profile: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(profile.join("apps"))
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

pub fn mini_app(ctx: &mut Ctx) -> Result<(), String> {
    let root = world_root(ctx)?;
    let profile = ctx.ensure_world().profile.clone();
    let alpha = ctx.ensure_world().alpha.clone();
    fs::write(alpha.join("page.html"), FILE_PAGE).map_err(|e| e.to_string())?;
    fs::write(alpha.join("script.js"), "alert(1)").map_err(|e| e.to_string())?;
    fs::write(alpha.join("note.txt"), "text").map_err(|e| e.to_string())?;
    // The folders of an earlier run: no panel owns them, so they are gone when Flare starts.
    let stray = profile.join("apps").join("a99");
    fs::create_dir_all(&stray).map_err(|e| e.to_string())?;
    fs::write(stray.join("index.html"), "old").map_err(|e| e.to_string())?;
    for (dpi, label) in [(96u32, "100%"), (216, "225%")] {
        if std::env::var("WEB_DPI_ONLY").is_ok_and(|o| o != label) {
            continue;
        }
        let mut app = launch_with(ctx, &root, dpi, &[])?;
        if dpi == 96 {
            ctx.ok(
                !stray.exists(),
                "the folders of an earlier run are removed at start",
            );
        }
        let r = app_on(ctx, &mut app, label, &profile);
        let left = app.close();
        ctx.ok(left, format!("{label}: the app quits by itself"));
        r?;
    }
    // The network switch on, and the policy off (the e2e seam) to hold the second wall.
    fs::create_dir_all(&profile).map_err(|e| e.to_string())?;
    fs::write(
        profile.join("settings.json"),
        json!({"flare.web.appNetwork": "1"}).to_string(),
    )
    .map_err(|e| e.to_string())?;
    let mut app = launch_with(ctx, &root, 96, &[("FLARE_E2E_APP_NO_POLICY", "1")])?;
    let r = wall_on(ctx, &mut app);
    let left = app.close();
    ctx.ok(left, "network on: the app quits by itself");
    r?;
    let mut app = launch_with(ctx, &root, 96, &[])?;
    let r = network_on(ctx, &mut app);
    let left = app.close();
    ctx.ok(left, "network on, policy kept: the app quits by itself");
    r?;
    // The browser's own walls, alone: the script and the policy are both switched off by seams.
    let mut app = launch_with(
        ctx,
        &root,
        96,
        &[
            ("FLARE_E2E_APP_NO_POLICY", "1"),
            ("FLARE_E2E_APP_NO_WEBRTC_SCRIPT", "1"),
        ],
    )?;
    let r = webrtc_walls(ctx, &mut app, &root);
    let left = app.close();
    ctx.ok(left, "webrtc walls: the app quits by itself");
    r
}

fn show(
    ctx: &mut Ctx,
    c: &mut Client<PipeEnd>,
    at: &str,
    params: Value,
) -> Result<Value, ClientError> {
    let r = c.call(Method::ViewHtml, params, LOAD);
    ctx.ok(
        r.as_ref().is_ok_and(|v| v["loaded"] == json!(true)),
        format!("{at}: view.html answers once the page has loaded ({r:?})"),
    );
    r
}

fn app_on(ctx: &mut Ctx, app: &mut App, at: &str, profile: &Path) -> Result<(), String> {
    app.session.call(&Request::Resize {
        w: 1000.0,
        h: 640.0,
    })?;
    prompt(app)?;
    let (pipe, _, token) = channel_of(app)?;
    let mut c = hello(&pipe, &token).map_err(|e| format!("hello: {e}"))?;

    // ---- a page renders, its script runs, and it asks nothing of the person ----
    let v = show(
        ctx,
        &mut c,
        at,
        json!({"html": PAGE, "title": "Chart", "allow_network": true}),
    )
    .map_err(|e| format!("view.html: {e}"))?;
    ctx.ok(
        v["panel"]["kind"] == json!("app")
            && v["panel"]["title"] == json!("Chart")
            && v["panel"]["network"] == json!(false)
            && v["panel"].get("url").is_none(),
        format!(
            "{at}: the answer names an app panel with no network, though the agent asked ({v})"
        ),
    );
    settle(ctx, app, "the mini-app to be shown and settled");
    let s = state(app);
    ctx.ok(
        s["controller"]["url"]
            .as_str()
            .is_some_and(|u| u.starts_with("https://flare-app-1.local/")),
        format!(
            "{at}: it is served from a host of its own ({})",
            s["controller"]["url"]
        ),
    );
    ctx.ok(
        s["controller"]["profile"] == json!("flare-app")
            && s["controller"]["private"] == json!(true),
        format!(
            "{at}: it runs in the private profile of its own, not the web panel's ({} / {})",
            s["controller"]["profile"], s["controller"]["private"]
        ),
    );
    ctx.ok(
        s["title"] == json!("Chart")
            && s["bar"]["h"].as_f64() == Some(0.0)
            && s["kind"] == json!(3),
        format!(
            "{at}: the header is the agent's title and there is no address bar ({} / {})",
            s["title"], s["bar"]["h"]
        ),
    );
    ctx.ok(
        same_rect(&s["controller"]["bounds"], &s["maths"])
            && same_rect(&s["controller"]["bounds"], &s["drawn"]),
        format!(
            "{at}: the browser window sits exactly on the slot ({} / {} / {})",
            s["controller"]["bounds"], s["maths"], s["drawn"]
        ),
    );
    let out = say(ctx, app, "document.getElementById('out').textContent");
    ctx.ok(
        out == "script ran",
        format!("{at}: the page's script ran ({out:?})"),
    );
    let meta = say(
        ctx,
        app,
        "document.querySelector('meta[http-equiv=\"Content-Security-Policy\"]').content",
    );
    ctx.ok(
        meta.contains("default-src 'self' 'unsafe-inline' data: blob:;")
            && meta.contains("connect-src 'self'")
            && !meta.contains("https:"),
        format!("{at}: the policy sits in the page, with no network ({meta})"),
    );
    let first_pid = s["controller"]["browser_pid"].as_u64().unwrap_or(0);
    ctx.ok(
        app_folders(profile) == ["a1"],
        format!(
            "{at}: the app has a folder of its own ({:?})",
            app_folders(profile)
        ),
    );
    let first_folder = profile.join("apps").join("a1");
    ctx.ok(
        fs::read_dir(&first_folder).map(|d| d.count()).unwrap_or(0) == 1,
        format!("{at}: only the page is in it: relative files are not served"),
    );

    // ---- the policy blocks what leaves; a violation event is the proof, not a refused connection ----
    say(
        ctx,
        app,
        "(function(){\
           fetch('http://127.0.0.1:9/probe').then(function(){window.__r='ok'},function(){window.__r='err'});\
           var i=new Image(); i.src='http://127.0.0.1:9/pic.png';\
           var s=document.createElement('script'); s.src='http://127.0.0.1:9/lib.js'; document.head.appendChild(s);\
           return 'started';\
         })()",
    );
    let (got, seen) = until_says(
        ctx,
        "the policy's violation events",
        15_000,
        app,
        "JSON.stringify(window.__v)",
        &|s| s.matches("127.0.0.1:9").count() >= 3,
    );
    ctx.ok(
        got && seen.contains("connect-src|http://127.0.0.1:9/probe")
            && seen.contains("img-src|http://127.0.0.1:9/pic.png")
            && seen.contains("script-src")
            && seen.contains("http://127.0.0.1:9/lib.js"),
        format!("{at}: a fetch, an image and a script from outside are each a policy violation ({seen})"),
    );
    // The page takes its own policy tag out and tries again: the policy stays.
    say(
        ctx,
        app,
        "(function(){\
           document.querySelector('meta[http-equiv=\"Content-Security-Policy\"]').remove();\
           window.__v.length = 0;\
           fetch('http://127.0.0.1:9/again').catch(function(){});\
           return 'started';\
         })()",
    );
    let (got, seen) = until_says(
        ctx,
        "a violation after the tag is removed",
        15_000,
        app,
        "JSON.stringify(window.__v)",
        &|s| s.contains("127.0.0.1:9/again"),
    );
    ctx.ok(
        got && seen.contains("connect-src|http://127.0.0.1:9/again"),
        format!("{at}: removing the policy tag lifts nothing ({seen})"),
    );

    // ---- navigation away, a link and a new window go nowhere ----
    let app_url = state(app)["controller"]["url"].clone();
    say(
        ctx,
        app,
        "(function(){ location.href = 'http://127.0.0.1:9/nav'; return 'go'; })()",
    );
    let noted = ctx.until("the refusal to be noted", 15_000, &mut || {
        state(app)["sub"]
            .as_str()
            .is_some_and(|t| t.starts_with("Not opened here"))
    });
    ctx.ok(
        noted,
        format!("{at}: a navigation away is cancelled and said in the header"),
    );
    say(
        ctx,
        app,
        "(function(){ location.href = '/other.html'; return 'go'; })()",
    );
    say(
        ctx,
        app,
        "(function(){ window.open('http://127.0.0.1:9/win'); return 'go'; })()",
    );
    // Let any wrong navigation show before the address is read back.
    let moved = ctx.poll(3_000, &mut || {
        let s = state(app);
        s["controller"]["url"] != app_url || s["controllers"] != json!(1)
    });
    ctx.ok(
        !moved,
        format!(
            "{at}: the page stays at its own address with one view ({})",
            state(app)["controller"]["url"]
        ),
    );
    let out = say(ctx, app, "document.getElementById('out').textContent");
    ctx.ok(
        out == "script ran",
        format!("{at}: it is still the same document ({out:?})"),
    );

    // ---- an external protocol launches nothing, from the page or from a frame in it ----
    say(
        ctx,
        app,
        "(function(){ location.href = 'flare-e2e-probe:a?body=secret'; return 'go'; })()",
    );
    say(
        ctx,
        app,
        r##"(function(){ var f=document.createElement('iframe'); f.srcdoc="<a id=l href='flare-e2e-probe:b?body=secret'>x</a><script>location.href='flare-e2e-probe:b?body=secret'<\/script>"; document.body.appendChild(f); return 'go'; })()"##,
    );
    let moved = ctx.poll(3_000, &mut || {
        let s = state(app);
        s["controller"]["url"] != app_url || s["controllers"] != json!(1)
    });
    let out = say(ctx, app, "document.getElementById('out').textContent");
    ctx.ok(
        !moved && out == "script ran",
        format!("{at}: an unregistered protocol (nothing can launch) from the page and from a frame goes nowhere ({out:?})"),
    );

    // ---- the dead proxy's port is not what the wall rests on: a listener there hears nothing ----
    if let Ok(l) = std::net::TcpListener::bind("127.0.0.1:9") {
        let _ = l.set_nonblocking(true);
        say(
            ctx,
            app,
            "(function(){ fetch('http://example.invalid/x',{mode:'no-cors'}).catch(function(){}); var i=new Image(); i.src='http://example.invalid/p.png'; return 'go'; })()",
        );
        let heard = ctx.poll(4_000, &mut || l.accept().is_ok());
        ctx.ok(
            !heard,
            format!("{at}: a program listening on the proxy's port is never reached"),
        );
    }

    // ---- the agent's browser tools work on the page ----
    let look = c
        .call(Method::BrowserLook, json!({}), LOAD)
        .map_err(|e| format!("look: {e}"))?;
    let button = look["elements"]
        .as_array()
        .and_then(|a| {
            a.iter()
                .find(|e| e["role"] == json!("button") && e["name"] == json!("Press"))
        })
        .and_then(|e| e["ref"].as_str())
        .map(str::to_string);
    ctx.ok(
        look["image"]["mime"] == json!("image/png") && button.is_some(),
        format!("{at}: browser.look sees the mini-app's button and a picture ({button:?})"),
    );
    if let Some(b) = button {
        let r = c.call(Method::BrowserClick, json!({ "ref": b }), LOAD);
        let t = say(ctx, app, "document.getElementById('out').textContent");
        ctx.ok(
            r.is_ok() && t == "clicked",
            format!("{at}: browser.click works on it ({r:?} / {t:?})"),
        );
    }
    let r = c.call(
        Method::BrowserNavigate,
        json!({"url": "http://127.0.0.1:9/x"}),
        LOAD,
    );
    ctx.ok(
        code_of(&r) == "unavailable",
        format!("{at}: browser.navigate cannot send a mini-app anywhere ({r:?})"),
    );

    // ---- storage of one app is not seen by the next (another origin) ----
    say(
        ctx,
        app,
        "(function(){ localStorage.setItem('mine','1'); document.cookie='k=v'; return 'set'; })()",
    );
    // A cookie for a parent name is refused (the app's site is its whole host, `local` is a top-level
    // name), and the page has no peer connections (the policy cannot stop those).
    let parent = say(
        ctx,
        app,
        "(function(){ document.cookie='x=1; domain=local; path=/'; document.cookie='y=1; domain=flare-app.local; path=/'; return document.cookie; })()",
    );
    ctx.ok(
        parent == "k=v",
        format!("{at}: a cookie for a parent name is refused, only the host-only one is there ({parent:?})"),
    );
    let rtc = say(
        ctx,
        app,
        "[typeof RTCPeerConnection, typeof webkitRTCPeerConnection, typeof RTCDataChannel].join('/')",
    );
    ctx.ok(
        rtc == "undefined/undefined/undefined",
        format!("{at}: the page has no peer connections ({rtc:?})"),
    );
    // No child frames at all under the policy; if one is made anyway (about:blank and srcdoc are local
    // documents), it must not hand out a usable peer connection. Read at once (the new frame's first
    // document) and again after the frame has settled.
    ctx.ok(
        meta.contains("frame-src 'none'")
            && meta.contains("child-src 'none'")
            && meta.contains("worker-src 'self' blob:"),
        format!("{at}: the policy shuts child frames and limits workers ({meta})"),
    );
    say(ctx, app, FRAME_PROBE);
    let (done, frames) = until_says(
        ctx,
        "the frame probe to settle",
        15_000,
        app,
        "window.__fr ? JSON.stringify(window.__fr) : 'pending'",
        &|s| s.contains("\"late\":true"),
    );
    let usable = |k: &str| {
        frames.contains(&format!("\"{k}\":\"function"))
            || frames.contains(&format!("\"{k}-late\":\"function"))
    };
    ctx.ok(
        done && !usable("srcdoc") && !usable("blank"),
        format!("{at}: a srcdoc or about:blank frame has no usable peer connection ({frames})"),
    );

    // ---- a second app: another host, a folder of its own, and the first one's folder goes ----
    let v = show(
        ctx,
        &mut c,
        at,
        json!({"html": "<p id=two>second</p>", "title": "Second"}),
    )
    .map_err(|e| format!("second view.html: {e}"))?;
    ctx.ok(
        v["panel"]["title"] == json!("Second"),
        format!("{at}: the second app replaces the first ({v})"),
    );
    settle(ctx, app, "the second mini-app to settle");
    let s = state(app);
    ctx.ok(
        s["controller"]["url"]
            .as_str()
            .is_some_and(|u| u.starts_with("https://flare-app-2.local/"))
            && s["controllers"] == json!(1),
        format!(
            "{at}: it is another host, one view ({})",
            s["controller"]["url"]
        ),
    );
    ctx.ok(
        s["controller"]["browser_pid"].as_u64().unwrap_or(1) == first_pid,
        format!(
            "{at}: the browser was not restarted for it ({} then {})",
            first_pid, s["controller"]["browser_pid"]
        ),
    );
    let stored = say(
        ctx,
        app,
        "String(localStorage.getItem('mine')) + '/' + document.cookie.length",
    );
    ctx.ok(
        stored == "null/0",
        format!("{at}: nothing of the first app's storage or cookies is here ({stored:?})"),
    );
    let gone = ctx.until("the first folder to be removed", 15_000, &mut || {
        !first_folder.exists()
    });
    ctx.ok(
        gone && app_folders(profile) == ["a2"],
        format!(
            "{at}: the first app's folder is gone ({:?})",
            app_folders(profile)
        ),
    );

    // ---- a page named by path ----
    let v = show(
        ctx,
        &mut c,
        at,
        json!({"path": "page.html", "title": "File"}),
    )
    .map_err(|e| format!("path view.html: {e}"))?;
    ctx.ok(
        v["panel"]["title"] == json!("File"),
        format!("{at}: a relative html path is resolved against the tab's folder ({v})"),
    );
    settle(ctx, app, "the file page to settle");
    let t = say(ctx, app, "document.getElementById('p').textContent");
    ctx.ok(
        t == "file script ran",
        format!("{at}: the file's script ran ({t:?})"),
    );
    for (p, want) in [
        ("script.js", "not_found"),
        ("note.txt", "not_found"),
        ("missing.html", "not_found"),
        ("..\\..\\Windows\\win.ini", "not_found"),
        ("\\\\server\\share\\a.html", "not_found"),
    ] {
        let r = c.call(Method::ViewHtml, json!({ "path": p }), LOAD);
        ctx.ok(
            code_of(&r) == want,
            format!("{at}: view.html path {p:?} is refused as {want} ({r:?})"),
        );
    }

    // ---- caps and params ----
    let big = "x".repeat(4 * 1024 * 1024 + 1);
    let r = c.call(Method::ViewHtml, json!({ "html": big }), LOAD);
    ctx.ok(
        code_of(&r) == "bad_params",
        format!("{at}: html over 4 MB is refused ({})", code_of(&r)),
    );
    for bad in [
        json!({}),
        json!({"html": ""}),
        json!({"html": "x", "path": "page.html"}),
    ] {
        let r = c.call(Method::ViewHtml, bad.clone(), LOAD);
        ctx.ok(
            code_of(&r) == "bad_params",
            format!("{at}: view.html {bad} is bad_params ({})", code_of(&r)),
        );
    }

    // ---- closing ends it, the browser and the folder ----
    let r = c.call(Method::PanelClose, json!({}), LOAD);
    ctx.ok(
        r.as_ref().is_ok_and(|v| v["closed"] == json!(true)),
        format!("{at}: panel.close closes the mini-app ({r:?})"),
    );
    let freed = ctx.until(
        "the browser view and the folders to go",
        20_000,
        &mut || state(app)["controllers"] == json!(0) && app_folders(profile).is_empty(),
    );
    ctx.ok(
        freed,
        format!(
            "{at}: no view and no folder is left ({} / {:?})",
            state(app)["controllers"],
            app_folders(profile)
        ),
    );
    Ok(())
}

/// The policy is switched off by the seam: only the second wall stands. A request to outside with
/// `no-cors` mode resolves (opaque) when Flare answered it itself and rejects when it was really tried.
fn wall_on(ctx: &mut Ctx, app: &mut App) -> Result<(), String> {
    let at = "wall";
    app.session.call(&Request::Resize {
        w: 1000.0,
        h: 640.0,
    })?;
    prompt(app)?;
    let (pipe, _, token) = channel_of(app)?;
    let mut c = hello(&pipe, &token).map_err(|e| format!("hello: {e}"))?;
    let v = show(
        ctx,
        &mut c,
        at,
        json!({"html": "<p>wall</p><script>window.__r='pending';</script>"}),
    )
    .map_err(|e| format!("view.html: {e}"))?;
    ctx.ok(
        v["panel"]["network"] == json!(false),
        format!("{at}: no network was asked for ({v})"),
    );
    settle(ctx, app, "the wall page to settle");
    let meta = say(
        ctx,
        app,
        "String(document.querySelector('meta[http-equiv]'))",
    );
    ctx.ok(
        meta == "null",
        format!("{at}: the seam wrote the page without a policy ({meta:?})"),
    );
    say(
        ctx,
        app,
        "(function(){ fetch('http://127.0.0.1:9/wall',{mode:'no-cors'}).then(function(r){window.__r='answered:'+r.type},function(e){window.__r='refused-connection'}); return 'started'; })()",
    );
    let (done, r) = until_says(
        ctx,
        "the fetch to settle",
        15_000,
        app,
        "window.__r",
        &|s| s != "pending",
    );
    ctx.ok(
        done && r == "answered:opaque",
        format!("{at}: with no policy a request to outside is still answered by Flare, not tried ({r:?})"),
    );
    // The wall covers requests that do not come from the main frame's own code: a frame with no
    // address of its own and a worker made from a blob.
    say(
        ctx,
        app,
        r##"(function(){ window.__f='pending'; window.__w='pending'; var f=document.createElement('iframe'); f.srcdoc="<script>fetch('http://127.0.0.1:9/f',{mode:'no-cors'}).then(function(r){parent.__f='answered:'+r.type},function(e){parent.__f='refused-connection'})<\/script>"; document.body.appendChild(f); try { var w=new Worker(URL.createObjectURL(new Blob(["fetch('http://127.0.0.1:9/w',{mode:'no-cors'}).then(function(r){postMessage('answered:'+r.type)},function(e){postMessage('refused-connection')})"]))); w.onmessage=function(m){window.__w=m.data}; } catch (e) { window.__w='no-worker'; } return 'started'; })()"##,
    );
    let (done, r) = until_says(
        ctx,
        "the frame's fetch to settle",
        15_000,
        app,
        "window.__f",
        &|s| s != "pending",
    );
    ctx.ok(
        done && r == "answered:opaque",
        format!("{at}: a request from a srcdoc frame is answered by Flare too ({r:?})"),
    );
    let (done, r) = until_says(
        ctx,
        "the worker's fetch to settle",
        15_000,
        app,
        "window.__w",
        &|s| s != "pending",
    );
    ctx.ok(
        done && r == "answered:opaque",
        format!("{at}: a request from a blob worker is answered by Flare too ({r:?})"),
    );
    // The app's own file is served (the wall lets the app's host through).
    let own = say(ctx, app, "document.title.length >= 0 ? 'fine' : 'no'");
    ctx.ok(
        own == "fine",
        format!("{at}: the page itself loaded ({own:?})"),
    );
    Ok(())
}

/// With the page's script and policy both off, the browser itself must still give a peer connection
/// nothing: its WebRTC policy forbids UDP that is not proxied and a sealed app's proxy is not there.
/// The STUN server is a UDP socket of this process on loopback, so "gathers nothing" is shown by that
/// socket hearing nothing, in the window and in about:blank and srcdoc frames, for an app with no
/// network and for one with it. A web panel (which has no such arguments) is the control: the same
/// probe there must reach the socket, or the test proves nothing.
fn webrtc_walls(ctx: &mut Ctx, app: &mut App, root: &Path) -> Result<(), String> {
    let at = "webrtc";
    app.session.call(&Request::Resize {
        w: 1000.0,
        h: 640.0,
    })?;
    prompt(app)?;
    let (pipe, _, token) = channel_of(app)?;
    let mut c = hello(&pipe, &token).map_err(|e| format!("hello: {e}"))?;
    let mut spy = Spy::new()?;
    for (label, network) in [("no network", false), ("network on", true)] {
        let v = show(
            ctx,
            &mut c,
            at,
            json!({"html": "<p id=ice>ice</p>", "allow_network": network}),
        )
        .map_err(|e| format!("view.html: {e}"))?;
        ctx.ok(
            v["panel"]["network"] == json!(network),
            format!("{at}: {label}: the app has the network it was given ({v})"),
        );
        settle(ctx, app, "the ice page to settle");
        let shown = say(ctx, app, "document.getElementById('ice').textContent");
        ctx.ok(
            shown == "ice",
            format!("{at}: {label}: the app still renders in its own browser ({shown:?})"),
        );
        // A WebSocket never reaches the request wall: with no network the dead proxy must stop it, with
        // the network on it reaches the listener (the control).
        let listener = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let lport = listener.local_addr().map_err(|e| e.to_string())?.port();
        say(
            ctx,
            app,
            &format!("(function(){{ window.__ws='pending'; try {{ var w=new WebSocket('ws://127.0.0.1:{lport}/'); w.onerror=function(){{window.__ws='error'}}; w.onclose=function(){{ if(window.__ws==='pending') window.__ws='closed'; }}; w.onopen=function(){{window.__ws='open'}}; }} catch(e) {{ window.__ws='threw'; }} return 'started'; }})()"),
        );
        let connected = ctx.poll(3_000, &mut || listener.accept().is_ok());
        let ws = say(ctx, app, "window.__ws");
        ctx.ok(
            connected == network,
            format!(
                "{at}: {label}: a WebSocket to loopback {} ({ws:?})",
                if network {
                    "reaches a listener (the control)"
                } else {
                    "is stopped by the browser's dead proxy"
                }
            ),
        );
        let _ = spy.fresh();
        for mode in ["top", "srcdoc", "blank"] {
            let (done, rec) = gather(ctx, app, &spy, mode);
            let heard = spy.fresh();
            let bad = ["srflx", "relay", "prflx", "typ host"]
                .iter()
                .any(|t| rec.contains(t));
            ctx.ok(
                done && heard == 0 && !bad,
                format!("{at}: {label}, {mode}: no STUN packet leaves and no candidate is gathered ({heard} heard, {rec})"),
            );
        }
    }
    // The control: the web panel's browser has no such arguments, so the same probe reaches the socket.
    fs::write(
        root.join("ice.html"),
        "<!doctype html><title>ice</title><p>ice</p>",
    )
    .map_err(|e| e.to_string())?;
    let r = c.call(
        Method::BrowserOpen,
        json!({"url": format!("{}/ice.html", super::web::HOST)}),
        LOAD,
    );
    ctx.ok(r.is_ok(), format!("{at}: the control page opens ({r:?})"));
    settle(ctx, app, "the control page to settle");
    let _ = spy.fresh();
    let (done, rec) = gather(ctx, app, &spy, "top");
    let heard = spy.fresh();
    ctx.ok(
        done && heard > 0,
        format!("{at}: control: the same probe in the web panel does reach the socket, so silence above means something ({heard} heard, {rec})"),
    );
    Ok(())
}

/// The network switch is on and the agent asks: the page may load https content, plain http never.
fn network_on(ctx: &mut Ctx, app: &mut App) -> Result<(), String> {
    let at = "network";
    app.session.call(&Request::Resize {
        w: 1000.0,
        h: 640.0,
    })?;
    prompt(app)?;
    let (pipe, _, token) = channel_of(app)?;
    let mut c = hello(&pipe, &token).map_err(|e| format!("hello: {e}"))?;
    let v = show(
        ctx,
        &mut c,
        at,
        json!({"html": PAGE, "allow_network": true}),
    )
    .map_err(|e| format!("view.html: {e}"))?;
    ctx.ok(
        v["panel"]["network"] == json!(true),
        format!("{at}: asked for and switched on: the page has the network ({v})"),
    );
    settle(ctx, app, "the page to settle");
    let meta = say(
        ctx,
        app,
        "document.querySelector('meta[http-equiv=\"Content-Security-Policy\"]').content",
    );
    ctx.ok(
        meta.contains("default-src 'self' 'unsafe-inline' data: blob: https:;")
            && meta.contains("connect-src 'self' data: blob: https:;")
            && !meta.contains("http:"),
        format!("{at}: the policy allows https and never plain http ({meta})"),
    );
    say(ctx, app, "(function(){ fetch('http://127.0.0.1:9/plain').catch(function(){}); return 'started'; })()");
    let (got, seen) = until_says(
        ctx,
        "a violation for plain http",
        15_000,
        app,
        "JSON.stringify(window.__v)",
        &|s| s.contains("127.0.0.1:9/plain"),
    );
    ctx.ok(
        got,
        format!("{at}: plain http is still blocked with the network on ({seen})"),
    );
    // Asking without the switch's agreement was covered in the first launch; here, not asking is no network.
    let v = show(ctx, &mut c, at, json!({"html": "<p>quiet</p>"}))
        .map_err(|e| format!("view.html: {e}"))?;
    ctx.ok(
        v["panel"]["network"] == json!(false),
        format!("{at}: a page that does not ask has none ({v})"),
    );
    Ok(())
}

const FLOW: &str =
    "graph TD\n  A[Start] --> B{Is it ready?}\n  B -->|yes| C[Ship it]\n  B -->|no| D[Wait]";
const SEQ: &str = "sequenceDiagram\n  Alice->>Bob: Hello\n  Bob-->>Alice: Hi";
const BAD: &str = "graph TD\n  A[Start --> ((";

pub fn diagram(ctx: &mut Ctx) -> Result<(), String> {
    let root = world_root(ctx)?;
    let mut app = launch_with(ctx, &root, 96, &[])?;
    let r = diagram_on(ctx, &mut app);
    let left = app.close();
    ctx.ok(left, "the app quits by itself");
    r
}

fn diagram_on(ctx: &mut Ctx, app: &mut App) -> Result<(), String> {
    let at = "diagram";
    app.session.call(&Request::Resize {
        w: 1000.0,
        h: 640.0,
    })?;
    prompt(app)?;
    let (pipe, _, token) = channel_of(app)?;
    let mut c = hello(&pipe, &token).map_err(|e| format!("hello: {e}"))?;

    let r = c.call(
        Method::ViewDiagram,
        json!({"mermaid": FLOW, "title": "Flow"}),
        LOAD,
    );
    ctx.ok(
        r.as_ref().is_ok_and(|v| {
            v["panel"]["kind"] == json!("app") && v["panel"]["title"] == json!("Flow")
        }),
        format!("{at}: show_diagram answers once the diagram has drawn ({r:?})"),
    );
    settle(ctx, app, "the diagram to settle");
    let n = say(
        ctx,
        app,
        "String(document.querySelectorAll('#out svg').length)",
    );
    ctx.ok(n == "1", format!("{at}: the page has one svg ({n:?})"));
    let nodes = say(
        ctx,
        app,
        "String(document.querySelectorAll('#out svg .node').length)",
    );
    ctx.ok(
        nodes == "4",
        format!("{at}: the flowchart has its four nodes ({nodes:?})"),
    );
    let text = say(
        ctx,
        app,
        "Array.from(document.querySelectorAll('#out svg .node')).map(function(n){return n.textContent.trim()}).join('|')",
    );
    ctx.ok(
        text.contains("Start") && text.contains("Is it ready?") && text.contains("Ship it"),
        format!("{at}: the labels are drawn ({text:?})"),
    );
    let err_hidden = say(ctx, app, "String(document.getElementById('err').hidden)");
    ctx.ok(
        err_hidden == "true",
        format!("{at}: no error is shown for a good diagram"),
    );
    let v = state(app);
    ctx.ok(
        v["title"] == json!("Flow") && v["bar"]["h"].as_f64() == Some(0.0),
        format!("{at}: the header is the title and there is no address bar"),
    );
    let violations = say(ctx, app, "'none'");
    ctx.ok(
        violations == "none",
        format!("{at}: the page answers scripts"),
    );

    // A second kind of diagram.
    let r = c.call(Method::ViewDiagram, json!({"mermaid": SEQ}), LOAD);
    ctx.ok(r.is_ok(), format!("{at}: a sequence diagram draws ({r:?})"));
    settle(ctx, app, "the sequence diagram to settle");
    let n = say(
        ctx,
        app,
        "String(document.querySelectorAll('#out svg').length)",
    );
    let t = say(
        ctx,
        app,
        "Array.from(document.querySelectorAll('#out svg text')).map(function(n){return n.textContent.trim()}).join('|')",
    );
    ctx.ok(
        n == "1" && t.contains("Alice") && t.contains("Bob"),
        format!("{at}: the sequence diagram has its svg and its actors ({n:?} {t:?})"),
    );
    ctx.ok(
        state(app)["title"] == json!("Diagram"),
        format!("{at}: with no title the header says Diagram"),
    );

    // Text that does not parse: the call fails with Mermaid's own words and the panel shows them.
    let r = c.call(
        Method::ViewDiagram,
        json!({"mermaid": BAD, "title": "Broken"}),
        LOAD,
    );
    let msg = match &r {
        Err(ClientError::Remote(e)) => e.message.clone(),
        _ => String::new(),
    };
    ctx.ok(
        code_of(&r) == "bad_params"
            && msg.starts_with("the diagram did not draw: ")
            && msg.len() > 40,
        format!("{at}: a diagram that does not parse fails with the parser's error ({msg:?})"),
    );
    settle(ctx, app, "the error page to settle");
    let shown = say(ctx, app, "document.getElementById('err').textContent");
    let hidden = say(ctx, app, "String(document.getElementById('err').hidden)");
    ctx.ok(
        hidden == "false" && !shown.is_empty() && msg.contains(shown.lines().next().unwrap_or("?")),
        format!("{at}: the panel shows the same error text ({shown:?})"),
    );
    ctx.ok(
        state(app)["title"] == json!("Broken"),
        format!("{at}: the panel is still titled by the agent"),
    );
    // Caps and params.
    let big = "A-->B\n".repeat(40_000);
    let r = c.call(Method::ViewDiagram, json!({ "mermaid": big }), LOAD);
    ctx.ok(
        code_of(&r) == "bad_params",
        format!("{at}: over 200 KB is refused ({})", code_of(&r)),
    );
    for bad in [json!({}), json!({"mermaid": ""}), json!({"mermaid": 3})] {
        let r = c.call(Method::ViewDiagram, bad.clone(), LOAD);
        ctx.ok(
            code_of(&r) == "bad_params",
            format!("{at}: {bad} is bad_params"),
        );
    }
    Ok(())
}

pub fn mcp_diagram(ctx: &mut Ctx) -> Result<(), String> {
    let exe = ctx
        .exe
        .parent()
        .map(|d| d.join("flare-mcp.exe"))
        .unwrap_or_default();
    if !exe.is_file() {
        ctx.ok(
            false,
            format!(
                "{} is missing; build it: cargo build --release -p flare -p flare-mcp",
                exe.display()
            ),
        );
        return Ok(());
    }
    let root = world_root(ctx)?;
    let mut app = launch_with(ctx, &root, 96, &[])?;
    let r = mcp_on(ctx, &mut app, &exe);
    let left = app.close();
    ctx.ok(left, "the app quits by itself");
    r
}

fn mcp_on(ctx: &mut Ctx, app: &mut App, exe: &Path) -> Result<(), String> {
    app.session.call(&Request::Resize {
        w: 1000.0,
        h: 640.0,
    })?;
    prompt(app)?;
    let info = probe(app, "ctl.info", Value::Null);
    let pipe = info["pipe"].as_str().ok_or("no pipe")?.to_string();
    let pid = u32::try_from(info["pid"].as_u64().ok_or("no pid")?).map_err(|e| e.to_string())?;
    let token = info["tabs"]
        .as_array()
        .and_then(|t| t.iter().find(|t| t["active"] == json!(true)))
        .and_then(|t| t["token"].as_str())
        .ok_or("no tab token")?
        .to_string();
    let cwd = ctx.ensure_world().alpha.clone();
    let mut s = Server::start(exe, &pipe, pid, &token, &cwd)?;
    s.ask(
        1,
        "initialize",
        json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "flare-e2e", "version": "1"}}),
    )?;
    let list = s.ask(2, "tools/list", json!({}))?;
    let tool = |name: &str| {
        list["result"]["tools"]
            .as_array()
            .and_then(|a| a.iter().find(|t| t["name"] == json!(name)))
            .cloned()
    };
    let html_desc = tool("show_html")
        .and_then(|t| t["description"].as_str().map(str::to_string))
        .unwrap_or_default();
    let dia_desc = tool("show_diagram")
        .and_then(|t| t["description"].as_str().map(str::to_string))
        .unwrap_or_default();
    ctx.ok(
        html_desc.contains("NO network")
            && html_desc.contains("4 MB")
            && html_desc.contains("allow_network"),
        "show_html is described for an agent: what it is for, the cap and the network",
    );
    ctx.ok(
        dia_desc.contains("Mermaid") && dia_desc.contains("error"),
        "show_diagram is described for an agent: Mermaid, and what an error does",
    );

    // ---- show_diagram through the tool ----
    let r = s.tool(3, "show_diagram", json!({"mermaid": FLOW, "title": "Flow"}))?;
    ctx.ok(
        !is_error(&r) && text_of(&r).contains("Showing the diagram"),
        format!("show_diagram draws ({})", text_of(&r)),
    );
    let (drawn, _) = until_says(
        ctx,
        "the svg to be in the panel",
        20_000,
        app,
        "String(document.querySelectorAll('#out svg').length)",
        &|s| s == "1",
    );
    ctx.ok(drawn, "the panel holds the svg");
    // ---- a diagram that does not parse comes back as the tool's error with Mermaid's words ----
    let r = s.tool(4, "show_diagram", json!({"mermaid": BAD}))?;
    let t = text_of(&r);
    ctx.ok(
        is_error(&r)
            && t.starts_with("The diagram did not draw. Mermaid said:")
            && t.contains("Fix the Mermaid text"),
        format!("a bad diagram is a tool error with the parser's words ({t})"),
    );
    // ---- show_html through the tool, and the agent looks at its own chart ----
    let r = s.tool(
        5,
        "show_html",
        json!({"html": PAGE, "title": "Page", "allow_network": true}),
    )?;
    let t = text_of(&r);
    ctx.ok(
        !is_error(&r)
            && t.contains("Showing the mini-app")
            && t.contains("setting")
            && t.contains("has none"),
        format!("show_html reports that the network asked for was not given ({t})"),
    );
    let r = s.tool(6, "browser_look", json!({}))?;
    ctx.ok(
        !is_error(&r) && text_of(&r).contains("Press"),
        format!(
            "browser_look sees the mini-app's button ({})",
            text_of(&r).chars().take(120).collect::<String>()
        ),
    );
    let r = s.tool(7, "show_html", json!({"path": "page.html"}));
    // page.html was not written in this scenario: a path that does not exist is a refusal, not a crash.
    let r = r?;
    ctx.ok(
        is_error(&r) && text_of(&r).contains("not_found"),
        format!("a path that is not there is refused ({})", text_of(&r)),
    );
    let r = s.tool(8, "show_html", json!({}))?;
    ctx.ok(
        is_error(&r),
        "show_html with nothing to show is a tool error",
    );
    Ok(())
}

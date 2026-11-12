//! The browser's parity with Windows on the Mac (rules 1175 to 1179), each a question the Windows host answers with
//! WebView2 and the Mac one must answer the same way with CEF:
//!
//! - `persist-write` then `persist-read` (two runs on one data folder, the engine shut down in between): a sign-in that
//!   was to be kept (a cookie with an expiry) and a page's `localStorage` come back after the restart, a session
//!   cookie does not, and all of it lives in Flare's own profile folder (rule 1175).
//! - `content`: the page follows the app's light or dark appearance (`prefers-color-scheme`) and says a sane user
//!   agent (rule 1176); each mini-app Flare writes (a page of the agent's, a Mermaid diagram, the 3D viewer on WebGL,
//!   the audio player, the markdown Compiled view) loads and paints in a windowless view (rule 1178).
//! - `plugins`: a plugin's fill (`flare_mods::fill::fill_script`, run as `web_ctl` runs it, through `run_script`)
//!   types into a sign-in form and stops on a page that moved (rule 1177); a plugin's bar and background page keep
//!   their seals in the same host as the web panel: no menu, no browser keys, no download, no dialog, and the bridge
//!   works from a page that is never shown (rule 1179).

use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, Instant};

use flare_surfaces::app::{self as app, AppSpec};
use flare_surfaces::modview::plugin_spec;
use flare_surfaces::web::PhysRect;
use flare_surfaces::{audio, mdview, model};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSAppearance, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSApplication};
use serde_json::{Value, json};

use super::{BOUNDS, H, Report, ev_text, pages, walk_names};
use crate::backend::{
    KeyInput, Mods, Pointer, PointerAct, PointerButton, Slot, ViewFrame, WebBackend, WebEvent,
};

/// The Mermaid build and the 3D viewer Flare ships beside those pages (`ctl_app.rs` includes the same files).
static MERMAID: &[u8] = include_bytes!("../../../../../assets/mermaid/mermaid.min.js");
static MODEL_VIEWER: &[u8] = include_bytes!("../../../../../assets/three/viewer.js");

/// How many different colours a sample of the frame has (every 97th pixel): one is a blank view.
fn colours(f: &ViewFrame) -> usize {
    f.bgra
        .as_chunks::<4>()
        .0
        .iter()
        .step_by(97)
        .copied()
        .collect::<HashSet<_>>()
        .len()
}

/// Waits up to `ms` for a frame of the slot at the panel's size with at least `want` colours; (size, colours).
fn painted(h: &mut H, slot: &Slot, want: usize, ms: u64) -> (Option<(u32, u32)>, usize) {
    let t = Instant::now();
    let mut seen = (None, 0);
    while t.elapsed() < Duration::from_millis(ms) {
        h.wait(50);
        if let Some(f) = h.host.frame(slot) {
            seen = (Some((f.width, f.height)), colours(&f));
            if f.width == BOUNDS.w as u32 && f.height == BOUNDS.h as u32 && seen.1 >= want {
                break;
            }
        }
    }
    seen
}

fn full(size: Option<(u32, u32)>) -> bool {
    size == Some((BOUNDS.w as u32, BOUNDS.h as u32))
}

/// A `run_script` answer that is a JSON text (`JSON.stringify` in the page), read.
fn json_of(r: &Result<String, String>) -> Value {
    r.as_deref()
        .ok()
        .and_then(|s| serde_json::from_str::<String>(s).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null)
}

fn data_port() -> Result<u16, String> {
    std::env::var("FLARE_WEB_PROBE_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .ok_or_else(|| "FLARE_WEB_PROBE_PORT is not set".to_string())
}

/// The first run: a kept cookie, a session cookie and a stored value, then the engine shuts down (CEF writes its
/// stores to disk then, as WebView2 does when its browser process ends).
pub fn persist_write(rep: &mut Report, data: &Path) -> Result<(), String> {
    let srv = pages::start_on(data_port()?)?;
    let mut h = H::new(data, Vec::new());
    h.host.set_scale(2.0);
    let from = h.mark();
    h.host.open("t1", &srv.url("/page2"), BOUNDS, true);
    let opened = h.opened(from, "t1", "Page two");
    let set = h.script(
        "t1",
        "document.cookie = 'kept=yes; max-age=31536000; path=/'; document.cookie = 'session=yes; path=/'; \
         localStorage.setItem('stored', 'yes'); JSON.stringify({cookie: document.cookie, stored: localStorage.getItem('stored')})",
    );
    let v = json_of(&set);
    let cookie = v["cookie"].as_str().unwrap_or_default().to_string();
    rep.check(
        "persist.write",
        opened
            && cookie.contains("kept=yes")
            && cookie.contains("session=yes")
            && v["stored"] == "yes",
        "the web panel's page sets a kept cookie, a session cookie and a localStorage value",
        json!({"opened": opened, "page": v}),
    );
    h.host.close("t1");
    h.wait(300);
    h.host.close_all();
    let all = walk_names(data);
    let under = |tail: &str| -> Vec<String> {
        all.iter()
            .filter(|p| p.ends_with(tail))
            .map(|p| {
                p.strip_prefix(&*data.to_string_lossy())
                    .unwrap_or(p)
                    .to_string()
            })
            .collect()
    };
    let (cookies, storage) = (under("/Cookies"), under("/Local Storage"));
    rep.check(
        "persist.own-folder",
        !cookies.is_empty() && !storage.is_empty(),
        "the web panel's cookies and storage are on disk in Flare's own profile folder (the host's data folder)",
        json!({"cookies": cookies, "local_storage": storage}),
    );
    Ok(())
}

/// The second run, a new engine on the same folder: what was to be kept is back, the session cookie is not.
pub fn persist_read(rep: &mut Report, data: &Path) -> Result<(), String> {
    let srv = pages::start_on(data_port()?)?;
    let mut h = H::new(data, Vec::new());
    h.host.set_scale(2.0);
    let from = h.mark();
    h.host.open("t1", &srv.url("/page2"), BOUNDS, true);
    let opened = h.opened(from, "t1", "Page two");
    let got = h.script(
        "t1",
        "JSON.stringify({cookie: document.cookie, stored: localStorage.getItem('stored')})",
    );
    let v = json_of(&got);
    let cookie = v["cookie"].as_str().unwrap_or_default().to_string();
    rep.check(
        "persist.read",
        opened && cookie.contains("kept=yes") && !cookie.contains("session=") && v["stored"] == "yes",
        "after the engine restarted on the same profile the kept cookie and the stored value are back and the session cookie is gone, as with WebView2",
        json!({"opened": opened, "page": v}),
    );
    h.host.close("t1");
    h.wait(300);
    h.host.close_all();
    Ok(())
}

/// Sets the app's appearance (None: the system's), as the window's theme would.
fn set_appearance(mtm: MainThreadMarker, dark: Option<bool>) {
    let app = NSApplication::sharedApplication(mtm);
    let appearance = dark.and_then(|d| {
        // SAFETY: the appearance names are AppKit's own constant strings.
        let name = unsafe {
            if d {
                NSAppearanceNameDarkAqua
            } else {
                NSAppearanceNameAqua
            }
        };
        NSAppearance::appearanceNamed(name)
    });
    app.setAppearance(appearance.as_deref());
}

fn app_is_dark(mtm: MainThreadMarker) -> bool {
    let app = NSApplication::sharedApplication(mtm);
    app.effectiveAppearance()
        .name()
        .to_string()
        .contains("Dark")
}

/// Turns until the page's `prefers-color-scheme: dark` is `dark`, or 5 s pass; what it said last.
fn scheme_becomes(h: &mut H, tab: &str, dark: bool) -> Value {
    let t = Instant::now();
    let mut last = Value::Null;
    while t.elapsed() < Duration::from_secs(5) {
        last = h
            .eval(tab, "matchMedia('(prefers-color-scheme: dark)').matches")
            .unwrap_or(Value::Null);
        if last == json!(dark) {
            break;
        }
        h.wait(100);
    }
    last
}

/// A mini-app of `files`, written as `ctl_app`'s `show_app` and the markdown view write theirs.
fn write(data: &Path, seq: u64, files: &[(&str, &[u8])]) -> Result<AppSpec, String> {
    app::write_app(&data.join("apps"), seq, files, false).map_err(|e| e.to_string())
}

/// A silent two-second track (8 kHz, 16 bit, mono): it plays without a sound on the machine running the test.
/// How many audio output devices this Mac has, from `system_profiler SPAudioDataType` (each device with output
/// channels says `coreaudio_device_output`); `None` when the tool could not answer, which never counts as none.
fn audio_output_devices() -> Option<usize> {
    let out = std::process::Command::new("/usr/sbin/system_profiler")
        .args(["SPAudioDataType", "-json"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let v: Value = serde_json::from_slice(&out.stdout).ok()?;
    let groups = v["SPAudioDataType"].as_array()?;
    Some(
        groups
            .iter()
            .filter_map(|g| g["_items"].as_array())
            .flatten()
            .filter(|d| d["coreaudio_device_output"].as_u64().is_some_and(|c| c > 0))
            .count(),
    )
}

fn silent_wav() -> Vec<u8> {
    let samples = 16_000u32;
    let bytes = samples * 2;
    let mut w = Vec::with_capacity(44 + bytes as usize);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + bytes).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&8_000u32.to_le_bytes());
    w.extend_from_slice(&16_000u32.to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&bytes.to_le_bytes());
    w.resize(44 + bytes as usize, 0);
    w
}

/// A pyramid in ASCII STL, its point toward the viewer's opening camera (the e2e `model` scenario's).
const PYRAMID_STL: &str = "solid p
facet normal 0 0 0
outer loop
vertex 1 1 1
vertex 1 0 0
vertex 0 1 0
endloop
endfacet
facet normal 0 0 0
outer loop
vertex 1 1 1
vertex 0 1 0
vertex 0 0 1
endloop
endfacet
facet normal 0 0 0
outer loop
vertex 1 1 1
vertex 0 0 1
vertex 1 0 0
endloop
endfacet
facet normal 0 0 0
outer loop
vertex 1 0 0
vertex 0 0 1
vertex 0 1 0
endloop
endfacet
endsolid p
";

pub fn content(rep: &mut Report, data: &Path) -> Result<(), String> {
    let mtm = MainThreadMarker::new().ok_or("not on the main thread")?;
    let srv = pages::start()?;
    let mut h = H::new(data, Vec::new());
    h.host.set_scale(2.0);

    // ---- the look: the page follows the app's appearance, and says what browser it is
    let from = h.mark();
    h.host.open("t1", &srv.url("/page2"), BOUNDS, true);
    let opened = h.opened(from, "t1", "Page two");
    let system_dark = app_is_dark(mtm);
    let at_start = scheme_becomes(&mut h, "t1", system_dark);
    set_appearance(mtm, Some(true));
    let dark = scheme_becomes(&mut h, "t1", true);
    set_appearance(mtm, Some(false));
    let light = scheme_becomes(&mut h, "t1", false);
    set_appearance(mtm, None);
    let back = scheme_becomes(&mut h, "t1", system_dark);
    rep.check(
        "look.color-scheme",
        opened
            && at_start == json!(system_dark)
            && dark == json!(true)
            && light == json!(false)
            && back == json!(system_dark),
        "a page's prefers-color-scheme is the app's appearance (the system's unless the app sets one) and follows it when it changes, as WebView2's follows Windows'",
        json!({"opened": opened, "system_dark": system_dark, "at_start": at_start, "dark": dark, "light": light, "back": back}),
    );
    let ua = h
        .eval(
            "t1",
            "({ua: navigator.userAgent, vendor: navigator.vendor, webdriver: navigator.webdriver, platform: navigator.platform})",
        )
        .unwrap_or(Value::Null);
    let s = ua["ua"].as_str().unwrap_or_default();
    let chrome_major = s
        .split("Chrome/")
        .nth(1)
        .and_then(|v| v.split('.').next())
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(0);
    rep.check(
        "look.user-agent",
        s.starts_with("Mozilla/5.0 (Macintosh; Intel Mac OS X")
            && chrome_major >= 120
            && s.contains("Safari/537.36")
            && !s.contains("Headless")
            && !s.contains("CEF")
            && ua["vendor"] == "Google Inc."
            && ua["webdriver"] == false,
        "the user agent is a desktop Chrome's on a Mac (what sites expect), never a headless or embedded marker",
        json!({"navigator": ua, "chrome_major": chrome_major}),
    );
    h.host.close("t1");
    h.wait(200);

    // ---- show_html: the agent's page behind its policy
    let page = app::wrap_html(
        "<title>Mini</title><body style=\"margin:0;background:#fff\"><div style=\"width:300px;height:150px;background:#3366cc\"></div><p>hello</p>",
        false,
    );
    let spec = write(data, 11, &[(app::PAGE, page.as_bytes())])?;
    let slot = Slot::panel("a1");
    let from = h.mark();
    h.host.open_app("a1", spec, BOUNDS, true);
    let opened = h.opened(from, "a1", "Mini");
    let (size, n) = painted(&mut h, &slot, 2, 10_000);
    rep.check(
        "mini.html",
        opened && full(size) && n >= 2,
        "show_html: the agent's page, its policy first, loads from its folder and paints",
        json!({"opened": opened, "size": size, "colours": n, "events": h.since(from)}),
    );
    h.host.close("a1");
    h.wait(200);

    // ---- show_diagram: Mermaid draws an svg
    let page = app::wrap_html(
        &app::diagram_page("graph TD\n  A[Start] --> B{Check}\n  B -->|yes| C[Done]"),
        false,
    );
    let spec = write(
        data,
        12,
        &[(app::PAGE, page.as_bytes()), (app::MERMAID_FILE, MERMAID)],
    )?;
    let slot = Slot::panel("a2");
    let from = h.mark();
    h.host.open_app("a2", spec, BOUNDS, true);
    let opened = h.opened(from, "a2", "Diagram");
    let drawn = h.eval("a2", "window.__flareDiagram").unwrap_or(Value::Null);
    let svg = h
        .eval(
            "a2",
            "(function () { var s = document.querySelector('#out svg'); return s ? s.querySelectorAll('g.node').length : -1; })()",
        )
        .unwrap_or(Value::Null);
    let (size, n) = painted(&mut h, &slot, 3, 10_000);
    rep.check(
        "mini.diagram",
        opened
            && drawn["ok"] == true
            && svg.as_i64().is_some_and(|c| c >= 3)
            && full(size)
            && n >= 3,
        "show_diagram: the bundled Mermaid draws the diagram as an svg (three nodes) and it paints",
        json!({"opened": opened, "result": drawn, "nodes": svg, "size": size, "colours": n}),
    );
    h.host.close("a2");
    h.wait(200);

    // ---- show_model: the 3D viewer draws on WebGL in a windowless view
    let file = model::model_name("stl");
    let page = model::page("stl", "Pyramid");
    let spec = write(
        data,
        13,
        &[
            (app::PAGE, page.as_bytes()),
            (model::VIEWER_FILE, MODEL_VIEWER),
            (file.as_str(), PYRAMID_STL.as_bytes()),
        ],
    )?;
    let slot = Slot::panel("a3");
    let from = h.mark();
    h.host.open_app("a3", spec, BOUNDS, true);
    let opened = h.opened(from, "a3", "Pyramid");
    let gl = h
        .eval(
            "a3",
            "(function () { var c = document.createElement('canvas'); var g = c.getContext('webgl2'); \
             return g ? g.getParameter(g.VERSION) : null; })()",
        )
        .unwrap_or(Value::Null);
    let settled = h.eval("a3", "window.__flareModel").unwrap_or(Value::Null);
    h.wait(500);
    let pixels = h
        .eval("a3", "window.__flareModelPixels()")
        .unwrap_or(Value::Null);
    let (size, n) = painted(&mut h, &slot, 3, 10_000);
    rep.check(
        "mini.model",
        opened
            && gl.as_str().is_some_and(|v| v.starts_with("WebGL 2"))
            && settled["ok"] == true
            && settled["triangles"] == 4
            && pixels.as_u64().is_some_and(|p| p > 0)
            && full(size)
            && n >= 3,
        "show_model: WebGL 2 is there in a windowless view, the viewer reads the model and draws it, and the view paints",
        json!({"opened": opened, "webgl": gl, "result": settled, "model_pixels": pixels, "size": size, "colours": n}),
    );
    h.host.close("a3");
    h.wait(200);

    // ---- show_audio: Flare's player loads the track, paints, and plays after the person's click
    let track = audio::track_name("wav");
    let page = audio::page(&track, "Silence", None, true);
    let wav = silent_wav();
    let spec = write(
        data,
        14,
        &[(app::PAGE, page.as_bytes()), (track.as_str(), &wav)],
    )?;
    let slot = Slot::panel("a4");
    let from = h.mark();
    h.host.open_app("a4", spec, BOUNDS, true);
    let opened = h.opened(from, "a4", "Silence");
    let (size, n) = painted(&mut h, &slot, 3, 10_000);
    h.wait(500);
    let auto = h
        .eval(
            "a4",
            "({paused: document.getElementById('a').paused, t: document.getElementById('a').currentTime})",
        )
        .unwrap_or(Value::Null);
    let loaded = h
        .eval(
            "a4",
            "new Promise(function (r) { var a = document.getElementById('a'); \
             var done = function () { r({state: a.readyState, duration: a.duration, error: a.error && a.error.code}); }; \
             if (a.readyState >= 1) { done(); } else { a.addEventListener('loadedmetadata', done); a.addEventListener('error', done); } })",
        )
        .unwrap_or(Value::Null);
    // The person's click (a real input event, so the page has their gesture), then play.
    h.click(&slot, 1800.0, 1300.0);
    let play = h
        .eval(
            "a4",
            "(function () { var a = document.getElementById('a'); a.currentTime = 0; \
             return a.play().then(function () { return 'playing'; }, function (e) { return 'refused ' + e.name; }); })()",
        )
        .unwrap_or(Value::Null);
    // The clock moves as the output device takes the sound, so a loaded runner gets up to 5 s for it.
    let end = Instant::now() + Duration::from_secs(5);
    let t = loop {
        h.wait(250);
        let t = h
            .eval("a4", "document.getElementById('a').currentTime")
            .unwrap_or(Value::Null);
        if t.as_f64().is_some_and(|t| t > 0.2) || Instant::now() > end {
            break t;
        }
    };
    let moved = t.as_f64().is_some_and(|t| t > 0.2);
    // A Mac with no audio output at all (GitHub's macOS VMs) never moves the clock: only then is the time check
    // skipped, with the reason in the report. Everything else (load, paint, the 2 s track, play() taking the click)
    // is still required, and a Mac that has an output device must still play.
    let output = if moved { None } else { audio_output_devices() };
    let skipped = (!moved && output == Some(0)).then_some(
        "no audio output device on this Mac (system_profiler SPAudioDataType lists none)",
    );
    rep.check(
        "mini.audio",
        opened
            && full(size)
            && n >= 3
            && loaded["duration"]
                .as_f64()
                .is_some_and(|d| (d - 2.0).abs() < 0.1)
            && play == "playing"
            && (moved || skipped.is_some()),
        "show_audio: the player loads the track beside it (2 s), paints, and plays once the person clicked (currentTime moves)",
        json!({"opened": opened, "size": size, "colours": n, "loaded": loaded, "autoplay": auto, "play": play, "time": t,
               "output_devices": output, "skipped": skipped}),
    );
    h.host.close("a4");
    h.wait(200);

    // ---- the markdown Compiled view
    let src =
        "# Notes\n\nSome *text* and `code`.\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n- one\n- two\n";
    let look = mdview::Look {
        light: true,
        ..Default::default()
    };
    let page = mdview::page_for(
        mdview::MdView::Compiled,
        src,
        "notes.md",
        look,
        false,
        &|| false,
    )
    .ok_or("no compiled page")?;
    let spec = write(data, 15, &[(app::PAGE, page.as_bytes())])?;
    let slot = Slot::panel("a5");
    let from = h.mark();
    h.host.open_app("a5", spec, BOUNDS, true);
    let opened = h.opened(from, "a5", "notes.md");
    let doc = h
        .eval(
            "a5",
            "({h1: (document.querySelector('h1') || {}).textContent, cells: document.querySelectorAll('td').length, \
              items: document.querySelectorAll('li').length, scripts: document.scripts.length})",
        )
        .unwrap_or(Value::Null);
    let (size, n) = painted(&mut h, &slot, 3, 10_000);
    rep.check(
        "mini.markdown",
        opened
            && doc["h1"] == "Notes"
            && doc["cells"] == 2
            && doc["items"] == 2
            && doc["scripts"] == 0
            && full(size)
            && n >= 3,
        "the markdown Compiled view: Flare's page of the file (heading, table, list, no script) loads and paints",
        json!({"opened": opened, "document": doc, "size": size, "colours": n}),
    );
    h.host.close("a5");
    h.wait(200);
    rep.set(
        "events",
        json!(h.seen.iter().map(ev_text).collect::<Vec<_>>()),
    );
    h.host.close_all();
    Ok(())
}

fn right_click(h: &mut H, slot: &Slot, x: f32, y: f32) {
    for act in [
        PointerAct::Move,
        PointerAct::Down(PointerButton::Right, 1),
        PointerAct::Up(PointerButton::Right, 1),
    ] {
        h.host.pointer(
            slot,
            Pointer {
                x,
                y,
                act,
                mods: Mods::default(),
            },
        );
        h.wait(30);
    }
}

/// Cmd plus a key, down and up: Cmd+R (reload), Cmd+= (zoom), Cmd+F (find), Cmd+P (print).
fn cmd_key(h: &mut H, slot: &Slot, vk: u32, mac: u16, text: &str) {
    for down in [true, false] {
        h.host.key(
            slot,
            KeyInput {
                down,
                repeat: false,
                vk,
                mac_code: mac,
                text: text.to_string(),
                mods: Mods {
                    cmd: true,
                    ..Mods::default()
                },
            },
        );
    }
    h.wait(50);
}

/// A plugin page that posts over its bridge as soon as it runs and keeps what comes back.
const PLUGIN_PAGE: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>Plugin</title></head>
<body style="margin:0;background:#fff"><div style="width:400px;height:60px;background:#3366cc"></div>
<a id="d" download href="data.txt">d</a><script>
window.__got = [];
document.addEventListener('contextmenu', function (e) { document.title = 'menu ' + e.isTrusted; });
document.addEventListener('keydown', function (e) { if (e.metaKey) { document.title = 'key ' + e.key + ' ' + e.isTrusted; } });
window.chrome.webview.addEventListener('message', function (e) { window.__got.push(e.data); });
window.chrome.webview.postMessage(JSON.stringify({t: 'call', id: 1, method: 'view.text', params: {}}));
</script></body></html>"#;

pub fn plugins(rep: &mut Report, data: &Path) -> Result<(), String> {
    let srv = pages::start()?;
    let mut h = H::new(data, Vec::new());
    h.host.set_scale(2.0);

    // ---- a plugin's fill, the way web_ctl runs it
    let url = srv.url("/login");
    let from = h.mark();
    h.host.open("t1", &url, BOUNDS, true);
    let opened = h.opened(from, "t1", "Sign in");
    let site =
        flare_mods::fill::check_login_for(&url, &[format!("http://127.0.0.1:{}/", srv.port)])
            .map_err(|e| format!("{e:?}"))?;
    let values = flare_mods::fill::Values {
        username: Some(flare_mods::fill::Secret::new("max@example.com".into())),
        password: Some(flare_mods::fill::Secret::new("p\"a'ss</script>".into())),
        totp: Some(flare_mods::fill::Secret::new("123456".into())),
    };
    let script = flare_mods::fill::fill_script(&values, &site);
    let answer = h.script("t1", script.expose());
    let filled = answer
        .as_deref()
        .map_err(String::clone)
        .and_then(|a| flare_mods::fill::read_fill_answer(a).map_err(|e| format!("{e:?}")));
    let typed = json_of(&h.script(
        "t1",
        "JSON.stringify({u: document.getElementById('u').value, p: document.getElementById('p').value, \
         c: document.getElementById('c').value, ev: window.__ev})",
    ));
    let ev: Vec<&str> = typed["ev"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    rep.check(
        "fill.types",
        opened
            && filled
                .as_ref()
                .is_ok_and(|f| f.filled == ["username", "password", "totp"] && f.missing.is_empty())
            && typed["u"] == "max@example.com"
            && typed["p"] == "p\"a'ss</script>"
            && typed["c"] == "123456"
            && ["input:u", "change:u", "input:p", "change:p", "input:c", "change:c"]
                .iter()
                .all(|e| ev.contains(e)),
        "browser.fill: the location-checked script, run through run_script as web_ctl runs it, types the three values and fires input and change; the answer names kinds of field only",
        json!({"opened": opened, "answer": format!("{answer:?}"), "filled": format!("{filled:?}"),
               "fields": {"u": typed["u"] == "max@example.com", "p": typed["p"] == "p\"a'ss</script>", "c": typed["c"] == "123456"}, "events": ev}),
    );
    // The tab moved on (another port is another site) before the script ran: nothing is typed.
    let _ = h.script(
        "t1",
        "['u','p','c'].forEach(function (i) { document.getElementById(i).value = ''; }); window.__ev = []; 1",
    );
    let other = flare_mods::fill::Site {
        port: site.port.map(|p| p.wrapping_add(1)),
        ..site.clone()
    };
    let moved_answer = h.script(
        "t1",
        flare_mods::fill::fill_script(&values, &other).expose(),
    );
    let moved = moved_answer
        .as_deref()
        .map_err(String::clone)
        .and_then(|a| flare_mods::fill::read_fill_answer(a).map_err(|e| format!("{e:?}")));
    let after = json_of(&h.script(
        "t1",
        "JSON.stringify({u: document.getElementById('u').value, p: document.getElementById('p').value, \
         c: document.getElementById('c').value, ev: window.__ev.length})",
    ));
    rep.check(
        "fill.moved",
        moved_answer
            .as_deref()
            .is_ok_and(|a| a.contains("\"ok\":false") && a.contains("moved"))
            && moved.is_err()
            && after["u"] == ""
            && after["p"] == ""
            && after["c"] == ""
            && after["ev"] == 0,
        "browser.fill on a page that is no longer the login's site answers {ok:false, why:'moved'} and types nothing",
        json!({"answer": format!("{moved_answer:?}"), "read": format!("{moved:?}"), "after": after}),
    );
    h.host.close("t1");
    h.wait(200);

    // ---- a plugin's bar and its background page, in the same host as the web panel
    let dir = data.join("plugin");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("index.html"), PLUGIN_PAGE).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("data.txt"), "x").map_err(|e| e.to_string())?;
    let dir = std::fs::canonicalize(&dir).map_err(|e| e.to_string())?;
    let bar_rect = PhysRect {
        w: 1200,
        h: 160,
        ..BOUNDS
    };
    let bg = Slot::background("plug");
    let from = h.mark();
    h.host.open_view(
        &bg,
        plugin_spec("plug", &dir, "index.html", false),
        bar_rect,
        false,
    );
    let bg_message = h.until(from, 20_000, |e| {
        matches!(e, WebEvent::View { slot, event } if slot.kind == bg.kind
            && matches!(&**event, WebEvent::ModMessage { json, .. } if json.contains("view.text")))
    });
    let bg_probe = h.host.probe_view(&bg).map(|p| (p.profile, p.private));
    rep.check(
        "plugin.background",
        bg_message.is_some() && bg_probe == Some(("flare-mod-plug".to_string(), false)),
        "a plugin's background page opens hidden in the same host, sealed in its plugin's profile, and talks over its bridge",
        json!({"message": bg_message.as_ref().map(ev_text), "probe": format!("{bg_probe:?}")}),
    );

    let bar = Slot::bar("t2");
    let from = h.mark();
    h.host.open_view(
        &bar,
        plugin_spec("plug", &dir, "index.html", false),
        bar_rect,
        true,
    );
    let bar_ready = h
        .until(from, 20_000, |e| {
            matches!(e, WebEvent::View { slot, event } if slot.kind == bar.kind
                && matches!(**event, WebEvent::ModMessage { .. }))
        })
        .is_some();
    let (size, n) = {
        let t = Instant::now();
        let mut seen = (None, 0);
        while t.elapsed() < Duration::from_secs(10) {
            h.wait(50);
            if let Some(f) = h.host.frame(&bar) {
                seen = (Some((f.width, f.height)), colours(&f));
                if seen.1 >= 2 {
                    break;
                }
            }
        }
        seen
    };
    let zoom_before = h.host.zoom_level(&bar);
    let from = h.mark();
    // A right click reaches the bar's page (its `contextmenu` event, trusted), and CEF's menu for it is emptied, so
    // nothing is shown (the test watches the window server for the whole run).
    right_click(&mut h, &bar, 100.0, 40.0);
    // Cmd+R, Cmd+=, Cmd+F, Cmd+P: a browser's keys, none of which a bar may use. They reach the page as keys only.
    cmd_key(&mut h, &bar, 0x52, 15, "r");
    cmd_key(&mut h, &bar, 0xBB, 24, "=");
    cmd_key(&mut h, &bar, 0x46, 3, "f");
    cmd_key(&mut h, &bar, 0x50, 35, "p");
    h.host
        .inspect_exec(&bar, "document.getElementById('d').click(); alert('x');");
    h.host
        .inspect_exec(&bg, "document.getElementById('d').click(); confirm('x');");
    h.wait(1_500);
    let ev = h.since(from);
    let zoom_after = h.host.zoom_level(&bar);
    let (rb, rg) = (h.host.refused(&bar), h.host.refused(&bg));
    let sealed = |r: &Option<flare_cef::macos::view::Refused>| {
        r.as_ref()
            .is_some_and(|r| r.downloads >= 1 && r.js_dialogs >= 1)
    };
    let saw = |t: &str| {
        ev.iter()
            .any(|e| e.starts_with("View Bar Title") && e.ends_with(t))
    };
    rep.check(
        "plugin.sealed",
        bar_ready
            && size.is_some()
            && n >= 2
            && saw("menu true")
            && saw("key p true")
            && sealed(&rb)
            && sealed(&rg)
            && rb.as_ref().is_some_and(|r| r.context_menus == 0 && r.popups == 0)
            && zoom_before == zoom_after
            && !ev.iter().any(|e| e.contains("Loading") || e.contains("Download") || e.contains("Popups")),
        "a plugin's bar and background page keep their seals beside the web panel: a right click shows no menu, Cmd+R, Cmd+=, Cmd+F and Cmd+P reach the page as keys and do nothing of the browser's (no reload, zoom, find or print), a download is cancelled before it starts and a dialog answered unseen",
        json!({"bar_ready": bar_ready, "bar_size": size, "bar_colours": n, "bar": format!("{rb:?}"), "background": format!("{rg:?}"),
               "zoom": [zoom_before, zoom_after], "events": ev}),
    );
    rep.set(
        "events",
        json!(h.seen.iter().map(ev_text).collect::<Vec<_>>()),
    );
    h.host.close_all();
    Ok(())
}

//! Panels and file types: `panelTypes`.
//!
//! Shows a picture, text, document and media file of each kind through the channel (`view.image`,
//! `view.file`) and through the browser panel (`browser.open` of the served folder), and records what
//! each one gives: the call's answer, what the panel holds, a snapshot of the window, and for a page
//! what the browser made of it. The files come from `FLARE_E2E_TYPES_DIR` (made by hand); without it
//! the scenario does nothing.

use std::fs;
use std::time::Duration;

use flare_ctl::Method;
use serde_json::{json, Value};

use super::input_b::{probe, prompt};
use super::web::{channel_of, code_of, hello, launch_with, script, state as web_state, HOST, LOAD};
use crate::ctx::Ctx;
use flare_e2e_proto::Request;

const WAIT: Duration = Duration::from_secs(10);

const IMAGES: &[&str] = &[
    "pic.png", "pic.jpg", "pic.webp", "pic.bmp", "pic.ico", "anim.gif", "pic.svg", "pic.tiff",
    "empty.png", "corrupt.png", "with space æ.png",
];
const TEXTS: &[&str] = &[
    "unicode.txt", "utf16.txt", "binary.txt", "data.csv", "notes.md", "doc.pdf", "doc.docx",
    "sheet.xlsx", "tone.mp3",
];
const PAGES: &[&str] = &[
    "doc.pdf", "tone.mp3", "tone.wav", "tone.flac", "tone.ogg", "pic.svg", "pic.tiff", "anim.gif",
    "notes.md", "unicode.txt", "data.csv", "doc.docx", "sheet.xlsx",
];

pub fn panel_types(ctx: &mut Ctx) -> Result<(), String> {
    let Ok(src) = std::env::var("FLARE_E2E_TYPES_DIR") else {
        ctx.ok(true, "no FLARE_E2E_TYPES_DIR: nothing to do");
        return Ok(());
    };
    let alpha = ctx.ensure_world().alpha.clone();
    let root = ctx.ensure_world().root.join("web");
    fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    for e in fs::read_dir(&src).map_err(|e| e.to_string())? {
        let p = e.map_err(|e| e.to_string())?.path();
        if let Some(n) = p.file_name() {
            fs::copy(&p, alpha.join(n)).map_err(|e| e.to_string())?;
            fs::copy(&p, root.join(n)).map_err(|e| e.to_string())?;
        }
    }
    let mut app = launch_with(ctx, &root, 96, &[])?;
    app.session.call(&Request::Resize {
        w: 1000.0,
        h: 640.0,
    })?;
    prompt(&mut app)?;
    let (pipe, _, token) = channel_of(&mut app)?;
    let mut c = hello(&pipe, &token).map_err(|e| format!("hello: {e}"))?;

    for (method, list, tag) in [
        (Method::ViewImage, IMAGES, "image"),
        (Method::ViewFile, TEXTS, "file"),
    ] {
        for name in list {
            let path = alpha.join(name).to_string_lossy().into_owned();
            let r = c.call(method, json!({ "path": path }), WAIT);
            std::thread::sleep(Duration::from_millis(1500));
            let s = probe(&mut app, "panel.state", Value::Null);
            let shot = ctx.shot(&mut app, &format!("{tag}-{}", name.replace(' ', "_")));
            let answer = match &r {
                Ok(v) => format!("ok {}", v["panel"]),
                Err(_) => format!("refused {} {:?}", code_of(&r), r.as_ref().err().map(|e| e.to_string())),
            };
            ctx.ok(
                true,
                format!("view.{tag} {name}: {answer} | panel {s} | shot {shot:?}"),
            );
        }
    }

    for name in PAGES {
        let url = format!("{HOST}/{}", name.replace(' ', "%20"));
        let r = c.call(Method::BrowserOpen, json!({ "url": url }), LOAD);
        std::thread::sleep(Duration::from_millis(2000));
        let what = script(
            ctx,
            &mut app,
            "JSON.stringify({ct: document.contentType, title: document.title, body: (document.body ? document.body.innerText.slice(0, 120) : null), media: (function(){var m=document.querySelector('video,audio'); return m ? {tag: m.tagName, dur: m.duration, err: m.error && m.error.code, rs: m.readyState} : null})(), embed: !!document.querySelector('embed,pdf-viewer,iframe')})",
        );
        let ws = web_state(&mut app);
        let shot = ctx.shot(&mut app, &format!("page-{name}"));
        ctx.ok(
            true,
            format!(
                "browser.open {name}: {} {:?} | page {what:?} | web title {} failed {} | shot {shot:?}",
                code_of(&r),
                r.as_ref().ok(),
                ws["controller"]["title"],
                ws["failed"]
            ),
        );
        // A download question may be up: answer no and move on.
        let d = probe(&mut app, "app.state", Value::Null)["dialog"].clone();
        if d["open"] == json!(true) {
            ctx.ok(true, format!("{name}: a download question is up ({d})"));
            let _ = app.session.key("Escape");
            std::thread::sleep(Duration::from_millis(500));
        }
    }
    // How long a menu over the terminal waits before the page is hidden (the still in its place).
    let _ = c.call(Method::BrowserOpen, json!({ "url": format!("{HOST}/doc.pdf") }), LOAD);
    std::thread::sleep(Duration::from_millis(1500));
    let g = probe(&mut app, "ui.rect", json!({"name": "grid"}));
    let (gx, gy) = (g["dx"].as_f64().unwrap_or(0.0), g["dy"].as_f64().unwrap_or(0.0));
    for i in 0..6 {
        let t0 = std::time::Instant::now();
        app.session.right_click((gx + 60.0) as f32, (gy + 60.0) as f32)?;
        let mut hidden_ms = None;
        while t0.elapsed() < Duration::from_secs(2) {
            let s = web_state(&mut app);
            if s["controller"]["visible"] == json!(false) {
                hidden_ms = Some(t0.elapsed().as_millis());
                break;
            }
        }
        ctx.ok(true, format!("menu {i}: the page hid after {hidden_ms:?} ms"));
        app.session.key("Escape")?;
        std::thread::sleep(Duration::from_millis(600));
    }
    let left = app.close();
    ctx.ok(left, "the app quits by itself");
    Ok(())
}

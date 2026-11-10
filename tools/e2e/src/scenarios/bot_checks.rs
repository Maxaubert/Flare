//! What a bot check sees of the web panel (#146): `webBotChecks`, opt-in, since it opens public pages on the network.
//!
//! The panel is the built release exe in its own world and profile, parked off the screen like every scenario: a fresh
//! browser folder, no account, no login, no payment. It opens public pages through the channel (`browser.open` and
//! `browser.navigate`, which are plain navigations, no DevTools call) and reads them through the e2e script probe only,
//! so the first pass is the panel exactly as the person has it. It records:
//!
//! - the fingerprint a page script reads: `navigator.userAgent`, `navigator.userAgentData.brands` and the
//!   `fullVersionList` of `getHighEntropyValues`, `navigator.webdriver`, `window.chrome` and its parts, the plugins;
//! - the request headers a header-echo page saw;
//! - what each bot-check page shows after it had time to run its challenge: its title and the start of its text.
//!
//! Then the agent looks at the page once (`browser.look`, the DevTools calls an agent makes) and the same pages are
//! read again, so a change made by the agent's use shows. Every record is one line starting `bot-check` and, with
//! `BOT_OUT` set, also appended to that file. `BOT_ARGS` adds browser arguments to the panel's browser (through
//! `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`), so one run can try a change without a build; `BOT_PAGES` replaces the
//! list of check pages (space-separated). The measurements are in `docs/web-bot-checks.md`.
//!
//! The checks only hold that the panel opened and the probe answered: what the pages decide is the measurement, not a
//! pass or a fail.

use std::fs::OpenOptions;
use std::io::Write;

use flare_ctl::{Client, Method, PipeEnd};
use flare_e2e_proto::Request;
use serde_json::{json, Value};

use super::input_b::prompt;
use super::web::{channel_of, hello, launch_with, LOAD};
use crate::ctx::{App, Ctx};

/// The header-echo page: it answers with the request headers it got, as JSON.
const ECHO: &str = "https://httpbin.org/headers";

/// Public pages behind Cloudflare or made to tell bots from people. The two demo pages answer every visitor first with
/// Cloudflare's managed challenge (`cf-mitigated: challenge`); the browser passes it when the page under it shows.
const PAGES: &[&str] = &[
    "https://www.scrapingcourse.com/cloudflare-challenge",
    "https://nopecha.com/demo/cloudflare",
    "https://nowsecure.nl/",
    "https://bot.sannysoft.com/",
    "https://www.cloudflare.com/cdn-cgi/trace",
    "https://client.cardinaltrusted.com/",
];

/// How long a check page gets to run its challenge before it is read.
const CHALLENGE_MS: u64 = 15_000;

/// Reads the fingerprint. `getHighEntropyValues` answers later, so the first call starts it and keeps the answer on
/// the page; a later call reads it.
const PRINT: &str = r#"(function(){var d=navigator.userAgentData;
if(d&&!window.__flareHev){window.__flareHev='pending';d.getHighEntropyValues(['fullVersionList','uaFullVersion','platformVersion','architecture','bitness','model']).then(function(v){window.__flareHev=v},function(e){window.__flareHev='error '+e})}
var c=window.chrome;
return JSON.stringify({ua:navigator.userAgent,brands:d?d.brands:null,mobile:d?d.mobile:null,platform:d?d.platform:null,
high:window.__flareHev||null,webdriver:navigator.webdriver,chrome:typeof c,chromeKeys:c?Object.keys(c):null,
plugins:navigator.plugins.length,mimeTypes:navigator.mimeTypes.length,languages:navigator.languages,
pdfViewer:navigator.pdfViewerEnabled,hardwareConcurrency:navigator.hardwareConcurrency,deviceMemory:navigator.deviceMemory,
outer:[window.outerWidth,window.outerHeight],inner:[window.innerWidth,window.innerHeight],
notification:typeof Notification==='undefined'?null:Notification.permission})})()"#;

/// What a page shows: its address, title and the start of its text. Cloudflare's trace page names the machine's
/// address on a line `ip=`: that line is hidden before anything is recorded.
const SHOWS: &str = "JSON.stringify({url:location.href,title:document.title,text:(document.body?document.body.innerText:'').replace(/^ip=.*$/m,'ip=(hidden)').slice(0,1500)})";

pub fn web_bot_checks(ctx: &mut Ctx) -> Result<(), String> {
    let root = ctx.ensure_world().root.join("web");
    std::fs::create_dir_all(&root).map_err(|e| format!("make the web folder: {e}"))?;
    let args = std::env::var("BOT_ARGS").unwrap_or_default();
    let mut extra = Vec::new();
    if !args.is_empty() {
        extra.push(("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS", args.as_str()));
    }
    let mut app = launch_with(ctx, &root, 96, &extra)?;
    let r = checks_on(ctx, &mut app, &args);
    let left = app.close();
    ctx.ok(left, "the app quits by itself");
    r
}

fn checks_on(ctx: &mut Ctx, app: &mut App, args: &str) -> Result<(), String> {
    app.session.call(&Request::Resize {
        w: 1200.0,
        h: 800.0,
    })?;
    prompt(app)?;
    let (pipe, _, token) = channel_of(app)?;
    let mut c = hello(&pipe, &token).map_err(|e| format!("hello: {e}"))?;
    let pages: Vec<String> = match std::env::var("BOT_PAGES") {
        Ok(p) if !p.trim().is_empty() => p.split_whitespace().map(str::to_string).collect(),
        _ => PAGES.iter().map(|p| p.to_string()).collect(),
    };
    record(&json!({"phase": "start", "args": args}));

    let opened = c.call(Method::BrowserOpen, json!({ "url": ECHO }), LOAD);
    ctx.ok(
        opened.is_ok(),
        format!("the panel opens the header-echo page ({opened:?})"),
    );
    if opened.is_err() {
        return Err("the panel did not open".into());
    }
    pass(ctx, app, &mut c, "panel", &pages);

    // The agent looks once: the DevTools calls an agent makes. Then the same pages again.
    let look = c.call(Method::BrowserLook, json!({}), LOAD);
    ctx.ok(
        look.is_ok(),
        format!(
            "the agent looks at the page ({})",
            look.as_ref()
                .map_or_else(|e| e.to_string(), |v| v["title"].to_string())
        ),
    );
    pass(ctx, app, &mut c, "after-look", &pages);
    Ok(())
}

/// One pass: the fingerprint and the headers on the echo page, then each check page.
fn pass(ctx: &mut Ctx, app: &mut App, c: &mut Client<PipeEnd>, phase: &str, pages: &[String]) {
    let nav = c.call(Method::BrowserNavigate, json!({ "url": ECHO }), LOAD);
    let print = read_print(ctx, app);
    ctx.ok(
        print.is_some(),
        format!("{phase}: the probe reads the fingerprint"),
    );
    record(&json!({"phase": phase, "what": "fingerprint", "value": print}));
    let echo = read(ctx, app, SHOWS);
    record(&json!({"phase": phase, "what": "headers", "loaded": nav.is_ok(), "value": echo}));
    for page in pages {
        let nav = c.call(Method::BrowserNavigate, json!({ "url": page }), LOAD);
        // The challenge runs, reloads and answers on its own: the page gets its time before it is read.
        ctx.poll(CHALLENGE_MS, &mut || false);
        let shows = read(ctx, app, SHOWS);
        ctx.ok(shows.is_some(), format!("{phase}: the probe reads {page}"));
        record(&json!({
            "phase": phase,
            "what": "page",
            "page": page,
            "loaded": nav.as_ref().map_or_else(|e| format!("error: {e}"), |_| "ok".to_string()),
            "value": shows,
        }));
    }
}

/// The fingerprint, with `getHighEntropyValues` given time to answer.
fn read_print(ctx: &mut Ctx, app: &mut App) -> Option<Value> {
    let mut last = read(ctx, app, PRINT);
    let ready = |v: &Option<Value>| {
        v.as_ref()
            .is_some_and(|v| v["high"] != json!("pending") && !v["high"].is_null())
    };
    if !ready(&last) {
        ctx.poll(1_500, &mut || false);
        last = read(ctx, app, PRINT);
    }
    last
}

/// Runs `js` (which answers a JSON string) in the panel's page and parses its answer.
fn read(ctx: &mut Ctx, app: &mut App, js: &str) -> Option<Value> {
    let raw = super::web::script(ctx, app, js)?;
    let inner: String = serde_json::from_str(&raw).ok()?;
    serde_json::from_str(&inner).ok()
}

fn record(v: &Value) {
    let line = format!("bot-check {v}");
    println!("{line}");
    if let Ok(path) = std::env::var("BOT_OUT") {
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "{line}");
        }
    }
}

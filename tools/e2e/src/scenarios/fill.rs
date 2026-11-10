//! The `fill` permission (2026-10-28): `pluginFill`.
//!
//! The scenario puts a test plugin in the world's own plugins folder (permissions `process`, `tools`, `fill`, one
//! tool `fill_login`, no page) whose helper is this exe in its fake-helper mode (`crate::fake_fill_helper`), and
//! seeds the profile's `mods.json` with the plugin on and those permissions approved, so no click on Settings is
//! needed. A local sign-in page is served as `https://flare-test.local/login.html` (the `WEB_ROOT` seam). Nothing
//! touches a real vault: the login is the fake helper's constants.
//!
//! The scenario plays the agent on the channel (`mods.call` with the tab's secret, as `flare-mcp` does) and the
//! person with the pointer on Flare's own dialog, through the e2e hook like every other dialog press. It holds: the
//! helper hears the tool call as a `tool` event; Flare asks before any fill, naming the plugin, the login and the
//! host, with the page hidden under the question; Enter presses Deny, never Fill; Fill by the pointer fills the
//! user name, the password and the one-time code and fires `input` and `change`; a login of another site is refused
//! with no question; and no value is in the agent's results, the helper's record, Flare's logs or the plugins' logs.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use flare_ctl::Method;
use serde_json::{json, Value};

use super::input_b::{probe, prompt};
use super::web::{channel_of, hello, launch_with, script, HOST, LOAD};
use crate::ctx::{App, Ctx};
use crate::fake_fill_helper::{CODE, PASS, TOOL, USER};

const ID: &str = "filltest";

const LOGIN: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>Sign in</title></head>
<body style="font:16px sans-serif"><form id="f" onsubmit="return false">
<label>Email <input id="u" name="login" autocomplete="username"></label>
<label>Password <input id="p" type="password" autocomplete="current-password"></label>
<label>Code <input id="o" inputmode="numeric" autocomplete="one-time-code"></label>
<button>Sign in</button></form>
<script>
window.__ev = {u: 0, p: 0, o: 0, cu: 0, cp: 0, co: 0};
['u', 'p', 'o'].forEach(function (k) {
  var e = document.getElementById(k);
  e.addEventListener('input', function () { window.__ev[k]++; });
  e.addEventListener('change', function () { window.__ev['c' + k]++; });
});
</script></body></html>"#;

/// The fields as hashes and lengths and the events they heard: the scenario compares, and no value passes the hook.
const FIELDS_JS: &str = "(function () {
  var h = function (s) { var a = 7; for (var i = 0; i < s.length; i++) { a = (Math.imul(a, 31) + s.charCodeAt(i)) >>> 0; } return a; };
  var v = function (id) { return document.getElementById(id).value; };
  return {u: h(v('u')), p: h(v('p')), o: h(v('o')), lu: v('u').length, lp: v('p').length, lo: v('o').length, ev: window.__ev};
})()";

/// The page's hash of a text (`FIELDS_JS`'s `h`).
fn page_hash(s: &str) -> u64 {
    let mut a: u32 = 7;
    for c in s.encode_utf16() {
        a = a.wrapping_mul(31).wrapping_add(u32::from(c));
    }
    u64::from(a)
}

fn manifest(record: &Path) -> String {
    json!({
        "id": ID,
        "name": "Fill test",
        "version": "1.0.0",
        "protocol": 2,
        "description": "The pluginFill scenario's plugin: its helper fills a fake login.",
        "author": "flare-e2e",
        "homepage": "https://example.com/filltest",
        "permissions": ["process", "tools", "fill"],
        "helper": {"command": "bin\\helper.exe", "args": ["--fake-fill-helper", record.to_string_lossy()]},
        "tools": [{"name": TOOL, "description": "Fills the saved login into the page.", "input_schema": {"type": "object"}}]
    })
    .to_string()
}

/// The plugin folder (this exe as its helper) and the approval in `mods.json`.
fn install(profile: &Path, record: &Path) -> Result<(), String> {
    let folder = profile.join("plugins").join(ID);
    fs::create_dir_all(folder.join("bin")).map_err(|e| format!("make the plugin folder: {e}"))?;
    let me = std::env::current_exe().map_err(|e| format!("this exe: {e}"))?;
    fs::copy(&me, folder.join("bin").join("helper.exe"))
        .map_err(|e| format!("copy the fake helper: {e}"))?;
    fs::write(folder.join("plugin.json"), manifest(record))
        .map_err(|e| format!("write plugin.json: {e}"))?;
    let switches = json!({"version": 1, "mods": {ID: {
        "on": true,
        "permissions": ["process", "tools", "fill"],
        "author": "flare-e2e",
        "homepage": "https://example.com/filltest",
    }}});
    fs::write(profile.join("mods.json"), switches.to_string())
        .map_err(|e| format!("write mods.json: {e}"))
}

fn web_root(root: &Path) -> Result<PathBuf, String> {
    let web = root.join("web");
    fs::create_dir_all(&web).map_err(|e| format!("make the web folder: {e}"))?;
    fs::write(web.join("login.html"), LOGIN).map_err(|e| format!("write login.html: {e}"))?;
    Ok(web)
}

fn records(file: &Path) -> Vec<Value> {
    fs::read_to_string(file)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

fn ask_state(app: &mut App) -> Value {
    probe(app, "plugin.ask", Value::Null)
}

/// The agent calls the plugin's tool from the tab whose secret it holds, on a thread of its own (the call waits for
/// the person). The answer is `mods.call`'s, or the error's code and message.
fn agent_tool(pipe: &str, token: &str, case: &str) -> std::thread::JoinHandle<Value> {
    let (pipe, token, case) = (pipe.to_string(), token.to_string(), case.to_string());
    std::thread::spawn(move || {
        let mut c = match hello(&pipe, &token) {
            Ok(c) => c,
            Err(e) => return json!({"error": format!("hello: {e}")}),
        };
        let name = format!("mod_{ID}_{TOOL}");
        match c.call(
            Method::ModsCall,
            json!({"name": name, "input": {"case": case}}),
            Duration::from_secs(60),
        ) {
            Ok(v) => v,
            Err(flare_ctl::ClientError::Remote(e)) => {
                json!({"error": e.code, "message": e.message})
            }
            Err(e) => json!({"error": e.to_string()}),
        }
    })
}

fn fields(ctx: &mut Ctx, app: &mut App) -> Value {
    script(ctx, app, FIELDS_JS)
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null)
}

/// The centre of one of the question's buttons, in css px.
fn press(app: &mut App, q: &Value, button: &str) -> Result<(), String> {
    let r = &q["buttons"][button];
    let (Some(x), Some(y), Some(w), Some(h)) = (
        r["x"].as_f64(),
        r["y"].as_f64(),
        r["w"].as_f64(),
        r["h"].as_f64(),
    ) else {
        return Err(format!("no rect for the {button} button ({q})"));
    };
    if w <= 0.0 || h <= 0.0 {
        return Err(format!("the {button} button is not drawn ({r})"));
    }
    app.session
        .click((x + w / 2.0) as f32, (y + h / 2.0) as f32)
}

/// Every file Flare and the plugin wrote that a secret must never reach: the logs, the plugins' logs and stores, the
/// switch file and the fake helper's record.
fn secret_free(ctx: &mut Ctx, profile: &Path, record: &Path, when: &str) {
    let mut files = vec![record.to_path_buf(), profile.join("mods.json")];
    for dir in ["logs", "plugins-logs", "plugins-storage"] {
        if let Ok(rd) = fs::read_dir(profile.join(dir)) {
            files.extend(rd.flatten().map(|e| e.path()).filter(|p| p.is_file()));
        }
    }
    let mut leaks = Vec::new();
    let mut read = 0;
    for f in &files {
        let Ok(bytes) = fs::read(f) else { continue };
        read += 1;
        let text = String::from_utf8_lossy(&bytes);
        for (name, v) in [("password", PASS), ("user name", USER), ("code", CODE)] {
            if text.contains(v) {
                leaks.push(format!("{name} in {}", f.display()));
            }
        }
    }
    ctx.ok(
        read >= 2 && leaks.is_empty(),
        format!("{when}: no value of the login is in Flare's logs, the plugins' logs or the helper's record ({read} files read; {leaks:?})"),
    );
}

pub fn plugin_fill(ctx: &mut Ctx) -> Result<(), String> {
    let (profile, root) = {
        let w = ctx.ensure_world();
        (w.profile.clone(), w.root.clone())
    };
    let record = root.join("fill-helper.jsonl");
    install(&profile, &record)?;
    let web = web_root(&root)?;
    let mut app = launch_with(ctx, &web, 96, &[])?;
    let r = run(ctx, &mut app, &profile, &record);
    let left = app.close();
    ctx.ok(left, "the app quits by itself");
    r?;
    // Everything is on the disk once Flare has quit.
    secret_free(ctx, &profile, &record, "after the quit");
    Ok(())
}

fn run(ctx: &mut Ctx, app: &mut App, profile: &Path, record: &Path) -> Result<(), String> {
    prompt(app)?;
    let tool = format!("mod_{ID}_{TOOL}");
    let on = ctx.until("the plugin to be on with its tool", 20_000, &mut || {
        probe(app, "mods.state", Value::Null)["tools"]
            .as_array()
            .is_some_and(|t| {
                t.iter()
                    .any(|x| x.as_str() == Some(&tool) || x["name"].as_str() == Some(&tool))
            })
    });
    ctx.ok(
        on,
        "the seeded switch turns the plugin on and the agent has its tool",
    );
    let said_hello = ctx.until("the fake helper to say hello", 30_000, &mut || {
        records(record).iter().any(|r| r["hello"]["Ok"].is_array())
    });
    let hello_line = records(record)
        .into_iter()
        .find(|r| r.get("hello").is_some());
    ctx.ok(
        said_hello
            && hello_line
                .as_ref()
                .is_some_and(|r| r["hello"]["Ok"].as_array().is_some_and(|p| p.iter().any(|x| x == "fill"))),
        format!("Flare starts the helper and it says hello with the plugin's token and fill ({hello_line:?})"),
    );

    let (pipe, _tab, token) = channel_of(app)?;
    let mut agent = hello(&pipe, &token).map_err(|e| format!("hello: {e}"))?;
    let opened = agent.call(
        Method::BrowserOpen,
        json!({"url": format!("{HOST}/login.html")}),
        LOAD,
    );
    ctx.ok(
        opened.as_ref().is_ok_and(|v| v["loaded"] == json!(true)),
        format!("the sign-in page loads in the web panel ({opened:?})"),
    );

    // ---- a key presses Deny, never Allow ----
    let call = agent_tool(&pipe, &token, "match");
    let asked = ctx.until("the fill question", 20_000, &mut || {
        ask_state(app)["open"] == json!(true)
    });
    let q = ask_state(app);
    ctx.ok(
        asked,
        format!("the helper's fill puts Flare's question up ({q})"),
    );
    ctx.ok(
        q["kind"] == json!("fill")
            && q["plugin"] == json!(ID)
            && q["title"] == json!("Fill E2E test login?")
            && q["site"] == json!("flare-test.local")
            && q["foot"] == json!("")
            && q["branded"] == json!(false),
        format!("the question names the login and the host, with no line under it, in Flare's own look (no brand) ({q})"),
    );
    ctx.ok(
        q["overlay_open"] == json!(true),
        "the web page hides under the question",
    );
    let before = fields(ctx, app);
    ctx.ok(
        before["lu"] == json!(0) && before["lp"] == json!(0) && before["lo"] == json!(0),
        format!("nothing is typed before the answer ({before})"),
    );
    app.session.key("Enter")?;
    let answer = call.join().unwrap_or(Value::Null);
    let mut agent_texts = vec![answer.to_string()];
    ctx.ok(
        answer["result"]["outcome"] == json!({"declined": true}),
        format!("Enter presses Deny, never Allow: the agent hears declined ({answer})"),
    );
    let after_enter = fields(ctx, app);
    ctx.ok(
        after_enter["lu"] == json!(0) && after_enter["lp"] == json!(0),
        format!("and nothing was typed ({after_enter})"),
    );

    // ---- Allow, pressed by the pointer, fills the page ----
    let call = agent_tool(&pipe, &token, "match");
    let asked = ctx.until("the fill question again", 20_000, &mut || {
        ask_state(app)["open"] == json!(true)
    });
    ctx.ok(asked, "the second fill asks again");
    let q = ask_state(app);
    // A press in the question's first moment does nothing (a click already on its way never approves).
    if q["allow_armed"] == json!(false) {
        press(app, &q, "allow")?;
        let early = ask_state(app);
        let typed = fields(ctx, app);
        ctx.ok(
            early["asking"] == json!(true) && typed["lp"] == json!(0),
            format!("a press on Allow as the question shows does nothing ({early}, {typed})"),
        );
    }
    // The agent cannot answer it: its key and its click go to the page, never to Flare's question.
    let quick = Duration::from_secs(10);
    let pressed = agent.call(Method::BrowserPress, json!({"key": "Enter"}), quick);
    let r = &q["buttons"]["allow"];
    let (ax, ay) = (
        r["x"].as_f64().unwrap_or(0.0) + r["w"].as_f64().unwrap_or(0.0) / 2.0,
        r["y"].as_f64().unwrap_or(0.0) + r["h"].as_f64().unwrap_or(0.0) / 2.0,
    );
    let clicked = agent.call(Method::BrowserClick, json!({"x": ax, "y": ay}), quick);
    let still = ask_state(app);
    let typed = fields(ctx, app);
    ctx.ok(
        still["asking"] == json!(true) && typed["lu"] == json!(0) && typed["lp"] == json!(0),
        format!("the agent's Enter and its click where Allow is answer nothing ({still}, {typed}; {pressed:?}, {clicked:?})"),
    );
    let armed = ctx.until("Allow to take presses", 5_000, &mut || {
        ask_state(app)["allow_armed"] == json!(true)
    });
    ctx.ok(armed, "Allow takes presses after its first moment");
    let q = ask_state(app);
    press(app, &q, "allow")?;
    let answer = call.join().unwrap_or(Value::Null);
    agent_texts.push(answer.to_string());
    ctx.ok(
        answer["result"]["outcome"] == json!({"filled": ["username", "password", "totp"], "missing": []}),
        format!("Allow fills the user name, the password and the code, and the agent hears that only ({answer})"),
    );
    let f = fields(ctx, app);
    // The page's JSON may write a large hash as a float (`3452147312.0`): compared as numbers.
    let same = |k: &str, v: &str| f[k].as_f64() == Some(page_hash(v) as f64);
    ctx.ok(
        same("u", USER) && same("p", PASS) && same("o", CODE),
        format!("the page's fields hold the login ({f})"),
    );
    let ev = &f["ev"];
    ctx.ok(
        ["u", "p", "o", "cu", "cp", "co"]
            .iter()
            .all(|k| ev[*k].as_i64().is_some_and(|n| n >= 1)),
        format!("each field heard input and change, as typing fires them ({ev})"),
    );
    ctx.ok(
        ask_state(app)["open"] == json!(false),
        "the question is gone",
    );

    // ---- a login of another site is refused, with no question ----
    let call = agent_tool(&pipe, &token, "mismatch");
    let answer = call.join().unwrap_or(Value::Null);
    agent_texts.push(answer.to_string());
    let out = &answer["result"]["outcome"];
    ctx.ok(
        out["error"] == json!("forbidden")
            && out["message"] == json!("this login is not for flare-test.local"),
        format!("a login of another site is refused: \"this login is not for <host>\" ({answer})"),
    );
    ctx.ok(
        ask_state(app)["asking"] == json!(false),
        "and nothing was asked",
    );

    // ---- no value anywhere the agent or the disk sees ----
    let heard = records(record);
    let tool_lines: Vec<&Value> = heard
        .iter()
        .filter(|r| r["event"] == json!("tool"))
        .collect();
    ctx.ok(
        tool_lines.len() == 3
            && tool_lines.iter().all(|r| {
                r["page"]["host"] == json!("flare-test.local") && r["respond"] == json!({})
            }),
        format!("the helper heard three tool events, read the page's host each time and answered ({heard:?})"),
    );
    ctx.ok(
        agent_texts.len() == 3
            && agent_texts
                .iter()
                .all(|t| !t.contains(PASS) && !t.contains(USER) && !t.contains(CODE)),
        "none of the agent's three results holds a value of the login",
    );
    secret_free(ctx, profile, record, "while running");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_hash_is_the_scripts() {
        // h('') = 7; h('a') = 7*31 + 97.
        assert_eq!(page_hash(""), 7);
        assert_eq!(page_hash("a"), 7 * 31 + 97);
        assert!(FIELDS_JS.contains("Math.imul(a, 31)"));
    }

    #[test]
    fn the_manifest_is_a_valid_plugin_with_a_helper_and_no_page() {
        let m = manifest(Path::new(r"C:\w\fill-helper.jsonl"));
        let v: Value = serde_json::from_str(&m).unwrap();
        assert_eq!(v["permissions"], json!(["process", "tools", "fill"]));
        assert!(v.get("background").is_none() && v.get("panel").is_none());
        assert_eq!(v["helper"]["args"][0], "--fake-fill-helper");
    }
}

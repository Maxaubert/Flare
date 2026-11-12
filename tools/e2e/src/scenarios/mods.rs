//! Mods (Surfaces stage 8): `modsSample`.
//!
//! The scenario starts the built release exe in its own world and profile and copies the sample mod of
//! `docs/surfaces/sample-mod` (and one broken folder) into the PROFILE's plugins folder, so the owner's real mods
//! are never read. It then plays the person with the pointer on Settings > Mods and the agent with the built
//! `flare-mcp.exe`, exactly as Claude Code would run it inside the tab. Nothing is fetched from the network
//! (the page's own `fetch` of an outside address is checked to be blocked, against nothing that listens), no
//! real window gets input, and the window is parked off the screen like every scenario.
//!
//! It holds: the page lists the sample (name, version, off; its Details button opens the details sheet with a tile per
//! permission) and the broken folder with
//! its reason; the switch turns the mod on (its token and its tool appear, `mods.json` holds the switch and
//! no token); `flare-mcp`'s `tools/list` offers `mod_notes_append_note`; `tools/call` opens the mod's panel in
//! the caller's tab, the page gets the call over its bridge and the note it was given is in the panel; the page
//! runs in a profile of its own (`flare-mod-notes`, not private, never the web panel's), can reach no outside
//! address, holds a frozen `window.flare` without a token, is refused every permission the manifest did not ask
//! for (`browser.open`, `view.text`), and is served from its own address; turning the mod off removes its tool,
//! closes its panel and frees its browser, and a call to the tool is refused; and a restart keeps the switch,
//! mints a new token and keeps the page's own storage (the notes it kept); and a manifest edited on disk to ask
//! for `browser` turns the mod off at the next look (tool gone, panel closed, the page says why) until the
//! person switches it on again, which approves the new list. The sheet's Agent tool switch takes the tool from the
//! agent while off, keeps that in `mods.json`, and gives it back when on (2026-11-08).

use std::fs;
use std::path::Path;

use serde_json::{json, Value};

use super::ctl::{info, mcp, mcp_exe_of, run_mcp};
use super::driver::{goto_page, open_settings, row, settings_front};
use super::input_b::{probe, prompt};
use super::mini_app::{say, until_says};
use super::web::{script, state};
use crate::ctx::{App, Ctx, Launch};

/// How many tools flare-mcp lists before any mod's (`flare_mcp`'s own list; 21 since `browser_state`).
const BUILT_IN_TOOLS: usize = 21;

const SAMPLE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docs/surfaces/sample-mod/notes"
);

/// A second page of the sample's folder: its own policy lets an image of its own folder load.
const OTHER: &str = r#"<!doctype html><html><head><meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src 'self'; script-src 'unsafe-inline'">
<title>Other</title></head><body><script>
var i = new Image();
i.onload = function () { document.title = 'img ' + i.naturalWidth; };
i.onerror = function () { document.title = 'img error'; };
i.src = '/logo.png';
</script></body></html>"#;

fn mods_state(app: &mut App) -> Value {
    probe(app, "mods.state", Value::Null)
}

fn copy_sample(plugins: &Path) -> Result<(), String> {
    let to = plugins.join("notes");
    fs::create_dir_all(&to).map_err(|e| format!("make the sample folder: {e}"))?;
    for name in ["mod.json", "index.html"] {
        fs::copy(Path::new(SAMPLE).join(name), to.join(name))
            .map_err(|e| format!("copy the sample {name}: {e}"))?;
    }
    write_logo(&to.join("logo.png"))?;
    fs::write(to.join("other.html"), OTHER).map_err(|e| format!("write other.html: {e}"))?;
    // A folder that is not a mod: the page names it and says why.
    let bad = plugins.join("broken-mod");
    fs::create_dir_all(&bad).map_err(|e| format!("make the broken folder: {e}"))?;
    fs::write(bad.join("mod.json"), "{ not json").map_err(|e| e.to_string())?;
    Ok(())
}

/// A 4 by 4 picture: the page loads it from its own folder.
fn write_logo(path: &Path) -> Result<(), String> {
    let file = fs::File::create(path).map_err(|e| format!("make the logo: {e}"))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), 4, 4);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().map_err(|e| e.to_string())?;
    let px: Vec<u8> = (0..16).flat_map(|_| [30u8, 160, 90, 255]).collect();
    w.write_image_data(&px).map_err(|e| e.to_string())
}

fn launch(ctx: &mut Ctx) -> Result<App, String> {
    let alpha = ctx.ensure_world().alpha.clone();
    ctx.launch(Launch {
        renderer: Some("gpu"),
        folders: vec![alpha],
        ..Launch::default()
    })
}

/// Starts a script that stores its answer in `window.__r`, then reads it once it is there.
fn say_later(ctx: &mut Ctx, app: &mut App, start: &str) -> String {
    let _ = say(ctx, app, &format!("window.__r = null; {start}; 'started'"));
    until_says(
        ctx,
        "the page to answer",
        15_000,
        app,
        "window.__r === null ? '' : String(window.__r)",
        &|s| !s.is_empty(),
    )
    .1
}

/// The pointer on a Settings row: its centre.
fn click_row(app: &mut App, id: &str) -> bool {
    let r = row(app, id);
    let rect = &r["rect"];
    let (Some(x), Some(y), Some(w), Some(h)) = (
        rect["x"].as_f64(),
        rect["y"].as_f64(),
        rect["w"].as_f64(),
        rect["h"].as_f64(),
    ) else {
        return false;
    };
    // The middle of the row's text, away from the switch at its right edge: a press anywhere on a switch row flips it.
    app.session
        .click((x + w / 3.0) as f32, (y + h / 2.0) as f32)
        .is_ok()
}

fn sheet(app: &mut App) -> Value {
    probe(app, "plugin.sheet", Value::Null)
}

/// Presses a card's Details button: the second from the row's right end (16 in, the trash 32 wide, a gap of 10), so
/// 16 px in from its right edge.
fn click_info(app: &mut App, id: &str) -> bool {
    let r = row(app, id);
    let rect = &r["rect"];
    let (Some(x), Some(y), Some(w), Some(h)) = (
        rect["x"].as_f64(),
        rect["y"].as_f64(),
        rect["w"].as_f64(),
        rect["h"].as_f64(),
    ) else {
        return false;
    };
    app.session
        .click(
            (x + w - 16.0 - 32.0 - 10.0 - 16.0) as f32,
            (y + h / 2.0) as f32,
        )
        .is_ok()
}

/// Turns a mod on as the person does: its card's switch alone turns it on, with what its details sheet would list as
/// the approval, and no sheet opens (Max, 2026-11-12: "when you turn on plugins have it just be flipping the switch").
fn turn_on_by_switch(ctx: &mut Ctx, app: &mut App, id: &str, mod_id: &str) -> bool {
    if !click_row(app, id) {
        return false;
    }
    let on = ctx.until("the switch to turn the mod on", 10_000, &mut || {
        mods_state(app)["mods"]
            .as_array()
            .and_then(|m| m.iter().find(|m| m["id"] == json!(mod_id)))
            .is_some_and(|m| m["enabled"] == json!(true))
    });
    let no_sheet = sheet(app)["open"] == json!(false);
    ctx.ok(
        on && no_sheet,
        "the card's switch turns the mod on by itself, and no details sheet opens",
    );
    on && no_sheet
}

/// The middle of the switch of the open sheet's tile `i`.
fn toggle_of(s: &Value, i: usize) -> (f32, f32) {
    let bx = s["box"]["x"].as_f64().unwrap_or(0.0);
    let gy = s["grid_y"].as_f64().unwrap_or(0.0);
    let (cw, ch) = (
        s["cell"]["w"].as_f64().unwrap_or(0.0),
        s["cell"]["h"].as_f64().unwrap_or(0.0),
    );
    let x = bx + (i % 2) as f64 * (cw + 1.0) + cw - 14.0 - 18.0;
    let y = gy + 1.0 + (i / 2) as f64 * (ch + 1.0) + ch / 2.0;
    (x as f32, y as f32)
}

fn names_of(reply: &Value) -> Vec<String> {
    reply["result"]["tools"]
        .as_array()
        .map(|t| {
            t.iter()
                .filter_map(|x| x["name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn handshake() -> Vec<Value> {
    vec![
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "flare-e2e", "version": "1"}}}),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
    ]
}

fn append(id: u64, text: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": "mod_notes_append_note", "arguments": {"text": text}}})
}

pub fn mods_sample(ctx: &mut Ctx) -> Result<(), String> {
    let exe = mcp_exe_of(ctx);
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
    let profile = ctx.ensure_world().profile.clone();
    copy_sample(&profile.join("plugins"))?;
    let mut app = launch(ctx)?;
    let r = first_run(ctx, &mut app, &exe, &profile);
    let left = app.close();
    ctx.ok(left, "the app quits by itself");
    r?;
    // A restart: the switch is kept, the token is new, the page's own storage is kept.
    let mut app = launch(ctx)?;
    let r = second_run(ctx, &mut app, &exe, &profile);
    let left = app.close();
    ctx.ok(left, "the second run quits by itself");
    r
}

fn first_run(ctx: &mut Ctx, app: &mut App, exe: &Path, profile: &Path) -> Result<(), String> {
    prompt(app)?;
    let up = ctx.until("the first scan", 15_000, &mut || {
        mods_state(app)["scanned"] == json!(true)
    });
    ctx.ok(up, "the plugins folder was scanned at start");
    let s = mods_state(app);
    ctx.ok(
        s["mods"].as_array().is_some_and(|m| m.len() == 1)
            && s["mods"][0]["id"] == json!("notes")
            && s["mods"][0]["enabled"] == json!(false)
            && s["tools"] == json!([]),
        format!("one mod found, off, with no tools ({s})"),
    );
    ctx.ok(
        s["errors"][0]["folder"] == json!("broken-mod"),
        format!("the broken folder is reported by name ({})", s["errors"]),
    );

    // ---- Settings > Mods lists it, off, with its words ----
    let opened = open_settings(ctx, app) && goto_page(ctx, app, "mods");
    ctx.ok(opened, "Settings is open on the Mods page");
    let listed = ctx.until("the mod's rows", 10_000, &mut || {
        row(app, "mod:notes")["rect"].is_object()
    });
    ctx.ok(listed, "the page lists the mod");
    let sw = row(app, "mod:notes");
    ctx.ok(
        sw["label"] == json!("Notes 1.0.0") && sw["on"] == json!(false),
        format!("its name and version, and the switch is off ({sw})"),
    );
    // The card's Details button opens the details sheet: what it may do, a tile per permission, before it is on.
    ctx.ok(
        click_info(app, "mod:notes"),
        "the Details button is pressed",
    );
    let up = ctx.until("the details sheet", 5_000, &mut || {
        sheet(app)["open"] == json!(true)
    });
    let s = sheet(app);
    let tiles: Vec<(String, String)> = s["tiles"]
        .as_array()
        .map(|t| {
            t.iter()
                .map(|x| {
                    (
                        x["label"].as_str().unwrap_or_default().to_string(),
                        x["words"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    ctx.ok(
        up && s["name"] == json!("Notes")
            && tiles.iter().map(|t| t.0.as_str()).collect::<Vec<_>>() == ["Panel", "Agent tool"],
        format!("the sheet lists its permissions as tiles before it is turned on ({s})"),
    );
    let _ = app.session.key("Escape");
    let shut = ctx.until("the sheet to close", 3_000, &mut || {
        sheet(app)["open"] == json!(false)
    });
    ctx.ok(shut, "Escape closes the sheet");
    let bad = row(app, "modbad:0");
    ctx.ok(
        bad["label"] == json!("broken-mod")
            && bad["sub"].as_str().is_some_and(|s| s.contains("mod.json")),
        format!("the broken folder is a warning with its reason ({bad})"),
    );
    ctx.ok(
        row(app, "plugins-folder")["rect"].is_object(),
        "the Open plugins folder row is there",
    );

    // Off: the agent has no mod tool.
    let inf = info(app).ok_or("ctl.info answers")?;
    // The shell tab's secret (Settings is in front now, so no shell tab is the active one).
    let token = inf.tabs.first().map(|t| t.2.clone()).ok_or("a shell tab")?;
    let off = run_mcp(&mcp(exe, &inf, Some(&token), None), &handshake())?;
    ctx.ok(
        names_of(&off[1]).len() == BUILT_IN_TOOLS
            && !names_of(&off[1]).iter().any(|n| n.starts_with("mod_")),
        format!(
            "off: tools/list has only the built-in tools ({:?})",
            names_of(&off[1])
        ),
    );

    // ---- the person turns it on: the switch alone ----
    let turned = turn_on_by_switch(ctx, app, "mod:notes", "notes");
    ctx.ok(turned, "the switch is pressed");
    let on = ctx.until("the mod to be on", 10_000, &mut || {
        let s = mods_state(app);
        s["mods"][0]["enabled"] == json!(true)
            && s["tools"] == json!(["mod_notes_append_note"])
            && row(app, "mod:notes")["on"] == json!(true)
    });
    ctx.ok(
        on,
        "the mod is on: its tool is offered and the switch shows it",
    );
    let saved = ctx.until("mods.json", 10_000, &mut || {
        fs::read_to_string(profile.join("mods.json"))
            .is_ok_and(|t| t.contains("\"notes\":{\"on\":true"))
    });
    ctx.ok(saved, "the switch was saved");
    let file = fs::read_to_string(profile.join("mods.json")).unwrap_or_default();
    ctx.ok(
        !file.to_lowercase().contains("token"),
        "mods.json holds the switch and no token",
    );

    // Back to the shell tab, where the agent works.
    let _ = app.session.key("Ctrl+Tab");
    let left = ctx.until("the shell tab in front", 8_000, &mut || {
        !settings_front(app)
    });
    ctx.ok(left, "the shell tab is in front");

    // ---- the agent lists and calls the tool ----
    let list = run_mcp(&mcp(exe, &inf, Some(&token), None), &handshake())?;
    let names = names_of(&list[1]);
    ctx.ok(
        names.len() == BUILT_IN_TOOLS + 1
            && names.last().map(String::as_str) == Some("mod_notes_append_note"),
        format!("on: tools/list ends with the mod's tool ({names:?})"),
    );
    let tool = list[1]["result"]["tools"][BUILT_IN_TOOLS].clone();
    ctx.ok(
        tool["inputSchema"]["type"] == json!("object")
            && tool["description"]
                .as_str()
                .is_some_and(|d| d.contains("Notes panel")),
        format!("it carries the manifest's description and an object schema ({tool})"),
    );
    // The call opens the mod's panel in the tab and waits for the page.
    let call = run_mcp(
        &mcp(exe, &inf, Some(&token), None),
        &[
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
            append(2, "buy milk"),
        ],
    )?;
    ctx.ok(
        call[1]["result"]["isError"] == json!(false)
            && call[1]["result"]["content"][0]["text"] == json!("{\"count\":1}"),
        format!("the page answered the call ({})", call[1]),
    );
    let s = mods_state(app);
    ctx.ok(
        s["panels"][0]["mod"] == json!("notes")
            && s["bound"][0]["ready"] == json!(true)
            && s["panel_kind"] == json!(3)
            && s["panel_title"] == json!("Notes")
            && s["pending"] == json!([]),
        format!("the mod's panel is open in the tab, its page ready, no call left waiting ({s})"),
    );
    let note = say(ctx, app, "document.getElementById('list').textContent");
    ctx.ok(
        note == "buy milk",
        format!("the panel shows the text the agent sent (\"{note}\")"),
    );
    let _ = run_mcp(
        &mcp(exe, &inf, Some(&token), None),
        &[
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
            append(2, "and eggs"),
        ],
    )?;
    let two = say(ctx, app, "document.querySelectorAll('#list li').length + ':' + document.querySelectorAll('#list li')[1].textContent");
    ctx.ok(
        two == "2:and eggs",
        format!("a second call adds a second line ({two})"),
    );

    // ---- the page: its own site, its own profile, no network, a frozen bridge without a token ----
    let w = state(app);
    ctx.ok(
        w["controller"]["url"] == json!("https://flare-mod-notes.local/")
            && w["controller"]["profile"] == json!("flare-mod-notes")
            && w["controller"]["private"] == json!(false)
            && w["controller"]["autofill"] == json!(false),
        format!(
            "a view of the mod's own address and own profile ({})",
            w["controller"]
        ),
    );
    let origin = say(ctx, app, "location.origin");
    ctx.ok(
        origin == "https://flare-mod-notes.local",
        format!("the page's origin is the mod's own ({origin})"),
    );
    let keys = say(ctx, app, "Object.keys(window.flare).join(',') + '|' + Object.isFrozen(window.flare) + '|' + JSON.stringify(window.flare).length");
    ctx.ok(
        keys == "on,respond,call|true|2",
        format!("window.flare has on, respond and call, is frozen and holds no data ({keys})"),
    );
    let bridge_off = say(
        ctx,
        app,
        "try { window.flare = 1; String(typeof window.flare) } catch (e) { 'threw' }",
    );
    ctx.ok(
        bridge_off == "object",
        format!("the page cannot replace window.flare ({bridge_off})"),
    );
    let net = say_later(
        ctx,
        app,
        "fetch('https://example.invalid/x').then(function () { window.__r = 'loaded' }, function () { window.__r = 'blocked' })",
    );
    ctx.ok(
        net == "blocked",
        format!("a request to an outside address is blocked ({net})"),
    );
    // Navigation inside the mod's own address is allowed and is served file by file: a second page of the
    // folder loads an image of its own (its own policy lets `img-src 'self'` through).
    let _ = say(ctx, app, "location.href = '/other.html'; 'going'");
    let (moved, title) = until_says(
        ctx,
        "the second page of the mod to load",
        15_000,
        app,
        "document.title",
        &|t| t.starts_with("img "),
    );
    ctx.ok(
        moved && title == "img 4",
        format!("a second page of the mod's folder loaded its image from the folder ({title})"),
    );
    let here = state(app);
    ctx.ok(
        here["controller"]["url"] == json!("https://flare-mod-notes.local/other.html"),
        format!(
            "the view is at the mod's own address ({})",
            here["controller"]["url"]
        ),
    );
    let still_flare = say(
        ctx,
        app,
        "typeof window.flare + ':' + Object.isFrozen(window.flare)",
    );
    ctx.ok(
        still_flare == "object:true",
        format!("the bridge is in the second page too ({still_flare})"),
    );
    let gone = say_later(
        ctx,
        app,
        "var i = new Image(); i.onload = function () { window.__r = 'loaded' }; i.onerror = function () { window.__r = 'error' }; i.src = '/missing.png'",
    );
    ctx.ok(
        gone == "error",
        format!("a file that is not there is a miss ({gone})"),
    );
    // Back to the page the agent's calls need.
    let _ = say(ctx, app, "location.href = '/'; 'back'");
    let (home, _) = until_says(
        ctx,
        "the first page again",
        15_000,
        app,
        "document.title",
        &|t| t == "Notes",
    );
    ctx.ok(home, "the mod's own page again");
    let own_fetch = say_later(
        ctx,
        app,
        "fetch('/mod.json').then(function () { window.__r = 'loaded' }, function () { window.__r = 'blocked' })",
    );
    ctx.ok(
        own_fetch == "blocked",
        format!("a fetch, even of its own address, is blocked by the policy ({own_fetch})"),
    );
    for (method, params, want) in [
        ("browser.open", "{url: 'https://example.com/'}", "forbidden"),
        ("view.text", "{text: 'x'}", "forbidden"),
        (
            "panel.open",
            "{kind: 'web', url: 'https://example.com/'}",
            "forbidden",
        ),
        ("panel.open", "{kind: 'mod:other'}", "forbidden"),
        ("convo.state", "{}", "forbidden"),
        ("settings.set", "{}", "forbidden"),
        ("nope.nothing", "{}", "unknown_method"),
    ] {
        let got = say_later(
            ctx,
            app,
            &format!(
                "window.flare.call('{method}', {params}).then(function () {{ window.__r = 'ok' }}, function (e) {{ window.__r = e.code }})"
            ),
        );
        ctx.ok(
            got == want,
            format!("{method} from the page: {got} (wanted {want})"),
        );
    }
    let tabs = say_later(
        ctx,
        app,
        "window.flare.call('tabs.list', {}).then(function (r) { window.__r = 'tabs:' + r.tabs.length }, function (e) { window.__r = e.code })",
    );
    ctx.ok(
        tabs == "tabs:1",
        format!("tabs.list is open to a mod with panel ({tabs})"),
    );

    // ---- the person turns it off ----
    let again_open = open_settings(ctx, app) && goto_page(ctx, app, "mods");
    ctx.ok(again_open, "Settings is open on the Mods page again");
    ctx.ok(
        click_row(app, "mod:notes"),
        "the switch row is pressed again",
    );
    let gone = ctx.until("the mod to be off", 10_000, &mut || {
        let s = mods_state(app);
        s["mods"][0]["enabled"] == json!(false)
            && s["tools"] == json!([])
            && s["panels"] == json!([])
            && s["bound"] == json!([])
            && row(app, "mod:notes")["on"] == json!(false)
    });
    ctx.ok(
        gone,
        "off: the tool is gone, the panel is closed, the page is unbound",
    );
    let freed = ctx.until("the browser view to be freed", 10_000, &mut || {
        state(app)["controllers"] == json!(0)
    });
    ctx.ok(freed, "the mod's browser view was freed");
    let off_list = run_mcp(&mcp(exe, &inf, Some(&token), None), &handshake())?;
    ctx.ok(
        names_of(&off_list[1]).len() == BUILT_IN_TOOLS,
        format!(
            "off again: tools/list is the built-in tools ({:?})",
            names_of(&off_list[1])
        ),
    );
    let refused = run_mcp(
        &mcp(exe, &inf, Some(&token), None),
        &[
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
            append(2, "too late"),
        ],
    )?;
    let text = refused[1]["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default();
    ctx.ok(
        refused[1]["result"]["isError"] == json!(true) && text.contains("no mod offers a tool"),
        format!("a call to the tool is refused ({text})"),
    );
    // Turned on again for the restart check.
    let turned = turn_on_by_switch(ctx, app, "mod:notes", "notes");
    ctx.ok(turned, "the switch is pressed a third time");
    let again = ctx.until("the mod to be on again", 10_000, &mut || {
        mods_state(app)["tools"] == json!(["mod_notes_append_note"])
    });
    ctx.ok(again, "on again");
    let saved = ctx.until("mods.json to say on", 10_000, &mut || {
        fs::read_to_string(profile.join("mods.json"))
            .is_ok_and(|t| t.contains("\"notes\":{\"on\":true"))
    });
    ctx.ok(saved, "the last switch was saved");
    // The sheet's Agent tool switch (the same for a protocol 1 mod): off takes the tool from the agent and is kept
    // with the switch; on gives it back.
    ctx.ok(
        click_info(app, "mod:notes"),
        "the Details button is pressed again",
    );
    let up = ctx.until("the details sheet", 5_000, &mut || {
        sheet(app)["open"] == json!(true)
    });
    ctx.ok(up, "the sheet is open");
    let tool_switch = toggle_of(&sheet(app), 1);
    let _ = app.session.click(tool_switch.0, tool_switch.1);
    let held = ctx.until("the tool to leave the agent", 10_000, &mut || {
        mods_state(app)["tools"] == json!([])
    });
    let kept = ctx.until("mods.json to hold it back", 10_000, &mut || {
        fs::read_to_string(profile.join("mods.json"))
            .is_ok_and(|t| t.contains("\"withheld\":[\"tools\"]"))
    });
    ctx.ok(
        held && kept,
        "the Agent tool switch off takes the tool from the agent and mods.json keeps it",
    );
    let _ = app.session.click(tool_switch.0, tool_switch.1);
    let back = ctx.until("the tool to come back", 10_000, &mut || {
        mods_state(app)["tools"] == json!(["mod_notes_append_note"])
    });
    ctx.ok(back, "on again, the agent has the tool back");
    Ok(())
}

fn second_run(ctx: &mut Ctx, app: &mut App, exe: &Path, profile: &Path) -> Result<(), String> {
    prompt(app)?;
    let up = ctx.until("the scan", 15_000, &mut || {
        let s = mods_state(app);
        s["scanned"] == json!(true) && s["tools"] == json!(["mod_notes_append_note"])
    });
    ctx.ok(
        up,
        "a restart keeps the switch: the mod is on and its tool is offered",
    );
    let inf = info(app).ok_or("ctl.info answers")?;
    // The shell tab's secret (Settings is in front now, so no shell tab is the active one).
    let token = inf
        .tabs
        .iter()
        .find(|t| t.1)
        .map(|t| t.2.clone())
        .ok_or("a tab in front")?;
    let call = run_mcp(
        &mcp(exe, &inf, Some(&token), None),
        &[
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
            append(2, "after the restart"),
        ],
    )?;
    ctx.ok(
        call[1]["result"]["content"][0]["text"] == json!("{\"count\":3}"),
        format!(
            "the page kept its own notes across the restart ({})",
            call[1]
        ),
    );
    let raw = script(
        ctx,
        app,
        "document.querySelectorAll('#list li').length + ':' + document.querySelectorAll('#list li')[2].textContent",
    );
    ctx.ok(
        raw.as_deref() == Some("\"3:after the restart\""),
        format!("the panel shows all three lines ({raw:?})"),
    );

    // ---- the manifest on disk asks for more than the person allowed ----
    let manifest = profile.join("plugins").join("notes").join("mod.json");
    let text = fs::read_to_string(&manifest).map_err(|e| format!("read the manifest: {e}"))?;
    let edited = text.replacen(
        r#""permissions": ["panel", "tools"]"#,
        r#""permissions": ["panel", "tools", "browser"]"#,
        1,
    );
    ctx.ok(
        edited != text,
        "the sample's manifest was edited to ask for browser",
    );
    fs::write(&manifest, edited).map_err(|e| format!("write the manifest: {e}"))?;
    let open = open_settings(ctx, app) && goto_page(ctx, app, "mods");
    ctx.ok(
        open,
        "Settings is open on the Mods page (it looks at the folder again)",
    );
    let off = ctx.until("the mod to be turned off by the scan", 15_000, &mut || {
        let s = mods_state(app);
        s["mods"][0]["enabled"] == json!(false)
            && s["mods"][0]["needs_approval"] == json!(true)
            && s["tools"] == json!([])
            && s["panels"] == json!([])
    });
    ctx.ok(
        off,
        "asking for a new permission turns the mod off: its tool is gone and its panel closed",
    );
    let say = row(app, "mod:notes");
    ctx.ok(
        say["on"] == json!(false)
            && say["sub"]
                .as_str()
                .is_some_and(|t| t.contains("Asks for new permissions")),
        format!("the Mods page says it needs to be switched on again ({say})"),
    );
    // Switching it on approves the new list.
    let turned = turn_on_by_switch(ctx, app, "mod:notes", "notes");
    ctx.ok(turned, "the switch is pressed to allow the new list");
    let back = ctx.until("the mod to be on with the new list", 10_000, &mut || {
        let s = mods_state(app);
        s["mods"][0]["enabled"] == json!(true)
            && s["mods"][0]["needs_approval"] == json!(false)
            && s["tools"] == json!(["mod_notes_append_note"])
    });
    ctx.ok(
        back,
        "switched on again, the mod is back with the new list approved",
    );
    let saved = ctx.until("mods.json to hold the new list", 10_000, &mut || {
        fs::read_to_string(profile.join("mods.json")).is_ok_and(|t| t.contains("\"browser\""))
    });
    ctx.ok(saved, "mods.json holds the approved list, browser included");
    Ok(())
}

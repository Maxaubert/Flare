//! A plugin's brand, its details sheet, its questions and its tab-menu item (2026-11-08): `pluginBrand`.
//!
//! The scenario puts a test plugin with a brand (a blue and a generic logo of its own, no real product's) in the
//! world's plugins folder, its helper the e2e exe in its fake-helper mode (`crate::fake_fill_helper`, permissions
//! `process`, `tools`, `actions`, `fill`, one tab-menu action), and seeds `mods.json` with it on. It runs offscreen
//! like every scenario, with the brand's colour and logo at 100, 150 and 225 percent, then dressed three other ways at
//! 100 percent: the colour alone (the chip on the brand surface), no brand (Flare's theme and the chip), and both of
//! those in the light theme. It saves a picture of each surface into `<shots>/plugin-brand/`: the card, the details
//! sheet (closed and with the Fills logins dropdown open), the unlock question, the fill approval and the tab menu.
//! It holds: the one action is a flat tab-menu row titled by the plugin, wearing the brand's logo or Flare's chip and
//! the plugin's name as its hint; `secret.ask` shows the plugin's heading, sub line, button and Flare's line naming it,
//! the eye shows and hides, Enter submits and the helper gets only that a secret came; the fill approval names the
//! login, the site and the tab the agent asked in (in quotes), and the first press on Deny answers it; every surface
//! wears the brand exactly when the manifest has one, and the logo only with a logo; the card's info button opens the
//! sheet with a tile per permission and the facts, a dropdown opens on its tile; turning the agent tool off takes the
//! tool from the agent and keeps the choice in `mods.json`, on gives it back; "Always allow" fills with no question
//! and the sheet's warning says so; and no secret, value or choice is in a log.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use flare_ctl::Method;
use serde_json::{json, Value};

use super::driver::{goto_page, open_settings, row};
use super::input_b::{probe, prompt};
use super::web::{channel_of, hello, launch_with, HOST, LOAD};
use crate::ctx::{App, Ctx, Launch};
use crate::fake_fill_helper::{ACTION, ACTION_TITLE, CODE, PASS, TOOL, USER};
use crate::tour::Tabs;

const ID: &str = "vaulttest";
const NAME: &str = "Vault test";
/// A generic logo (a rounded square with a shield, half of it filled): the test plugin's own, no real product's.
const LOGO: &str = "M4 1h16a3 3 0 0 1 3 3v16a3 3 0 0 1-3 3H4a3 3 0 0 1-3-3V4a3 3 0 0 1 3-3zM6 4.5v7.2c0 3.6 3 6 6 7.8 3-1.8 6-4.2 6-7.8V4.5zM12 6v12.2c2.4-1.5 4.5-3.5 4.5-6.5V6z";
/// What the person types into the secret field: a fake, checked to reach no file.
const TYPED: &str = "e2e-typed-secret-41";

const LOGIN: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>Sign in</title></head>
<body style="font:16px sans-serif"><form onsubmit="return false">
<label>Email <input id="u" autocomplete="username"></label>
<label>Password <input id="p" type="password" autocomplete="current-password"></label>
<label>Code <input id="o" autocomplete="one-time-code"></label></form></body></html>"#;

/// How the test plugin is dressed: its brand's colour and logo, the colour alone, or no brand at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dress {
    Logo,
    Colour,
    Plain,
}

impl Dress {
    fn brand(self) -> Value {
        match self {
            Dress::Logo => json!({"color": "#175ddc", "icon": LOGO}),
            Dress::Colour => json!({"color": "#175ddc"}),
            Dress::Plain => Value::Null,
        }
    }
    fn branded(self) -> bool {
        self != Dress::Plain
    }
}

fn manifest(record: &Path, dress: Dress) -> String {
    let mut m = json!({
        "id": ID,
        "name": NAME,
        "version": "0.2.0",
        "protocol": 2,
        "description": "Signs the agent in with your saved logins.",
        "author": "flare-e2e",
        "homepage": "https://example.com/vaulttest",
        "permissions": ["process", "tools", "actions", "fill"],
        "helper": {"command": "bin\\helper.exe", "args": ["--fake-fill-helper", record.to_string_lossy()]},
        "contributes": {"actions": [{"key": ACTION, "title": "Vault", "where": ["tab-menu"]}]},
        "tools": [{"name": TOOL, "description": "Fills the saved login into the page.", "input_schema": {"type": "object"}}]
    });
    if dress != Dress::Plain {
        m["brand"] = dress.brand();
    }
    m.to_string()
}

fn install(profile: &Path, record: &Path, dress: Dress) -> Result<(), String> {
    let folder = profile.join("plugins").join(ID);
    fs::create_dir_all(folder.join("bin")).map_err(|e| format!("make the plugin folder: {e}"))?;
    let me = std::env::current_exe().map_err(|e| format!("this exe: {e}"))?;
    fs::copy(&me, folder.join("bin").join("helper.exe"))
        .map_err(|e| format!("copy the fake helper: {e}"))?;
    fs::write(folder.join("plugin.json"), manifest(record, dress))
        .map_err(|e| format!("write plugin.json: {e}"))?;
    let switches = json!({"version": 1, "mods": {ID: {
        "on": true,
        "permissions": ["process", "tools", "actions", "fill"],
        "author": "flare-e2e",
        "homepage": "https://example.com/vaulttest",
    }}});
    fs::write(profile.join("mods.json"), switches.to_string())
        .map_err(|e| format!("write mods.json: {e}"))
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

fn sheet(app: &mut App) -> Value {
    probe(app, "plugin.sheet", Value::Null)
}

fn tools_now(app: &mut App) -> Vec<String> {
    probe(app, "mods.state", Value::Null)["tools"]
        .as_array()
        .map(|t| {
            t.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The agent calls the plugin's tool from its tab on a thread of its own (the call may wait for the person).
fn agent_tool(pipe: &str, token: &str, case: &str) -> std::thread::JoinHandle<Value> {
    let (pipe, token, case) = (pipe.to_string(), token.to_string(), case.to_string());
    std::thread::spawn(move || {
        let mut c = match hello(&pipe, &token) {
            Ok(c) => c,
            Err(e) => return json!({"error": format!("hello: {e}")}),
        };
        match c.call(
            Method::ModsCall,
            json!({"name": format!("mod_{ID}_{TOOL}"), "input": {"case": case}}),
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

fn centre(r: &Value) -> Option<(f32, f32)> {
    let (x, y, w, h) = (
        r["x"].as_f64()?,
        r["y"].as_f64()?,
        r["w"].as_f64()?,
        r["h"].as_f64()?,
    );
    (w > 0.0 && h > 0.0).then_some(((x + w / 2.0) as f32, (y + h / 2.0) as f32))
}

fn press(app: &mut App, q: &Value, button: &str) -> Result<(), String> {
    let (x, y) = centre(&q["buttons"][button]).ok_or(format!("no {button} button ({q})"))?;
    app.session.click(x, y)
}

fn shot(ctx: &mut Ctx, app: &mut App, dir: &Path, name: &str) {
    std::thread::sleep(Duration::from_millis(250)); // sync-ok: e2e driver, the frame settles
    let path = dir.join(format!("{name}.png"));
    let taken = app.session.snapshot(&path);
    ctx.ok(
        taken.is_ok(),
        format!("a picture of {name} ({})", path.display()),
    );
}

/// The middle of tile `i` of the open sheet, and the middle of its toggle.
fn tile_points(s: &Value, i: usize) -> ((f32, f32), (f32, f32)) {
    let bx = s["box"]["x"].as_f64().unwrap_or(0.0);
    let gy = s["grid_y"].as_f64().unwrap_or(0.0);
    let (cw, ch) = (
        s["cell"]["w"].as_f64().unwrap_or(0.0),
        s["cell"]["h"].as_f64().unwrap_or(0.0),
    );
    let (col, rw) = ((i % 2) as f64, (i / 2) as f64);
    let x = bx + col * (cw + 1.0);
    let y = gy + 1.0 + rw * (ch + 1.0);
    (
        ((x + cw / 2.0) as f32, (y + ch / 2.0) as f32),
        ((x + cw - 14.0 - 18.0) as f32, (y + ch / 2.0) as f32),
    )
}

/// The middle of option `j` of the open dropdown on tile `i` (it hangs `menu_drop` under its tile's top).
fn option_point(s: &Value, i: usize, j: usize) -> (f32, f32) {
    let bx = s["box"]["x"].as_f64().unwrap_or(0.0);
    let gy = s["grid_y"].as_f64().unwrap_or(0.0);
    let (cw, ch) = (
        s["cell"]["w"].as_f64().unwrap_or(0.0),
        s["cell"]["h"].as_f64().unwrap_or(0.0),
    );
    let x = bx + (i % 2) as f64 * (cw + 1.0) + 10.0 + 90.0;
    let drop = s["menu_drop"].as_f64().unwrap_or(0.0);
    let y = gy + 1.0 + (i / 2) as f64 * (ch + 1.0) + drop + 4.0 + 30.0 * j as f64 + 15.0;
    (x as f32, y as f32)
}

fn tile_of<'a>(s: &'a Value, key: &str) -> &'a Value {
    s["tiles"]
        .as_array()
        .and_then(|t| t.iter().find(|t| t["key"] == json!(key)))
        .unwrap_or(&Value::Null)
}

/// No value of the login, no typed secret and no word of the person's choices is in Flare's logs, the plugins' logs
/// or the helper's record.
fn nothing_leaks(ctx: &mut Ctx, profile: &Path, record: &Path) {
    let mut files = vec![record.to_path_buf()];
    for dir in ["logs", "plugins-logs"] {
        if let Ok(rd) = fs::read_dir(profile.join(dir)) {
            files.extend(rd.flatten().map(|e| e.path()).filter(|p| p.is_file()));
        }
    }
    let mut leaks = Vec::new();
    for f in &files {
        let Ok(bytes) = fs::read(f) else { continue };
        let text = String::from_utf8_lossy(&bytes);
        for v in [PASS, USER, CODE, TYPED, "withheld", "Always allow"] {
            if text.contains(v) {
                leaks.push(format!("{v} in {}", f.display()));
            }
        }
    }
    ctx.ok(
        leaks.is_empty(),
        format!(
            "no secret, value or choice is in a log or the helper's record ({} files; {leaks:?})",
            files.len()
        ),
    );
}

pub fn plugin_brand(ctx: &mut Ctx) -> Result<(), String> {
    let (profile, root) = {
        let w = ctx.ensure_world();
        (w.profile.clone(), w.root.clone())
    };
    let record = root.join("brand-helper.jsonl");
    let web = root.join("web");
    fs::create_dir_all(&web).map_err(|e| format!("make the web folder: {e}"))?;
    fs::write(web.join("login.html"), LOGIN).map_err(|e| format!("write login.html: {e}"))?;
    let dir: PathBuf = ctx.shots.join("plugin-brand");
    fs::create_dir_all(&dir).map_err(|e| format!("make the shots folder: {e}"))?;
    // (dpi, the dress, the light theme, whether the choices are walked through)
    let runs = [
        (96, Dress::Logo, false, true),
        (144, Dress::Logo, false, false),
        (216, Dress::Logo, false, false),
        (96, Dress::Colour, false, false),
        (96, Dress::Plain, false, false),
        (96, Dress::Plain, true, false),
        (96, Dress::Colour, true, false),
    ];
    for (dpi, dress, light, choices) in runs {
        install(&profile, &record, dress)?;
        let pct = dpi * 100 / 96;
        let tag = match (dress, light) {
            (Dress::Logo, _) => format!("{pct}"),
            (Dress::Colour, false) => format!("{pct}-colour"),
            (Dress::Plain, false) => format!("{pct}-plain"),
            (Dress::Colour, true) => format!("{pct}-colour-light"),
            (Dress::Plain, true) => format!("{pct}-plain-light"),
        };
        let mut app = launch(ctx, &web, dpi, light)?;
        let r = run(ctx, &mut app, &profile, &record, &dir, &tag, dress, choices);
        let left = app.close();
        ctx.ok(left, format!("{tag}: the app quits by itself"));
        r?;
    }
    nothing_leaks(ctx, &profile, &record);
    Ok(())
}

/// `launch_with` with the light theme (the `paper` preset) when asked.
fn launch(ctx: &mut Ctx, web: &Path, dpi: u32, light: bool) -> Result<App, String> {
    if !light {
        return launch_with(ctx, web, dpi, &[]);
    }
    let alpha = ctx.ensure_world().alpha.clone();
    ctx.launch(Launch {
        renderer: Some("gpu"),
        folders: vec![alpha],
        scale_dpi: Some(dpi),
        env: vec![(
            "FLARE_E2E_WEB_ROOT".to_string(),
            web.to_string_lossy().into_owned(),
        )],
        args: vec!["--theme".into(), "paper".into()],
        ..Launch::default()
    })
}

#[allow(clippy::too_many_arguments)]
fn run(
    ctx: &mut Ctx,
    app: &mut App,
    profile: &Path,
    record: &Path,
    dir: &Path,
    pct: &str,
    dress: Dress,
    choices: bool,
) -> Result<(), String> {
    prompt(app)?;
    let tool = format!("mod_{ID}_{TOOL}");
    let on = ctx.until("the plugin to be on with its tool", 20_000, &mut || {
        tools_now(app).contains(&tool)
    });
    ctx.ok(on, format!("{pct}: the seeded switch turns the plugin on"));
    let titled = ctx.until("the helper to title its action", 30_000, &mut || {
        records(record)
            .iter()
            .any(|r| r.get("actions.set").is_some())
    });
    ctx.ok(titled, format!("{pct}: the helper titled its action"));

    // ---- the tab menu: the one action is a flat row with the plugin's title ----
    let tabs = Tabs::read(app);
    if let Some((x, y)) = tabs.as_ref().and_then(|t| t.centre(0)) {
        let _ = app.session.right_click(x.floor(), y.floor());
        let open = ctx.until("the tab menu", 4_000, &mut || {
            probe(app, "app.state", Value::Null)["menu"]["open"] == json!(true)
        });
        let rows = probe(app, "app.state", Value::Null)["menu"]["rows"].clone();
        let labels: Vec<String> = rows
            .as_array()
            .map(|r| {
                r.iter()
                    .filter_map(|x| x["label"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        ctx.ok(
            open && labels.iter().any(|l| l == ACTION_TITLE) && !labels.iter().any(|l| l == NAME),
            format!(
                "{pct}: the plugin's one action is a flat row titled by it, no flyout ({labels:?})"
            ),
        );
        // Its logo with a logo, else Flare's chip (never the action's own icon), and the plugin's name as its hint.
        let item = rows
            .as_array()
            .and_then(|r| r.iter().find(|x| x["label"] == json!(ACTION_TITLE)))
            .cloned()
            .unwrap_or(Value::Null);
        let chip_or_logo = if dress == Dress::Logo {
            item["logo"] == json!(true)
        } else {
            item["logo"] == json!(false) && item["icon"].as_str().is_some_and(|i| !i.is_empty())
        };
        ctx.ok(
            chip_or_logo && item["hint"] == json!(NAME),
            format!("{pct}: the row wears the logo or the chip, and the plugin's name ({item})"),
        );
        shot(ctx, app, dir, &format!("tab-menu-{pct}"));
        let _ = app.session.key("Escape");
    } else {
        ctx.ok(false, format!("{pct}: the first tab has a rectangle"));
    }

    // ---- a page in the tab, for the fill ----
    let (pipe, _tab, token) = channel_of(app)?;
    let mut agent = hello(&pipe, &token).map_err(|e| format!("hello: {e}"))?;
    let opened = agent.call(
        Method::BrowserOpen,
        json!({"url": format!("{HOST}/login.html")}),
        LOAD,
    );
    ctx.ok(
        opened.as_ref().is_ok_and(|v| v["loaded"] == json!(true)),
        format!("{pct}: the sign-in page loads ({opened:?})"),
    );

    // ---- the unlock question: heading, sub, button, the eye, Enter ----
    let call = agent_tool(&pipe, &token, "secret");
    let asked = ctx.until("the secret question", 20_000, &mut || {
        ask_state(app)["open"] == json!(true)
    });
    // The field takes the keys as the dialog opens (Slint runs the `changed open` handler a moment later).
    ctx.until("the field to take the keys", 3_000, &mut || {
        ask_state(app)["field_focus"] == json!(true)
    });
    let q = ask_state(app);
    ctx.ok(
        asked
            && q["kind"] == json!("secret")
            && q["title"] == json!("Your vault is locked")
            && q["sub"] == json!("e2e@flare-test.local")
            && q["label"] == json!("Master password")
            && q["button"] == json!("Unlock")
            && q["branded"] == json!(dress.branded())
            && q["plate"] == json!(dress == Dress::Logo)
            && q["field_focus"] == json!(true)
            && q["foot"].as_str().is_some_and(|f| f.contains(NAME)),
        format!("{pct}: the unlock question is the plugin's, centred, with Flare's line naming it ({q})"),
    );
    shot(ctx, app, dir, &format!("unlock-{pct}"));
    if let Some((x, y)) = centre(&q["buttons"]["eye"]) {
        let _ = app.session.click(x, y);
        let shown = ctx.until("the eye to show", 3_000, &mut || {
            ask_state(app)["revealed"] == json!(true)
        });
        let _ = app.session.click(x, y);
        let hidden = ctx.until("the eye to hide", 3_000, &mut || {
            ask_state(app)["revealed"] == json!(false)
        });
        ctx.ok(
            shown && hidden,
            format!("{pct}: the eye shows and hides what is typed"),
        );
    }
    app.session.type_text(TYPED)?;
    app.session.key("Enter")?;
    let answer = call.join().unwrap_or(Value::Null);
    ctx.ok(
        answer["result"]["outcome"] == json!({"answered": true, "length": TYPED.chars().count()}),
        format!("{pct}: Enter submits, and the plugin gets what was typed ({answer})"),
    );

    // ---- the fill approval: the login, the site, the tab the agent asked in ----
    let call = agent_tool(&pipe, &token, "match");
    let asked = ctx.until("the fill question", 20_000, &mut || {
        ask_state(app)["open"] == json!(true)
    });
    let q = ask_state(app);
    ctx.ok(
        asked
            && q["kind"] == json!("fill")
            && q["title"] == json!("Fill E2E test login?")
            && q["site"] == json!("flare-test.local")
            && q["asker"]
                .as_str()
                .is_some_and(|a| a.starts_with("asked by the agent in the tab \""))
            && q["branded"] == json!(dress.branded()),
        format!("{pct}: the fill approval names the login, the site and the agent's tab ({q})"),
    );
    shot(ctx, app, dir, &format!("fill-{pct}"));
    // ONE press: a press the dialog does not take is a failure, not something to press again.
    let q = ask_state(app);
    press(app, &q, "deny")?;
    let gone = ctx.until("the first Deny press to answer", 3_000, &mut || {
        ask_state(app)["open"] == json!(false)
    });
    ctx.ok(
        gone,
        format!(
            "{pct}: the first press on Deny answers the question ({})",
            ask_state(app)
        ),
    );
    let answer = call.join().unwrap_or(Value::Null);
    ctx.ok(
        answer["result"]["outcome"] == json!({"declined": true}),
        format!("{pct}: Deny says no ({answer})"),
    );

    // ---- the card and the details sheet ----
    let opened = open_settings(ctx, app) && goto_page(ctx, app, "mods");
    ctx.ok(
        opened,
        format!("{pct}: Settings is open on the Plugins page"),
    );
    let card = format!("mod:{ID}");
    let listed = ctx.until("the plugin's card", 10_000, &mut || {
        row(app, &card)["rect"].is_object()
    });
    ctx.ok(listed, format!("{pct}: the card is listed"));
    shot(ctx, app, dir, &format!("card-{pct}"));
    let r = row(app, &card);
    let (x, y, w, h) = (
        r["rect"]["x"].as_f64().unwrap_or(0.0),
        r["rect"]["y"].as_f64().unwrap_or(0.0),
        r["rect"]["w"].as_f64().unwrap_or(0.0),
        r["rect"]["h"].as_f64().unwrap_or(0.0),
    );
    // The info button is the second from the right end: 16 in, the trash (32) and a gap of 10, then its middle.
    let _ = app.session.click(
        (x + w - 16.0 - 32.0 - 10.0 - 16.0) as f32,
        (y + h / 2.0) as f32,
    );
    let up = ctx.until("the details sheet", 5_000, &mut || {
        sheet(app)["open"] == json!(true)
    });
    let s = sheet(app);
    let keys: Vec<String> = s["tiles"]
        .as_array()
        .map(|t| {
            t.iter()
                .filter_map(|x| x["key"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    ctx.ok(
        up && s["name"] == json!(NAME)
            && s["sub"] == json!("0.2.0 \u{b7} flare-e2e")
            && s["branded"] == json!(dress.branded())
            && s["plate"] == json!(dress == Dress::Logo)
            && s["can_turn_on"] == json!(false)
            && keys
                == [
                    "process",
                    "fill",
                    "tools",
                    "actions",
                    "fact:program",
                    "fact:homepage",
                ]
            && tile_of(&s, "process")["value_name"] == json!("Full control")
            && tile_of(&s, "process")["tone"] == json!(2)
            && tile_of(&s, "fill")["value_name"] == json!("Ask each time")
            && tile_of(&s, "fill")["tone"] == json!(1)
            && tile_of(&s, "actions")["label"] == json!("Tab menu"),
        format!(
            "{pct}: the info button opens the sheet, a tile per permission and the facts ({s})"
        ),
    );
    shot(ctx, app, dir, &format!("sheet-{pct}"));
    let (fill_mid, _) = tile_points(&s, 1);
    let _ = app.session.click(fill_mid.0, fill_mid.1);
    let dropped = ctx.until("the Fills logins dropdown", 3_000, &mut || {
        sheet(app)["menu_tile"] == json!(1)
    });
    ctx.ok(
        dropped,
        format!("{pct}: a press on the Fills logins tile opens its dropdown"),
    );
    shot(ctx, app, dir, &format!("sheet-dropdown-{pct}"));

    if choices {
        // "Always allow": picked in the dropdown, kept in mods.json, and a fill goes in with no question.
        let (ox, oy) = option_point(&s, 1, 1);
        let _ = app.session.click(ox, oy);
        let always = ctx.until("Always allow", 5_000, &mut || {
            tile_of(&sheet(app), "fill")["value"] == json!("always")
        });
        let kept = ctx.until("mods.json to keep it", 5_000, &mut || {
            fs::read_to_string(profile.join("mods.json"))
                .is_ok_and(|t| t.contains("\"fill\":\"always\""))
        });
        let warned = sheet(app)["warnings"].as_str().is_some_and(|w| {
            w.contains("Fills logins with no question") && !w.contains("after you approve")
        });
        ctx.ok(
            always && kept && tile_of(&sheet(app), "fill")["tone"] == json!(2) && warned,
            format!("{pct}: Always allow is chosen, amber, kept with the switch, and the warning says so"),
        );
        let call = agent_tool(&pipe, &token, "match");
        let answer = call.join().unwrap_or(Value::Null);
        ctx.ok(
            answer["result"]["outcome"]["filled"]
                .as_array()
                .is_some_and(|f| f.len() == 3)
                && ask_state(app)["asking"] == json!(false),
            format!("{pct}: with Always allow the fill goes in with no question ({answer})"),
        );
        let s = sheet(app);
        let (fill_mid, _) = tile_points(&s, 1);
        let _ = app.session.click(fill_mid.0, fill_mid.1);
        ctx.until("the dropdown again", 3_000, &mut || {
            sheet(app)["menu_tile"] == json!(1)
        });
        let (ox, oy) = option_point(&s, 1, 0);
        let _ = app.session.click(ox, oy);
        let back = ctx.until("Ask each time again", 5_000, &mut || {
            tile_of(&sheet(app), "fill")["value"] == json!("ask")
        });
        ctx.ok(back, format!("{pct}: Ask each time again"));

        // The agent tool off: the agent loses the tool, the choice is kept; on gives it back.
        let s = sheet(app);
        let (_, tool_switch) = tile_points(&s, 2);
        let _ = app.session.click(tool_switch.0, tool_switch.1);
        let gone = ctx.until("the tool to leave the agent", 10_000, &mut || {
            !tools_now(app).contains(&tool)
        });
        let kept = ctx.until("mods.json to hold it back", 5_000, &mut || {
            fs::read_to_string(profile.join("mods.json"))
                .is_ok_and(|t| t.contains("\"withheld\":[\"tools\"]"))
        });
        ctx.ok(
            gone && kept && tile_of(&sheet(app), "tools")["on"] == json!(false),
            format!("{pct}: the Agent tool switch off takes the tool from the agent and is kept"),
        );
        let s = sheet(app);
        let (_, tool_switch) = tile_points(&s, 2);
        let _ = app.session.click(tool_switch.0, tool_switch.1);
        let back = ctx.until("the tool to come back", 20_000, &mut || {
            tools_now(app).contains(&tool)
        });
        ctx.ok(
            back,
            format!("{pct}: on again, the agent has the tool back"),
        );
        let clean = ctx.until("mods.json to hold nothing back", 10_000, &mut || {
            fs::read_to_string(profile.join("mods.json"))
                .is_ok_and(|t| !t.contains("withheld") && !t.contains("\"fill\":"))
        });
        ctx.ok(clean, format!("{pct}: mods.json holds nothing back now"));
    } else {
        let _ = app.session.key("Escape");
    }
    let _ = app.session.key("Escape");
    let closed = ctx.until("the sheet to close", 3_000, &mut || {
        sheet(app)["open"] == json!(false)
    });
    ctx.ok(
        closed,
        format!("{pct}: Escape closes the dropdown, then the sheet"),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_manifest_is_a_branded_plugin_with_one_tab_menu_action_and_a_helper() {
        let m = manifest(Path::new(r"C:\w\brand-helper.jsonl"), Dress::Logo);
        let v: Value = serde_json::from_str(&m).unwrap();
        assert_eq!(v["brand"]["color"], "#175ddc");
        assert_eq!(v["brand"]["icon"], LOGO);
        let c: Value = serde_json::from_str(&manifest(Path::new("r"), Dress::Colour)).unwrap();
        assert!(c["brand"]["icon"].is_null() && c["brand"]["color"] == "#175ddc");
        let p: Value = serde_json::from_str(&manifest(Path::new("r"), Dress::Plain)).unwrap();
        assert!(p.get("brand").is_none(), "no brand at all");
        assert_eq!(v["contributes"]["actions"].as_array().unwrap().len(), 1);
        assert_eq!(v["helper"]["args"][0], "--fake-fill-helper");
        assert!(!m.to_lowercase().contains("bitwarden"));
    }
}

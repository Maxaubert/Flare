//! The device panel (issue #248, PR 4a): `devicePanel`, on the fake source the app has under `--e2e`.
//!
//! Opens the device panel on the front tab with `DeviceOpen` (the stand-in for the panel menu's Device entry, PR 4b)
//! on the fake iPhone, which is shut down: the panel says so and offers Boot, and nothing boots on its own. The
//! scenario presses Boot with a real pointer click, waits for frames handed to Slint (read from `device.state`, not
//! from pixels: a Mac e2e window may count as occluded, rule 1117), clicks the middle of the picture and checks that
//! the fake got a touch start and end at the screen's middle in device points, then that a click in the letterbox's
//! bar is dropped, that `DeviceUserInput` counts as the person's input, and that closing the panel lets the slot go.
//! Runs on the Windows and the Mac e2e jobs: the panel is Slint, the source the fake, and the CPU path is taken.
//!
//! PR 5 adds `deviceAgent` and `mcpDevice`: the agent's `device.*` calls on the same fake. `deviceAgent` speaks the
//! channel with the `flare-ctl` client as the front tab's agent: `hello` lists the device tools, a boot shows the
//! fake in the tab's panel, a look gives a PNG and the UI tree with refs, a tap by ref lands at the element's middle in
//! device points, typing and a button reach the fake, the person's input (`DeviceUserInput`) ends a long press
//! `interrupted`, `panel.open` refuses kind device naming `device_boot`, and a shutdown (the agent booted it) closes
//! the panel. `mcpDevice` drives the same through the built `flare-mcp` over stdio, as Claude Code would: 36 tools,
//! the look as text then a PNG with the device content between its markers, a tap by ref.
//!
//! Nothing here touches the user's windows or a simulator: the app is the built exe, offscreen, in its own world.

use std::time::Duration;

use flare_ctl::Method;
use flare_e2e_proto::Request;
use serde_json::{json, Value};

use super::agent_web::{is_error, text_of, Server};
use super::input_b::{probe, prompt};
use super::web::{channel_of, code_of, hello};
use crate::ctx::{App, Ctx, Launch};

/// The fake source's one device (`flare_device::fake::FakeSource::default`).
const FAKE: &str = "sim/00000000-0000-4000-8000-000000000001";
/// Its screen in points (402 x 874).
const PT: (f64, f64) = (402.0, 874.0);

fn state(app: &mut App) -> Value {
    probe(app, "device.state", Value::Null)
}

fn num(v: &Value) -> f64 {
    v.as_f64().unwrap_or(f64::NAN)
}

/// The middle of a `{x, y, w, h}` rect.
fn middle(r: &Value) -> (f32, f32) {
    (
        (num(&r["x"]) + num(&r["w"]) / 2.0) as f32,
        (num(&r["y"]) + num(&r["h"]) / 2.0) as f32,
    )
}

/// The touches the fake was given: (phase, x, y).
fn touches(s: &Value) -> Vec<(String, f64, f64)> {
    s["inputs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|i| Some((i["touch"].as_str()?.to_string(), num(&i["x"]), num(&i["y"]))))
        .collect()
}

pub fn device_panel(ctx: &mut Ctx) -> Result<(), String> {
    let alpha = ctx.ensure_world().alpha.clone();
    let mut app = ctx.launch(Launch {
        folders: vec![alpha],
        ..Launch::default()
    })?;
    let r = run(ctx, &mut app);
    app.close();
    r
}

fn run(ctx: &mut Ctx, app: &mut App) -> Result<(), String> {
    app.session.call(&Request::Resize {
        w: 1200.0,
        h: 800.0,
    })?;
    prompt(app)?;
    let s0 = state(app);
    ctx.ok(
        s0["offered"] == Value::Bool(true) && s0["tab"].is_null(),
        "the device panel is offered under --e2e and none is open",
    );

    // ---- open: the fake iPhone is shut down; the panel offers Boot and boots nothing ----
    app.session.call(&Request::DeviceOpen {
        target: Some(FAKE.into()),
    })?;
    let shut = ctx.until(
        "the panel to say the device is shut down",
        5_000,
        &mut || {
            let s = state(app);
            s["state"] == "shut_down" && s["button"] == "Boot" && s["kind"] == 5
        },
    );
    let s = state(app);
    ctx.ok(
        shut,
        format!(
            "the panel shows {} with {:?} ({:?})",
            s["title"], s["button"], s["line"]
        ),
    );
    ctx.ok(
        s["attached"] == Value::Bool(false) && touches(&s).is_empty(),
        "opening boots nothing and sends nothing",
    );

    // ---- Boot, with a real click on the setup state's button ----
    let (bx, by) = middle(&s["button_rect"]);
    app.session.click(bx, by)?;
    let live = ctx.until("frames handed to Slint", 8_000, &mut || {
        let s = state(app);
        s["state"] == "live" && num(&s["frames_shown"]) >= 1.0 && s["has_frame"] == true
    });
    let s = state(app);
    ctx.ok(
        live,
        format!(
            "the device boots and is live: {} frame(s) shown",
            s["frames_shown"]
        ),
    );
    ctx.ok(
        s["slow"].as_str().is_some_and(|w| !w.is_empty()),
        "the CPU path wears the slow mark",
    );
    ctx.ok(
        s["controls"] == true,
        "the header shows the device's buttons",
    );
    // The picture keeps the device's aspect inside the box (letterboxed).
    let (pw, ph) = (num(&s["picture"]["w"]), num(&s["picture"]["h"]));
    ctx.ok(
        (pw / ph - PT.0 / PT.1).abs() < 0.01,
        format!("the picture is {pw} x {ph}, the device's aspect"),
    );

    // ---- a click in the middle of the picture is a touch at the screen's middle, in device points ----
    let before = touches(&s).len();
    let (cx, cy) = middle(&s["picture"]);
    app.session.click(cx, cy)?;
    let got = ctx.until("the touch to reach the fake", 3_000, &mut || {
        touches(&state(app)).len() >= before + 2
    });
    let t = touches(&state(app));
    let new: Vec<_> = t.iter().skip(before).collect();
    let near = |v: f64, want: f64| (v - want).abs() <= 2.0;
    ctx.ok(
        got && new.len() == 2
            && new[0].0 == "start"
            && new[1].0 == "end"
            && new
                .iter()
                .all(|(_, x, y)| near(*x, PT.0 / 2.0) && near(*y, PT.1 / 2.0)),
        format!("a click is a touch start and end at the middle: {new:?}"),
    );

    // ---- a click in the letterbox's bar (left of the picture) is dropped ----
    let bar_x = ((num(&s["box"]["x"]) + num(&s["picture"]["x"])) / 2.0) as f32;
    let n = touches(&state(app)).len();
    if num(&s["picture"]["x"]) - num(&s["box"]["x"]) > 8.0 {
        app.session.click(bar_x, cy)?;
        ctx.ok(
            touches(&state(app)).len() == n,
            "a click beside the picture sends no touch",
        );
    }

    // ---- the person's input (the takeover's stand-in) ----
    let p0 = num(&state(app)["person_inputs"]);
    app.session.call(&Request::DeviceUserInput)?;
    ctx.ok(
        num(&state(app)["person_inputs"]) == p0 + 1.0,
        "DeviceUserInput counts as the person's input",
    );

    // ---- close: the slot goes, the terminal keeps the page ----
    app.session.key("Ctrl+Shift+P")?;
    let closed = ctx.until("the panel to close", 3_000, &mut || {
        state(app)["tab"].is_null()
    });
    ctx.ok(closed, "the toggle chord closes the device panel");
    Ok(())
}

/// How long a device call may take here (the fake answers at once; the agent's glide and settle are under a second).
const CALL: Duration = Duration::from_secs(20);

fn launch_alpha(ctx: &mut Ctx) -> Result<App, String> {
    let alpha = ctx.ensure_world().alpha.clone();
    ctx.launch(Launch {
        folders: vec![alpha],
        ..Launch::default()
    })
}

pub fn device_agent(ctx: &mut Ctx) -> Result<(), String> {
    let mut app = launch_alpha(ctx)?;
    let r = agent_run(ctx, &mut app);
    app.close();
    r
}

/// The fake's touches since `from`.
fn touches_since(app: &mut App, from: usize) -> Vec<(String, f64, f64)> {
    touches(&state(app)).into_iter().skip(from).collect()
}

fn agent_run(ctx: &mut Ctx, app: &mut App) -> Result<(), String> {
    app.session.call(&Request::Resize {
        w: 1200.0,
        h: 800.0,
    })?;
    prompt(app)?;
    let (pipe, _id, token) = channel_of(app)?;
    let mut c = hello(&pipe, &token).map_err(|e| format!("hello: {e}"))?;
    let mut probe_c = flare_ctl::connect(&pipe, CALL).map_err(|e| e.to_string())?;
    let hi = probe_c
        .hello(
            &flare_ctl::HelloParams {
                client: "flare-e2e".into(),
                version: "1".into(),
                token: None,
                tab_token: Some(token.clone()),
                no_events: true,
            },
            CALL,
        )
        .map_err(|e| e.to_string())?;
    ctx.ok(
        hi.capabilities.iter().any(|m| m == "device.look")
            && hi
                .capabilities
                .iter()
                .filter(|m| m.starts_with("device."))
                .count()
                == 12,
        format!(
            "hello lists the twelve device tools under --e2e ({:?})",
            hi.capabilities
        ),
    );

    // ---- list, then boot: the fake shows in the tab's own panel and goes live ----
    let list = c
        .call(Method::DeviceList, json!({}), CALL)
        .map_err(|e| e.to_string())?;
    ctx.ok(
        list["devices"]
            .as_array()
            .is_some_and(|d| d.iter().any(|d| d["target"] == FAKE)),
        format!("device.list names the fake ({list})"),
    );
    let boot = c.call(Method::DeviceBoot, json!({}), CALL);
    ctx.ok(
        boot.as_ref()
            .is_ok_and(|v| v["target"] == FAKE && v["booted"] == true),
        format!("device.boot boots the newest iPhone and answers once booted ({boot:?})"),
    );
    let live = ctx.until("the panel to be live", 8_000, &mut || {
        let s = state(app);
        s["kind"] == 5 && s["state"] == "live" && s["attached"] == true
    });
    ctx.ok(live, "the tab's own panel shows the device, live");

    // ---- look: a PNG and the tree with refs ----
    let look = c
        .call(Method::DeviceLook, json!({}), CALL)
        .map_err(|e| e.to_string())?;
    let tap_me = look["tree"]
        .as_array()
        .and_then(|t| t.iter().find(|e| e["label"] == "Tap me"))
        .cloned()
        .unwrap_or(Value::Null);
    ctx.ok(
        look["image"]["mime"] == "image/png"
            && look["image"]["data"]
                .as_str()
                .is_some_and(|d| !d.is_empty())
            && tap_me["ref"].as_str().is_some_and(|r| r.starts_with('d')),
        format!("device.look gives a PNG and the tree with refs ({tap_me})"),
    );

    // ---- a tap by ref lands at the element's middle, in device points ----
    let n = touches(&state(app)).len();
    let r = c.call(Method::DeviceTap, json!({"ref": tap_me["ref"]}), CALL);
    let t = touches_since(app, n);
    let (mx, my) = (
        num(&tap_me["frame"]["x"]) + num(&tap_me["frame"]["w"]) / 2.0,
        num(&tap_me["frame"]["y"]) + num(&tap_me["frame"]["h"]) / 2.0,
    );
    ctx.ok(
        r.is_ok()
            && t.len() == 2
            && t[0].0 == "start"
            && t[1].0 == "end"
            && t.iter()
                .all(|(_, x, y)| (x - mx).abs() < 0.5 && (y - my).abs() < 0.5),
        format!("device.tap by ref is a touch at the element's middle ({r:?} {t:?})"),
    );
    let stale = c.call(Method::DeviceTap, json!({"ref": "d999"}), CALL);
    ctx.ok(
        code_of(&stale) == "not_found",
        format!("a stale ref is not_found ({})", code_of(&stale)),
    );

    // ---- typing and a button ----
    let r = c.call(
        Method::DeviceType,
        json!({"text": "hi", "submit": true}),
        CALL,
    );
    let keys = state(app)["inputs"]
        .as_array()
        .map_or(0, |a| a.iter().filter(|i| i.get("key").is_some()).count());
    ctx.ok(
        r.is_ok() && keys >= 6,
        format!("device.type sends its keys and Return ({keys} key events)"),
    );
    let r = c.call(Method::DevicePress, json!({"button": "home"}), CALL);
    let home = state(app)["inputs"]
        .as_array()
        .is_some_and(|a| a.iter().any(|i| i["button"] == "home"));
    ctx.ok(r.is_ok() && home, "device.press home reaches the device");

    // ---- the person takes over a long press: the call ends `interrupted` ----
    let (pipe2, token2) = (pipe.clone(), token.clone());
    let long = std::thread::spawn(move || {
        let mut c2 = hello(&pipe2, &token2).map_err(|e| e.to_string())?;
        Ok::<_, String>(code_of(&c2.call(
            Method::DeviceTap,
            json!({"x": 200, "y": 400, "hold_ms": 8000}),
            CALL,
        )))
    });
    let running = ctx.until("the agent's long press to run", 5_000, &mut || {
        state(app)["agent"]["jobs"]
            .as_array()
            .is_some_and(|j| !j.is_empty())
    });
    ctx.ok(running, "the long press runs, its dot drawn");
    app.session.call(&Request::DeviceUserInput)?;
    let code = long.join().map_err(|_| "the long press thread")??;
    ctx.ok(
        code == "interrupted",
        format!("the person's input ends the call interrupted ({code})"),
    );
    let s = state(app);
    ctx.ok(
        s["takeovers"].as_u64() >= Some(1) && s["agent"]["dot"].is_null(),
        format!("a takeover is counted and the dot is gone ({})", s["agent"]),
    );

    // ---- panel.open refuses the device kind and names device.boot ----
    let r = c.call(Method::PanelOpen, json!({"kind": "device"}), CALL);
    let said = match &r {
        Err(flare_ctl::ClientError::Remote(e)) => e.message.clone(),
        _ => String::new(),
    };
    ctx.ok(
        code_of(&r) == "bad_params" && said.contains("device_boot"),
        format!("panel.open kind device is refused and names device_boot ({said})"),
    );

    // ---- the agent booted it, so it may shut it down: the panel closes ----
    let r = c.call(Method::DeviceShutdown, json!({}), CALL);
    let closed = ctx.until("the panel to close", 3_000, &mut || {
        state(app)["tab"].is_null()
    });
    ctx.ok(
        r.is_ok() && closed,
        format!("device.shutdown shuts down what Flare booted and closes the panel ({r:?})"),
    );
    Ok(())
}

pub fn mcp_device(ctx: &mut Ctx) -> Result<(), String> {
    let exe = ctx
        .exe
        .parent()
        .map(|d| d.join(format!("flare-mcp{}", std::env::consts::EXE_SUFFIX)))
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
    let mut app = launch_alpha(ctx)?;
    let r = mcp_run(ctx, &mut app, &exe);
    app.close();
    r
}

fn mcp_run(ctx: &mut Ctx, app: &mut App, exe: &std::path::Path) -> Result<(), String> {
    app.session.call(&Request::Resize {
        w: 1200.0,
        h: 800.0,
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
    let init = s.ask(
        1,
        "initialize",
        json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "flare-e2e", "version": "1"}}),
    )?;
    ctx.ok(
        init["result"]["instructions"]
            .as_str()
            .is_some_and(|i| i.contains("device_* tools")),
        "the instructions name the device tools where they are listed",
    );
    let list = s.ask(2, "tools/list", json!({}))?;
    let names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .map(|a| a.iter().filter_map(|t| t["name"].as_str()).collect())
        .unwrap_or_default();
    let builtin = names.iter().filter(|n| !n.starts_with("mod_")).count();
    ctx.ok(
        builtin == 37 && names.contains(&"device_look") && names.contains(&"device_tap"),
        format!("tools/list has 37 built-in tools with the device ones ({builtin})"),
    );
    let r = s.tool(3, "device_boot", json!({}))?;
    ctx.ok(!is_error(&r), format!("device_boot ({})", text_of(&r)));
    ctx.until("the panel to be live", 8_000, &mut || {
        let st = state(app);
        st["state"] == "live" && st["attached"] == true
    });
    let r = s.tool(4, "device_look", json!({}))?;
    let items = r["result"]["content"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let text = text_of(&r);
    ctx.ok(
        items.first().is_some_and(|c| c["type"] == "text")
            && items
                .get(1)
                .is_some_and(|c| c["type"] == "image" && c["mimeType"] == "image/png"),
        "device_look answers the text first, then a PNG",
    );
    ctx.ok(
        text.contains("BEGIN DEVICE CONTENT")
            && text.lines().filter(|l| *l == "END DEVICE CONTENT").count() == 1,
        "the device's words sit between the device content markers",
    );
    let tap_ref = text
        .lines()
        .find(|l| l.starts_with("[d") && l.contains("\"Tap me\""))
        .and_then(|l| l.split(']').next())
        .map(|r| r.trim_start_matches('[').to_string());
    ctx.ok(
        tap_ref.is_some(),
        format!("the list names the button with a ref ({tap_ref:?})"),
    );
    let n = touches(&state(app)).len();
    let r = s.tool(5, "device_tap", json!({"ref": tap_ref.unwrap_or_default()}))?;
    ctx.ok(
        !is_error(&r) && touches(&state(app)).len() >= n + 2,
        format!("device_tap by ref ({})", text_of(&r)),
    );
    let r = s.tool(6, "device_tap", json!({"ref": "d1", "x": 1, "y": 1}))?;
    ctx.ok(
        is_error(&r),
        "ref and x, y together are refused before Flare hears of it",
    );
    let r = s.tool(7, "device_state", json!({}))?;
    ctx.ok(
        !is_error(&r) && text_of(&r).contains("is booted"),
        format!("device_state ({})", text_of(&r)),
    );
    Ok(())
}

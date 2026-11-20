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
//! Nothing here touches the user's windows or a simulator: the app is the built exe, offscreen, in its own world.

use flare_e2e_proto::Request;
use serde_json::Value;

use super::input_b::{probe, prompt};
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

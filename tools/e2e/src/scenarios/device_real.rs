//! `deviceReal` (issue #248, device panel PR 4b): the device panel on a real simulator, opt-in only (a filter word
//! that is its whole name), on a Mac with Xcode and an iOS runtime (the GitHub macOS runner).
//!
//! The scenario makes its own simulator (`simctl create`, never one of the owner's) and deletes it at the end, starts
//! the app with `FLARE_DEVICE_REAL=1` so `--e2e` takes the real source instead of the fake, opens the device panel on
//! it with `DeviceOpen`, checks that it shows Boot and boots nothing on its own (rule 1317), presses Boot, waits for
//! the sim host's ring frames (the zero-copy path, rule 1311: a frame record reached the window; the picture's count
//! is read from `device.state`, since a parked Mac e2e window may render no frame, rule 1117), then quits the app and
//! checks that the simulator Flare booted is shut down after the quit (rule 1319). It needs the sim host next to the
//! app (`cargo build --release -p flare-device`, which the `e2e-macos` job does). Nothing opens Simulator.app: the
//! simulator boots headless.
//!
//! Where the runner has no Xcode or no iOS runtime (open check 6), the scenario says so and skips the rest.

use std::process::Command;
use std::time::Instant;

use flare_e2e_proto::Request;
use serde_json::Value;

use super::input_b::{probe, prompt};
use crate::ctx::{App, Ctx, Launch};

/// `xcrun simctl <args>`: stdout, or the first line of what went wrong.
fn simctl(args: &[&str]) -> Result<String, String> {
    let out = Command::new("/usr/bin/xcrun")
        .arg("simctl")
        .args(args)
        .output()
        .map_err(|e| format!("xcrun did not start: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(err
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("failed")
            .to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// The newest available iOS runtime's identifier.
fn ios_runtime() -> Result<String, String> {
    let v: Value = serde_json::from_str(&simctl(&["list", "-j", "runtimes", "available"])?)
        .map_err(|e| format!("simctl list runtimes: {e}"))?;
    v["runtimes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| {
            r["platform"] == "iOS" || r["name"].as_str().is_some_and(|n| n.starts_with("iOS"))
        })
        .filter_map(|r| r["identifier"].as_str())
        .next_back()
        .map(str::to_string)
        .ok_or_else(|| "no iOS runtime is installed".to_string())
}

/// An iPhone device type, the newest Pro if there is one.
fn iphone_type() -> Result<String, String> {
    let v: Value = serde_json::from_str(&simctl(&["list", "-j", "devicetypes"])?)
        .map_err(|e| format!("simctl list devicetypes: {e}"))?;
    let phones: Vec<&Value> = v["devicetypes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|t| t["name"].as_str().is_some_and(|n| n.starts_with("iPhone")))
        .collect();
    phones
        .iter()
        .rev()
        .find(|t| t["name"].as_str().is_some_and(|n| n.ends_with(" Pro")))
        .or(phones.last())
        .and_then(|t| t["identifier"].as_str())
        .map(str::to_string)
        .ok_or_else(|| "no iPhone device type".to_string())
}

/// The state `simctl list` gives the device ("Shutdown", "Booted", ...).
fn state_of(udid: &str) -> Option<String> {
    let v: Value = serde_json::from_str(&simctl(&["list", "-j", "devices"]).ok()?).ok()?;
    v["devices"]
        .as_object()?
        .values()
        .flat_map(|l| l.as_array().into_iter().flatten())
        .find(|d| {
            d["udid"]
                .as_str()
                .is_some_and(|u| u.eq_ignore_ascii_case(udid))
        })
        .and_then(|d| d["state"].as_str())
        .map(str::to_string)
}

fn state(app: &mut App) -> Value {
    probe(app, "device.state", Value::Null)
}

fn num(v: &Value) -> f64 {
    v.as_f64().unwrap_or(f64::NAN)
}

pub fn device_real(ctx: &mut Ctx) -> Result<(), String> {
    if !cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        ctx.skip("deviceReal runs only on a Mac with Apple silicon");
        ctx.ok(
            true,
            "not a Mac with Apple silicon: the device panel is not offered here",
        );
        return Ok(());
    }
    let xcode = Command::new("/usr/bin/xcode-select").arg("-p").output();
    let has_xcode = xcode
        .as_ref()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains(".app/"));
    ctx.ok(
        has_xcode,
        "an Xcode is selected (open check 6: the runner image has one)",
    );
    if !has_xcode {
        return Ok(());
    }
    let (runtime, kind) = match (ios_runtime(), iphone_type()) {
        (Ok(r), Ok(k)) => (r, k),
        (Err(e), _) | (_, Err(e)) => {
            ctx.skip(&format!(
                "no iOS simulator on this runner (open check 6): {e}"
            ));
            return Ok(());
        }
    };
    let name = format!("flare-e2e-{}", std::process::id());
    let udid = simctl(&["create", &name, &kind, &runtime])?
        .trim()
        .to_lowercase();
    ctx.ok(
        !udid.is_empty(),
        format!("a simulator made for the run: {kind}, {runtime}"),
    );
    let r = run(ctx, &udid);
    // Always: the run's simulator goes (shut down first, in case the quit did not).
    let _ = simctl(&["shutdown", &udid]);
    let deleted = simctl(&["delete", &udid]);
    ctx.ok(
        deleted.is_ok(),
        format!("the run's simulator is deleted: {deleted:?}"),
    );
    r
}

fn run(ctx: &mut Ctx, udid: &str) -> Result<(), String> {
    let alpha = ctx.ensure_world().alpha.clone();
    let mut app = ctx.launch(Launch {
        folders: vec![alpha],
        env: vec![("FLARE_DEVICE_REAL".into(), "1".into())],
        ..Launch::default()
    })?;
    let r = steps(ctx, &mut app, udid);
    // The quit shuts down the simulator Flare booted (rule 1319), detached: the quit does not wait for it.
    let t0 = Instant::now();
    let quit_alone = app.close();
    ctx.ok(
        quit_alone,
        format!("the app quits by itself in {} ms", t0.elapsed().as_millis()),
    );
    if r.is_ok() {
        let down = ctx.until(
            "the simulator Flare booted to shut down after the quit",
            90_000,
            &mut || state_of(udid).as_deref() == Some("Shutdown"),
        );
        ctx.ok(
            down,
            "quit shut down the simulator Flare booted (setting on)",
        );
    }
    r
}

fn steps(ctx: &mut Ctx, app: &mut App, udid: &str) -> Result<(), String> {
    app.session.call(&Request::Resize {
        w: 1200.0,
        h: 800.0,
    })?;
    prompt(app)?;
    let ready = ctx.until("the Xcode pick to answer", 20_000, &mut || {
        let s = state(app);
        s["offered"] == true && s["menu"] == true
    });
    ctx.ok(
        ready,
        "the real source is offered and the menu entry shows (Xcode found)",
    );
    app.session.call(&Request::DeviceOpen {
        target: Some(format!("sim/{udid}")),
    })?;
    let shut = ctx.until("the panel to show Boot", 60_000, &mut || {
        let s = state(app);
        s["state"] == "shut_down" && s["button"] == "Boot"
    });
    let s = state(app);
    ctx.ok(
        shut,
        format!(
            "restore and open boot nothing: {} {:?}",
            s["state"], s["line"]
        ),
    );
    ctx.ok(
        state_of(udid).as_deref() == Some("Shutdown"),
        "the simulator is still shut down (rule 1317)",
    );
    let r = &s["button_rect"];
    let (bx, by) = (
        (num(&r["x"]) + num(&r["w"]) / 2.0) as f32,
        (num(&r["y"]) + num(&r["h"]) / 2.0) as f32,
    );
    app.session.click(bx, by)?;
    let booted = ctx.until("the boot", 240_000, &mut || {
        state_of(udid).as_deref() == Some("Booted")
    });
    ctx.ok(booted, "Boot boots the simulator, headless");
    let live = ctx.until("the ring's first frame", 120_000, &mut || {
        let s = state(app);
        s["state"] == "live" && s["attached"] == true && s["has_frame"] == true
    });
    let s = state(app);
    ctx.ok(
        live,
        format!(
            "live on the ring: path {}, {} ring frame(s), {} slot(s) held, hello {}",
            s["path"], s["ring_frames"], s["slots_held"], s["hello"]
        ),
    );
    ctx.ok(
        s["booted_by_flare"].as_array().is_some_and(|l| {
            l.iter()
                .any(|u| u.as_str() == Some(format!("sim/{udid}").as_str()))
        }),
        "Flare remembers it booted this simulator (booted.json)",
    );
    ctx.ok(
        s["still"].as_str().is_some_and(str::is_empty),
        format!("no still mode: {:?}", s["still"]),
    );
    if num(&s["ring_frames"]) >= 1.0 {
        ctx.ok(
            s["path"] == "gpu" && s["slow"].as_str().is_some_and(str::is_empty),
            format!(
                "the ring is shown with no CPU copy and no slow mark: {}",
                s["path"]
            ),
        );
    } else {
        ctx.skip(
            "the parked window drew no frame (rule 1117): the ring's import is not checked here",
        );
    }
    // The person's tap reaches the device (no crash, the panel stays live).
    let p = &s["picture"];
    let (cx, cy) = (
        (num(&p["x"]) + num(&p["w"]) / 2.0) as f32,
        (num(&p["y"]) + num(&p["h"]) / 2.0) as f32,
    );
    let before = num(&s["person_inputs"]);
    app.session.click(cx, cy)?;
    ctx.until("the person's touch is counted", 5000, &mut || {
        num(&state(app)["person_inputs"]) > before
    });
    let s = state(app);
    ctx.ok(
        num(&s["person_inputs"]) > before && s["state"] == "live",
        "a click on the picture is the person's touch; the panel stays live",
    );
    Ok(())
}

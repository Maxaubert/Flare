//! Lifecycle scenarios (Task 16, 20): spawn, hookSmoke, cwdLabel, restore, handoff, quitManyShells, lastTab, exitClosesTab, promptLayout. Each is a stub until its port task fills it.

use std::path::Path;
use std::time::Duration;

use flare_e2e_proto::{Reply, Request, SnapMode};

use crate::ctx::{App, Ctx, Launch};
use crate::proc::reap_by_profile;
use crate::world::{prompt_timeout, rows_text, wait_for_prompt, Shell};

/// How many tabs the app shows now (0 when the probe fails).
fn tab_count(app: &mut App) -> usize {
    app.session.state().map(|s| s.tabs.len()).unwrap_or(0)
}

/// The tab titles' tooltips, in strip order.
fn tooltips(app: &mut App) -> Vec<String> {
    app.session
        .state()
        .map(|s| s.tabs.into_iter().map(|t| t.tooltip).collect())
        .unwrap_or_default()
}

/// True when a `#rrggbb` colour is chromatic (its channels are not all alike): the accent is,
/// a grey is not.
pub fn is_chromatic(hex: &str) -> bool {
    let h = hex.trim_start_matches('#');
    if h.len() != 6 {
        return false;
    }
    let ch = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    match (ch(0), ch(2), ch(4)) {
        (Some(r), Some(g), Some(b)) => !(r == g && g == b),
        _ => false,
    }
}

/// True when the PNG at `path` has at least two different pixels (a blank frame has one colour).
pub fn png_is_not_blank(path: &Path) -> bool {
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    let Ok(mut reader) = png::Decoder::new(std::io::BufReader::new(file)).read_info() else {
        return false;
    };
    let mut buf = vec![0; reader.output_buffer_size().unwrap_or(0)];
    let Ok(info) = reader.next_frame(&mut buf) else {
        return false;
    };
    let px = &buf[..info.buffer_size()];
    let step = match info.color_type {
        png::ColorType::Rgba => 4,
        png::ColorType::Rgb => 3,
        png::ColorType::GrayscaleAlpha => 2,
        _ => 1,
    };
    if px.len() < step * 2 {
        return false;
    }
    let first = &px[..step];
    px.chunks_exact(step).any(|c| c != first)
}

/// The reference port. PT: run.mjs:473-509. New terminal opens in the user's own folder with
/// nothing asked, Ctrl+T does the same from inside a shell, and nothing writes the registry.
pub fn spawn(ctx: &mut Ctx) -> Result<(), String> {
    let mut app = ctx.launch(Launch {
        renderer: Some("software"),
        ..Launch::default()
    })?;
    let home = std::env::var("USERPROFILE").unwrap_or_default();

    // PT: run.mjs:478
    let start = app.session.probe_str("start.state")?;
    ctx.ok(
        start["shown"] == true,
        "an empty launch lands on the start screen",
    );

    // PT: run.mjs:479-490. Owner, 2026-10-12: the app's mark, in the accent, on the start page.
    let colour = start["mark_colour"].as_str().unwrap_or("").to_string();
    ctx.ok(
        is_chromatic(&colour),
        format!("the start screen shows the app's mark in the accent ({colour})"),
    );
    ctx.shot(&mut app, "start-screen");

    // PT: run.mjs:491-492. The click goes to the New row where the layout put it.
    let (cx, cy) = (
        start["new_centre"]["x"].as_f64().unwrap_or(0.0) as f32,
        start["new_centre"]["y"].as_f64().unwrap_or(0.0) as f32,
    );
    app.session.click(cx, cy)?;
    let opened = ctx.until("the first tab", 15_000, &mut || tab_count(&mut app) == 1);
    ctx.ok(opened, "New terminal opens a tab with nothing asked");

    // PT: run.mjs:493
    let tips = tooltips(&mut app);
    ctx.ok(
        tips.first()
            .is_some_and(|t| !home.is_empty() && t.eq_ignore_ascii_case(&home)),
        format!("in the user's own folder, and the tooltip is the full path ({tips:?})"),
    );

    // PT: run.mjs:494-495
    let front = app
        .session
        .state()
        .ok()
        .and_then(|s| s.front)
        .unwrap_or_default();
    let prompt = wait_for_prompt(
        &mut app,
        &front,
        Shell::PowerShell,
        prompt_timeout(Shell::PowerShell),
    );
    ctx.ok(prompt, "the shell shows its first prompt");
    app.session.type_text("echo flare-$(20+22)-ok")?;
    app.session.key("Enter")?;
    let echoed = ctx.until("the echo", 15_000, &mut || {
        rows_text(&mut app, &front).contains("flare-42-ok")
    });
    ctx.ok(echoed, "echo round-trips through the pty");

    // PT: run.mjs:497-499. From INSIDE the shell: the terminal has to yield the chord to the app.
    app.session.key("Ctrl+T")?;
    let second = ctx.until("the second tab", 15_000, &mut || tab_count(&mut app) == 2);
    ctx.ok(second, "Ctrl+T opens a tab over a focused shell");

    // PT: run.mjs:500-505 (ask mode): Flare has no plus-button rectangle to click and no
    // pref seam yet; the Settings port (Task 21, `newTabFolder`) holds that check.
    ctx.skip(
        "ask mode: the + opens the folder the chooser answered (held by newTabFolder, Task 21)",
    );

    // PT: run.mjs:506
    let reg = app.session.probe_str("registry.writes")?;
    // The verb switch writes into the run's own sandbox root (never the real registry), so a write
    // is fine there; any other root with a write is the failure.
    let root = reg["root"].as_str().unwrap_or("").to_string();
    let sandboxed = root.to_ascii_lowercase().starts_with(r"software\flaree2e\");
    let writes = reg["count"].as_u64().unwrap_or(u64::MAX);
    ctx.ok(
        writes == 0 || sandboxed,
        format!("no registry write was attempted outside the sandbox under --e2e ({writes} in {root:?})"),
    );

    app.close();
    Ok(())
}

/// What a quit leaves: true when the child exited 0 within `secs` of `Quit`.
fn quit_exits_zero(app: &mut App, secs: u64) -> bool {
    let _ = app.session.call(&Request::Quit { force: false });
    let child = &app.child;
    child.wait_for(|| child.exited().is_some(), Duration::from_secs(secs))
        && app.child.exited() == Some(0)
}

/// The scale a `DpiChange` should give: 96 is 100 percent.
fn dpi_scale(dpi: u32) -> f32 {
    dpi as f32 / 96.0
}

/// One renderer's proof that the hook is live.
fn smoke_one(ctx: &mut Ctx, renderer: &'static str) -> Result<(), String> {
    let mut app = ctx.launch(Launch {
        renderer: Some(renderer),
        ..Launch::default()
    })?;
    let hello = app.hello()?;
    ctx.ok(
        !hello.version.is_empty() && hello.pid == app.child.pid(),
        format!(
            "[{renderer}] Hello answers (version {}, renderer {})",
            hello.version, hello.renderer
        ),
    );

    // Auto picks a mode; Slint is tried on its own so the log says which one worked.
    let png_path = ctx.shots.join(format!("hookSmoke-{renderer}-auto.png"));
    let _ = std::fs::create_dir_all(&ctx.shots);
    let auto = app.session.call(&Request::Snapshot {
        path: png_path.to_string_lossy().into_owned(),
        mode: Some(SnapMode::Auto),
    });
    let slint_path = ctx.shots.join(format!("hookSmoke-{renderer}-slint.png"));
    let slint = app.session.call(&Request::Snapshot {
        path: slint_path.to_string_lossy().into_owned(),
        mode: Some(SnapMode::Slint),
    });
    let slint_ok = matches!(slint, Ok(Reply::Png { .. })) && png_is_not_blank(&slint_path);
    ctx.ok(
        matches!(auto, Ok(Reply::Png { .. })) && png_is_not_blank(&png_path),
        format!(
            "[{renderer}] Snapshot (Auto) writes a non-blank PNG (the Slint mode alone: {})",
            if slint_ok {
                "works"
            } else {
                "fails, so Auto used PrintWindow"
            }
        ),
    );

    let _ = app.session.call(&Request::Frames { reset: true });
    app.session.key("Ctrl+T")?;
    let opened = ctx.until("the Ctrl+T tab", 15_000, &mut || tab_count(&mut app) == 1);
    ctx.ok(opened, format!("[{renderer}] Key Ctrl+T opens a tab"));
    let mut frames = 0;
    ctx.until("a frame to be counted", 3_000, &mut || {
        frames = match app.session.call(&Request::Frames { reset: false }) {
            Ok(Reply::Frames { count, .. }) => count,
            _ => 0,
        };
        frames >= 1
    });
    ctx.ok(frames >= 1, format!("[{renderer}] Frames counted {frames}"));
    // 120 is 125 percent, 288 is 300, back to 96. Message mode is the default; the Event mode
    // (FLARE_E2E_DPI_MODE=event) is the fallback if a step here fails.
    for dpi in [120u32, 288, 96] {
        let want = dpi_scale(dpi);
        let _ = app.session.call(&Request::DpiChange {
            dpi,
            x: None,
            y: None,
        });
        let followed = ctx.until(&format!("the scale to reach {want}"), 2_000, &mut || {
            app.session
                .state()
                .is_ok_and(|s| (s.scale - want).abs() < 0.01)
        });
        ctx.ok(
            followed,
            format!("[{renderer}] DpiChange({dpi}) makes the scale {want} (Message mode)"),
        );
    }

    let exited = quit_exits_zero(&mut app, 5);
    ctx.ok(
        exited,
        format!("[{renderer}] Quit ends the process within 5 s with exit 0"),
    );
    let strays = reap_by_profile(&app.profile);
    ctx.ok(
        strays == 0,
        format!("[{renderer}] no stray process is left ({strays})"),
    );
    Ok(())
}

/// The hook proof, on both renderers. `FLARE_E2E_HOOKSMOKE_RENDERERS=software` limits it to one
/// (a machine that must not render on the GPU).
pub fn hook_smoke(ctx: &mut Ctx) -> Result<(), String> {
    let asked = std::env::var("FLARE_E2E_HOOKSMOKE_RENDERERS").unwrap_or_default();
    let want = |r: &str| asked.is_empty() || asked.split(',').any(|w| w.trim() == r);
    for renderer in ["software", "gpu"] {
        if !want(renderer) {
            ctx.skip(&format!("[{renderer}] not asked for"));
            continue;
        }
        smoke_one(ctx, renderer)?;
    }
    Ok(())
}

/// Stub until Task 20 ports "cwdLabel".
pub fn cwd_label(ctx: &mut Ctx) -> Result<(), String> {
    ctx.not_ported(20)
}

/// Stub until Task 20 ports "restore".
pub fn restore(ctx: &mut Ctx) -> Result<(), String> {
    ctx.not_ported(20)
}

/// Stub until Task 20 ports "handoff".
pub fn handoff(ctx: &mut Ctx) -> Result<(), String> {
    ctx.not_ported(20)
}

/// Stub until Task 20 ports "quitManyShells".
pub fn quit_many_shells(ctx: &mut Ctx) -> Result<(), String> {
    ctx.not_ported(20)
}

/// Stub until Task 20 ports "lastTab".
pub fn last_tab(ctx: &mut Ctx) -> Result<(), String> {
    ctx.not_ported(20)
}

/// Stub until Task 20 ports "exitClosesTab".
pub fn exit_closes_tab(ctx: &mut Ctx) -> Result<(), String> {
    ctx.not_ported(20)
}

/// Stub until Task 20 ports "promptLayout".
pub fn prompt_layout(ctx: &mut Ctx) -> Result<(), String> {
    ctx.not_ported(20)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_accent_is_chromatic_and_a_grey_is_not() {
        assert!(is_chromatic("#fe8f34"));
        assert!(!is_chromatic("#808080"));
        assert!(!is_chromatic("nope"));
    }

    #[test]
    fn a_blank_png_is_blank_and_a_painted_one_is_not() {
        let dir = tempfile::tempdir().unwrap();
        for (name, paint) in [("blank.png", false), ("painted.png", true)] {
            let p = dir.path().join(name);
            let mut data = vec![20u8; 4 * 4 * 4];
            if paint {
                data[0] = 200;
            }
            let file = std::fs::File::create(&p).unwrap();
            let mut enc = png::Encoder::new(file, 4, 4);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header().unwrap().write_image_data(&data).unwrap();
            assert_eq!(png_is_not_blank(&p), paint, "{name}");
        }
        assert!(!png_is_not_blank(&dir.path().join("missing.png")));
    }

    #[test]
    fn the_dpi_steps_give_the_scales_the_plan_names() {
        assert_eq!(dpi_scale(96), 1.0);
        assert_eq!(dpi_scale(120), 1.25);
        assert_eq!(dpi_scale(288), 3.0);
    }
}

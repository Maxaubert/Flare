//! Lifecycle scenarios (Tasks 16 and 20): spawn, hookSmoke, cwdLabel, restore, handoff, quitManyShells, lastTab, exitClosesTab, promptLayout.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use flare_e2e_proto::{Reply, Request, SnapMode};
use flare_tabs::restore::{parse_tabs, SavedTab, SavedTabs};
use flare_tabs::tabs_store::TabsStore;
use serde_json::json;

use crate::ctx::{App, Ctx, Launch};
use crate::guards::ReapGuard;
use crate::proc::{reap_by_profile, Child};
use crate::session::{Rect, TabState};
use crate::world::{prompt_showing, prompt_timeout, rows_text, wait_for_prompt, Shell};

/// The folder the picker seam answers with in `spawn`'s ask-mode check.
const ASK_FOLDER: &str = "ask-answer";

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
    let chosen = ctx.ensure_world().root.join(ASK_FOLDER);
    fs::create_dir_all(&chosen).map_err(|e| e.to_string())?;
    let mut app = ctx.launch(Launch {
        renderer: Some("software"),
        env: vec![(
            "FLARE_E2E_PICK".to_string(),
            chosen.to_string_lossy().into_owned(),
        )],
        ..Launch::default()
    })?;
    let home = std::env::var("USERPROFILE").unwrap_or_default();

    // PT: run.mjs:478
    let start = app.session.probe_str("start.state")?;
    ctx.ok(
        start["shown"] == true,
        "an empty launch lands on the start screen",
    );

    // PT: run.mjs:479-490. Owner, 2026-10-12: the app's mark on the start page; 2026-10-22: the icon's own orange and gold flame, not the accent.
    let colour = start["mark_colour"].as_str().unwrap_or("").to_string();
    let gold = start["mark_gold"].as_str().unwrap_or("").to_string();
    ctx.ok(
        colour == "#ff4800" && gold == "#ffc500",
        format!("the start screen shows the icon's orange flame with the golden one over it ({colour}, {gold})"),
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

    // PT: run.mjs:500-505 (ask mode). Flare has no ask mode (2026-10-27): the folder menu's first
    // row, "Choose a folder…", asks instead. A right-click on the strip's + opens the menu, and the
    // row opens the folder the chooser answered: the FLARE_E2E_PICK seam answers at once, with no
    // dialog.
    let before = tab_count(&mut app);
    let plus = app.session.probe("ui.rect", json!({ "name": "plus" })).ok();
    let menu_up = match plus.as_ref().and_then(|r| {
        Some((
            r["x"].as_f64()? + r["w"].as_f64()? / 2.0,
            r["y"].as_f64()? + r["h"].as_f64()? / 2.0,
        ))
    }) {
        Some((x, y)) => {
            app.session.right_click(x as f32, y as f32)?;
            ctx.until("the folder menu", 5_000, &mut || {
                app.session
                    .probe_str("app.state")
                    .is_ok_and(|s| s["menu"]["open"] == json!(true))
            })
        }
        None => false,
    };
    let first = app
        .session
        .probe_str("app.state")
        .ok()
        .and_then(|s| s["menu"]["rows"][0]["label"].as_str().map(str::to_string))
        .unwrap_or_default();
    let picked = match app
        .session
        .probe("ui.rect", json!({ "name": "menu" }))
        .ok()
        .and_then(|m| Some((m["x"].as_f64()?, m["y"].as_f64()?)))
    {
        // The first row is 28 px tall, under the panel's hairline border.
        Some((x, y)) => app
            .session
            .click((x + 30.0) as f32, (y + 15.0) as f32)
            .is_ok(),
        None => false,
    };
    let opened = ctx.until(
        "a tab in the folder the chooser answered",
        15_000,
        &mut || tab_count(&mut app) == before + 1,
    );
    let last = tooltips(&mut app).pop().unwrap_or_default();
    ctx.ok(
        menu_up
            && first.starts_with("Choose a folder")
            && picked
            && opened
            && last.to_lowercase().contains(ASK_FOLDER),
        format!("Choose a folder… opens the folder the chooser answered ({first:?}, {last:?})"),
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
        format!(
            "no registry write was attempted outside the sandbox under --e2e ({writes} in {root:?})"
        ),
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
    // The second launch of this scenario shares the profile, so it restores the first launch's tab:
    // the key is proven by one more tab than before it, not by an absolute count.
    let before = tab_count(&mut app);
    app.session.key("Ctrl+T")?;
    let opened = ctx.until("the Ctrl+T tab", 15_000, &mut || {
        tab_count(&mut app) == before + 1
    });
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

/// The tabs the app shows now, in strip order (empty when the probe is not answered).
fn tabs_of(app: &mut App) -> Vec<TabState> {
    app.session.state().map(|s| s.tabs).unwrap_or_default()
}

/// The id of the tab in front, or an empty string when the probe is not answered.
fn front_id(app: &mut App) -> String {
    app.session
        .state()
        .ok()
        .and_then(|s| s.front)
        .unwrap_or_default()
}

/// True when `cwd` (a Windows path) ends in the folder `name`, ignoring case and trailing slashes.
fn folder_is(cwd: &str, name: &str) -> bool {
    cwd.trim_end_matches(['\\', '/'])
        .rsplit(['\\', '/'])
        .next()
        .is_some_and(|last| last.eq_ignore_ascii_case(name))
}

/// Types `text` into the front shell and presses Enter (PT `typeLine`).
fn type_line(app: &mut App, text: &str) -> Result<(), String> {
    app.session.type_text(text)?;
    app.session.key("Enter")
}

/// The centre of the named rectangle the window exports (`ui.rect`), in client pixels.
fn centre_of(app: &mut App, name: &str) -> Option<(f32, f32)> {
    let v = app.session.probe("ui.rect", json!({ "name": name })).ok()?;
    let r: Rect = serde_json::from_value(v).ok()?;
    Some((r.x + r.w / 2.0, r.y + r.h / 2.0))
}

/// The save debounce of the tabs store the scenario writes `tabs.json` with.
const SAVE_DEBOUNCE: Duration = Duration::from_millis(400);

/// The width of the terminal grid (`ui.rect grid`), in client pixels.
fn grid_width(app: &mut App) -> Option<f32> {
    let v = app
        .session
        .probe("ui.rect", json!({ "name": "grid" }))
        .ok()?;
    let r: Rect = serde_json::from_value(v).ok()?;
    Some(r.w)
}

/// Writes `tabs.json` into `profile` with the app's own store: one saved tab per folder, the
/// first one active. Flushed at once, so the file is there before the app starts.
pub(super) fn write_tabs(profile: &Path, folders: &[PathBuf]) -> Result<(), String> {
    fs::create_dir_all(profile).map_err(|e| format!("the profile folder: {e}"))?;
    let store = TabsStore::new(profile.join("tabs.json"), SAVE_DEBOUNCE);
    let saved = SavedTabs {
        tabs: folders
            .iter()
            .map(|f| SavedTab {
                cwd: f.to_string_lossy().into_owned(),
                agent: None,
            })
            .collect(),
        active: 0,
    };
    store.save(&saved);
    store.flush();
    Ok(())
}

/// The label follows the shell's own folder report. PT: run.mjs:510-518.
pub fn cwd_label(ctx: &mut Ctx) -> Result<(), String> {
    let alpha = ctx.ensure_world().alpha.clone();
    let mut app = ctx.launch(Launch {
        renderer: Some("software"),
        folders: vec![alpha],
        ..Launch::default()
    })?;
    let opened = ctx.until("the launch folder's tab", 15_000, &mut || {
        tabs_of(&mut app)
            .first()
            .is_some_and(|t| t.title.contains("alpha"))
    });
    ctx.ok(opened, "a launch folder opens as a tab named for it");

    let front = front_id(&mut app);
    ctx.until("the first prompt", 45_000, &mut || {
        prompt_showing(&rows_text(&mut app, &front), Shell::PowerShell)
    });
    type_line(&mut app, "mkdir sub | Out-Null; cd sub")?;
    let renamed = ctx.until("the tab to be renamed", 15_000, &mut || {
        tabs_of(&mut app)
            .first()
            .is_some_and(|t| t.title.contains("sub"))
    });
    ctx.ok(renamed, "cd renames the tab");
    let tip = tabs_of(&mut app)
        .first()
        .map(|t| t.tooltip.clone())
        .unwrap_or_default();
    ctx.ok(
        folder_is(&tip, "sub"),
        format!("and the tooltip follows ({tip})"),
    );
    app.close();
    Ok(())
}

/// The saved strip comes back in its folders, in order. PT: run.mjs:3378-3394. The file is written
/// with the app's store before the first launch; the quit's flush is what is then checked.
pub fn restore(ctx: &mut Ctx) -> Result<(), String> {
    let (alpha, beta, profile) = {
        let w = ctx.ensure_world();
        (w.alpha.clone(), w.beta.clone(), w.profile.clone())
    };
    write_tabs(&profile, &[alpha.clone(), beta.clone()])?;
    let mut app = ctx.launch(Launch {
        renderer: Some("software"),
        ..Launch::default()
    })?;
    let two = ctx.until("two tabs back", 20_000, &mut || {
        tabs_of(&mut app).len() == 2
    });
    ctx.ok(two, "two tabs open");
    app.close();

    let saved = parse_tabs(&fs::read_to_string(profile.join("tabs.json")).unwrap_or_default());
    ctx.ok(
        saved.tabs.len() == 2,
        format!(
            "tabs.json holds both ({})",
            saved
                .tabs
                .iter()
                .map(|t| t.cwd.rsplit('\\').next().unwrap_or_default().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    );

    let mut app = ctx.launch(Launch {
        renderer: Some("software"),
        ..Launch::default()
    })?;
    let back = ctx.until("a relaunch bringing both back", 20_000, &mut || {
        tabs_of(&mut app).len() == 2
    });
    ctx.ok(back, "a relaunch brings both back");
    if back {
        let tabs = tabs_of(&mut app);
        ctx.ok(
            folder_is(&tabs[0].cwd, "alpha") && folder_is(&tabs[1].cwd, "beta"),
            "in their folders, in order",
        );
    }
    app.close();
    Ok(())
}

/// A second launch hands its folder to the running window and exits. PT: run.mjs:3396-3417. The
/// second instance runs software too (no GPU device for a hand-off), and the same folder again is
/// still a new tab: two shells in one folder is normal.
pub fn handoff(ctx: &mut Ctx) -> Result<(), String> {
    let (alpha, beta, env) = {
        let w = ctx.ensure_world();
        (w.alpha.clone(), w.beta.clone(), w.env())
    };
    let exe = ctx.exe.clone();
    let mut app = ctx.launch(Launch {
        renderer: Some("software"),
        folders: vec![alpha],
        ..Launch::default()
    })?;
    ctx.until("the first tab", 15_000, &mut || {
        tabs_of(&mut app).len() == 1
    });

    let beta_arg = beta.to_string_lossy().into_owned();
    let args = ["--renderer", "software", beta_arg.as_str()];
    let child = Child::launch(&exe, &args, &env, &app.profile);
    let left = child.wait_for(|| child.exited().is_some(), Duration::from_secs(5));
    ctx.ok(
        left && child.exited() == Some(0),
        "the second instance exits with code 0 within 5 s",
    );
    let added = ctx.until("its folder as a new tab in the first", 15_000, &mut || {
        tabs_of(&mut app).len() == 2
    });
    ctx.ok(added, "and its folder is a new tab in the first");

    let again = Child::launch(&exe, &args, &env, &app.profile);
    again.wait_for(|| again.exited().is_some(), Duration::from_secs(5));
    let twice = ctx.until(
        "a folder some tab already holds still gets its own tab",
        15_000,
        &mut || tabs_of(&mut app).len() == 3,
    );
    ctx.ok(
        twice,
        "a folder some tab already holds still gets its own tab",
    );
    app.close();
    Ok(())
}

/// Ten live shells, quit, three times: the process exits by itself, quickly, with 0 and no stray is
/// left. PT: run.mjs:3419-3450 (#127: node-pty's exit threads raced, and the prebuild asserted). A
/// fresh strip per round, or the last round's ten tabs would come back.
pub fn quit_many_shells(ctx: &mut Ctx) -> Result<(), String> {
    let (alpha, profile, root) = {
        let w = ctx.ensure_world();
        (w.alpha.clone(), w.profile.clone(), w.root.clone())
    };
    for round in 1..=3 {
        let _ = fs::remove_file(profile.join("tabs.json"));
        let mut app = ctx.launch(Launch {
            renderer: Some("software"),
            folders: vec![alpha.clone()],
            ..Launch::default()
        })?;
        ctx.until("the first tab", 15_000, &mut || {
            tabs_of(&mut app).len() == 1
        });
        for n in 2..=10usize {
            app.session.key("Ctrl+T")?;
            ctx.until(&format!("tab {n}"), 8_000, &mut || {
                tabs_of(&mut app).len() == n
            });
            let front = front_id(&mut app);
            let _ = wait_for_prompt(
                &mut app,
                &front,
                Shell::PowerShell,
                prompt_timeout(Shell::PowerShell),
            );
        }
        ctx.ok(
            tabs_of(&mut app).len() == 10,
            format!("round {round}: ten shells open"),
        );

        let started = Instant::now();
        let _ = app.session.call(&Request::Quit { force: true });
        let child = &app.child;
        let left = child.wait_for(|| child.exited().is_some(), Duration::from_secs(15));
        let code = app.child.exited();
        ctx.ok(
            left && code == Some(0),
            format!(
                "round {round}: the app quit by itself with exit code 0 ({:?} ms, {code:?})",
                started.elapsed().as_millis()
            ),
        );

        let mut reap = ReapGuard::new(root.clone());
        let strays = reap.reap();
        ctx.ok(
            strays == 0,
            format!("round {round}: no stray process is left ({strays})"),
        );
    }
    Ok(())
}

/// Closing the last tab lands on the start screen; the window stays; the folders are offered; the X
/// really quits, and what was open when it quit comes back. PT: run.mjs:3452-3478. The start screen
/// offers the New row only in a click, so one press opens a shell through it.
pub fn last_tab(ctx: &mut Ctx) -> Result<(), String> {
    let (alpha, beta) = {
        let w = ctx.ensure_world();
        (w.alpha.clone(), w.beta.clone())
    };
    let mut app = ctx.launch(Launch {
        renderer: Some("software"),
        folders: vec![alpha, beta],
        ..Launch::default()
    })?;
    ctx.until("two tabs", 15_000, &mut || tabs_of(&mut app).len() == 2);
    app.session.key("Ctrl+W")?;
    ctx.until("one tab left", 6_000, &mut || tabs_of(&mut app).len() == 1);
    app.session.key("Ctrl+W")?;
    let shown = ctx.until("the start screen", 6_000, &mut || {
        app.session
            .probe_str("start.state")
            .ok()
            .and_then(|v| v["shown"].as_bool())
            == Some(true)
    });
    ctx.ok(shown, "closing the last tab lands on the start screen");
    let quitting = app.session.state().map(|s| s.quitting).unwrap_or(true);
    ctx.ok(!quitting, "and the window stays");
    // DROPPED: "the start screen says which version this is" (run.mjs:3462-3465): the start screen's
    // version line is not probed; the version is the ersion field of the state reply (core.rs).

    let start = app.session.probe_str("start.state")?;
    let places = start["places"].as_u64().unwrap_or(0);
    ctx.ok(
        places == 2,
        format!("and offers the folders you were just in ({places})"),
    );
    let new_at = (
        start["new_centre"]["x"].as_f64().unwrap_or(0.0) as f32,
        start["new_centre"]["y"].as_f64().unwrap_or(0.0) as f32,
    );
    app.session.click(new_at.0, new_at.1)?;
    let opened = ctx.until("one press opening a shell", 15_000, &mut || {
        tabs_of(&mut app).len() == 1
    });
    ctx.ok(opened, "one press opens a shell in one of them");
    // Past the 400 ms save debounce, as PT waits: until the saved strip holds the one tab.
    let tabs_file = ctx.ensure_world().profile.join("tabs.json");
    ctx.until("the one tab to be saved", 10_000, &mut || {
        fs::read_to_string(&tabs_file)
            .map(|raw| parse_tabs(&raw).tabs.len() == 1)
            .unwrap_or(false)
    });

    // The X really quits: no resident process is left holding the lock.
    if let Some((x, y)) = centre_of(&mut app, "close") {
        app.session.click(x, y)?;
    }
    let gone = app
        .child
        .wait_for(|| app.child.exited().is_some(), Duration::from_secs(10));
    ctx.ok(gone, "the X ends the process");

    let mut app = ctx.launch(Launch {
        renderer: Some("software"),
        ..Launch::default()
    })?;
    let back = ctx.until("the tab open when it quit", 20_000, &mut || {
        tabs_of(&mut app).len() == 1
    });
    ctx.ok(back, "and the tab that was open when it quit comes back");
    app.close();
    Ok(())
}

/// `exit` closes the tab it was typed in. PT: run.mjs:8086-8105. Ctrl+W closes a tab from inside a
/// focused shell (owner, 2026-09-19): the terminal must yield the chord, or nothing closes.
pub fn exit_closes_tab(ctx: &mut Ctx) -> Result<(), String> {
    let (alpha, beta) = {
        let w = ctx.ensure_world();
        (w.alpha.clone(), w.beta.clone())
    };
    let mut app = ctx.launch(Launch {
        renderer: Some("software"),
        folders: vec![alpha, beta],
        ..Launch::default()
    })?;
    ctx.until("two tabs", 15_000, &mut || tabs_of(&mut app).len() == 2);
    app.session.key("Ctrl+T")?;
    ctx.until("a third tab", 15_000, &mut || tabs_of(&mut app).len() == 3);
    let front = front_id(&mut app);
    let _ = wait_for_prompt(
        &mut app,
        &front,
        Shell::PowerShell,
        prompt_timeout(Shell::PowerShell),
    );
    type_line(&mut app, "echo third")?;
    app.session.key("Ctrl+W")?;
    let closed = ctx.until(
        "Ctrl+W closes the tab over a focused shell",
        6_000,
        &mut || tabs_of(&mut app).len() == 2,
    );
    ctx.ok(closed, "Ctrl+W closes the tab over a focused shell");

    if let Some((x, y)) = centre_of(&mut app, "tab:1") {
        app.session.click(x, y)?;
    }
    type_line(&mut app, "exit")?;
    let ended = ctx.until("the shell ending takes its tab", 15_000, &mut || {
        tabs_of(&mut app).len() == 1
    });
    ctx.ok(ended, "the shell ending takes its tab");
    let tabs = tabs_of(&mut app);
    ctx.ok(
        tabs.first().is_some_and(|t| folder_is(&t.cwd, "alpha")),
        "and the other tab comes to the front",
    );
    app.close();
    Ok(())
}

/// A prompt survives the window getting narrower and wider again (ConPTY sends nothing on a
/// resize). PT: run.mjs:8107-8128.
pub fn prompt_layout(ctx: &mut Ctx) -> Result<(), String> {
    let alpha = ctx.ensure_world().alpha.clone();
    let mut app = ctx.launch(Launch {
        renderer: Some("software"),
        folders: vec![alpha],
        ..Launch::default()
    })?;
    ctx.until("the first tab", 15_000, &mut || {
        tabs_of(&mut app).len() == 1
    });
    let front = front_id(&mut app);
    let _ = wait_for_prompt(
        &mut app,
        &front,
        Shell::PowerShell,
        prompt_timeout(Shell::PowerShell),
    );
    type_line(&mut app, "echo ready")?;
    ctx.until("the echo", 15_000, &mut || {
        rows_text(&mut app, &front).contains("ready")
    });

    let start_w = grid_width(&mut app).unwrap_or(0.0);
    app.session.call(&Request::Resize { w: 620.0, h: 600.0 })?;
    ctx.until("the grid to narrow", 10_000, &mut || {
        grid_width(&mut app).is_some_and(|w| w < start_w.min(700.0))
    });
    let narrow_w = grid_width(&mut app).unwrap_or(0.0);
    app.session.call(&Request::Resize {
        w: 1200.0,
        h: 600.0,
    })?;
    ctx.until("the grid to widen again", 10_000, &mut || {
        grid_width(&mut app).is_some_and(|w| w > narrow_w + 100.0)
    });

    let last = rows_text(&mut app, &front)
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or_default()
        .trim_end()
        .to_string();
    ctx.ok(
        last.starts_with("PS ") && last.ends_with("alpha>"),
        format!("the prompt is whole after a narrow-then-wide resize (\"{last}\")"),
    );
    type_line(&mut app, "echo after-$(1+1)")?;
    let typed = ctx.until("the echo after the resize", 15_000, &mut || {
        rows_text(&mut app, &front).contains("after-2")
    });
    ctx.ok(typed, "and typing lands where the prompt is");
    app.close();
    Ok(())
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
    fn a_folder_is_the_last_segment_of_its_path() {
        assert!(folder_is(r"C:\Users\me\alpha", "alpha"));
        assert!(folder_is(r"C:\Users\me\ALPHA\", "alpha"));
        assert!(!folder_is(r"C:\Users\me\alphabet", "alpha"));
        assert!(!folder_is("", "alpha"));
    }

    #[test]
    fn write_tabs_saves_a_strip_the_app_reads_back_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let folders = vec![PathBuf::from(r"C:\w\alpha"), PathBuf::from(r"C:\w\beta")];
        write_tabs(dir.path(), &folders).unwrap();
        let raw = std::fs::read_to_string(dir.path().join("tabs.json")).unwrap();
        let saved = parse_tabs(&raw);
        assert_eq!(saved.tabs.len(), 2);
        assert_eq!(saved.tabs[0].cwd, r"C:\w\alpha");
        assert_eq!(saved.tabs[1].cwd, r"C:\w\beta");
    }

    #[test]
    fn the_dpi_steps_give_the_scales_the_plan_names() {
        assert_eq!(dpi_scale(96), 1.0);
        assert_eq!(dpi_scale(120), 1.25);
        assert_eq!(dpi_scale(288), 3.0);
    }
}

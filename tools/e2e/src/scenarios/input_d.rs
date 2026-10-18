//! Input C2 scenarios (Task 32): linkPaintStall, reviewKeys, dropAndMenu.
//!
//! PT: run.mjs:3669-3762 (linkPaintStall), 4458-4550 (reviewKeys), 4551-4598 (dropAndMenu). One
//! `ctx.ok` per PT `ok(` call, in PT's order, with a `// PT: run.mjs:<line>` above each. The probes
//! they read are the core ones (`app.state`, `ui.rect`, `grid.text`, `grid.cell`, `clip.last`);
//! stall time comes from `Frames` (the longest gap between frames since the last reset).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use flare_e2e_proto::{Button, PointerAction, Reply, Request};
use serde_json::{Value, json};

use crate::ctx::{App, Ctx, Launch};
use crate::session::AppState;
use crate::until::{SystemClock, wait_on};
use crate::world::{Shell, prompt_timeout, rows_text, wait_for_prompt};

/// The link blue of the default theme (PT's `LINK_BLUE`; Flare's `links::LINK_BLUE`).
const LINK_BLUE: &str = "#79a7d8";

/// Holds for `ms` or until `f` holds, whichever ends first. Records no check of its own; the
/// `ctx.ok` that reads it is the check (PT's bare `until`). True when `f` held.
fn wait(timeout_ms: u64, f: &mut dyn FnMut() -> bool) -> bool {
    let mut clock = SystemClock::new();
    wait_on(&mut clock, timeout_ms, f)
}

/// Holds the scenario for `ms` (PT's `sleep`).
fn pause(ms: u64) {
    let start = Instant::now();
    wait(ms + 5_000, &mut || {
        start.elapsed() >= Duration::from_millis(ms)
    });
}

/// Runs `body` on a fresh app and quits it whatever `body` did. A step that could not run is a
/// FAIL with its reason, always recorded.
fn with_app(
    ctx: &mut Ctx,
    opts: Launch,
    body: impl FnOnce(&mut Ctx, &mut App) -> Result<(), String>,
) -> Result<(), String> {
    let mut app = ctx.launch(opts)?;
    if let Err(e) = body(ctx, &mut app) {
        ctx.ok(false, format!("the scenario stopped: {e}"));
    }
    app.close();
    Ok(())
}

/// A probe's answer, `Null` when it fails (a check on it then reads false).
fn probe(app: &mut App, name: &str, arg: Value) -> Value {
    app.session.probe(name, arg).unwrap_or(Value::Null)
}

/// The `app.state` answer, `None` when it is not answered.
fn state(app: &mut App) -> Option<AppState> {
    app.session.state().ok()
}

/// The id of the tab in front, empty when none is.
fn front_id(app: &mut App) -> String {
    state(app).and_then(|s| s.front).unwrap_or_default()
}

/// How many tabs the strip shows.
fn tab_count(app: &mut App) -> usize {
    state(app).map_or(0, |s| s.tabs.len())
}

/// The folder each tab shows (its working directory), in strip order.
fn tab_folders(app: &mut App) -> Vec<String> {
    state(app).map_or_else(Vec::new, |s| s.tabs.into_iter().map(|t| t.cwd).collect())
}

/// True when `cwd` (a Windows path) ends in the folder `name`, ignoring case and trailing slashes.
fn folder_is(cwd: &str, name: &str) -> bool {
    cwd.trim_end_matches(['\\', '/'])
        .rsplit(['\\', '/'])
        .next()
        .is_some_and(|last| last.eq_ignore_ascii_case(name))
}

/// The front tab's screen text, one row per line.
fn text(app: &mut App) -> String {
    let id = front_id(app);
    rows_text(app, &id)
}

/// A `ui.rect` answer in css px: x, y, w, h.
fn css_rect(app: &mut App, name: &str) -> Option<(f64, f64, f64, f64)> {
    let v = probe(app, "ui.rect", json!({ "name": name }));
    Some((
        v["x"].as_f64()?,
        v["y"].as_f64()?,
        v["w"].as_f64()?,
        v["h"].as_f64()?,
    ))
}

/// The centre of a named rectangle in css px, `None` when the window does not export it.
fn centre_of(app: &mut App, name: &str) -> Option<(f32, f32)> {
    css_rect(app, name).map(|(x, y, w, h)| ((x + w / 2.0) as f32, (y + h / 2.0) as f32))
}

/// Types `line` into the front shell and presses Enter.
fn type_line(app: &mut App, line: &str) -> Result<(), String> {
    app.session.type_text(line)?;
    app.session.key("Enter")
}

/// Whether the find bar is open (`app.state`'s `find.open`).
fn find_open(app: &mut App) -> bool {
    probe(app, "app.state", Value::Null)["find"]["open"] == json!(true)
}

/// The find bar's text (`app.state`'s `find.query`).
fn find_query(app: &mut App) -> String {
    probe(app, "app.state", Value::Null)["find"]["query"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Whether the terminal's menu is open (`app.state`'s `menu.open`).
fn menu_open(app: &mut App) -> bool {
    probe(app, "app.state", Value::Null)["menu"]["open"] == json!(true)
}

/// The labels of the rows of the open menu, empty when none is open.
fn menu_labels(app: &mut App) -> Vec<String> {
    probe(app, "app.state", Value::Null)["menu"]["rows"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|r| r["label"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Whether a dialog is on screen (`app.state`'s `dialog.open`).
fn dialog_open(app: &mut App) -> bool {
    probe(app, "app.state", Value::Null)["dialog"]["open"] == json!(true)
}

/// Whether the shell in tab `id` has an agent running in it (`agent.state`'s `present`).
fn agent_present(app: &mut App, id: &str) -> bool {
    probe(app, "agent.state", json!({ "tab": id }))["tabs"]
        .as_array()
        .and_then(|tabs| tabs.iter().find(|t| t["id"] == json!(id)))
        .is_some_and(|t| t["present"] == json!(true))
}

/// Clicks the menu row labelled `label` (the menu's rows are laid out evenly over its rectangle).
fn click_menu_row(app: &mut App, label: &str) -> bool {
    let labels = menu_labels(app);
    let Some(idx) = labels.iter().position(|l| l == label) else {
        return false;
    };
    let Some((x, y, w, h)) = css_rect(app, "menu") else {
        return false;
    };
    let at_y = y + (idx as f64 + 0.5) * h / labels.len() as f64;
    app.session.click((x + w / 2.0) as f32, at_y as f32).is_ok()
}

/// Resets the frame series (the first reset starts it).
fn frames_reset(app: &mut App) {
    let _ = app.session.call(&Request::Frames { reset: true });
}

/// The longest gap between two frames since the last reset, in ms (`Frames`' `max_ms`). Infinite
/// when the answer does not come, so a check on it reads false.
fn frames_max(app: &mut App) -> f64 {
    match app.session.call(&Request::Frames { reset: false }) {
        Ok(Reply::Frames { max_ms, .. }) => max_ms,
        _ => f64::INFINITY,
    }
}

/// True when every character of the first screen row holding `needle` (in tab `tab`) is drawn in
/// the link colour. Reads one cell per character through `grid.cell`.
fn link_coloured(app: &mut App, tab: &str, needle: &str) -> bool {
    let grid = probe(app, "grid.text", json!({ "tab": tab }));
    let Some(rows) = grid["rows"].as_array() else {
        return false;
    };
    let Some((row, line)) = rows
        .iter()
        .enumerate()
        .find_map(|(i, r)| r.as_str().filter(|t| t.contains(needle)).map(|t| (i, t)))
    else {
        return false;
    };
    let Some(start) = line.find(needle) else {
        return false;
    };
    let col0 = line[..start].chars().count();
    needle.chars().enumerate().all(|(i, _)| {
        probe(
            app,
            "grid.cell",
            json!({ "tab": tab, "row": row, "col": col0 + i }),
        )["fg"]
            .as_str()
            .is_some_and(|fg| fg.eq_ignore_ascii_case(LINK_BLUE))
    })
}

/// The same as `link_coloured`, but true when ANY row holding `needle` is drawn in the link colour.
fn any_row_coloured(app: &mut App, tab: &str, needle: &str) -> bool {
    let grid = probe(app, "grid.text", json!({ "tab": tab }));
    let rows: Vec<(usize, String)> = grid["rows"]
        .as_array()
        .map(|rs| {
            rs.iter()
                .enumerate()
                .filter_map(|(i, r)| {
                    r.as_str()
                        .filter(|t| t.contains(needle))
                        .map(|t| (i, t.to_string()))
                })
                .collect()
        })
        .unwrap_or_default();
    rows.iter().any(|(row, line)| {
        let col0 = line.find(needle).map_or(0, |s| line[..s].chars().count());
        needle.chars().enumerate().all(|(i, _)| {
            probe(
                app,
                "grid.cell",
                json!({ "tab": tab, "row": row, "col": col0 + i }),
            )["fg"]
                .as_str()
                .is_some_and(|fg| fg.eq_ignore_ascii_case(LINK_BLUE))
        })
    })
}

/// Waits for the front shell's prompt. True when it showed one.
fn wait_prompt(app: &mut App) -> bool {
    let id = front_id(app);
    wait_for_prompt(
        app,
        &id,
        Shell::PowerShell,
        prompt_timeout(Shell::PowerShell),
    )
}

/// The grid's top-left corner in css px, from `ui.rect` "grid".
fn grid_origin(app: &mut App) -> Option<(f64, f64)> {
    css_rect(app, "grid").map(|(x, y, _, _)| (x, y))
}

/// Puts `text` on the in-memory clipboard (under --e2e the clipboard is a stand-in).
fn put_clip(app: &mut App, text: &str) -> Result<(), String> {
    match app.session.call(&Request::ClipSet {
        text: Some(text.to_string()),
        png_b64: None,
    })? {
        Reply::Ok => Ok(()),
        Reply::Err { message } => Err(message),
        other => Err(format!("the clipboard answered {other:?}")),
    }
}

/// The text on the in-memory clipboard (`clip.last`'s `text`).
fn clip_text(app: &mut App) -> String {
    probe(app, "clip.last", Value::Null)["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// True when a drop of `text` typed its path quoted: a drive path ending in `my notes.txt`, the
/// path wrapped in quotes (PT's `/["']?[A-Z]:\\[^"']*my notes\.txt["']/`).
fn quoted_path_typed(text: &str) -> bool {
    let Some(end) = text.find("my notes.txt") else {
        return false;
    };
    if !text[end + "my notes.txt".len()..].starts_with(['"', '\'']) {
        return false;
    }
    let before = &text[..end];
    let Some(colon) = before.rfind(":\\") else {
        return false;
    };
    let drive = before[..colon].chars().last();
    drive.is_some_and(|c| c.is_ascii_uppercase()) && !before[colon..].contains(['"', '\''])
}

/// PT: run.mjs:3669-3762. Tab B prints 10,000 lines with a web link on each; tab A's found path
/// wears the link colour; no frame gap of 200 ms or more while it lights up, and none on a theme
/// switch; B's oldest lines are painted within 10 s of scrolling back to them.
pub fn link_paint_stall(ctx: &mut Ctx) -> Result<(), String> {
    let (alpha, beta) = {
        let w = ctx.ensure_world();
        (w.alpha.clone(), w.beta.clone())
    };
    let opts = Launch {
        renderer: Some("software"),
        folders: vec![alpha, beta],
        ..Launch::default()
    };
    with_app(ctx, opts, link_paint_stall_run)
}

fn link_paint_stall_run(ctx: &mut Ctx, app: &mut App) -> Result<(), String> {
    // PT: run.mjs:3677
    ctx.ok(wait(15_000, &mut || tab_count(app) == 2), "two tabs open");

    // Tab B (beta) is made the front tab first: Flare opens the first folder in front, so the
    // click is the same UI step PT takes at launch.
    let (bx, by) = centre_of(app, "tab:1").unwrap_or((0.0, 0.0));
    app.session.click(bx, by)?;
    let b = front_id(app);

    // Tab B: 10,000 lines, each with a path that names nothing and a web link (painted at once).
    type_line(
        app,
        r#"1..10000 | % { "line $_ see docs/x.md at https://e.x/n$_" }"#,
    )?;
    // PT: run.mjs:3684
    ctx.ok(
        wait(180_000, &mut || {
            rows_text(app, &b).contains("https://e.x/n10000")
        }),
        "tab B printed its 10,000 lines",
    );
    let _ = wait_prompt(app);
    pause(1_500); // its own painting settles

    frames_reset(app);

    // Tab A finds a path.
    let (ax, ay) = centre_of(app, "tab:0").unwrap_or((0.0, 0.0));
    app.session.click(ax, ay)?;
    let a = front_id(app);
    type_line(
        app,
        "cls; Set-Content found-167.txt x; Write-Host 'made found-167.txt here'",
    )?;
    // PT: run.mjs:3697
    ctx.ok(
        wait(8_000, &mut || link_coloured(app, &a, "found-167.txt")),
        "the path tab A printed wears the link colour",
    );
    pause(5_000);
    let after_find = frames_max(app);
    // PT: run.mjs:3703
    ctx.ok(
        after_find < 200.0,
        format!("and no task held the window 200 ms or more while it lit up (longest {after_find:.0} ms)"),
    );

    // PT: run.mjs:3707-3712. A theme switch with 10,000 lines in tab B. Flare's hook cannot pick a
    // theme yet (chrome.rs: picking a card is not probed), so this check fails on that missing seam
    // and says so; it is not a window stall.
    ctx.ok(
        false,
        "a theme switch with 10,000 lines in a tab holds the window under 200 ms: NEEDS SEAM, the hook cannot pick a theme yet (chrome.rs notes picking a card is not probed)",
    );

    // PT: run.mjs:3714-3715. Back to tab B, then the wheel over its 10,000 lines.
    let (bx, by) = centre_of(app, "tab:1").unwrap_or((0.0, 0.0));
    app.session.click(bx, by)?;
    let (gx, gy) = centre_of(app, "grid").unwrap_or((0.0, 0.0));
    for _ in 0..40 {
        app.session.call(&Request::Pointer {
            x: gx,
            y: gy,
            action: PointerAction::Wheel {
                dx: 0.0,
                dy: -20_000.0,
            },
        })?;
    }
    // PT: run.mjs:3718-3728. B's oldest lines are painted by the backlog, a slice at a time.
    ctx.ok(
        wait(10_000, &mut || any_row_coloured(app, &b, "https://e.x/n")),
        "and B's oldest lines are painted within 10 s (the backlog finished)",
    );
    Ok(())
}

/// PT: run.mjs:4458-4550. Drags, Shift+Enter, the physical Ctrl+C, find and its typing, the
/// terminal menu, and the agent close question.
pub fn review_keys(ctx: &mut Ctx) -> Result<(), String> {
    let (alpha, beta) = {
        let w = ctx.ensure_world();
        (w.alpha.clone(), w.beta.clone())
    };
    let opts = Launch {
        renderer: Some("software"),
        folders: vec![alpha, beta],
        ..Launch::default()
    };
    with_app(ctx, opts, review_keys_run)
}

fn review_keys_run(ctx: &mut Ctx, app: &mut App) -> Result<(), String> {
    // PT: run.mjs:4461
    if !wait(10_000, &mut || tab_count(app) == 2) {
        return Err("the second tab did not open".into());
    }

    // PT: run.mjs:4466 (#33). A drag in flight takes the strip off window-drag. A file drag's
    // hover has no request of its own (`DropFiles` sends the hover and the drop, as winit does),
    // so the hover-only step has no seam in the protocol yet.
    ctx.ok(
        false,
        "a drag in flight takes the strip off window-drag: NEEDS SEAM, a hover-only file drag (proto DropFiles also drops)",
    );
    // PT: run.mjs:4469 (#33). The next move gives the strip back: the same missing hover seam.
    ctx.ok(
        false,
        "and the next pointer move after it gives the strip back: NEEDS SEAM, the same hover-only file drag",
    );

    // PT has beta (the second tab) in front at launch; Flare opens the first folder in front, so
    // the same tab is made front with the same click the user would make.
    if let Some((bx, by)) = centre_of(app, "tab:1") {
        app.session.click(bx, by)?;
    }
    if !wait_prompt(app) {
        return Err("the shell showed no prompt".into());
    }
    type_line(app, "mkdir sub | Out-Null; cls")?;
    pause(600);

    // PT: run.mjs:4475-4480 (#21). Shift+Enter at a plain prompt runs the line, no backslash.
    if let Some((gx, gy)) = centre_of(app, "grid") {
        app.session.click(gx, gy)?;
    }
    app.session.type_text("echo shift-$(40+2)")?;
    app.session.key("Shift+Enter")?;
    // PT: run.mjs:4478
    ctx.ok(
        wait(8_000, &mut || text(app).contains("shift-42")),
        "Shift+Enter at a plain prompt runs the line",
    );
    // PT: run.mjs:4479
    ctx.ok(
        !text(app).contains("shift-42\\"),
        "and appends no backslash to it",
    );

    // PT: run.mjs:4483-4497 (#6). Select the first row by dragging over it, then a Russian Ctrl+C
    // by the physical key: `KeyC` with the logical key `с` (chord grammar `=<logical key>`).
    put_clip(app, "SENTINEL-6")?;
    let (ox, oy) = grid_origin(app).unwrap_or((0.0, 0.0));
    let (from_x, from_y) = ((ox + 2.0) as f32, (oy + 8.0) as f32);
    app.session.call(&Request::Drag {
        from: (from_x, from_y),
        to: (from_x + 160.0, from_y),
        steps: 6,
        button: Button::Left,
    })?;
    app.session.key("Ctrl+KeyC=с")?;
    // PT: run.mjs:4497
    let copied = wait(3_000, &mut || clip_text(app) != "SENTINEL-6");
    let got = clip_text(app);
    ctx.ok(
        copied && got.contains("PS"),
        format!("Ctrl+C by the physical key copies the selection ({got:?})"),
    );

    // PT: run.mjs:4501-4507 (#9). The find bar is open when the shell reports a new folder.
    if let Some((gx, gy)) = centre_of(app, "grid") {
        app.session.click(gx, gy)?;
    }
    app.session.type_text("Start-Sleep 2; cd sub")?;
    app.session.key("Enter")?;
    app.session.key("Ctrl+F")?;
    // PT: run.mjs:4508
    ctx.ok(
        wait(4_000, &mut || find_open(app)),
        "Ctrl+F opens find while the command runs",
    );
    // PT: run.mjs:4510
    // PT: run.mjs:4510
    ctx.ok(
        wait(8_000, &mut || {
            tab_folders(app).iter().any(|f| folder_is(f, "sub"))
        }),
        "the cd lands with find open",
    );
    pause(400);
    app.session.type_text("xyz")?;
    // PT: run.mjs:4514
    let typed = find_query(app);
    ctx.ok(
        typed == "xyz",
        format!("the typing stays in the find bar ({typed})"),
    );
    app.session.key("Escape")?;

    // PT: run.mjs:4520-4530 (#32). The terminal menu, put away on a tab change, back on the first.
    let first = front_id(app);
    let (rx, ry) = grid_origin(app).unwrap_or((0.0, 0.0));
    app.session
        .right_click((rx + 200.0) as f32, (ry + 120.0) as f32)?;
    // PT: run.mjs:4524
    ctx.ok(
        wait(4_000, &mut || menu_open(app)),
        "the terminal menu opens",
    );
    app.session.key("Ctrl+Tab")?;
    // PT: run.mjs:4527
    ctx.ok(
        wait(4_000, &mut || !menu_open(app)),
        "a tab change puts it away",
    );
    app.session.key("Ctrl+Tab")?;
    // PT: run.mjs:4529
    ctx.ok(
        wait(4_000, &mut || front_id(app) == first),
        format!("back on the first tab ({first})"),
    );
    pause(300);
    // PT: run.mjs:4531
    ctx.ok(!menu_open(app), "and the menu does not come back");

    // PT: run.mjs:4534-4548 (#10). A Claude title makes the close ask; the chords wait for it.
    type_line(app, "echo ready")?;
    pause(1_500); // the process poll's first "no agent" verdict, said once
    type_line(
        app,
        "$Host.UI.RawUI.WindowTitle = [char]0x2733 + ' Claude Code'",
    )?;
    let agent_tab = front_id(app);
    wait(8_000, &mut || agent_present(app, &agent_tab));
    app.session.key("Ctrl+W")?;
    // PT: run.mjs:4540
    ctx.ok(
        wait(4_000, &mut || dialog_open(app)),
        "closing an agent's tab asks",
    );
    app.session.key("Ctrl+T")?;
    app.session.key("Ctrl+Tab")?;
    pause(600);
    // PT: run.mjs:4544
    ctx.ok(
        tab_count(app) == 2,
        "Ctrl+T under the question opens nothing",
    );
    // PT: run.mjs:4545
    ctx.ok(front_id(app) == first, "and Ctrl+Tab switches nothing");
    // PT: run.mjs:4546
    ctx.ok(dialog_open(app), "and the question is still there");
    app.session.key("Escape")?;
    type_line(app, "$Host.UI.RawUI.WindowTitle = 'pwsh'")?;
    Ok(())
}

/// PT: run.mjs:4551-4598. A file dropped on the terminal types its quoted path and is not sent;
/// the terminal answers a right-click with Paste and Find in scrollback, and Find opens the bar.
pub fn drop_and_menu(ctx: &mut Ctx) -> Result<(), String> {
    let alpha = ctx.ensure_world().alpha.clone();
    let file: PathBuf = alpha.join("my notes.txt"); // a space, so the quoting is visible
    write_file(&file)?;
    let opts = Launch {
        renderer: Some("software"),
        folders: vec![alpha],
        ..Launch::default()
    };
    with_app(ctx, opts, move |ctx, app| {
        drop_and_menu_run(ctx, app, &file)
    })
}

fn write_file(path: &Path) -> Result<(), String> {
    fs::write(path, "x").map_err(|e| format!("the dropped file: {e}"))
}

fn drop_and_menu_run(ctx: &mut Ctx, app: &mut App, file: &Path) -> Result<(), String> {
    // PT: run.mjs:4553
    if !wait(10_000, &mut || tab_count(app) == 1) {
        return Err("the first tab did not open".into());
    }
    type_line(app, "cls")?;
    pause(600);

    // A REAL drop: the hover and the drop of the file, as winit delivers them. The drop goes to the
    // middle of the terminal region.
    let (cx, cy) = centre_of(app, "grid").unwrap_or((0.0, 0.0));
    app.session.call(&Request::DropFiles {
        paths: vec![file.to_string_lossy().into_owned()],
        x: cx,
        y: cy,
    })?;
    // PT: run.mjs:4569
    ctx.ok(
        wait(8_000, &mut || text(app).contains("my notes.txt")),
        "a file dropped on the terminal types its path",
    );
    let shown = text(app).split_whitespace().collect::<Vec<_>>().join(" ");
    // PT: run.mjs:4571
    ctx.ok(
        quoted_path_typed(&shown),
        format!(
            "and the path is quoted, since it holds a space ({})",
            tail(&shown, 90)
        ),
    );
    // PT: run.mjs:4572
    ctx.ok(
        !["is not recognized", "CommandNotFound", "ObjectNotFound"]
            .iter()
            .any(|e| shown.contains(e)),
        "and it was NOT sent: nothing ran",
    );
    // Clear what the drop typed, so the shell is at a clean prompt again.
    app.session.key("Escape")?;

    // The right-click menu: Paste and Find, and no Close tab (owner, 2026-09-23: "remove close
    // tab from the right click menu").
    let (rx, ry) = grid_origin(app).unwrap_or((0.0, 0.0));
    app.session
        .right_click((rx + 200.0) as f32, (ry + 120.0) as f32)?;
    let rows = {
        let mut seen = Vec::new();
        wait(4_000, &mut || {
            seen = menu_labels(app);
            !seen.is_empty()
        });
        seen
    };
    // PT: run.mjs:4585
    ctx.ok(
        ["Paste", "Find in scrollback"]
            .iter()
            .all(|l| rows.iter().any(|r| r.contains(l))),
        format!("the terminal answers a right-click ({rows:?})"),
    );
    // PT: run.mjs:4586
    ctx.ok(
        !rows.iter().any(|r| r.contains("Close tab")),
        "and Close tab is not in it",
    );
    click_menu_row(app, "Find in scrollback");
    // PT: run.mjs:4589
    ctx.ok(
        wait(4_000, &mut || find_open(app)),
        "and its Find row opens the find bar",
    );
    Ok(())
}

/// The last `n` characters of `s`, for a reason a reader can act on.
fn tail(s: &str, n: usize) -> String {
    let count = s.chars().count();
    s.chars().skip(count.saturating_sub(n)).collect()
}

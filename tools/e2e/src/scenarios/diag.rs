//! The diagnostics log scenario (Task 38): diagLog. PT: run.mjs:5231-5375.
//!
//! The log is `<profile>\logs\diag.jsonl`, read with M9's reader (`flare_diag::reader::read_lines`).
//! The stall lines come from the native watchdog (M9 D12): PT's `page-stall` and `page-stack` are
//! Flare's `ui-stack` (the UI thread held past the 2000 ms stack gap, once per stall), PT's
//! `ipc-slow` is `work-slow` (a named work item past 500 ms), and PT's `page-error` is the `panic`
//! line a worker thread leaves. PT's `page-rejection` has no Rust form (no promises), so it is
//! DROPPED. PT's "crumbs said before it" is ported to the `inflight` field M9 gives `ui-stack`.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use flare_diag::reader::{read_lines, Line};
use flare_e2e_proto::Request;
use serde_json::{json, Value};

use crate::ctx::{App, Ctx, Launch};
use crate::world::{prompt_timeout, wait_for_prompt, Shell};

/// The log folder of a profile (`<profile>\logs`; the Mac world's, `world::logs_dir`).
fn log_dir(profile: &Path) -> PathBuf {
    crate::world::logs_dir(profile)
}

/// Every line the log holds now, in file order.
fn lines(profile: &Path) -> Vec<Line> {
    read_lines(&log_dir(profile))
}

/// A probe's answer (`Null` when the call fails), as the other scenarios take it.
fn probe(app: &mut App, name: &str, arg: Value) -> Value {
    app.session.probe(name, arg).unwrap_or(Value::Null)
}

/// A string field of a log line.
fn text<'a>(l: &'a Line, name: &str) -> Option<&'a str> {
    l.raw.get(name)?.as_str()
}

/// True when the line's kind (`k`) is `kind`.
fn is_kind(l: &Line, kind: &str) -> bool {
    text(l, "k") == Some(kind)
}

/// A whole-number field of a log line (0 when absent).
fn num(l: &Line, name: &str) -> u64 {
    l.raw.get(name).and_then(Value::as_u64).unwrap_or(0)
}

/// The wall clock in milliseconds since the epoch (the log's own clock).
fn epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// Waits (through the one wait) until a line of the log matches `want`; the first such line, or
/// None after `timeout_ms`.
fn await_line(
    ctx: &mut Ctx,
    profile: &Path,
    what: &str,
    timeout_ms: u64,
    want: impl Fn(&Line) -> bool,
) -> Option<Line> {
    let mut found = None;
    let hit = ctx.until(what, timeout_ms, &mut || {
        found = lines(profile).into_iter().find(|l| want(l));
        found.is_some()
    });
    if hit {
        found
    } else {
        None
    }
}

/// How many `tab-open` crumbs from the app the log holds now.
fn tab_open_crumbs(profile: &Path) -> usize {
    lines(profile)
        .iter()
        .filter(|l| {
            is_kind(l, "crumb") && text(l, "a") == Some("tab-open") && text(l, "src") == Some("app")
        })
        .count()
}

/// Holds for `ms` without a FAIL (the wait is the one clock; PT's `sleep`).
fn pause(ctx: &mut Ctx, ms: u64) {
    let start = Instant::now();
    ctx.until("a pause to pass", ms + 5_000, &mut || {
        start.elapsed() >= Duration::from_millis(ms)
    });
}

/// The number of tabs the app shows now.
fn tab_count(app: &mut App) -> usize {
    app.session.state().map(|s| s.tabs.len()).unwrap_or(0)
}

/// The ids of the tabs, in strip order.
fn tab_ids(app: &mut App) -> Vec<String> {
    app.session
        .state()
        .map(|s| s.tabs.into_iter().map(|t| t.id).collect())
        .unwrap_or_default()
}

/// A rectangle value (`x`, `y`, `w`, `h`) as a tuple.
fn boxed(v: &Value) -> Option<(f64, f64, f64, f64)> {
    Some((
        v["x"].as_f64()?,
        v["y"].as_f64()?,
        v["w"].as_f64()?,
        v["h"].as_f64()?,
    ))
}

/// The Settings probe's `front` flag.
fn settings_front(app: &mut App) -> bool {
    probe(app, "settings.state", Value::Null)["front"] == json!(true)
}

/// The page the Settings pane is drawing (`None` when there is none).
fn page_of(app: &mut App) -> Option<String> {
    probe(app, "settings.rows", json!({}))["page"]
        .as_str()
        .map(str::to_string)
}

/// Opens Settings from the cog and waits for it to be in front.
fn open_settings(ctx: &mut Ctx, app: &mut App) -> bool {
    if settings_front(app) {
        return true;
    }
    if let Some((x, y, w, h)) = boxed(&probe(app, "ui.rect", json!({ "name": "cog" }))) {
        let _ = app
            .session
            .click((x + w / 2.0) as f32, (y + h / 2.0) as f32);
    }
    ctx.until("Settings is in front", 8_000, &mut || settings_front(app))
}

/// Opens the Diagnostics page: Settings in front, then its rail item, then the page drawn.
fn open_diagnostics(ctx: &mut Ctx, app: &mut App) -> bool {
    if !open_settings(ctx, app) {
        return false;
    }
    if page_of(app).as_deref() != Some("diagnostics") {
        if let Some((x, y, w, h)) =
            boxed(&probe(app, "settings.rail", json!({ "id": "diagnostics" })))
        {
            let _ = app
                .session
                .click((x + w / 2.0) as f32, (y + h / 2.0) as f32);
        }
    }
    ctx.until("the Diagnostics page is drawn", 6_000, &mut || {
        page_of(app).as_deref() == Some("diagnostics")
    })
}

/// The `settings.row` answer for row `id`.
fn row(app: &mut App, id: &str) -> Value {
    probe(app, "settings.row", json!({ "id": id }))
}

/// Whether row `id` is drawn in the pane now.
fn row_visible(app: &mut App, id: &str) -> bool {
    row(app, id)["visible"] == json!(true)
}

/// The on or off state of the switch row `id` (`None` when it is not drawn).
fn row_on(app: &mut App, id: &str) -> Option<bool> {
    row(app, id)["on"].as_bool()
}

/// The subtext of a settings row (the log folder, for Log files), `""` when there is none.
fn row_sub(app: &mut App, id: &str) -> String {
    probe(app, "settings.rows", json!({}))["rows"]
        .as_array()
        .and_then(|rs| rs.iter().find(|r| r["id"] == json!(id)))
        .and_then(|r| r["sub"].as_str())
        .unwrap_or("")
        .to_string()
}

/// The word a row's first button shows now (`Mark`, or `Marked` while the mark is fresh).
fn button_word(app: &mut App, id: &str) -> String {
    probe(app, "diag.buttons", json!({ "id": id }))["buttons"][0]["word"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

/// Clicks the switch of row `id`: 16 px padding and the 18 px switch in from the row's right end.
fn click_switch(app: &mut App, id: &str) -> bool {
    let Some((x, y, w, h)) = boxed(&row(app, id)["rect"]) else {
        return false;
    };
    app.session
        .click((x + w - 16.0 - 18.0) as f32, (y + h / 2.0) as f32)
        .is_ok()
}

/// Clicks the button of row `id`: buttons sit 16 px in from the row's right end, so a point 30 px
/// in lands on a button wider than 14 px.
fn click_button(app: &mut App, id: &str) -> bool {
    let Some((x, y, w, h)) = boxed(&row(app, id)["rect"]) else {
        return false;
    };
    app.session
        .click((x + w - 30.0) as f32, (y + h / 2.0) as f32)
        .is_ok()
}

/// Whether the open-folder recorder holds a folder call for `dir` (trailing separators ignored).
fn opened_folder(app: &mut App, dir: &str) -> bool {
    let want = dir.trim_end_matches(['\\', '/']);
    probe(app, "open.log", Value::Null)["calls"]
        .as_array()
        .is_some_and(|calls| {
            calls.iter().any(|c| {
                c["kind"] == json!("folder")
                    && c["path"]
                        .as_str()
                        .is_some_and(|p| p.trim_end_matches(['\\', '/']).eq_ignore_ascii_case(want))
            })
        })
}

/// The lines of every file in the log folder that are not one JSON object (the count).
fn bad_json_lines(profile: &Path) -> usize {
    let Ok(entries) = fs::read_dir(log_dir(profile)) else {
        return usize::MAX;
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|e| fs::read_to_string(e.path()).ok())
        .flat_map(|text| {
            text.lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| serde_json::from_str::<Value>(l).is_ok())
                .collect::<Vec<_>>()
        })
        .filter(|ok| !ok)
        .count()
}

/// `diagLog`: the session line, the crumbs, the quiet level, the stalls, the errors, the page,
/// Mark, detailed logging, and what survives a relaunch.
pub fn diag_log(ctx: &mut Ctx) -> Result<(), String> {
    let (profile, alpha) = {
        let w = ctx.ensure_world();
        (w.profile.clone(), w.alpha.clone())
    };
    let mut app = ctx.launch(Launch {
        folders: vec![alpha.clone()],
        renderer: Some("software"),
        ..Launch::default()
    })?;
    let first = first_session(ctx, &mut app, &profile);
    // PT: run.mjs closeApp. The quit line is what closing writes last.
    app.close();
    first?;

    // PT: run.mjs:5366 a quit is the session's last line.
    let last = lines(&profile)
        .last()
        .and_then(|l| text(l, "k").map(str::to_string))
        .unwrap_or_default();
    ctx.ok(
        last == "quit",
        format!("a quit is the session's last line ({last})"),
    );

    let mut app = ctx.launch(Launch {
        folders: vec![alpha],
        renderer: Some("software"),
        ..Launch::default()
    })?;
    let second = second_session(ctx, &mut app, &profile);
    app.close();
    second
}

/// The first run: every check from the session line to the last call logged. PT: run.mjs:5245-5349.
fn first_session(ctx: &mut Ctx, app: &mut App, profile: &Path) -> Result<(), String> {
    // PT: run.mjs:5245-5247. The session line opens the file.
    let session = await_line(ctx, profile, "the session line", 15_000, |l| {
        is_kind(l, "session")
    });
    let session_ok = session.as_ref().is_some_and(|s| {
        text(s, "src") == Some("main")
            && s.raw.get("e2e") == Some(&json!(true))
            && s.raw.get("verbose") == Some(&json!(false))
            && text(s, "version").is_some()
            && num(s, "pid") > 0
    });
    ctx.ok(
        session_ok,
        format!(
            "a session line opens the log (version {}, verbose {})",
            session
                .as_ref()
                .and_then(|s| text(s, "version"))
                .unwrap_or("none"),
            session.as_ref().map_or("none".to_string(), |s| s
                .raw
                .get("verbose")
                .map_or("none".to_string(), Value::to_string))
        ),
    );

    // PT: run.mjs:5248. The first tab's shell is a crumb, from the app (the launch's folder tab).
    let shell = await_line(ctx, profile, "the first tab's shell crumb", 15_000, |l| {
        is_kind(l, "crumb") && text(l, "a") == Some("shell-spawn") && text(l, "src") == Some("app")
    });
    ctx.ok(
        shell.is_some(),
        "the first tab's shell is a crumb, from the app",
    );

    // PT: run.mjs:5249-5252. Ctrl+T opens a second tab, and the tab that opened is a crumb.
    let one = ctx.until("one tab", 15_000, &mut || tab_count(app) == 1);
    if one {
        for id in tab_ids(app) {
            wait_for_prompt(
                app,
                &id,
                Shell::PowerShell,
                prompt_timeout(Shell::PowerShell),
            );
        }
    }
    // The launch's own folder tab is a tab-open too, so the crumb must be a NEW one.
    let opens_before = tab_open_crumbs(profile);
    app.session.key("Ctrl+T")?;
    let two = ctx.until("two tabs", 15_000, &mut || tab_count(app) == 2);
    ctx.ok(two, "Ctrl+T opens a second tab");
    let opened = ctx.until("the tab-open crumb of Ctrl+T", 10_000, &mut || {
        tab_open_crumbs(profile) > opens_before
    });
    ctx.ok(opened, "and the tab that opened is a crumb, from the app");

    // PT: run.mjs:5256-5264 QUIET IS QUIET: once both shells are at their prompts, an idle 3 s
    // writes nothing.
    for id in tab_ids(app) {
        wait_for_prompt(
            app,
            &id,
            Shell::PowerShell,
            prompt_timeout(Shell::PowerShell),
        );
    }
    pause(ctx, 1_000);
    let from = lines(profile).len();
    pause(ctx, 3_000);
    let idle_lines: Vec<_> = lines(profile).into_iter().skip(from).collect();
    let idle: Vec<String> = idle_lines
        .iter()
        .map(|l| text(l, "k").unwrap_or("").to_string())
        .collect();
    // The lines themselves too, so a lag says how long and what ran (the Mac runner wrote two main-lag lines).
    let whole: Vec<String> = idle_lines
        .iter()
        .map(|l| {
            Value::Object(l.raw.clone())
                .to_string()
                .chars()
                .take(300)
                .collect()
        })
        .collect();
    ctx.ok(
        idle.is_empty(),
        format!("an idle 3 s at the quiet level writes nothing ({idle:?}; {whole:?})"),
    );

    // PT: run.mjs:5266-5277 A STALL: the UI thread is held for 2.6 s. The watchdog writes one
    // ui-stack line once the thread has been silent 2000 ms, and the beat that returns is a
    // main-lag of the same length.
    let before = lines(profile).len();
    let blocked = app.session.call(&Request::BlockUi { ms: 2_600 });
    let stall = await_line(ctx, profile, "the ui-stack line", 12_000, |l| {
        is_kind(l, "ui-stack") && num(l, "ms") >= 2_000
    });
    let lag = await_line(ctx, profile, "the main-lag line", 12_000, |l| {
        is_kind(l, "main-lag") && num(l, "ms") >= 2_000
    });
    let stall_ms = stall.as_ref().map_or(0, |s| num(s, "ms"));
    let lag_ms = lag.as_ref().map_or(0, |l| num(l, "ms"));
    ctx.ok(
        blocked.is_ok() && stall.is_some() && lag.is_some(),
        format!("the busy UI thread is a ui-stack of {stall_ms} ms, and the beat that returns a main-lag of {lag_ms} ms"),
    );
    // PT: run.mjs:5278-5279. The stall names the thread's frames (the script that ran, in PT).
    let frames = stall
        .as_ref()
        .and_then(|s| text(s, "stack"))
        .unwrap_or("")
        .trim()
        .lines()
        .count();
    ctx.ok(
        frames > 0,
        format!("with the frames of the UI thread named ({frames} lines)"),
    );
    // PT: run.mjs:5280-5281. PT's "crumbs said before it" is the in-flight work M9 gives the line.
    ctx.ok(
        stall
            .as_ref()
            .is_some_and(|s| s.raw.get("inflight").is_some_and(Value::is_array)),
        "and the work in flight at the time is on the line (inflight)",
    );
    // PT: run.mjs:5282-5283. Main took the stack once for this stall (page-stack, once).
    let stacks = lines(profile)
        .iter()
        .skip(before)
        .filter(|l| is_kind(l, "ui-stack") && num(l, "ms") >= 2_000)
        .count();
    ctx.ok(
        stacks == 1,
        format!("one ui-stack line for the stall, at 2000 ms or more ({stacks})"),
    );

    // PT: run.mjs:5285-5287 A SLOW CALL: a named work item that takes 600 ms.
    let slow_reply = app.session.call(&Request::Slow {
        name: "e2e-slow".into(),
        ms: 600,
    });
    ctx.ok(slow_reply.is_ok(), "the slow call answers");
    let slow = await_line(ctx, profile, "the work-slow line", 10_000, |l| {
        is_kind(l, "work-slow") && text(l, "name") == Some("e2e-slow")
    });
    ctx.ok(
        slow.as_ref().is_some_and(|s| num(s, "ms") >= 500),
        format!(
            "and is a work-slow line naming the item ({} ms)",
            slow.as_ref().map_or(0, |s| num(s, "ms"))
        ),
    );

    // PT: run.mjs:5289-5296 ERRORS: a worker thread panics; the panic line has its message and stack.
    let panicked = app.session.call(&Request::PanicWorker);
    let panic_line = await_line(ctx, profile, "the panic line", 10_000, |l| {
        // Audit L12 (A14): the line keeps the payload's length and hash, never its text.
        is_kind(l, "panic")
            && text(l, "msg").is_some_and(|m| m.contains("panic payload withheld ("))
    });
    ctx.ok(
        panicked.is_ok()
            && panic_line.as_ref().is_some_and(|p| {
                text(p, "msg").is_some_and(|m| m.contains("17 bytes, hash "))
                    && !text(p, "msg").is_some_and(|m| m.contains("e2e: worker panic"))
                    && text(p, "stack").is_some_and(|s| !s.trim().is_empty())
            }),
        "a worker panic is a panic line, with its length, hash and stack but not its text",
    );
    // DROPPED: a rejected promise is a page-rejection (PT run.mjs:5297) because Rust has no
    // promises; the worker panic above is the one error no handler holds.

    // THE PAGE: Settings > Diagnostics. PT: run.mjs:5300-5303.
    let page = open_diagnostics(ctx, app);
    let crumb = await_line(ctx, profile, "the settings-page crumb", 10_000, |l| {
        is_kind(l, "crumb")
            && text(l, "a") == Some("settings-page")
            && text(l, "page") == Some("diagnostics")
    });
    ctx.ok(page && crumb.is_some(), "opening the page is a crumb");
    let logs = log_dir(profile).to_string_lossy().into_owned();
    let shown = ctx.until("the log folder on the Log files row", 5_000, &mut || {
        row_sub(app, "diag-folder").contains(&logs)
    });
    ctx.ok(shown, "Log files shows the folder the log is in");
    click_button(app, "diag-folder");
    let folder = ctx.until("the folder open call", 4_000, &mut || {
        opened_folder(app, &logs)
    });
    ctx.ok(
        folder,
        "Open folder opens that folder (recorded under --e2e)",
    );
    // DROPPED: PT's check that Mark's word is centred in its button (run.mjs:5317-5322). Flare
    // lays the button out in Slint and no probe reads the word's own box, so there is no
    // measurement to make; it is an open item for the parity sweep.

    // Mark: the word changes and the moment is stamped.
    let mark_at = epoch_ms();
    click_button(app, "diag-mark");
    let marked = ctx.until("Mark says Marked", 2_000, &mut || {
        button_word(app, "diag-mark") == "Marked"
    });
    ctx.ok(marked, "Mark says Marked");
    let mark = await_line(ctx, profile, "the mark line", 5_000, |l| is_kind(l, "mark"));
    let stamped = mark
        .as_ref()
        .is_some_and(|m| text(m, "src") == Some("app") && (m.ms - mark_at).abs() < 3_000);
    ctx.ok(
        stamped,
        format!(
            "and stamps the moment in the log (off by {} ms)",
            mark.as_ref().map_or(-1, |m| m.ms - mark_at)
        ),
    );

    // Detailed logging: off by default, a line when on, and every call logged from then on.
    let shown_off = ctx.until("the Detailed logging row", 5_000, &mut || {
        row_visible(app, "diag-verbose")
    });
    let default_off = shown_off && row_on(app, "diag-verbose") == Some(false);
    ctx.ok(default_off, "Detailed logging is off by default");
    click_switch(app, "diag-verbose");
    let verbose = await_line(ctx, profile, "the verbose line", 5_000, |l| {
        is_kind(l, "verbose") && l.raw.get("on") == Some(&json!(true))
    });
    ctx.ok(verbose.is_some(), "switching it on is a line");
    // PT: run.mjs:5343-5344. Every call is logged: a timed work item writes a `work` line now.
    let _ = app.session.call(&Request::Slow {
        name: "e2e-call".into(),
        ms: 0,
    });
    let every = await_line(ctx, profile, "a work line for the call", 5_000, |l| {
        is_kind(l, "work") && text(l, "name") == Some("e2e-call")
    });
    ctx.ok(every.is_some(), "and every call is logged from then on");
    Ok(())
}

/// The second run: the detailed level is kept, the switch says so, and every line is JSON.
/// PT: run.mjs:5353-5370.
fn second_session(ctx: &mut Ctx, app: &mut App, profile: &Path) -> Result<(), String> {
    let sessions = ctx.until("a second session line", 15_000, &mut || {
        lines(profile)
            .iter()
            .filter(|l| is_kind(l, "session"))
            .count()
            >= 2
    });
    let second = lines(profile)
        .into_iter()
        .filter(|l| is_kind(l, "session"))
        .nth(1);
    let verbose_kept = second
        .as_ref()
        .is_some_and(|s| s.raw.get("verbose") == Some(&json!(true)));
    ctx.ok(
        sessions && verbose_kept,
        format!(
            "the next session starts detailed ({})",
            second
                .as_ref()
                .and_then(|s| s.raw.get("verbose"))
                .map_or("none".to_string(), Value::to_string)
        ),
    );

    let page = open_diagnostics(ctx, app);
    let kept = ctx.until("the switch reads on", 5_000, &mut || {
        page && row_visible(app, "diag-verbose") && row_on(app, "diag-verbose") == Some(true)
    });
    ctx.ok(kept, "and the switch says so");

    let bad = bad_json_lines(profile);
    ctx.ok(
        bad == 0,
        format!("every line in the file is one JSON object ({bad} not)"),
    );
    Ok(())
}

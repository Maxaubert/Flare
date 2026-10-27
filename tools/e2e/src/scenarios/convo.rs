//! The conversation panel (Surfaces stage 6): `convoPanels`.
//!
//! The built release exe runs in its own world and profile with `CLAUDE_CONFIG_DIR` pointing at a folder of the
//! scenario's own, so it never reads the owner's `~/.claude`. A shell stands in for Claude: it prints the hook line
//! Claude's plugin sends (`OSC 777;flare-agent;state=working;session=<uuid>`), and the scenario writes a synthetic
//! transcript for that session. The scenario then plays the agent on the control channel (`panel.open` with kind
//! `changes`, `convo.state`) and the person (a click on a file row), and reads what the window shows through the
//! `convo.state` probe. At 100 and 225 percent.
//!
//! Held: nothing is read while no panel is open (no tracker); Changes shows the edited file and, after a click, its
//! diff as removed and added rows (and builds only the rows in view); a line appended to the transcript updates it;
//! `panel.open` kind `tasks` is refused (the Tasks panel was removed 2026-10-26); a new session switches the file;
//! closing the panel stops the tracker.
//!
//! Nothing here touches the user's windows, pipes, Claude settings or real transcripts.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use flare_ctl::{connect, Client, ClientError, HelloParams, Method, PipeEnd};
use flare_e2e_proto::Request;
use serde_json::{json, Value};

use super::input_b::{front_tab, probe, prompt, type_line};
use crate::ctx::{App, Ctx, Launch};

const WAIT: Duration = Duration::from_secs(10);
const SESSION_A: &str = "3f2a9c1e-77aa-4c0d-9b1e-0a1b2c3d4e5f";
const SESSION_B: &str = "11111111-2222-4333-8444-555555555555";

fn edit_line(id: &str, path: &str, old: &str, new: &str) -> String {
    json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": id, "name": "Edit",
        "input": {"file_path": path, "old_string": old, "new_string": new}}]}})
    .to_string()
}

/// A TodoWrite call: the reader ignores it (the Tasks panel that showed it was removed, 2026-10-26).
fn todo_line() -> String {
    json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "todo",
        "name": "TodoWrite", "input": {"todos": [
            {"content": "Change it", "status": "in_progress", "activeForm": "Changing it"}]}}]}})
    .to_string()
}

/// The PowerShell that prints the hook line of a finished Claude turn (a working one would ask to be closed) in `session`.
fn say_session(session: &str) -> String {
    format!(
        "[Console]::Write([char]27 + ']777;flare-agent;state=done;session={session}' + [char]7)"
    )
}

/// One command line that stands in for a running Claude for the whole scenario: it names session A, waits until the
/// scenario creates `flag`, names session B, and then stays busy. The shell never returns to its prompt, because a
/// prompt in front of the shell takes the agent (and its session) back at once, as it does for a real Claude that
/// has exited.
fn stand_in(flag: &Path) -> String {
    let flag = flag.to_string_lossy().replace('\'', "''");
    format!(
        "{}; while (-not (Test-Path -LiteralPath '{flag}')) {{ Start-Sleep -Milliseconds 100 }}; {}; Start-Sleep -Seconds 600",
        say_session(SESSION_A),
        say_session(SESSION_B)
    )
}

fn append(path: &Path, lines: &[String]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("make {}: {e}", dir.display()))?;
    }
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("open {}: {e}", path.display()))?;
    for l in lines {
        writeln!(f, "{l}").map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn state(app: &mut App) -> Value {
    probe(app, "convo.state", Value::Null)
}

fn names(v: &Value) -> Vec<String> {
    v["files"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|f| {
                    format!(
                        "{}|{}",
                        f["name"].as_str().unwrap_or_default(),
                        f["meta"].as_str().unwrap_or_default()
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

fn diff_has(v: &Value, kind: i64, text: &str) -> bool {
    v["diff"].as_array().is_some_and(|rows| {
        rows.iter()
            .any(|r| r["kind"].as_i64() == Some(kind) && r["text"] == json!(text))
    })
}

/// The pipe and the front tab's secret, from the `ctl.info` probe.
fn channel(app: &mut App) -> Option<(String, String)> {
    let v = probe(app, "ctl.info", Value::Null);
    let pipe = v["pipe"].as_str()?.to_string();
    let token = v["tabs"]
        .as_array()?
        .iter()
        .find(|t| t["active"] == json!(true))?["token"]
        .as_str()?
        .to_string();
    Some((pipe, token))
}

fn hello(pipe: &str, token: &str) -> Result<Client<PipeEnd>, ClientError> {
    let mut c = connect(pipe, WAIT)?;
    c.hello(
        &HelloParams {
            client: "flare-e2e".into(),
            version: "1".into(),
            token: None,
            tab_token: Some(token.to_string()),
            no_events: false,
        },
        WAIT,
    )?;
    Ok(c)
}

pub fn convo_panels(ctx: &mut Ctx) -> Result<(), String> {
    let (root, alpha) = {
        let w = ctx.ensure_world();
        (w.root.clone(), w.alpha.clone())
    };
    for (dpi, label) in [(96u32, "100%"), (216, "225%")] {
        let cfg = root.join(format!("claude-cfg-{dpi}"));
        fs::create_dir_all(&cfg).map_err(|e| e.to_string())?;
        let mut app = ctx.launch(Launch {
            renderer: Some("gpu"),
            folders: vec![alpha.clone()],
            scale_dpi: Some(dpi),
            env: vec![(
                "CLAUDE_CONFIG_DIR".into(),
                cfg.to_string_lossy().into_owned(),
            )],
            ..Launch::default()
        })?;
        let r = convo_on(ctx, &mut app, dpi, label, &cfg, &alpha);
        let left = app.close();
        ctx.ok(left, format!("{label}: the app quits by itself"));
        r?;
    }
    Ok(())
}

/// Where the transcript of `session` is for the tab folder `alpha` under `cfg`.
fn transcript(cfg: &Path, alpha: &Path, session: &str) -> PathBuf {
    let slug = flare_agent::resume::encode_cwd(&alpha.to_string_lossy());
    cfg.join("projects")
        .join(slug)
        .join(format!("{session}.jsonl"))
}

fn convo_on(
    ctx: &mut Ctx,
    app: &mut App,
    dpi: u32,
    at: &str,
    cfg: &Path,
    alpha: &Path,
) -> Result<(), String> {
    let scale = f64::from(dpi) / 96.0;
    app.session.call(&Request::Resize {
        w: 1000.0,
        h: 640.0,
    })?;
    let sized = ctx.until("the window to be 1000 css px wide", 5_000, &mut || {
        probe(app, "dpi.state", Value::Null)["client_w_px"].as_f64()
            == Some((1000.0 * scale).round())
    });
    ctx.ok(sized, format!("{at}: the window is 1000 css px wide"));
    prompt(app)?;
    let file = alpha.join("main.rs").to_string_lossy().into_owned();
    let main_log = transcript(cfg, alpha, SESSION_A);
    append(
        &main_log,
        &[
            edit_line(
                "e1",
                &file,
                "fn old() {}\nlet keep = 1;",
                "fn fresh() {}\nlet keep = 1;",
            ),
            todo_line(),
        ],
    )?;

    // ---- nothing runs while no panel is open, even with a session and a transcript ----
    let s0 = state(app);
    ctx.ok(
        s0["trackers"] == json!(0) && s0["kind"] != json!(4),
        format!("{at}: no panel, no transcript tracker ({})", s0["trackers"]),
    );
    // The process poll has said its first word (a hook that lands before it can be taken back by it).
    let tab = front_tab(app);
    crate::world::wait_for_agent_verdict(app, &tab, Duration::from_secs(30));
    let flag = cfg.join("switch-session.flag");
    let _ = fs::remove_file(&flag);
    type_line(app, &stand_in(&flag))?;
    let heard = ctx.until("the hook to name the session", 15_000, &mut || {
        state(app)["front_session"] == json!(SESSION_A)
    });
    ctx.ok(
        heard,
        format!("{at}: the hook names the tab's Claude session"),
    );
    // Held for a while: a tracker that started late would show in any sample.
    let start = std::time::Instant::now();
    let mut started = false;
    ctx.until("a pause to pass", 5_000, &mut || {
        started |= state(app)["trackers"] != json!(0);
        start.elapsed() >= Duration::from_millis(1_500)
    });
    ctx.ok(
        !started,
        format!("{at}: a known session alone starts no tracker"),
    );

    // ---- the agent opens its own Changes panel on the channel ----
    let (pipe, token) = channel(app).ok_or("ctl.info has no front tab")?;
    let mut c = hello(&pipe, &token).map_err(|e| format!("hello: {e}"))?;
    let opened = c.call(Method::PanelOpen, json!({"kind": "changes"}), WAIT);
    ctx.ok(
        opened.as_ref().is_ok_and(|v| {
            v["panel"]["kind"] == json!("changes") && v["panel"]["title"] == json!("Changes")
        }),
        format!("{at}: panel.open kind changes answers with the panel ({opened:?})"),
    );
    let listed = ctx.until("Changes to list the edited file", 15_000, &mut || {
        let s = state(app);
        s["kind"] == json!(4) && s["shown"] == json!(true) && names(&s) == ["main.rs|1 edit"]
    });
    ctx.ok(
        listed,
        format!("{at}: Changes shows main.rs with its edit count"),
    );
    let s = state(app);
    ctx.ok(
        s["trackers"] == json!(1) && s["sub"] == json!("1 file") && s["title"] == json!("Changes"),
        format!("{at}: one tracker, the header says 1 file"),
    );
    ctx.shot(app, &format!("convo-changes-{dpi}"));

    // ---- convo.state on the channel: the agent reads its own tab's summary ----
    let st = c.call(Method::ConvoState, json!({}), WAIT);
    ctx.ok(
        st.as_ref().is_ok_and(|v| {
            v["tracking"] == json!(true)
                && v["session"] == json!(SESSION_A)
                && v["files"] == json!(1)
                && v["edits"] == json!(1)
                && v.get("tasks").is_none()
        }),
        format!("{at}: convo.state gives the file counts and no task counts ({st:?})"),
    );

    // ---- a click on the row shows the diff ----
    let rect = probe(app, "ui.rect", json!({"name": "convo-file-0"}));
    let (x, y) = (
        rect["x"].as_f64().unwrap_or(-1.0) + rect["w"].as_f64().unwrap_or(0.0) / 2.0,
        rect["y"].as_f64().unwrap_or(-1.0) + rect["h"].as_f64().unwrap_or(0.0) / 2.0,
    );
    let _ = app.session.click(x as f32, y as f32);
    let diffed = ctx.until("the diff of main.rs", 10_000, &mut || {
        let s = state(app);
        s["selected"] == json!(file)
            && diff_has(&s, 2, "fn old() {}")
            && diff_has(&s, 3, "fn fresh() {}")
    });
    ctx.ok(
        diffed,
        format!("{at}: a click on the file shows its edit as a removed and an added row"),
    );
    let s = state(app);
    ctx.ok(
        diff_has(&s, 1, "let keep = 1;"),
        format!("{at}: the unchanged line is context"),
    );
    ctx.ok(
        s["diff_built"].as_i64().is_some_and(|n| n > 0 && n < 80),
        format!(
            "{at}: only the rows in view are built ({})",
            s["diff_built"]
        ),
    );
    ctx.shot(app, &format!("convo-diff-{dpi}"));
    // The agent edits again: the diff grows without a reload.
    let before = s["diff_rows"].as_i64().unwrap_or(0);
    append(
        &main_log,
        &[edit_line("e2", &file, "let keep = 1;", "let keep = 2;")],
    )?;
    let grew = ctx.until("the second edit in the diff", 15_000, &mut || {
        let s = state(app);
        s["diff_rows"].as_i64().unwrap_or(0) > before && diff_has(&s, 3, "let keep = 2;")
    });
    ctx.ok(
        grew,
        format!("{at}: a line appended to the transcript extends the diff"),
    );
    let counted = ctx.until("two edits in the list", 5_000, &mut || {
        names(&state(app)) == ["main.rs|2 edits"]
    });
    ctx.ok(counted, format!("{at}: the list says 2 edits"));

    // ---- the Tasks panel was removed (2026-10-26): its kind is refused and the Changes panel stays ----
    let t = c.call(Method::PanelOpen, json!({"kind": "tasks"}), WAIT);
    ctx.ok(
        matches!(&t, Err(ClientError::Remote(e)) if e.code == flare_ctl::code::BAD_PARAMS),
        format!("{at}: panel.open kind tasks is refused ({t:?})"),
    );
    let s = state(app);
    ctx.ok(
        s["kind"] == json!(4) && s["trackers"] == json!(1),
        format!("{at}: the refusal leaves the Changes panel and its one tracker"),
    );

    // ---- a new session switches the file ----
    let other = alpha.join("other.rs").to_string_lossy().into_owned();
    append(
        &transcript(cfg, alpha, SESSION_B),
        &[edit_line("n1", &other, "a", "b")],
    )?;
    let _ = c.call(Method::PanelOpen, json!({"kind": "changes"}), WAIT);
    fs::write(&flag, "go").map_err(|e| e.to_string())?;
    let switched = ctx.until("the new session's file", 15_000, &mut || {
        let s = state(app);
        s["front_session"] == json!(SESSION_B)
            && names(&s) == ["other.rs|1 edit"]
            && s["sessions"] == json!([SESSION_B])
    });
    ctx.ok(
        switched,
        format!("{at}: a new session switches to its own transcript"),
    );
    ctx.ok(
        state(app)["trackers"] == json!(1),
        format!("{at}: still one tracker"),
    );

    // ---- closing the panel stops the tracker ----
    let closed = c.call(Method::PanelClose, json!({}), WAIT);
    ctx.ok(closed.is_ok(), format!("{at}: panel.close answers"));
    let stopped = ctx.until("the tracker to stop", 10_000, &mut || {
        let s = state(app);
        s["trackers"] == json!(0) && s["kind"] != json!(4) && names(&s).is_empty()
    });
    ctx.ok(
        stopped,
        format!("{at}: closing the panel stops the tracker and lets the rows go"),
    );
    let again = c.call(Method::ConvoState, json!({}), WAIT);
    ctx.ok(
        again.as_ref().is_ok_and(|v| v["tracking"] == json!(false)),
        format!("{at}: convo.state says it is not reading ({again:?})"),
    );
    Ok(())
}

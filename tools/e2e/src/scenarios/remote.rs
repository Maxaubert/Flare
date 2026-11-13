//! Remote sessions PR 2: `remoteLoopback`, the host end of a remote link against a live host Flare.
//!
//! The host is the built exe in its own world and profile (offscreen, as every launch here). The scenario plays the
//! viewing Flare: it starts `flare --serve-remote` with the host's profile the way ssh would start it, its stdin and
//! stdout piped to an in-process `StdioWire` (a loopback, no ssh and no other machine), and speaks `remote.*` over it.
//! The serve process relays to the host's pipe with the bridge key the host wrote into its profile.
//!
//! Steps: open a tab in the world's folder, attach, wait for its prompt in the polled output, type a command whose full
//! text this file wrote, read its output, resize, detach, attach again and poll on from the last `to` (no byte twice),
//! read a snapshot, close the tab, and close the link (the serve process ends by itself). Every wait is for an answer or
//! for bytes, never a fixed count (rule 259).

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use base64::Engine as _;
use flare_ctl::{Client, ClientError, Method, RemoteHelloParams, StdioWire};
use flare_e2e_proto::Request;
use serde_json::{json, Value};

use super::input_b::{prompt, type_line};
use crate::ctx::{App, Ctx, Launch};
use crate::standins::agent_proc;

const WAIT: Duration = Duration::from_secs(20);
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// The command typed over the link. Written here in full: nothing else is ever sent to the host's shell.
const MARK: &str = "remote-loop-7";

fn serve(exe: &Path, profile: &Path, env: &[(String, String)]) -> Result<Child, String> {
    let mut cmd = Command::new(exe);
    cmd.arg("--serve-remote")
        .arg(format!("--user-data-dir={}", profile.display()))
        .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd.spawn()
        .map_err(|e| format!("start the serve process: {e}"))
}

fn call(c: &mut Client<StdioWire>, m: Method, p: Value) -> Result<Value, String> {
    c.call(m, p, WAIT).map_err(|e| format!("{}: {e}", m.name()))
}

fn bytes_of(v: &Value) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(v["data"].as_str().unwrap_or_default())
        .unwrap_or_default()
}

/// Polls `remote.output` from `since` until `want` says the text so far is enough, or the time is up. Returns the text
/// read and the `to` of the last poll.
fn poll_until(
    ctx: &mut Ctx,
    c: &mut Client<StdioWire>,
    tab: &str,
    since: u64,
    what: &str,
    want: impl Fn(&str) -> bool,
) -> Result<(String, u64), String> {
    let mut text = String::new();
    let mut at = since;
    let mut err = None;
    let ok = ctx.until(what, 30_000, &mut || {
        match c.call(Method::RemoteOutput, json!({"tab": tab, "since": at}), WAIT) {
            Ok(v) => {
                text.push_str(&String::from_utf8_lossy(&bytes_of(&v)));
                at = v["to"].as_u64().unwrap_or(at);
            }
            Err(e) => err = Some(e.to_string()),
        }
        err.is_some() || want(&text)
    });
    if let Some(e) = err {
        return Err(format!("remote.output: {e}"));
    }
    if !ok {
        return Err(format!("{what}: not seen in the polled output"));
    }
    Ok((text, at))
}

pub fn remote_loopback(ctx: &mut Ctx) -> Result<(), String> {
    let (alpha, env) = {
        let w = ctx.ensure_world();
        (w.alpha.clone(), w.env())
    };
    let mut app = ctx.launch(Launch {
        folders: vec![alpha.clone()],
        ..Launch::default()
    })?;
    let r = loopback_on(ctx, &mut app, &alpha, &env);
    let left = app.close();
    ctx.ok(left, "the host quits by itself after the link closed");
    r
}

fn loopback_on(
    ctx: &mut Ctx,
    app: &mut App,
    alpha: &Path,
    env: &[(String, String)],
) -> Result<(), String> {
    prompt(app)?;
    let mut child = serve(&ctx.exe.clone(), &app.profile, env)?;
    let wire = StdioWire::from_child(
        child.stdin.take().ok_or("the serve process has no stdin")?,
        child
            .stdout
            .take()
            .ok_or("the serve process has no stdout")?,
    )
    .map_err(|e| format!("a wire over the serve process: {e}"))?;
    let mut c = Client::new(wire);
    let hello = c
        .remote_hello(
            &RemoteHelloParams {
                machine: "e2e-viewer".into(),
                flare_version: "e2e".into(),
            },
            WAIT,
        )
        .map_err(|e| format!("remote.hello: {e}"))?;
    ctx.ok(
        hello.capabilities.iter().any(|m| m == "remote.output"),
        "the serve process says remote.hello and lists the remote methods",
    );

    let folder = alpha.to_string_lossy().into_owned();
    let opened = call(&mut c, Method::RemoteOpen, json!({"folder": folder}))?;
    let tab = opened["tab"].as_str().unwrap_or_default().to_string();
    ctx.ok(!tab.is_empty(), "remote.open opens a tab on the host");
    let tabs = call(&mut c, Method::RemoteTabs, json!({}))?;
    ctx.ok(
        tabs["tabs"]
            .as_array()
            .is_some_and(|l| l.iter().any(|t| t["tab"] == tab.as_str())),
        "remote.tabs lists the new tab",
    );
    let at = call(&mut c, Method::RemoteAttach, json!({"tab": tab}))?;
    let seq0 = at["seq"].as_u64().unwrap_or(0);

    // The new shell's prompt arrives in the polled output.
    let (_, after_prompt) = poll_until(ctx, &mut c, &tab, seq0, "the new tab's prompt", |t| {
        t.contains("PS ") || t.contains('>') || t.contains('$')
    })?;
    let typed = call(
        &mut c,
        Method::RemoteInput,
        json!({"tab": tab, "intents": [
            {"text": format!("echo {MARK}")},
            {"key": {"key": "Enter", "code": "Enter"}},
        ]}),
    )?;
    ctx.ok(typed["typed"] == 2, "remote.input types a text and a key");
    // The echo shows twice: the command line, and its output on a line of its own.
    let (text, last_to) = poll_until(
        ctx,
        &mut c,
        &tab,
        after_prompt,
        "the command's output",
        |t| t.matches(MARK).count() >= 2,
    )?;
    ctx.ok(
        text.contains(MARK),
        "the typed command's output comes back over the link",
    );

    let sized = call(
        &mut c,
        Method::RemoteResize,
        json!({"tab": tab, "cols": 70, "rows": 20}),
    )?;
    ctx.ok(
        sized["cols"] == 70 && sized["rows"] == 20,
        "remote.resize gives the host tab the viewer's size",
    );

    let det = call(&mut c, Method::RemoteDetach, json!({"tab": tab}))?;
    ctx.ok(det["detached"] == true, "remote.detach answers");
    let gone = c.call(
        Method::RemoteOutput,
        json!({"tab": tab, "since": last_to}),
        WAIT,
    );
    ctx.ok(
        matches!(&gone, Err(ClientError::Remote(e)) if e.code == "no_tab"),
        "a detached tab is not polled",
    );
    let again = call(&mut c, Method::RemoteAttach, json!({"tab": tab}))?;
    ctx.ok(
        again["seq"].as_u64().is_some_and(|s| s >= last_to),
        "the tab is still there after a detach, its output numbered on",
    );
    // Poll on from the last `to`: nothing read before comes again.
    let more = call(
        &mut c,
        Method::RemoteOutput,
        json!({"tab": tab, "since": last_to}),
    )?;
    let more_text = String::from_utf8_lossy(&bytes_of(&more)).into_owned();
    ctx.ok(
        more["from"].as_u64() == Some(last_to) && more["gap"] == false && !more_text.contains(MARK),
        "polling on from the last `to` gives no byte twice",
    );
    let snap = call(&mut c, Method::RemoteSnapshot, json!({"tab": tab}))?;
    ctx.ok(
        String::from_utf8_lossy(&bytes_of(&snap)).contains(MARK),
        "remote.snapshot holds the screen's text",
    );

    let closed = call(&mut c, Method::RemoteClose, json!({"tab": tab}))?;
    ctx.ok(
        closed["closed"] == true,
        "remote.close closes a plain shell tab at once",
    );
    let after = c.call(Method::RemoteOutput, json!({"tab": tab, "since": 0}), WAIT);
    ctx.ok(
        matches!(&after, Err(ClientError::Remote(e)) if e.code == "no_tab"),
        "a closed tab is followed no more",
    );

    drop(c);
    let mut ended = None;
    ctx.until("the serve process ends with its link", 10_000, &mut || {
        ended = child.try_wait().ok().flatten();
        ended.is_some()
    });
    if ended.is_none() {
        let _ = child.kill();
    }
    ctx.ok(
        ended.is_some_and(|s| s.success()),
        "the serve process ends with exit code 0 when its link closes",
    );
    Ok(())
}

// ---- remote sessions PR 3: `remoteViewer`, a viewing Flare and a host Flare ----

/// What is typed into the remote tab. Written here in full: nothing else is ever sent to the host's shell.
const VIEW_MARK: &str = "remote-view-7";
const VIEW_MARK_2: &str = "remote-view-8";

/// The env of a viewing Flare whose link is a loopback `flare --serve-remote` of the host's profile (never ssh), and,
/// when `open`, the remote tab it opens at start in `folder`.
fn viewer_env(exe: &Path, host: &Path, folder: &Path, open: bool) -> Vec<(String, String)> {
    let cmd = json!([
        exe.to_string_lossy(),
        "--serve-remote",
        format!("--user-data-dir={}", host.display()),
    ]);
    let mut env = vec![(
        concat!("FLARE_E2E", "_REMOTE_COMMAND").to_string(),
        cmd.to_string(),
    )];
    if open {
        env.push((
            concat!("FLARE_E2E", "_REMOTE_OPEN").to_string(),
            json!({"machine": "loop", "folder": folder.to_string_lossy()}).to_string(),
        ));
    }
    env
}

/// The front tab of `app` once its grid holds `want`; the text then.
fn front_rows_until(
    ctx: &mut Ctx,
    app: &mut App,
    what: &str,
    want: impl Fn(&str) -> bool,
) -> Result<String, String> {
    let mut text = String::new();
    let ok = ctx.until(what, 30_000, &mut || {
        text = super::input_b::rows(app);
        want(&text)
    });
    if ok {
        Ok(text)
    } else {
        Err(format!("{what}: not on the viewer's screen: {text:?}"))
    }
}

/// `remoteViewer`: Flare A (the viewer) shows a tab that runs in Flare B (the host), joined by a stdio loopback. A opens
/// it, types into it and sees the output; A quits and starts again, and the restored tab attaches to the same host
/// session with no line twice. Both are offscreen on the hidden desktop, as every launch here.
pub fn remote_viewer(ctx: &mut Ctx) -> Result<(), String> {
    let alpha = ctx.ensure_world().alpha.clone();
    let mut host = ctx.launch(Launch {
        folders: vec![alpha.clone()],
        ..Launch::default()
    })?;
    let r = viewer_on(ctx, &mut host, &alpha);
    let left = host.close();
    ctx.ok(left, "the host quits by itself");
    r
}

fn viewer_on(ctx: &mut Ctx, host: &mut App, alpha: &Path) -> Result<(), String> {
    prompt(host)?;
    let host_tabs = host.session.state()?.tabs.len();
    let world = ctx.new_world();
    let exe = ctx.exe.clone();
    let mut a = ctx.launch_in(
        &world,
        Launch {
            env: viewer_env(&exe, &host.profile, alpha, true),
            ..Launch::default()
        },
    )?;
    let typed = (|| -> Result<(), String> {
        front_rows_until(ctx, &mut a, "the host's prompt in the remote tab", |t| {
            t.contains("PS ")
        })?;
        let mut more = 0;
        ctx.until("the host has the new tab", 20_000, &mut || {
            more = host.session.state().map_or(0, |s| s.tabs.len());
            more > host_tabs
        });
        ctx.ok(
            more == host_tabs + 1,
            "remote.open opened one tab on the host",
        );
        type_line(&mut a, &format!("echo {VIEW_MARK}"))?;
        let t = front_rows_until(ctx, &mut a, "the typed command's output", |t| {
            t.matches(VIEW_MARK).count() >= 2
        })?;
        ctx.ok(
            t.matches(VIEW_MARK).count() == 2,
            "the command and its output show once each on the viewer",
        );
        Ok(())
    })();
    let quit = a.close();
    typed?;
    ctx.ok(quit, "the viewer quits");
    ctx.ok(
        host.session.state().map_or(0, |s| s.tabs.len()) == host_tabs + 1,
        "the viewer's quit leaves the host's session running",
    );

    // The viewer starts again: the restored tab attaches to the same session and shows its screen, no line twice.
    let mut a = ctx.launch_in(
        &world,
        Launch {
            env: viewer_env(&exe, &host.profile, alpha, false),
            ..Launch::default()
        },
    )?;
    let again = (|| -> Result<(), String> {
        let t = front_rows_until(ctx, &mut a, "the restored remote tab", |t| {
            t.contains(VIEW_MARK)
        })?;
        ctx.ok(
            t.matches(VIEW_MARK).count() == 2,
            "after a restart the host's lines show once each",
        );
        ctx.ok(
            host.session.state().map_or(0, |s| s.tabs.len()) == host_tabs + 1,
            "the restore attached and opened no new host tab",
        );
        type_line(&mut a, &format!("echo {VIEW_MARK_2}"))?;
        let t = front_rows_until(ctx, &mut a, "typing after the restore", |t| {
            t.matches(VIEW_MARK_2).count() >= 2
        })?;
        ctx.ok(
            t.matches(VIEW_MARK).count() == 2 && t.matches(VIEW_MARK_2).count() == 2,
            "the restored tab takes typing and no earlier line comes again",
        );
        Ok(())
    })();
    let quit = a.close();
    again?;
    ctx.ok(quit, "the viewer quits again");
    Ok(())
}

// ---- remote sessions PR 5: `remoteOpen`, `flare open --on` and the host's hidden start ----

/// `remoteOpen`: Flare A (the viewer) runs; Flare B (the host) does not. `flare open --on loop <folder>` (the command,
/// run here with no window) asks A for a tab on machine `loop`, which `machines.json` in A's profile names and whose
/// link is a loopback `flare --serve-remote` of B's profile (never ssh). The serve process finds no Flare for B and
/// starts one with `--start-hidden` (and `--e2e`, so it is a test app with its hook), waits for its pipe and answers the
/// hello. The tab shows B's prompt on A; B runs, parked off every screen, not in front and no-activate (rule 301), and
/// holds the session. Both on the hidden desktop: B is A's grandchild and inherits it.
pub fn remote_open(ctx: &mut Ctx) -> Result<(), String> {
    let hw = ctx.new_world();
    let world = ctx.new_world();
    std::fs::create_dir_all(&world.profile).map_err(|e| e.to_string())?;
    std::fs::write(
        world.profile.join("machines.json"),
        json!({"machines": [{"id": "loop", "name": "Loopback", "kind": "pc", "ssh": "loop"}]})
            .to_string(),
    )
    .map_err(|e| e.to_string())?;
    let exe = ctx.exe.clone();
    let cmd = json!([
        exe.to_string_lossy(),
        "--serve-remote",
        format!("--user-data-dir={}", hw.profile.display()),
        "--e2e",
    ]);
    let mut a = ctx.launch_in(
        &world,
        Launch {
            env: vec![(
                concat!("FLARE_E2E", "_REMOTE_COMMAND").to_string(),
                cmd.to_string(),
            )],
            folders: vec![world.alpha.clone()],
            ..Launch::default()
        },
    )?;
    let r = open_on(ctx, &mut a, &exe, &world, &hw);
    let quit = a.close();
    // B was started by the serve process, not by this harness: it is asked to quit, then reaped by its profile.
    if let Ok(mut b) = crate::session::Session::connect(&hw.profile, Duration::from_secs(2)) {
        let _ = b.call(&Request::Quit { force: true });
    }
    let t = std::time::Instant::now();
    ctx.until("the hidden host to quit", 15_000, &mut || {
        t.elapsed() > Duration::from_secs(1)
            && crate::session::Session::connect(&hw.profile, Duration::from_millis(200)).is_err()
    });
    crate::proc::reap_by_profile(&hw.profile);
    ctx.ok(quit, "the viewer quits");
    r
}

fn open_on(
    ctx: &mut Ctx,
    a: &mut App,
    exe: &Path,
    world: &crate::world::World,
    hw: &crate::world::World,
) -> Result<(), String> {
    prompt(a)?;
    ctx.ok(
        crate::session::Session::connect(&hw.profile, Duration::from_millis(500)).is_err(),
        "no Flare runs for the host's profile before the command",
    );
    let folder = hw.alpha.to_string_lossy().into_owned();
    let mut cmd = Command::new(exe);
    cmd.args([
        "open".to_string(),
        "--on".to_string(),
        "loop".to_string(),
        folder.clone(),
        format!("--user-data-dir={}", world.profile.display()),
    ])
    .envs(world.env())
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    let out = cmd.output().map_err(|e| format!("run flare open: {e}"))?;
    let said = String::from_utf8_lossy(&out.stdout).into_owned();
    ctx.ok(
        out.status.success() && said.contains("Opened a tab on loop"),
        format!(
            "flare open --on answers and exits 0 ({:?}: {said:?} {:?})",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        ),
    );
    // The serve process starts the host hidden and waits for it; the tab then shows the host's prompt.
    front_rows_until(ctx, a, "the hidden host's prompt in the remote tab", |t| {
        t.contains("PS ")
    })?;
    let mut b = crate::session::Session::connect(&hw.profile, Duration::from_secs(20))
        .map_err(|e| format!("the hidden host's hook: {e}"))?;
    b.hello()?;
    let park = b.probe_str("window.park")?;
    ctx.ok(
        crate::ctx::park_problems(&park).is_empty(),
        format!("the hidden host's window is parked, no-activate and not in front ({park})"),
    );
    let tabs = b.state()?.tabs.len();
    ctx.ok(
        tabs >= 1,
        format!("the host holds the session ({tabs} tabs)"),
    );
    type_line(a, &format!("echo {OPEN_MARK}"))?;
    let t = front_rows_until(ctx, a, "typing reaches the hidden host", |t| {
        t.matches(OPEN_MARK).count() >= 2
    })?;
    ctx.ok(
        t.matches(OPEN_MARK).count() == 2,
        "the command and its output show once each on the viewer",
    );
    Ok(())
}

/// What is typed into the tab `remoteOpen` opens. Written here in full: nothing else is sent to the host's shell.
const OPEN_MARK: &str = "remote-open-5";

// ---- remote sessions PR 4: `remoteStatus`, the host's agent marks on the viewer ----

/// "Notify me when an agent asks" on (it is off by default), stored in the profile before the launch.
fn notify_on(profile: &Path) -> Result<(), String> {
    std::fs::create_dir_all(profile).map_err(|e| e.to_string())?;
    std::fs::write(
        profile.join("settings.json"),
        json!({"flare.window.notifyAsk": "1"}).to_string(),
    )
    .map_err(|e| e.to_string())
}

/// `[Console]::Write` of an OSC with the payload: the line the stand-in agent runs on the host.
fn osc_line(payload: &str) -> String {
    let p = payload.replace('\'', "''");
    format!("[Console]::Write(([string][char]27)+']{p}'+([string][char]7))")
}

fn hook(state: &str) -> String {
    osc_line(&format!("777;flare-agent;state={state}"))
}

/// The state word of the viewer's or the host's tab `id`: `none`, `working`, `question`, `finished` or `failed`.
fn tab_state(app: &mut App, id: &str) -> String {
    let v = super::input_b::probe(app, "agent.state", Value::Null);
    v["tabs"]
        .as_array()
        .and_then(|t| t.iter().find(|t| t["id"] == id))
        .and_then(|t| t["state"].as_str())
        .unwrap_or("?")
        .to_string()
}

fn toast_count(app: &mut App) -> usize {
    super::input_b::probe(app, "notify.log", Value::Null)["toasts"]
        .as_array()
        .map_or(0, Vec::len)
}

/// Lets `ms` of real time go by (a wait that holds nothing, to see that nothing happens).
fn pass(ctx: &mut Ctx, ms: u64) {
    let t = std::time::Instant::now();
    ctx.until("time to pass", ms + 5_000, &mut || {
        t.elapsed() >= Duration::from_millis(ms)
    });
}

/// `remoteStatus`: Flare B (the host) runs a stand-in agent in the tab Flare A (the viewer) follows, started by what A
/// types. It works, asks and finishes. A shows the work, the question (one notification on A, none on B, rule 295) and
/// its own Finished mark (rule 294); A quits and starts again, attaches, and the finished turn is not raised a second
/// time. Both are offscreen on the hidden desktop, as every launch here.
pub fn remote_status(ctx: &mut Ctx) -> Result<(), String> {
    let hw = ctx.new_world();
    notify_on(&hw.profile)?;
    let mut host = ctx.launch_in(
        &hw,
        Launch {
            folders: vec![hw.alpha.clone()],
            ..Launch::default()
        },
    )?;
    let r = status_on(ctx, &mut host, &hw.alpha);
    let left = host.close();
    ctx.ok(left, "the host quits by itself");
    r
}

fn status_on(ctx: &mut Ctx, host: &mut App, alpha: &Path) -> Result<(), String> {
    prompt(host)?;
    let world = ctx.new_world();
    notify_on(&world.profile)?;
    let exe = ctx.exe.clone();
    let mut a = ctx.launch_in(
        &world,
        Launch {
            env: viewer_env(&exe, &host.profile, alpha, true),
            ..Launch::default()
        },
    )?;
    let marked = (|| -> Result<(), String> {
        front_rows_until(ctx, &mut a, "the host's prompt in the remote tab", |t| {
            t.contains("PS ")
        })?;
        let tab = super::input_b::front_tab(&mut a);
        // The viewer is away (its window does not have the keyboard): a finished turn is news there.
        a.session.call(&Request::Focus { on: false })?;
        // A stand-in agent process on the host, so the host's own poll believes the hooks (review #47).
        type_line(&mut a, &agent_proc::command(&host.profile)?)?;
        pass(ctx, agent_proc::SETTLE_MS);
        type_line(&mut a, &hook("working"))?;
        let w = ctx.until("the viewer shows the work", 20_000, &mut || {
            tab_state(&mut a, &tab) == "working"
        });
        ctx.ok(w, "the host's work shows on the viewer's remote tab");
        type_line(&mut a, &hook("question"))?;
        let q = ctx.until("the viewer shows the question", 20_000, &mut || {
            tab_state(&mut a, &tab) == "question"
        });
        ctx.ok(q, "the host's question shows on the viewer");
        let told = ctx.until("the viewer's notification", 15_000, &mut || {
            toast_count(&mut a) >= 1
        });
        ctx.ok(told, "the viewer announces the question");
        pass(ctx, 2_000);
        ctx.ok(
            toast_count(&mut a) == 1,
            format!(
                "one notification for one question on the viewer ({})",
                toast_count(&mut a)
            ),
        );
        ctx.ok(
            toast_count(host) == 0,
            format!(
                "none on the host for a tab the viewer follows ({})",
                toast_count(host)
            ),
        );
        type_line(&mut a, &hook("done"))?;
        let d = ctx.until("the viewer's finished mark", 20_000, &mut || {
            tab_state(&mut a, &tab) == "finished"
        });
        ctx.ok(d, "the turn's end is a finished mark on the viewer");
        Ok(())
    })();
    let quit = a.close();
    marked?;
    ctx.ok(quit, "the viewer quits");

    // The viewer starts again: the restored tab attaches and hears the host still say `done`; nothing is raised again.
    let mut a = ctx.launch_in(
        &world,
        Launch {
            env: viewer_env(&exe, &host.profile, alpha, false),
            ..Launch::default()
        },
    )?;
    let again = (|| -> Result<(), String> {
        front_rows_until(ctx, &mut a, "the restored remote tab", |t| {
            t.contains("PS ")
        })?;
        let tab = super::input_b::front_tab(&mut a);
        a.session.call(&Request::Focus { on: false })?;
        // Two status polls and more go by.
        pass(ctx, 4_000);
        let s = tab_state(&mut a, &tab);
        ctx.ok(
            s != "finished",
            format!("a reconnect does not raise the finished turn again ({s})"),
        );
        ctx.ok(toast_count(&mut a) == 0, "and announces no old question");
        Ok(())
    })();
    let quit = a.close();
    again?;
    ctx.ok(quit, "the viewer quits again");
    Ok(())
}

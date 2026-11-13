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
use serde_json::{json, Value};

use super::input_b::prompt;
use crate::ctx::{App, Ctx, Launch};

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

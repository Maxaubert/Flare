//! The control channel (Surfaces stage 2): `ctlChannel` and `mcpRoundTrip`.
//!
//! Both start the built release exe in its own world and profile, so its pipe has its own name and
//! never meets the owner's Flare. A scenario plays a shell: it reads a tab's secret through the
//! `ctl.info` probe (a real shell gets it through its environment, which the first scenario also
//! reads from inside a real shell) and talks to the pipe with the `flare-ctl` client.
//!
//! `ctlChannel` holds the channel itself: a tab's secret opens its panel, a wrong secret and a tab
//! name are refused, a path nobody may open is `not_found`, a tab that is behind keeps its place.
//! `mcpRoundTrip` runs the built `flare-mcp.exe` the way Claude Code would (stdio, the tab's
//! environment), calls its tools and reads what the window shows.
//!
//! Nothing here touches the user's windows, pipes or Claude settings: the only pipe connected to is
//! the one the app under test reported about itself.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use flare_ctl::{connect, Client, ClientError, HelloParams, Method, PipeEnd};
use serde_json::{json, Value};

use super::input_b::{front_tab, probe, prompt, rows, type_line};
use crate::ctx::{App, Ctx, Launch};

const WAIT: Duration = Duration::from_secs(10);
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const PIC: &str = "ctl-pic.png";
const NOTE: &str = "ctl-note.txt";
const NOTE_TEXT: &str = "one\ntwo\nthree\n";

fn write_picture(path: &Path) -> Result<(), String> {
    let file = fs::File::create(path).map_err(|e| format!("make the picture: {e}"))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), 64, 32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().map_err(|e| e.to_string())?;
    let px: Vec<u8> = (0..64 * 32).flat_map(|_| [30u8, 160, 90, 255]).collect();
    w.write_image_data(&px).map_err(|e| e.to_string())
}

/// The pipe and every shell tab's `(id, active, token)`, from the `ctl.info` probe.
struct Info {
    pipe: String,
    mcp_exe: Option<String>,
    tabs: Vec<(String, bool, String)>,
}

fn info(app: &mut App) -> Option<Info> {
    let v = probe(app, "ctl.info", Value::Null);
    let tabs = v["tabs"]
        .as_array()?
        .iter()
        .filter_map(|t| {
            Some((
                t["id"].as_str()?.to_string(),
                t["active"].as_bool()?,
                t["token"].as_str()?.to_string(),
            ))
        })
        .collect();
    Some(Info {
        pipe: v["pipe"].as_str()?.to_string(),
        mcp_exe: v["mcp_exe"].as_str().map(str::to_string),
        tabs,
    })
}

fn panel(app: &mut App) -> Value {
    probe(app, "panel.state", Value::Null)
}

fn hello(pipe: &str, tab_token: Option<&str>) -> Result<Client<PipeEnd>, ClientError> {
    let mut c = connect(pipe, WAIT)?;
    c.hello(
        &HelloParams {
            client: "flare-e2e".into(),
            version: "1".into(),
            token: None,
            tab_token: tab_token.map(str::to_string),
        },
        WAIT,
    )?;
    Ok(c)
}

fn remote_code<T>(r: &Result<T, ClientError>) -> String {
    match r {
        Err(ClientError::Remote(e)) => e.code.clone(),
        Err(e) => format!("other: {e}"),
        Ok(_) => "ok".into(),
    }
}

pub fn ctl_channel(ctx: &mut Ctx) -> Result<(), String> {
    let (alpha, beta) = {
        let w = ctx.ensure_world();
        (w.alpha.clone(), w.beta.clone())
    };
    write_picture(&alpha.join(PIC))?;
    // The same name in both folders: whichever tab is in front reads it by its relative name.
    write_picture(&beta.join(PIC))?;
    fs::write(alpha.join(NOTE), NOTE_TEXT).map_err(|e| format!("make the note: {e}"))?;
    // A program: the channel must not show it, whatever its name says.
    fs::write(alpha.join("ctl-run.exe"), "MZ").map_err(|e| format!("make the program: {e}"))?;
    fs::write(alpha.join("ctl-run.png.exe"), "MZ").map_err(|e| format!("make the program: {e}"))?;
    let mut app = ctx.launch(Launch {
        renderer: Some("gpu"),
        folders: vec![alpha.clone(), beta.clone()],
        ..Launch::default()
    })?;
    let r = channel_on(ctx, &mut app, &alpha);
    // The app quits cleanly with a client still connected (the server stops inside the quit).
    let left = app.close();
    ctx.ok(left, "the app quits by itself with a client connected");
    r
}

fn channel_on(ctx: &mut Ctx, app: &mut App, alpha: &Path) -> Result<(), String> {
    prompt(app)?;
    let front = front_tab(app);
    let Some(inf) = info(app) else {
        ctx.ok(false, "ctl.info answers");
        return Ok(());
    };
    ctx.ok(
        inf.tabs.len() == 2 && inf.tabs.iter().filter(|t| t.1).count() == 1,
        format!("two tabs, one in front ({} tabs)", inf.tabs.len()),
    );
    let tokens_fine = inf
        .tabs
        .iter()
        .all(|t| t.2.len() == 64 && t.2.chars().all(|c| c.is_ascii_hexdigit()) && t.2 != t.0);
    ctx.ok(
        tokens_fine && inf.tabs[0].2 != inf.tabs[1].2,
        "each tab has its own 64 digit secret, which is not its id",
    );
    ctx.ok(
        inf.pipe.starts_with("\\\\.\\pipe\\flare-ctl-") && inf.pipe.matches('-').count() >= 3,
        format!("the pipe is scoped to this profile ({})", inf.pipe),
    );
    let Some((front_id, _, front_token)) = inf.tabs.iter().find(|t| t.1).cloned() else {
        return Ok(());
    };
    let Some((behind_id, _, behind_token)) = inf.tabs.iter().find(|t| !t.1).cloned() else {
        return Ok(());
    };
    ctx.ok(front_id == front, "the tab in front is the active one");

    // ---- a real shell has the same secret, the pipe and the server's path in its environment ----
    let mcp_ok = inf
        .mcp_exe
        .as_deref()
        .is_some_and(|p| Path::new(p).is_file());
    ctx.ok(
        mcp_ok,
        format!(
            "flare-mcp.exe sits beside the app ({:?}); build it with: cargo build --release -p flare -p flare-mcp",
            inf.mcp_exe
        ),
    );
    type_line(
        app,
        &format!(
            "cls; Write-Host ('TOKLEN=' + $env:FLARE_TAB_TOKEN.Length + ' TOKOK=' + ($env:FLARE_TAB_TOKEN -ceq '{front_token}') + ' PIPEOK=' + ($env:FLARE_CTL_PIPE -ceq '{}') + ' MCPOK=' + (Test-Path $env:FLARE_MCP_EXE) + ' IDOK=' + ($env:FLARE_TAB_ID -cne '{front_token}'))",
            inf.pipe
        ),
    )?;
    let env_seen = ctx.until("the shell to print its environment", 10_000, &mut || {
        rows(app).contains("TOKLEN=64 TOKOK=True PIPEOK=True MCPOK=True IDOK=True")
    });
    ctx.ok(
        env_seen,
        "the shell's own FLARE_TAB_TOKEN, FLARE_CTL_PIPE and FLARE_MCP_EXE are the channel's",
    );

    // ---- the front tab's secret opens its panel ----
    let mut f = hello(&inf.pipe, Some(&front_token)).map_err(|e| format!("hello: {e}"))?;
    let h = f.call(Method::PanelState, json!({}), WAIT);
    ctx.ok(
        h.as_ref().is_ok_and(|v| v["open"] == json!(false)),
        "panel.state: nothing open yet",
    );
    let list = f
        .call(Method::TabsList, json!({}), WAIT)
        .map_err(|e| format!("tabs.list: {e}"))?;
    ctx.ok(
        list["tabs"].as_array().is_some_and(|t| t.len() == 1)
            && list["tabs"][0]["id"] == json!(front_id)
            && list["tabs"][0]["active"] == json!(true),
        format!("tabs.list gives the caller its own tab only ({list})"),
    );
    let pic = alpha.join(PIC).to_string_lossy().into_owned();
    let shown = f.call(
        Method::ViewImage,
        json!({"path": pic, "title": "Logo"}),
        WAIT,
    );
    ctx.ok(
        shown.as_ref().is_ok_and(|v| {
            v["panel"]["kind"] == json!("image") && v["panel"]["title"] == json!("Logo")
        }),
        format!("view.image answers with the panel ({shown:?})"),
    );
    let drawn = ctx.until("the panel to show the picture", 10_000, &mut || {
        let s = panel(app);
        s["shown"] == json!(true) && s["kind"] == json!(1) && s["title"] == json!("Logo")
    });
    ctx.ok(drawn, "the window shows the picture under the given title");
    let s = panel(app);
    ctx.ok(
        s["sub"] == json!("64 x 32") && s["has_focus"] == json!(false),
        format!(
            "it is the 64 by 32 picture and the keys stayed in the terminal ({} / focus {})",
            s["sub"], s["has_focus"]
        ),
    );
    let st = f.call(Method::PanelState, json!({}), WAIT);
    ctx.ok(
        st.as_ref().is_ok_and(|v| {
            v["open"] == json!(true)
                && v["panel"]["kind"] == json!("image")
                && v["width"].is_number()
        }),
        format!("panel.state reports it ({st:?})"),
    );
    // A relative path is the tab's own folder's.
    let rel = f.call(Method::ViewImage, json!({"path": PIC}), WAIT);
    ctx.ok(
        rel.is_ok(),
        format!("a relative path is read from the tab's folder ({rel:?})"),
    );
    let text = f.call(
        Method::ViewText,
        json!({"text": "hello\nworld\nthird", "title": "From the channel"}),
        WAIT,
    );
    ctx.ok(text.is_ok(), format!("view.text answers ({text:?})"));
    let text_up = ctx.until("the panel to show the text", 10_000, &mut || {
        let s = panel(app);
        s["kind"] == json!(2)
            && s["lines"] == json!(3)
            && s["title"] == json!("From the channel")
            && s["row0"] == json!("hello")
    });
    ctx.ok(text_up, "the window shows the text, three lines, titled");
    let note = alpha.join(NOTE).to_string_lossy().into_owned();
    let file = f.call(Method::ViewFile, json!({"path": note}), WAIT);
    ctx.ok(file.is_ok(), format!("view.file answers ({file:?})"));
    let file_up = ctx.until("the panel to show the note", 10_000, &mut || {
        let s = panel(app);
        s["kind"] == json!(2)
            && s["lines"] == json!(3)
            && s["title"] == json!(NOTE)
            && s["row0"] == json!("one")
    });
    ctx.ok(file_up, "the window shows the file under its own name");

    // ---- what the channel refuses ----
    let missing = alpha.join("ctl-missing.png").to_string_lossy().into_owned();
    let program = alpha.join("ctl-run.exe").to_string_lossy().into_owned();
    let program_png = alpha.join("ctl-run.png.exe").to_string_lossy().into_owned();
    for (what, r) in [
        (
            "a missing file",
            f.call(Method::ViewImage, json!({"path": missing}), WAIT),
        ),
        (
            "a program",
            f.call(Method::ViewFile, json!({"path": program}), WAIT),
        ),
        (
            "a program named like a picture",
            f.call(Method::ViewImage, json!({"path": program_png}), WAIT),
        ),
        (
            "a network path",
            f.call(
                Method::ViewImage,
                json!({"path": "\\\\flare-e2e-no-such-host\\share\\a.png"}),
                WAIT,
            ),
        ),
    ] {
        ctx.ok(
            remote_code(&r) == "not_found",
            format!("view of {what} is not_found ({})", remote_code(&r)),
        );
    }
    let wrong_kind = f.call(Method::ViewFile, json!({"path": pic}), WAIT);
    ctx.ok(
        remote_code(&wrong_kind) == "bad_params",
        format!(
            "a picture through view.file is bad_params ({})",
            remote_code(&wrong_kind)
        ),
    );
    let no_path = f.call(Method::ViewImage, json!({}), WAIT);
    ctx.ok(
        remote_code(&no_path) == "bad_params",
        "view.image without a path is bad_params",
    );
    let big = f.call(
        Method::ViewText,
        json!({"text": "x".repeat(4 * 1024 * 1024 + 1)}),
        WAIT,
    );
    ctx.ok(
        remote_code(&big) == "bad_params",
        format!("text over 4 MB is bad_params ({})", remote_code(&big)),
    );
    let web = f.call(Method::PanelOpen, json!({"kind": "web"}), WAIT);
    ctx.ok(
        remote_code(&web) == "unavailable",
        format!(
            "panel.open web is unavailable in this build ({})",
            remote_code(&web)
        ),
    );
    let eval = f.call(Method::BrowserEval, json!({"expression": "1"}), WAIT);
    ctx.ok(
        remote_code(&eval) == "forbidden",
        format!(
            "browser.eval is forbidden to the agent ({})",
            remote_code(&eval)
        ),
    );
    let shell = f.call(Method::ShellInput, json!({"text": "calc\r"}), WAIT);
    ctx.ok(
        remote_code(&shell) == "forbidden",
        format!("shell.input is forbidden ({})", remote_code(&shell)),
    );

    // ---- who may connect as a tab ----
    let wrong = hello(&inf.pipe, Some(&"0".repeat(64)));
    ctx.ok(
        remote_code(&wrong) == "unauthorized",
        format!("a wrong secret is refused ({})", remote_code(&wrong)),
    );
    let by_id = hello(&inf.pipe, Some(&front_id));
    ctx.ok(
        remote_code(&by_id) == "unauthorized",
        format!("the tab's id is not its secret ({})", remote_code(&by_id)),
    );
    let mut raw = connect(&inf.pipe, WAIT).map_err(|e| format!("connect: {e}"))?;
    let named = raw.call(
        Method::Hello,
        json!({"client": "flare-e2e", "version": "1", "protocol": 1, "tab": front_id}),
        WAIT,
    );
    ctx.ok(
        remote_code(&named) == "bad_params",
        format!("a tab named in hello is refused ({})", remote_code(&named)),
    );
    let mut anon = hello(&inf.pipe, None).map_err(|e| format!("hello without a secret: {e}"))?;
    let none = anon.call(Method::ViewText, json!({"text": "x"}), WAIT);
    ctx.ok(
        remote_code(&none) == "no_tab",
        format!(
            "a connection with no secret has no tab ({})",
            remote_code(&none)
        ),
    );
    let none_list = anon.call(Method::TabsList, json!({}), WAIT);
    ctx.ok(
        none_list
            .as_ref()
            .is_ok_and(|v| v["tabs"].as_array().is_some_and(Vec::is_empty)),
        "and sees no tabs",
    );
    let after = panel(app);
    ctx.ok(
        after["title"] == json!(NOTE),
        "none of the refused calls changed the panel",
    );

    // ---- a tab that is behind: its panel is stored, no tab is switched ----
    let mut b = hello(&inf.pipe, Some(&behind_token)).map_err(|e| format!("hello behind: {e}"))?;
    let behind_list = b
        .call(Method::TabsList, json!({}), WAIT)
        .map_err(|e| format!("tabs.list: {e}"))?;
    ctx.ok(
        behind_list["tabs"].as_array().is_some_and(|t| t.len() == 1)
            && behind_list["tabs"][0]["id"] == json!(behind_id)
            && behind_list["tabs"][0]["active"] == json!(false),
        format!("the behind tab's secret lists the behind tab ({behind_list})"),
    );
    let stored = b.call(
        Method::ViewText,
        json!({"text": "for the tab behind", "title": "Behind"}),
        WAIT,
    );
    ctx.ok(
        stored.is_ok(),
        format!("a view call for a tab that is behind answers ({stored:?})"),
    );
    ctx.ok(
        front_tab(app) == front_id,
        "the tab in front is still the same: no tab was switched",
    );
    let still = panel(app);
    ctx.ok(
        still["title"] == json!(NOTE),
        format!("the front tab's panel is untouched ({})", still["title"]),
    );
    let bs = b.call(Method::PanelState, json!({}), WAIT);
    ctx.ok(
        bs.as_ref()
            .is_ok_and(|v| v["open"] == json!(true) && v["panel"]["title"] == json!("Behind")),
        format!("the behind tab's panel is stored ({bs:?})"),
    );

    // ---- close ----
    let c1 = f.call(Method::PanelClose, json!({}), WAIT);
    ctx.ok(
        c1.as_ref().is_ok_and(|v| v["closed"] == json!(true)),
        format!("panel.close closes it ({c1:?})"),
    );
    let gone = ctx.until("the panel to go", 8_000, &mut || {
        panel(app)["shown"] == json!(false)
    });
    ctx.ok(
        gone,
        "the window has no panel and the terminal has the page",
    );
    let c2 = f.call(Method::PanelClose, json!({}), WAIT);
    ctx.ok(
        c2.as_ref().is_ok_and(|v| v["closed"] == json!(false)),
        "closing again says false and is not an error",
    );
    Ok(())
}

// ---- the MCP server ----

/// Kills the child on every way out, so a failed check never leaves `flare-mcp.exe` running.
struct Reaped(Child);

impl Drop for Reaped {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Runs `flare-mcp.exe` with the tab's environment, sends `lines`, closes its input and returns every
/// reply line. With `token` None the tab variable is not set at all.
fn run_mcp(
    exe: &Path,
    pipe: &str,
    token: Option<&str>,
    lines: &[Value],
) -> Result<Vec<Value>, String> {
    let mut cmd = Command::new(exe);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .env_remove("FLARE_TAB_TOKEN")
        .env_remove("FLARE_TAB_ID")
        .env("FLARE_CTL_PIPE", pipe);
    if let Some(t) = token {
        cmd.env("FLARE_TAB_TOKEN", t);
    }
    let mut child = Reaped(
        cmd.spawn()
            .map_err(|e| format!("start flare-mcp.exe: {e}"))?,
    );
    let mut stdin = child.0.stdin.take().ok_or("no stdin")?;
    for l in lines {
        writeln!(stdin, "{l}").map_err(|e| format!("write to flare-mcp.exe: {e}"))?;
    }
    drop(stdin);
    let out = BufReader::new(child.0.stdout.take().ok_or("no stdout")?);
    let mut replies = Vec::new();
    for line in out.lines() {
        let line = line.map_err(|e| format!("read from flare-mcp.exe: {e}"))?;
        replies.push(
            serde_json::from_str(&line).map_err(|e| format!("not JSON on stdout: {e}: {line}"))?,
        );
    }
    Ok(replies)
}

fn mcp_exe_of(ctx: &Ctx) -> PathBuf {
    ctx.exe
        .parent()
        .map(|d| d.join("flare-mcp.exe"))
        .unwrap_or_default()
}

pub fn mcp_round_trip(ctx: &mut Ctx) -> Result<(), String> {
    let exe = mcp_exe_of(ctx);
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
    let alpha = ctx.ensure_world().alpha.clone();
    write_picture(&alpha.join(PIC))?;
    let mut app = ctx.launch(Launch {
        renderer: Some("gpu"),
        folders: vec![alpha.clone()],
        ..Launch::default()
    })?;
    let r = mcp_on(ctx, &mut app, &exe, &alpha);
    app.close();
    r
}

fn call(id: u64, name: &str, args: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": args}})
}

fn handshake() -> Vec<Value> {
    vec![
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "flare-e2e", "version": "1"}}}),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
    ]
}

fn mcp_on(ctx: &mut Ctx, app: &mut App, exe: &Path, alpha: &Path) -> Result<(), String> {
    prompt(app)?;
    let Some(inf) = info(app) else {
        ctx.ok(false, "ctl.info answers");
        return Ok(());
    };
    let Some((_, _, token)) = inf.tabs.iter().find(|t| t.1).cloned() else {
        ctx.ok(false, "a tab is in front");
        return Ok(());
    };

    // ---- the handshake, the list and show_text, as Claude Code would run them ----
    let mut lines = handshake();
    lines.push(call(
        3,
        "show_text",
        json!({"text": "from mcp\nsecond line", "title": "MCP"}),
    ));
    let replies = run_mcp(exe, &inf.pipe, Some(&token), &lines)?;
    ctx.ok(
        replies.len() == 3,
        format!(
            "three replies (the notification gets none): {}",
            replies.len()
        ),
    );
    ctx.ok(
        replies.first().is_some_and(|r| {
            r["result"]["protocolVersion"] == json!("2025-06-18")
                && r["result"]["serverInfo"]["name"] == json!("flare-mcp")
        }),
        "initialize names the server and the version",
    );
    let names: Vec<String> = replies
        .get(1)
        .and_then(|r| r["result"]["tools"].as_array())
        .map(|t| {
            t.iter()
                .filter_map(|x| x["name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    ctx.ok(
        names == ["show_image", "show_file", "show_text", "close_panel"],
        format!("tools/list names the four tools ({names:?})"),
    );
    ctx.ok(
        replies
            .get(2)
            .is_some_and(|r| r["result"]["isError"] == json!(false)),
        format!("show_text succeeded ({:?})", replies.get(2)),
    );
    let up = ctx.until("the panel to show the text", 10_000, &mut || {
        let s = panel(app);
        s["shown"] == json!(true)
            && s["kind"] == json!(2)
            && s["title"] == json!("MCP")
            && s["lines"] == json!(2)
            && s["row0"] == json!("from mcp")
    });
    ctx.ok(
        up,
        "the window shows the text show_text sent, titled MCP, two lines",
    );

    // ---- show_image with a relative path, then close_panel ----
    let pic = alpha.join(PIC).to_string_lossy().into_owned();
    let lines = vec![
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
        call(2, "show_image", json!({"path": pic})),
        call(
            3,
            "show_file",
            json!({"path": alpha.join("ctl-missing.txt").to_string_lossy()}),
        ),
        call(4, "show_text", json!({"text": ""})),
    ];
    let replies = run_mcp(exe, &inf.pipe, Some(&token), &lines)?;
    ctx.ok(
        replies
            .get(1)
            .is_some_and(|r| r["result"]["isError"] == json!(false)),
        "show_image succeeded",
    );
    let refused = replies
        .get(2)
        .and_then(|r| r["result"]["content"][0]["text"].as_str())
        .unwrap_or_default()
        .to_string();
    ctx.ok(
        replies
            .get(2)
            .is_some_and(|r| r["result"]["isError"] == json!(true))
            && refused.contains("not_found"),
        format!("a file that is not there comes back as a tool error naming not_found ({refused})"),
    );
    ctx.ok(
        replies
            .get(3)
            .is_some_and(|r| r["result"]["isError"] == json!(true)),
        "an empty text is a tool error, not a call",
    );
    let pic_up = ctx.until("the panel to show the picture", 10_000, &mut || {
        let s = panel(app);
        s["kind"] == json!(1) && s["sub"] == json!("64 x 32")
    });
    ctx.ok(pic_up, "the window shows the picture show_image sent");
    let replies = run_mcp(
        exe,
        &inf.pipe,
        Some(&token),
        &[
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
            call(2, "close_panel", json!({})),
        ],
    )?;
    ctx.ok(
        replies
            .get(1)
            .is_some_and(|r| r["result"]["content"][0]["text"] == json!("Closed the panel.")),
        "close_panel says it closed it",
    );
    let gone = ctx.until("the panel to close", 8_000, &mut || {
        panel(app)["shown"] == json!(false)
    });
    ctx.ok(gone, "the window has no panel after close_panel");

    // ---- a client that is not in the tab ----
    let replies = run_mcp(
        exe,
        &inf.pipe,
        Some(&"0".repeat(64)),
        &[
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
            call(2, "show_text", json!({"text": "not allowed"})),
        ],
    )?;
    let t = replies
        .get(1)
        .and_then(|r| r["result"]["content"][0]["text"].as_str())
        .unwrap_or_default()
        .to_string();
    ctx.ok(
        replies
            .get(1)
            .is_some_and(|r| r["result"]["isError"] == json!(true))
            && t.contains("unauthorized"),
        format!("a wrong secret is refused and the model is told so ({t})"),
    );
    let no_env = run_mcp(
        exe,
        &inf.pipe,
        None,
        &[
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
            call(2, "show_text", json!({"text": "not allowed"})),
        ],
    )?;
    let t = no_env
        .get(1)
        .and_then(|r| r["result"]["content"][0]["text"].as_str())
        .unwrap_or_default()
        .to_string();
    ctx.ok(
        t.contains("Flare is not running this session"),
        format!("outside a Flare tab (no secret) the tools say so ({t})"),
    );
    ctx.ok(
        panel(app)["shown"] == json!(false),
        "neither refused run opened a panel",
    );
    Ok(())
}

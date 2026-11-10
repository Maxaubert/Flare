//! `quitStress` (#63): many shells, some of them busy, quit through the normal path, many times over.
//!
//! Opt-in only (`scenarios::OPT_IN`): a plain run never picks it, `cargo e2e -- quitStress` does. Each round
//! restores a strip of shell tabs from a `tabs.json` the scenario writes with the app's own store, visits every
//! tab with Ctrl+Tab and starts a busy command in the first few, opens a web page in some rounds, lets the
//! output flow, then sends `Quit { force: true }` (the e2e hook's normal close: `on_close_requested`, the
//! window hidden, the event loop ended, the end of life) and records the exit code. An access violation
//! shows as 0xC0000005 (3221225477, or -1073741819 as an i32). Rounds alternate the software and the GPU
//! renderer, since the GPU teardown is the one that once rewrote the exit code to 2170 (`end_process_clean`).
//!
//! The busy commands are written here in full; nothing is pasted and no line of unknown content is sent.
//! Knobs (environment of the harness, all optional): `FLARE_QUIT_STRESS_ROUNDS` (default 50),
//! `FLARE_QUIT_STRESS_TABS` (24), `FLARE_QUIT_STRESS_BUSY` (8), `FLARE_QUIT_STRESS_WEB_EVERY` (4, 0 for never)
//! and `FLARE_QUIT_STRESS_OUT` (a file that gets one `round,renderer,web,code,ms` line per round).

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use flare_ctl::Method;
use flare_e2e_proto::Request;
use serde_json::json;

use super::input_b::front_tab;
use super::lifecycle::write_tabs;
use super::web::{channel_of, hello, web_root, HOST, LOAD};
use crate::ctx::{App, Ctx, Launch};
use crate::guards::ReapGuard;
use crate::world::{wait_for_prompt, Shell};

/// The exit code of an access violation, as `ExitStatus::code` gives it on Windows.
pub const ACCESS_VIOLATION: i32 = 0xC000_0005_u32 as i32;

/// How long a busy tab's prompt is waited for (a busy PC starts two dozen PowerShells at once).
const PROMPT_WAIT: Duration = Duration::from_secs(30);

/// What a busy tab runs: floods from PowerShell itself, a flood from a child console process, and a slow
/// child that keeps the tree alive. Each line is complete and fixed here.
const BUSY: [&str; 4] = [
    "while ($true) { 'flare quit stress: a line of output to keep the reader and the pump busy' }",
    "cmd /c \"for /L %i in (1,1,100000000) do @echo flare quit stress from a child console %i\"",
    "1..100000000 | ForEach-Object { \"flare quit stress row $_ \" + ('x' * 60) }",
    "ping -t 127.0.0.1",
];

/// A whole number from the harness environment, or `default`.
fn knob(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

/// An exit code as the report writes it: the hex of an NTSTATUS, the plain number otherwise.
pub fn code_text(code: Option<i32>) -> String {
    match code {
        None => "none (still running)".to_string(),
        Some(c) if c < 0 => format!("0x{:08X}", c as u32),
        Some(c) => c.to_string(),
    }
}

/// The tally of a run: how many rounds ended with each exit code, in code order.
pub fn tally(codes: &[Option<i32>]) -> BTreeMap<String, usize> {
    let mut out = BTreeMap::new();
    for c in codes {
        *out.entry(code_text(*c)).or_insert(0) += 1;
    }
    out
}

fn tab_count(app: &mut App) -> usize {
    app.session.state().map(|s| s.tabs.len()).unwrap_or(0)
}

/// Opens the test page in the front tab's web panel through the control channel. True when it loaded.
fn open_page(app: &mut App) -> bool {
    let Ok((pipe, _, token)) = channel_of(app) else {
        return false;
    };
    let Ok(mut c) = hello(&pipe, &token) else {
        return false;
    };
    c.call(
        Method::BrowserOpen,
        json!({ "url": format!("{HOST}/index.html") }),
        LOAD,
    )
    .is_ok()
}

pub fn quit_stress(ctx: &mut Ctx) -> Result<(), String> {
    let rounds = knob("FLARE_QUIT_STRESS_ROUNDS", 50).max(1);
    let tabs = knob("FLARE_QUIT_STRESS_TABS", 24).max(2);
    let busy = knob("FLARE_QUIT_STRESS_BUSY", 8).min(tabs);
    let web_every = knob("FLARE_QUIT_STRESS_WEB_EVERY", 4);
    let out = std::env::var_os("FLARE_QUIT_STRESS_OUT").map(PathBuf::from);
    let web = web_root(ctx)?;
    let (alpha, beta, profile, root) = {
        let w = ctx.ensure_world();
        (
            w.alpha.clone(),
            w.beta.clone(),
            w.profile.clone(),
            w.root.clone(),
        )
    };
    let folders: Vec<PathBuf> = (0..tabs)
        .map(|i| {
            if i % 2 == 0 {
                alpha.clone()
            } else {
                beta.clone()
            }
        })
        .collect();
    let mut codes = Vec::new();
    let mut launch_fails: Vec<String> = Vec::new();
    let mut lines = vec!["round,renderer,web,code,ms".to_string()];

    for round in 1..=rounds {
        let renderer = if round % 2 == 0 { "gpu" } else { "software" };
        let with_web = web_every > 0 && round % web_every == 0;
        write_tabs(&profile, &folders)?;
        let launched = ctx.launch(Launch {
            renderer: Some(renderer),
            env: vec![(
                "FLARE_E2E_WEB_ROOT".to_string(),
                web.to_string_lossy().into_owned(),
            )],
            ..Launch::default()
        });
        // A launch that fails is one result of the round (its message carries the app's exit code), not
        // the end of the run.
        let mut app = match launched {
            Ok(app) => app,
            Err(e) => {
                launch_fails.push(format!("round {round}: {e}"));
                lines.push(format!("{round},{renderer},false,launch failed,0"));
                ctx.ok(
                    false,
                    format!("round {round} ({renderer}): the launch failed: {e}"),
                );
                let strays = ReapGuard::new(root.clone()).reap();
                ctx.ok(
                    strays == 0,
                    format!("round {round}: no stray process is left ({strays})"),
                );
                continue;
            }
        };
        ctx.until("every restored tab", 30_000, &mut || {
            tab_count(&mut app) == tabs
        });

        // Every tab is visited once (a restored tab that starts on its first show starts now), and the
        // first `busy` ones get a busy command once their prompt shows.
        let mut started = 0;
        for i in 0..tabs {
            if i > 0 {
                let _ = app.session.key("Ctrl+Tab");
            }
            if i < busy {
                let front = front_tab(&mut app);
                if wait_for_prompt(&mut app, &front, Shell::PowerShell, PROMPT_WAIT) {
                    let _ = app.session.type_text(BUSY[i % BUSY.len()]);
                    let _ = app.session.key("Enter");
                    started += 1;
                }
            }
        }
        let paged = with_web && open_page(&mut app);
        // The output flows for a moment before the quit.
        ctx.poll(1_500, &mut || false);

        let t0 = Instant::now();
        let _ = app.session.call(&Request::Quit { force: true });
        let child = &app.child;
        let left = child.wait_for(|| child.exited().is_some(), Duration::from_secs(20));
        let code = app.child.exited();
        let ms = t0.elapsed().as_millis();
        codes.push(code);
        lines.push(format!(
            "{round},{renderer},{paged},{},{ms}",
            code_text(code)
        ));
        ctx.ok(
            left && code == Some(0),
            format!(
                "round {round} ({renderer}, {started} busy of {tabs}, web page {paged}): exit {} in {ms} ms",
                code_text(code)
            ),
        );
        drop(app);
        let strays = ReapGuard::new(root.clone()).reap();
        ctx.ok(
            strays == 0,
            format!("round {round}: no stray process is left ({strays})"),
        );
        if let Some(path) = &out {
            let _ = fs::write(path, lines.join("\n") + "\n");
        }
    }

    let t = tally(&codes);
    println!(
        "quitStress tally over {rounds} rounds: {t:?}; {} launch(es) failed {launch_fails:?}",
        launch_fails.len()
    );
    let _ = std::io::stdout().flush();
    ctx.ok(
        !codes.contains(&Some(ACCESS_VIOLATION))
            && !launch_fails
                .iter()
                .any(|f| f.contains(&ACCESS_VIOLATION.to_string())),
        format!("no round ended in an access violation ({t:?})"),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_access_violation_reads_as_its_ntstatus() {
        assert_eq!(ACCESS_VIOLATION, -1_073_741_819);
        assert_eq!(code_text(Some(ACCESS_VIOLATION)), "0xC0000005");
        assert_eq!(code_text(Some(0)), "0");
        assert_eq!(code_text(None), "none (still running)");
    }

    #[test]
    fn the_tally_counts_each_exit_code() {
        let t = tally(&[Some(0), Some(0), Some(ACCESS_VIOLATION), None]);
        assert_eq!(t.get("0"), Some(&2));
        assert_eq!(t.get("0xC0000005"), Some(&1));
        assert_eq!(t.get("none (still running)"), Some(&1));
    }
}

//! Install when idle (2026-10-29): idleInstall.
//!
//! A temp install inside the world (`<world>\inst\current\Flare.exe`, a copy of the exe under test with its
//! ConPTY beside it) and a build staged beside it (`<world>\inst\staged\`, the same exe plus a marker file, and
//! `staged.json` last, as `tools/stage-install.ps1` writes it). The real installs are never named. The app runs from
//! the temp install, so it is an installed copy; a stand-in Claude works in a tab, which must hold the install;
//! once it stops, the idle rule (shortened by `FLARE_E2E_STAGED_HOLD_MS`) and the countdown run, the app swaps the
//! files, quits through its normal end of life and starts the new exe on the same profile, still under `--e2e`
//! (offscreen). The scenario then talks to the new process on the same pipe.
//!
//! The app writes `staged-state.json` into its profile under `--e2e`: the chip, the toast, the phase, what the start
//! found, and its pid.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use flare_e2e_proto::Request;
use serde_json::{json, Value};

use crate::ctx::{Ctx, Launch};
use crate::session::Session;
use crate::standins::agent_cmd::{FakeClaude, Step};
use crate::standins::hooklines::hook_line;
use crate::world::{prompt_timeout, wait_for_prompt, Shell};

const IDLE_TITLE: &str = "\u{2733} Claude Code";
const WORKING_TITLE: &str = "\u{25D0} Claude Code";
const HOLD_VAR: &str = concat!("FLARE_E2E", "_STAGED_HOLD_MS");
const COUNTDOWN_VAR: &str = concat!("FLARE_E2E", "_STAGED_COUNTDOWN_MS");
/// The idle hold and the countdown the scenario sets (the real ones are 60 s and 10 s).
const HOLD_MS: u64 = 3_000;
const COUNTDOWN_MS: u64 = 1_500;
/// The commit the staged build names.
const COMMIT: &str = "e2e1234stagedbuild";

/// The stand-in's steps: idle at once; working once `go` exists; idle again (its `done` hook) once `stop` exists.
fn steps_script(go: &Path, stop: &Path) -> String {
    let strip = |s: String| {
        s.trim_start_matches('\u{1b}')
            .trim_end_matches('\u{7}')
            .to_string()
    };
    let working = strip(hook_line("working", &json!({})));
    let done = strip(hook_line("done", &json!({})));
    format!(
        "$ErrorActionPreference = 'Stop'\r\n\
         $e = [string][char]27\r\n\
         $b = [string][char]7\r\n\
         [Console]::Write($e + ']0;{IDLE_TITLE}' + $b)\r\n\
         while (-not (Test-Path -LiteralPath '{go}')) {{ Start-Sleep -Milliseconds 50 }}\r\n\
         [Console]::Write($e + ']0;{WORKING_TITLE}' + $b)\r\n\
         [Console]::Write($e + '{working}' + $b)\r\n\
         while (-not (Test-Path -LiteralPath '{stop}')) {{ Start-Sleep -Milliseconds 50 }}\r\n\
         [Console]::Write($e + ']0;{IDLE_TITLE}' + $b)\r\n\
         [Console]::Write($e + '{done}' + $b)\r\n\
         Start-Sleep -Seconds 3600\r\n",
        go = go.display(),
        stop = stop.display(),
    )
}

fn sha256(path: &Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    Ok(Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// The temp install: `current\Flare.exe` and its ConPTY, from the exe under test.
fn make_install(exe: &Path, inst: &Path) -> Result<PathBuf, String> {
    let cur = inst.join("current");
    std::fs::create_dir_all(&cur).map_err(|e| format!("the install folder: {e}"))?;
    let from = exe.parent().ok_or("the exe has no folder")?;
    std::fs::copy(exe, cur.join("Flare.exe")).map_err(|e| format!("copy the exe: {e}"))?;
    for f in ["conpty.dll", "OpenConsole.exe"] {
        std::fs::copy(from.join(f), cur.join(f)).map_err(|e| format!("copy {f}: {e}"))?;
    }
    std::fs::write(cur.join("marker.txt"), b"old").map_err(|e| e.to_string())?;
    Ok(cur.join("Flare.exe"))
}

/// Stages the "new" build: the same exe, a marker that says new, a file the old build did not have; the manifest last.
fn stage(exe: &Path, inst: &Path) -> Result<(), String> {
    let s = inst.join("staged");
    std::fs::create_dir_all(&s).map_err(|e| e.to_string())?;
    std::fs::copy(exe, s.join("Flare.exe")).map_err(|e| format!("stage the exe: {e}"))?;
    std::fs::write(s.join("marker.txt"), b"new").map_err(|e| e.to_string())?;
    std::fs::write(s.join("added.txt"), b"added").map_err(|e| e.to_string())?;
    let mut files = BTreeMap::new();
    for f in ["Flare.exe", "marker.txt", "added.txt"] {
        files.insert(f.to_string(), sha256(&s.join(f))?);
    }
    let m = json!({"commit": COMMIT, "built_at": "2026-10-29T10:00:00Z", "files": files});
    std::fs::write(s.join("staged.json.tmp"), m.to_string()).map_err(|e| e.to_string())?;
    std::fs::rename(s.join("staged.json.tmp"), s.join("staged.json")).map_err(|e| e.to_string())
}

/// `staged-state.json` in the profile, `Null` while there is none.
fn state(profile: &Path) -> Value {
    std::fs::read_to_string(profile.join("staged-state.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(Value::Null)
}

fn tab_marks(session: &mut Session) -> Vec<String> {
    session
        .probe("app.state", Value::Null)
        .ok()
        .and_then(|v| v["tabs"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .map(|t| t["mark"]["state"].as_str().unwrap_or("").to_string())
        .collect()
}

pub fn idle_install(ctx: &mut Ctx) -> Result<(), String> {
    let (root, alpha, profile, mut claude) = {
        let w = ctx.ensure_world();
        (
            w.root.clone(),
            w.alpha.clone(),
            w.profile.clone(),
            FakeClaude::new(w),
        )
    };
    let inst = root.join("inst");
    let exe = make_install(&ctx.exe.clone(), &inst)?;
    stage(&exe, &inst)?;
    let go = claude.dir.join("go-working");
    let stop = claude.dir.join("stop-working");
    claude.script(&[Step::Title(IDLE_TITLE.into())]);
    let body = steps_script(&go, &stop);
    let bytes = [b"\xEF\xBB\xBF".as_slice(), body.as_bytes()].concat();
    std::fs::write(claude.bin().join("claude-steps.ps1"), bytes)
        .map_err(|e| format!("write the stand-in's steps: {e}"))?;

    let mut env = claude.env();
    env.push((HOLD_VAR.into(), HOLD_MS.to_string()));
    env.push((COUNTDOWN_VAR.into(), COUNTDOWN_MS.to_string()));
    let real_exe = std::mem::replace(&mut ctx.exe, exe.clone());
    let launched = ctx.launch(Launch {
        renderer: Some("software"),
        folders: vec![alpha],
        env,
        ..Launch::default()
    });
    ctx.exe = real_exe;
    let mut app = launched?;
    let old_pid = app.hello()?.pid;

    // The chip shows once the stage's hashes check.
    let chip = ctx.until("the Build ready chip", 15_000, &mut || {
        state(&profile)["chip"] == json!(true)
    });
    ctx.ok(chip, "an installed copy shows the staged build's chip");
    let st = state(&profile);
    ctx.ok(
        st["label"] == json!("Build ready"),
        "the chip says Build ready",
    );
    ctx.ok(
        st["commit"] == json!("e2e1234"),
        "and knows the short commit",
    );

    // A stand-in Claude starts working in the tab.
    let tab = app
        .session
        .probe("app.state", Value::Null)
        .ok()
        .and_then(|v| v["tabs"][0]["id"].as_str().map(str::to_string))
        .ok_or("the tab has no id")?;
    wait_for_prompt(
        &mut app,
        &tab,
        Shell::PowerShell,
        prompt_timeout(Shell::PowerShell),
    );
    app.session.type_text("claude")?;
    app.session.key("Enter")?;
    std::fs::write(&go, b"").map_err(|e| format!("the go file: {e}"))?;
    let working = ctx.until("the agent to work", 10_000, &mut || {
        tab_marks(&mut app.session).iter().any(|m| m == "working")
    });
    ctx.ok(working, "the agent works");

    // While it works the build waits, for well over the idle hold.
    let held = !ctx.poll(HOLD_MS * 2 + 1_000, &mut || {
        state(&profile)["phase"] != json!("waiting")
    });
    ctx.ok(held, "a working agent holds the install past the idle hold");
    ctx.ok(
        app.child.exited().is_none(),
        "and the app is still the old one",
    );
    ctx.ok(
        std::fs::read(inst.join("current").join("marker.txt")).unwrap_or_default() == b"old",
        "nothing was swapped",
    );

    // It stops: the quiet hold, the toast, the swap, the restart.
    std::fs::write(&stop, b"").map_err(|e| format!("the stop file: {e}"))?;
    let toast = ctx.until("the countdown toast", HOLD_MS + 8_000, &mut || {
        state(&profile)["phase"] == json!("countdown")
    });
    ctx.ok(toast, "once idle, the toast counts down");
    let st = state(&profile);
    ctx.ok(
        st["toast"] == json!("Installing the new build and restarting")
            && st["button"] == json!("Cancel"),
        "it says Installing the new build and restarting, with Cancel",
    );
    let exited = ctx.until("the old app to quit", COUNTDOWN_MS + 15_000, &mut || {
        app.child.exited().is_some()
    });
    ctx.ok(exited, "the old app quits on its own after the countdown");
    ctx.ok(
        app.child.exited() == Some(0),
        "through a clean end of life (exit code 0)",
    );
    let cur = inst.join("current");
    ctx.ok(
        std::fs::read(cur.join("marker.txt")).unwrap_or_default() == b"new",
        "the new files are in current",
    );
    ctx.ok(
        cur.join("added.txt").exists(),
        "a file the build added is there",
    );
    ctx.ok(
        cur.join("Flare.exe.old").exists(),
        "the old exe is kept aside until the new one is good",
    );

    // The new process: same profile, same pipe, offscreen, and it says what it found.
    let mut new = Session::connect(&profile, Duration::from_secs(30))?;
    let hello = new.hello()?;
    ctx.ok(
        hello.pid != old_pid,
        "a new process answers on the same profile",
    );
    let installed = ctx.until("the new run's state", 15_000, &mut || {
        let s = state(&profile);
        s["pid"] == json!(hello.pid)
            && s["start"]
                .as_str()
                .is_some_and(|t| t.starts_with("Installed"))
    });
    ctx.ok(installed, "the new run knows it is a new build");
    ctx.ok(
        state(&profile)["toast"] == json!("Installed e2e1234"),
        "and says Installed e2e1234",
    );
    ctx.ok(!inst.join("staged").exists(), "the staged folder is gone");
    let tabs = new
        .probe("app.state", Value::Null)
        .ok()
        .and_then(|v| v["tabs"].as_array().map(Vec::len))
        .unwrap_or(0);
    ctx.ok(tabs == 1, "the tab came back");
    let park = new.probe("window.park", Value::Null).unwrap_or(Value::Null);
    ctx.ok(
        crate::ctx::park_problems(&park).is_empty(),
        format!("the restarted window is parked offscreen: {park}"),
    );

    // After 20 s the new build is good: the old files and the record go.
    let good = ctx.until("the new build to be confirmed", 30_000, &mut || {
        !inst.join("install.json").exists() && !cur.join("Flare.exe.old").exists()
    });
    ctx.ok(
        good,
        "after 20 s the new build is good and the .old files are gone",
    );

    let _ = new.call(&Request::Quit { force: false });
    let gone = ctx.until("the new app to quit", 15_000, &mut || {
        Session::connect(&profile, Duration::from_millis(100)).is_err()
    });
    ctx.ok(gone, "the restarted app quits");
    crate::proc::reap_by_profile(&profile);
    drop(app);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stand_in_works_on_go_and_stops_on_stop() {
        let s = steps_script(Path::new(r"C:\t\go"), Path::new(r"C:\t\stop"));
        let go = s.find(r"C:\t\go").unwrap();
        let working = s.find("state=working").unwrap();
        let stop = s.find(r"C:\t\stop").unwrap();
        let done = s.find("state=done").unwrap();
        assert!(go < working && working < stop && stop < done);
    }

    #[test]
    fn the_stage_names_every_file_with_its_hash() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("flare.exe");
        std::fs::write(&exe, b"exe").unwrap();
        let inst = dir.path().join("inst");
        stage(&exe, &inst).unwrap();
        let layout = flare_update::staged::Layout::new(&inst, "Flare.exe");
        let m = flare_update::staged::check(&layout).expect("the app would accept it");
        assert_eq!(m.short_commit(), "e2e1234");
        assert_eq!(m.files.len(), 3);
    }
}

//! The scenario context: the checks, the skips, the launch, the shots and the one wait `until`.
//! Task 1 wrote the counters, `ok` and `not_ported`; Task 13 adds the launch, the wait and the
//! output mode the parent runner reads (`json`: one JSON line per check, skip and world).

use std::path::{Path, PathBuf};
use std::time::Duration;

use flare_e2e_proto::Request;
use serde_json::json;

use crate::proc::{reap_by_profile, Child};
use crate::session::{Hello, Session};
use crate::until::{wait_on, SystemClock};
use crate::world::World;

/// The release build the driver runs when no `--exe` is given.
pub const DEFAULT_EXE: &str = r"target\release\flare.exe";
/// The registry-sandbox variable the e2e environment always sets (M9 D5).
const VERB_ROOT_VAR: &str = concat!("FLARE_E2E", "_VERB_ROOT");
/// How long `App::close` lets a quit take before it kills the tree.
const CLOSE_GRACE: Duration = Duration::from_secs(15);
/// How long a launch waits for the app's pipe to open.
const CONNECT_WAIT: Duration = Duration::from_secs(20);
/// The longest `settle` may hold a scenario still (a "nothing changed" proof).
const SETTLE_MAX_MS: u64 = 50;

/// What a scenario gets: its name, its counters and its output mode.
pub struct Ctx {
    pub name: &'static str,
    /// The scenario's world, made by the first `launch`.
    pub world: Option<World>,
    /// Where `shot` writes `<scenario>-<name>.png`.
    pub shots: PathBuf,
    /// The app binary `launch` starts (unless `Launch.packaged`).
    pub exe: PathBuf,
    /// `--packaged`: launches the installed build.
    pub packaged: bool,
    pub checks: u32,
    pub skips: u32,
    pub fails: Vec<String>,
    /// True in a `--run-one` child: every check, skip and the world root print as JSON lines.
    pub json: bool,
    /// The wait a timed-out `until` gave up on, not yet recorded. The next check takes it: a failing
    /// check is then the same failure with both texts, one entry; any other check, a skip or the end
    /// of the scenario records it alone. So a timeout is one FAIL, never two.
    pending_timeout: Option<String>,
}

/// What to start: the app's own arguments, its environment on top of the sandbox's, the folders
/// to open, the renderer, the scale to set after `Hello`, and the installed layout.
#[derive(Debug, Clone, Default)]
pub struct Launch {
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub folders: Vec<PathBuf>,
    pub renderer: Option<&'static str>,
    pub scale_dpi: Option<u32>,
    pub packaged: bool,
}

/// One running app and its pipe. Dropping it reaps the process.
pub struct App {
    pub child: Child,
    pub session: Session,
    pub profile: PathBuf,
}

impl App {
    /// Quits the app, waiting up to 15 s for it to exit before the tree is killed.
    pub fn close(&mut self) -> bool {
        self.close_within(CLOSE_GRACE)
    }

    /// As `close`, with the grace given.
    pub fn close_within(&mut self, grace: Duration) -> bool {
        let session = &mut self.session;
        close_child(
            &self.child,
            &self.profile,
            || {
                let _ = session.call(&Request::Quit { force: false });
            },
            grace,
        )
    }

    /// The app's `Hello` reply (version, renderer, scale, pid).
    pub fn hello(&mut self) -> Result<Hello, String> {
        self.session.hello()
    }
}

/// Runs `quit`, waits `grace` for the child to exit, and otherwise kills everything whose command
/// line holds the profile. Returns true when the child left by itself.
pub fn close_child(child: &Child, profile: &Path, quit: impl FnOnce(), grace: Duration) -> bool {
    quit();
    if child.wait_for(|| child.exited().is_some(), grace) {
        return true;
    }
    reap_by_profile(profile);
    child.wait_for(|| child.exited().is_some(), Duration::from_secs(5));
    false
}

/// Writes one snapshot to `dir/<scenario>-<name>.png`. A failed snapshot is not a result and adds
/// no FAIL: the function answers with the path only when the take succeeded.
pub fn shot_file(
    dir: &Path,
    scenario: &str,
    name: &str,
    take: impl FnOnce(&Path) -> Result<(u32, u32), String>,
) -> Option<PathBuf> {
    let _ = std::fs::create_dir_all(dir);
    let path = dir.join(format!("{scenario}-{name}.png"));
    take(&path).ok().map(|_| path)
}

/// The launch guard: the installed build may only run with its registry sandbox (M9 D5).
fn packaged_guard(packaged: bool, env: &[(String, String)]) -> Result<(), String> {
    if packaged && !env.iter().any(|(k, _)| k == VERB_ROOT_VAR) {
        return Err(format!(
            "--packaged refuses to run without {VERB_ROOT_VAR}: it would write the real registry"
        ));
    }
    Ok(())
}

/// `%LOCALAPPDATA%\Flare\current\Flare.exe`, the installed layout (M9 D4).
fn installed_exe() -> Result<PathBuf, String> {
    let local = std::env::var("LOCALAPPDATA").map_err(|_| {
        "LOCALAPPDATA is not set, so the installed build cannot be found".to_string()
    })?;
    Ok(PathBuf::from(local)
        .join("Flare")
        .join("current")
        .join("Flare.exe"))
}

impl Ctx {
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            world: None,
            shots: PathBuf::from(".e2e-shots"),
            exe: PathBuf::from(DEFAULT_EXE),
            packaged: false,
            checks: 0,
            skips: 0,
            fails: Vec::new(),
            json: false,
            pending_timeout: None,
        }
    }

    /// Prints one check and counts it; a failing one is kept in `fails`.
    fn record(&mut self, cond: bool, msg: &str) {
        self.checks += 1;
        if self.json {
            println!("{}", json!({"pass": cond, "msg": msg}));
        } else if cond {
            println!("  pass  {msg}");
        } else {
            println!("  FAIL  {msg}");
        }
        if !cond {
            self.fails.push(msg.to_string());
        }
    }

    /// Records a timed-out wait on its own, if one is still pending.
    fn flush_timeout(&mut self) {
        if let Some(what) = self.pending_timeout.take() {
            self.record(false, &format!("timed out waiting for {what}"));
        }
    }

    /// Records a pending timeout alone, at the end of a scenario that did not check it.
    pub fn finish(&mut self) {
        self.flush_timeout();
    }

    /// True while a timed-out `until` has not been recorded yet.
    pub fn has_pending_timeout(&self) -> bool {
        self.pending_timeout.is_some()
    }

    /// Prints a pass or a FAIL line and counts it. A failing check straight after a timed-out
    /// `until` is that same failure: one FAIL carrying both texts. A passing one leaves the timeout
    /// standing as its own failure.
    pub fn ok(&mut self, cond: bool, msg: impl AsRef<str>) {
        let msg = msg.as_ref();
        match self.pending_timeout.take() {
            Some(what) if !cond => {
                self.record(false, &format!("{msg} (timed out waiting for {what})"));
            }
            Some(what) => {
                self.record(false, &format!("timed out waiting for {what}"));
                self.record(true, msg);
            }
            None => self.record(cond, msg),
        }
    }

    /// Prints a skip line. A skip is counted apart from passes and fails; it is never a pass.
    pub fn skip(&mut self, why: &str) {
        self.flush_timeout();
        self.skips += 1;
        if self.json {
            println!("{}", json!({"skip": why}));
        } else {
            println!("  skip  {why}");
        }
    }

    /// Records one failed check saying the scenario is not ported yet, and returns it as the error.
    pub fn not_ported(&mut self, task: u32) -> Result<(), String> {
        let msg = format!("not ported yet (Task {task})");
        self.ok(false, &msg);
        Err(msg)
    }

    /// The one wait. Polls `f` every 25 ms and returns whether it held. A timeout records nothing
    /// yet: the next check, skip or `finish` records it once (see `pending_timeout`).
    pub fn until(&mut self, what: &str, timeout_ms: u64, f: &mut dyn FnMut() -> bool) -> bool {
        self.flush_timeout();
        let mut clock = SystemClock::new();
        if wait_on(&mut clock, timeout_ms, f) {
            return true;
        }
        self.pending_timeout = Some(what.to_string());
        false
    }

    /// Holds the scenario still for at most 50 ms. Only for "nothing changed" proofs.
    pub fn settle(&mut self, ms: u64) {
        assert!(
            ms <= SETTLE_MAX_MS,
            "settle takes at most {SETTLE_MAX_MS} ms; use until for a longer wait"
        );
        std::thread::sleep(Duration::from_millis(ms));
    }

    /// A snapshot of the app into `<shots>/<scenario>-<name>.png`. Failures are ignored.
    pub fn shot(&mut self, app: &mut App, name: &str) -> Option<PathBuf> {
        let dir = self.shots.clone();
        shot_file(&dir, self.name, name, |p| app.session.snapshot(p))
    }

    /// The scenario's world, made on first use. A `--run-one` child prints its root once, here.
    pub fn ensure_world(&mut self) -> &World {
        if self.world.is_none() {
            let w = World::new();
            if self.json {
                println!("{}", json!({"world": w.root.to_string_lossy()}));
            }
            self.world = Some(w);
        }
        self.world.as_ref().expect("world was just created")
    }

    /// Starts the app on this scenario's sandbox (one profile per scenario), connects its pipe,
    /// waits for `Hello`, and sets the scale when asked.
    pub fn launch(&mut self, opts: Launch) -> Result<App, String> {
        let packaged = opts.packaged || self.packaged;
        self.ensure_world();
        let w = self.world.as_ref().expect("world was just created");
        let mut env = w.env();
        env.extend(opts.env.iter().cloned());
        packaged_guard(packaged, &env)?;
        let profile = w.profile.clone();
        let exe = if packaged {
            installed_exe()?
        } else {
            self.exe.clone()
        };
        if !exe.is_file() {
            return Err(format!("{} is missing", exe.display()));
        }
        let mut args = opts.args.clone();
        if let Some(r) = opts.renderer {
            args.push("--renderer".into());
            args.push(r.into());
        }
        args.extend(
            opts.folders
                .iter()
                .map(|f| f.to_string_lossy().into_owned()),
        );
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();

        let child = Child::launch(&exe, &refs, &env, &profile);
        let mut session = Session::connect(&profile, CONNECT_WAIT)?;
        session.hello()?;
        assert_parked(&mut session)?;
        if let Some(dpi) = opts.scale_dpi {
            session.call(&Request::DpiChange {
                dpi,
                x: None,
                y: None,
            })?;
        }
        Ok(App {
            child,
            session,
            profile,
        })
    }
}

/// How long a launch waits for the window to be styled and parked (the app does it by timer, as
/// soon as the native window exists).
const PARK_WAIT_MS: u64 = 10_000;

/// Pure: what the `window.park` answer says is wrong. A reply that is not the expected shape is a
/// problem too, so a probe that stops answering can never pass.
pub fn park_problems(v: &serde_json::Value) -> Vec<String> {
    match v.get("problems").and_then(|p| p.as_array()) {
        Some(list) => list
            .iter()
            .map(|p| p.as_str().unwrap_or("?").to_string())
            .collect(),
        None => vec![format!("window.park gave no problems list: {v}")],
    }
}

/// The hard safety line of every launch: the window is off every monitor, a no-activate tool
/// window, off the taskbar and not in the foreground. Fails the launch when it never holds.
fn assert_parked(session: &mut Session) -> Result<(), String> {
    let mut last = vec!["the window never answered window.park".to_string()];
    let held = wait_on(&mut SystemClock::new(), PARK_WAIT_MS, &mut || match session
        .probe_str("window.park")
    {
        Ok(v) => {
            last = park_problems(&v);
            last.is_empty()
        }
        Err(e) => {
            last = vec![e];
            false
        }
    });
    if held {
        Ok(())
    } else {
        Err(format!("the --e2e window is not safe: {}", last.join("; ")))
    }
}

/// One row of the scenario table: its name, its group, its body and its time limit in seconds.
pub struct Scenario {
    pub name: &'static str,
    pub group: &'static str,
    pub run: fn(&mut Ctx) -> Result<(), String>,
    pub limit_secs: u64,
}

#[cfg(test)]
mod park_tests {
    use super::*;

    #[test]
    fn a_clean_answer_has_no_problems_and_a_bad_shape_always_has() {
        assert!(park_problems(&json!({"problems": []})).is_empty());
        assert_eq!(
            park_problems(&json!({"problems": ["WS_EX_NOACTIVATE is not set"]})),
            vec!["WS_EX_NOACTIVATE is not set".to_string()]
        );
        assert_eq!(park_problems(&json!(null)).len(), 1);
        assert_eq!(park_problems(&json!({"rect": [0, 0, 1, 1]})).len(), 1);
    }
}

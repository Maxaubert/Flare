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
        }
    }

    /// Prints a pass or a FAIL line and counts it.
    pub fn ok(&mut self, cond: bool, msg: impl AsRef<str>) {
        self.checks += 1;
        let msg = msg.as_ref();
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

    /// Prints a skip line. A skip is counted apart from passes and fails; it is never a pass.
    pub fn skip(&mut self, why: &str) {
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

    /// The one wait. Polls `f` every 25 ms; on timeout records one FAIL `timed out waiting for
    /// <what>` and returns false.
    pub fn until(&mut self, what: &str, timeout_ms: u64, f: &mut dyn FnMut() -> bool) -> bool {
        let mut clock = SystemClock::new();
        if wait_on(&mut clock, timeout_ms, f) {
            return true;
        }
        self.ok(false, format!("timed out waiting for {what}"));
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

    /// Starts the app on this scenario's sandbox (one profile per scenario), connects its pipe,
    /// waits for `Hello`, and sets the scale when asked.
    pub fn launch(&mut self, opts: Launch) -> Result<App, String> {
        let packaged = opts.packaged || self.packaged;
        if self.world.is_none() {
            let w = World::new();
            if self.json {
                println!("{}", json!({"world": w.root.to_string_lossy()}));
            }
            self.world = Some(w);
        }
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

/// One row of the scenario table: its name, its group, its body and its time limit in seconds.
pub struct Scenario {
    pub name: &'static str,
    pub group: &'static str,
    pub run: fn(&mut Ctx) -> Result<(), String>,
    pub limit_secs: u64,
}

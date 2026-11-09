//! The parent runner: one child per scenario, the timeout, the table and the exit code (Task 13).
//!
//! The parent (`flare-e2e [filters]`) starts `flare-e2e --run-one <name>` for each selected
//! scenario. The child runs that scenario and prints one JSON line per check, skip and world root
//! (`{"pass":bool,"msg":...}`, `{"skip":...}`, `{"world":...}`, then `{"done":true}`). The parent
//! prints them in PT's format, kills a child that overruns its limit and reaps the world it made.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::ctx::{Ctx, Scenario, DEFAULT_EXE};
use crate::proc::reap_by_profile;
use crate::report::{self, Row};
use crate::scenarios::{ALL, OPT_IN};

const POLL: Duration = Duration::from_millis(25);
/// How long the parent waits for a child's last lines after the child is gone.
const DRAIN: Duration = Duration::from_millis(500);

/// The parsed command line.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Cli {
    pub filters: Vec<String>,
    pub list: bool,
    pub exe: Option<PathBuf>,
    pub packaged: bool,
    pub shots: Option<PathBuf>,
    pub keep: bool,
    pub perf: bool,
    pub run_one: Option<String>,
    /// `perf` only: `gpu`, `software` or `both`.
    pub renderer: Option<String>,
    /// `perf` only: launches that count for the start-up median.
    pub runs: Option<usize>,
    /// `perf` only: the display scaling in percent for the frame measure.
    pub scale: Option<u32>,
    /// `perf` only: `cmd` or `pwsh` for the start-up measure.
    pub shell: Option<String>,
    /// `perf` only: where the report goes.
    pub out: Option<PathBuf>,
    /// `perf` only: the Prism Terminal baseline.
    pub baseline: Option<PathBuf>,
    /// Downloads the dictation fixtures into the cache, then stops (M8 Task 28, D10). Never a scenario.
    pub fetch_fixtures: bool,
}

fn value(it: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    it.next().ok_or_else(|| format!("{flag} needs a value"))
}

impl Cli {
    /// Parses the driver's arguments. `FLARE_E2E_PACKAGED=1` is the same as `--packaged`.
    pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Cli, String> {
        let packaged_var = concat!("FLARE_E2E", "_PACKAGED");
        let mut cli = Cli {
            packaged: std::env::var(packaged_var).is_ok_and(|v| v == "1"),
            ..Cli::default()
        };
        let mut it = args.into_iter();
        while let Some(a) = it.next() {
            match a.as_str() {
                "--list" => cli.list = true,
                "--packaged" => cli.packaged = true,
                "--keep" => cli.keep = true,
                "--fetch-fixtures" => cli.fetch_fixtures = true,
                "--exe" => cli.exe = Some(PathBuf::from(value(&mut it, "--exe")?)),
                "--shots" => cli.shots = Some(PathBuf::from(value(&mut it, "--shots")?)),
                "--run-one" => cli.run_one = Some(value(&mut it, "--run-one")?),
                "perf" => cli.perf = true,
                "--renderer" => cli.renderer = Some(value(&mut it, "--renderer")?),
                "--shell" => cli.shell = Some(value(&mut it, "--shell")?),
                "--out" => cli.out = Some(PathBuf::from(value(&mut it, "--out")?)),
                "--baseline" => cli.baseline = Some(PathBuf::from(value(&mut it, "--baseline")?)),
                "--runs" => {
                    cli.runs = Some(
                        value(&mut it, "--runs")?
                            .parse()
                            .map_err(|_| "--runs needs a whole number".to_string())?,
                    );
                }
                "--scale" => {
                    cli.scale = Some(
                        value(&mut it, "--scale")?
                            .parse()
                            .map_err(|_| "--scale needs a whole percent".to_string())?,
                    );
                }
                s if s.starts_with("--") => return Err(format!("unknown flag {s}")),
                s => cli.filters.push(s.to_string()),
            }
        }
        Ok(cli)
    }
}

/// The scenarios whose names contain any filter word (case-insensitive). No words selects all.
pub fn select<'a>(all: &'a [Scenario], filters: &[String]) -> Vec<&'a Scenario> {
    let words: Vec<String> = filters.iter().map(|w| w.to_lowercase()).collect();
    all.iter()
        .filter(|s| {
            let name = s.name.to_lowercase();
            words.is_empty() || words.iter().any(|w| name.contains(w.as_str()))
        })
        .collect()
}

/// What a run runs: the gate's scenarios that match, and (only when a filter names it) an opt-in one.
pub fn pick(filters: &[String]) -> Vec<&'static Scenario> {
    let mut picked = select(ALL, filters);
    if !filters.is_empty() {
        picked.extend(select(OPT_IN, filters));
    }
    picked
}

/// The app binary for this run: the installed layout with `--packaged`, else `--exe`, else the
/// release build.
pub fn exe_path(cli: &Cli) -> PathBuf {
    if cli.packaged {
        return match std::env::var("LOCALAPPDATA") {
            Ok(local) => PathBuf::from(local)
                .join("Flare")
                .join("current")
                .join("Flare.exe"),
            Err(_) => PathBuf::from(r"Flare\current\Flare.exe"),
        };
    }
    cli.exe
        .clone()
        .unwrap_or_else(|| PathBuf::from(DEFAULT_EXE))
}

/// A scenario's returned `Err` is always kept as a failure, even after earlier failures. Only a
/// failure whose text is exactly `e` (`not_ported` records its own text) is not said twice; a
/// substring match is not enough, or a longer earlier failure would drop the error. A timeout
/// still pending is folded into the error's one entry.
pub fn keep_error(ctx: &mut Ctx, e: &str) {
    if !ctx.has_pending_timeout() && ctx.fails.iter().any(|f| f == e) {
        return;
    }
    ctx.ok(false, e);
}

/// The failure a scenario that ran no check at all records: a scenario that only skips (or returns
/// early) proves nothing, and used to pass (M10 D21).
pub const NO_CHECKS: &str = "the scenario ran no checks (a skip is never a pass)";

/// Fails a scenario that finished without one check.
pub fn require_checks(ctx: &mut Ctx) {
    if ctx.checks == 0 {
        ctx.ok(false, NO_CHECKS);
    }
}

/// The child: runs one scenario in this process and prints its JSON lines. Exit 0 when it passed.
pub fn run_one(name: &str, cli: &Cli) -> i32 {
    let Some(s) = ALL.iter().chain(OPT_IN).find(|s| s.name == name) else {
        println!(
            "{}",
            json!({"pass": false, "msg": format!("no scenario named {name}")})
        );
        return 2;
    };
    let mut ctx = Ctx::new(s.name);
    ctx.json = true;
    ctx.exe = exe_path(cli);
    ctx.packaged = cli.packaged;
    if let Some(dir) = &cli.shots {
        ctx.shots = dir.clone();
    }
    match (s.run)(&mut ctx) {
        Ok(()) => {
            ctx.finish();
            require_checks(&mut ctx);
        }
        Err(e) => keep_error(&mut ctx, &e),
    }
    println!(
        "{}",
        json!({"done": true, "checks": ctx.checks, "fails": ctx.fails.len(), "skips": ctx.skips})
    );
    if ctx.fails.is_empty() {
        0
    } else {
        1
    }
}

/// How a supervised child ended.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Outcome {
    Exited(i32),
    TimedOut,
}

/// A supervised child: its stdout lines, how it ended, and its pid.
#[derive(Debug)]
pub struct Supervised {
    pub lines: Vec<String>,
    pub outcome: Outcome,
    pub pid: u32,
}

fn read_lines(stdout: impl std::io::Read + Send + 'static) -> Receiver<String> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            match line {
                Ok(l) => {
                    if tx.send(l).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    rx
}

fn drain(rx: &Receiver<String>, lines: &mut Vec<String>) {
    while let Ok(l) = rx.recv_timeout(DRAIN) {
        lines.push(l);
    }
}

/// Runs `cmd` with stdout piped and a time limit. On overrun the child is killed, and every
/// process holding a world root it announced is reaped. Each world root is removed unless
/// `keep`.
pub fn supervise(mut cmd: Command, limit: Duration, keep: bool) -> Supervised {
    cmd.stdout(Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(_) => {
            return Supervised {
                lines: Vec::new(),
                outcome: Outcome::Exited(-1),
                pid: 0,
            };
        }
    };
    let pid = child.id();
    let stdout = child.stdout.take().expect("stdout was piped");
    let rx = read_lines(stdout);
    let deadline = Instant::now() + limit;
    let mut lines = Vec::new();
    let outcome = loop {
        while let Ok(l) = rx.try_recv() {
            lines.push(l);
        }
        match child.try_wait() {
            Ok(Some(st)) => break Outcome::Exited(st.code().unwrap_or(-1)),
            Ok(None) => {}
            Err(_) => break Outcome::Exited(-1),
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break Outcome::TimedOut;
        }
        thread::sleep(POLL);
    };
    drain(&rx, &mut lines);
    for root in world_roots(&lines) {
        reap_by_profile(&root);
        if !keep {
            let _ = std::fs::remove_dir_all(&root);
        }
    }
    Supervised {
        lines,
        outcome,
        pid,
    }
}

/// Every world root the child announced, in order (a scenario that runs two copies announces two).
fn world_roots(lines: &[String]) -> Vec<PathBuf> {
    lines
        .iter()
        .filter_map(|l| match parse_line(l) {
            Line::World(w) => Some(PathBuf::from(w)),
            _ => None,
        })
        .collect()
}

/// One line the child printed.
#[derive(Debug, Clone, PartialEq)]
enum Line {
    Check { pass: bool, msg: String },
    Skip(String),
    World(String),
    Done,
    Other(String),
}

fn parse_line(line: &str) -> Line {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return Line::Other(line.to_string());
    };
    if let Some(pass) = v.get("pass").and_then(Value::as_bool) {
        let msg = v
            .get("msg")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        return Line::Check { pass, msg };
    }
    if let Some(why) = v.get("skip").and_then(Value::as_str) {
        return Line::Skip(why.to_string());
    }
    if let Some(w) = v.get("world").and_then(Value::as_str) {
        return Line::World(w.to_string());
    }
    if v.get("done").is_some() {
        return Line::Done;
    }
    Line::Other(line.to_string())
}

/// Runs one scenario in its own child and prints its lines in PT's format.
fn run_scenario(s: &Scenario, cli: &Cli, exe: &Path) -> Row {
    let me = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("flare-e2e.exe"));
    let mut cmd = Command::new(me);
    cmd.arg("--run-one").arg(s.name).arg("--exe").arg(exe);
    if cli.packaged {
        cmd.arg("--packaged");
    }
    if let Some(dir) = &cli.shots {
        cmd.arg("--shots").arg(dir);
    }
    let started = Instant::now();
    let sup = supervise(cmd, Duration::from_secs(s.limit_secs), cli.keep);
    let mut row = Row {
        name: s.name.to_string(),
        checks: 0,
        fails: 0,
        skips: 0,
        secs: 0.0,
    };
    let mut done = false;
    for line in &sup.lines {
        match parse_line(line) {
            Line::Check { pass, msg } => {
                row.checks += 1;
                if pass {
                    println!("  pass  {msg}");
                } else {
                    row.fails += 1;
                    println!("  FAIL  {msg}");
                }
            }
            Line::Skip(why) => {
                row.skips += 1;
                println!("  skip  {why}");
            }
            Line::Done => done = true,
            Line::World(_) => {}
            Line::Other(text) => println!("{text}"),
        }
    }
    match sup.outcome {
        Outcome::TimedOut => {
            row.fails += 1;
            println!("  FAIL  scenario timed out after {} s", s.limit_secs);
        }
        Outcome::Exited(code) if !done => {
            row.fails += 1;
            println!("  FAIL  scenario ended without a result (exit code {code})");
        }
        Outcome::Exited(_) => {}
    }
    row.secs = started.elapsed().as_secs_f64();
    row
}

/// The parent: runs every selected scenario, prints the table and returns the exit code.
pub fn run_all(cli: &Cli) -> i32 {
    let exe = exe_path(cli);
    if !exe.is_file() {
        println!(
            "{} is missing: run \"cargo build --release -p flare\" first.",
            exe.display()
        );
        return 1;
    }
    let picked = pick(&cli.filters);
    if picked.is_empty() {
        println!("no scenario matches {}", cli.filters.join(" "));
        return 1;
    }
    #[cfg(target_os = "macos")]
    let (picked, gated) = mac::split_gated(picked);
    #[cfg(target_os = "macos")]
    let mut clip = crate::guards::ClipboardGuard::new(crate::guards::MacPasteboard);
    #[cfg(target_os = "macos")]
    clip.begin();
    let mut rows = Vec::new();
    for s in picked {
        println!("\n{}", s.name);
        #[cfg(target_os = "macos")]
        let watch = mac::Watch::start();
        #[cfg(target_os = "macos")]
        let mut row = run_scenario(s, cli, &exe);
        #[cfg(not(target_os = "macos"))]
        let row = run_scenario(s, cli, &exe);
        #[cfg(target_os = "macos")]
        for problem in watch.stop() {
            row.fails += 1;
            println!("  FAIL  {problem}");
        }
        rows.push(row);
    }
    println!();
    print!("{}", report::table(&rows));
    #[cfg(target_os = "macos")]
    {
        let problems = clip.end();
        for p in &problems {
            println!("FAIL  {p}");
        }
        if !problems.is_empty() {
            return 1;
        }
        if !gated.is_empty() {
            println!(
                "{} gated on macOS (docs/e2e.md, \"macOS E2E\"):",
                gated.len()
            );
            for (name, why) in &gated {
                println!("  gated  {name}: {why}");
            }
        }
    }
    let skips: u32 = rows.iter().map(|r| r.skips).sum();
    if skips > 0 {
        println!("{skips} skipped (a skip is never a pass)");
    }
    let failed = rows.iter().filter(|r| r.fails > 0).count();
    if failed > 0 {
        println!("{failed} scenario(s) FAILED");
        1
    } else {
        println!("all scenarios passed");
        0
    }
}

/// macOS (rules 331 and 332): the gate and the screen guard around each scenario.
#[cfg(target_os = "macos")]
pub mod mac {
    use std::collections::{BTreeSet, HashSet};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread::JoinHandle;
    use std::time::Duration;

    use crate::ctx::Scenario;
    use crate::scenarios::mac_gate;

    /// The picked scenarios a Mac run starts, and the gated ones with their reasons (never run, never a pass).
    pub fn split_gated(
        picked: Vec<&'static Scenario>,
    ) -> (Vec<&'static Scenario>, Vec<(&'static str, &'static str)>) {
        let mut run = Vec::new();
        let mut gated = Vec::new();
        for s in picked {
            match mac_gate::reason(s.name) {
                Some(why) => gated.push((s.name, why)),
                None => run.push(s),
            }
        }
        (run, gated)
    }

    /// How often the guard looks at the desktop while a scenario runs.
    const EVERY: Duration = Duration::from_millis(200);

    /// Pure: what one look found wrong, given the run's processes, the frontmost app and how many
    /// windows of the run show on a display.
    pub fn look(run: &HashSet<u32>, front: Option<u32>, shown: &[String]) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(pid) = front.filter(|p| run.contains(p)) {
            out.push(format!("the scenario became the frontmost app (pid {pid})"));
        }
        for w in shown {
            out.push(format!("the scenario showed a window on a display: {w}"));
        }
        out
    }

    /// The run's processes: every `--run-one` child of this runner and everything below it.
    fn run_processes() -> HashSet<u32> {
        let me = std::process::id();
        let rows = crate::proc::process_rows();
        let roots: Vec<u32> = rows
            .iter()
            .filter(|(_, ppid, cmd)| *ppid == me && cmd.contains("--run-one"))
            .map(|(pid, _, _)| *pid)
            .collect();
        let table: Vec<(u32, u32)> = rows.iter().map(|(p, pp, _)| (*p, *pp)).collect();
        crate::guards::run_set(&roots, &table)
    }

    /// The screen guard: a thread that looks every 200 ms while a scenario runs. A Mac run must never
    /// take the keyboard or show a window (the Windows foreground guard's job, plus the window server's
    /// list, since a Mac has no hidden desktop).
    pub struct Watch {
        stop: Arc<AtomicBool>,
        found: Arc<Mutex<BTreeSet<String>>>,
        thread: Option<JoinHandle<()>>,
    }

    impl Watch {
        pub fn start() -> Watch {
            let stop = Arc::new(AtomicBool::new(false));
            let found = Arc::new(Mutex::new(BTreeSet::new()));
            let (s, f) = (Arc::clone(&stop), Arc::clone(&found));
            let thread = std::thread::spawn(move || {
                while !s.load(Ordering::SeqCst) {
                    let run = run_processes();
                    if !run.is_empty() {
                        let pids: Vec<u32> = run.iter().copied().collect();
                        let shown: Vec<String> = crate::mac::shown_windows_of(&pids)
                            .iter()
                            .map(|w| {
                                let (x, y, ww, h) = w.bounds;
                                format!(
                                    "{} (pid {}), level {}, at ({x}, {y}) size {ww} by {h}",
                                    w.owner, w.pid, w.layer
                                )
                            })
                            .collect();
                        let problems = look(&run, crate::mac::frontmost_pid(), &shown);
                        if let Ok(mut f) = f.lock() {
                            f.extend(problems);
                        }
                    }
                    std::thread::sleep(EVERY);
                }
            });
            Watch {
                stop,
                found,
                thread: Some(thread),
            }
        }

        /// Stops the guard and returns what it found, each problem once.
        pub fn stop(mut self) -> Vec<String> {
            self.stop.store(true, Ordering::SeqCst);
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
            self.found
                .lock()
                .map(|f| f.iter().cloned().collect())
                .unwrap_or_default()
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn the_guard_flags_a_window_or_the_keyboard_of_the_run_and_only_those() {
            let run: HashSet<u32> = [10, 11].into_iter().collect();
            let two = ["a".to_string(), "b".to_string()];
            assert!(
                look(&run, Some(99), &[]).is_empty(),
                "another app in front is not a failure"
            );
            assert!(look(&run, None, &[]).is_empty());
            assert_eq!(look(&run, Some(11), &[]).len(), 1);
            assert_eq!(look(&run, Some(99), &two).len(), 2);
            assert_eq!(look(&run, Some(10), &two[..1]).len(), 2);
        }

        #[test]
        fn a_gated_scenario_is_never_started() {
            let picked = crate::runner::pick(&[]);
            let total = picked.len();
            let (run, gated) = split_gated(picked);
            assert_eq!(run.len() + gated.len(), total);
            assert!(run.iter().all(|s| mac_gate::reason(s.name).is_none()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_world_a_scenario_announces_is_found_not_only_the_first() {
        let lines = vec![
            r#"{"world":"C:\\t\\flare-e2e-a"}"#.to_string(),
            r#"{"pass":true,"msg":"x"}"#.to_string(),
            r#"{"world":"C:\\t\\flare-e2e-b"}"#.to_string(),
            "not json".to_string(),
        ];
        let roots = world_roots(&lines);
        assert_eq!(
            roots,
            [
                PathBuf::from(r"C:\t\flare-e2e-a"),
                PathBuf::from(r"C:\t\flare-e2e-b")
            ]
        );
        assert!(world_roots(&[]).is_empty());
    }

    #[test]
    fn fetch_fixtures_is_a_flag_of_its_own_and_runs_no_scenario() {
        let cli = Cli::parse(["--fetch-fixtures".to_string()]).expect("the flag parses");
        assert!(cli.fetch_fixtures);
        assert!(cli.filters.is_empty());
        assert!(!cli.list && cli.run_one.is_none() && !cli.perf);
        assert!(
            !Cli::parse(Vec::<String>::new())
                .expect("no args")
                .fetch_fixtures
        );
    }
}

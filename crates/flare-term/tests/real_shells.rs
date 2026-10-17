//! Real-shell tests through the rig: prompts, folder reports, environment, cd lines, exit.

mod common;

use common::{Rig, process_alive, temp_dir_named};
use flare_shells::cwd::cd_command;
use flare_shells::detect::{RealProbe, detect_shells_with};
use flare_shells::env::pty_env;
use flare_shells::list::ShellDef;
use flare_shells::sys::sys_tools;
use flare_term::emulator::TermEvent;
use std::path::Path;
use std::time::{Duration, Instant};

#[test]
fn rig_runs_cmd_and_sees_the_prompt() {
    let dir = temp_dir_named("rig-prompt");
    let rig = Rig::spawn("cmd.exe", &[], dir.path(), None, 80, 24);
    let seen = rig.wait_for(10_000, |e| {
        (0..e.rows()).any(|r| e.row_text(i32::try_from(r).unwrap_or(0)).ends_with('>'))
    });
    assert!(
        seen,
        "cmd never printed a prompt; screen: {:?}",
        rig.screen()
    );
}

#[test]
fn rig_drop_kills_the_shell() {
    let dir = temp_dir_named("rig-drop");
    let rig = Rig::spawn("cmd.exe", &[], dir.path(), None, 80, 24);
    let pid = rig.pid();
    assert!(pid != 0, "the shell reported no process id");
    assert!(process_alive(pid), "cmd was not running before the drop");
    drop(rig);
    let deadline = Instant::now() + Duration::from_secs(5);
    while process_alive(pid) {
        assert!(
            Instant::now() < deadline,
            "cmd (pid {pid}) still ran 5 s after the rig was dropped"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

// ---- Task 24: real shells, their folder reports and their environment ----

fn std_env() -> Vec<(String, String)> {
    std::env::vars().collect()
}

fn shell_def(id: &str) -> Option<ShellDef> {
    detect_shells_with(&RealProbe::default(), sys_tools(), "C:\\")
        .into_iter()
        .find(|d| d.id == id)
}

fn norm_path(p: &str) -> String {
    p.strip_prefix(r"\\?\")
        .unwrap_or(p)
        .trim_end_matches('\\')
        .to_lowercase()
}

/// Every folder the shell has reported so far, in order.
fn cwds(rig: &Rig) -> Vec<String> {
    rig.events
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .filter_map(|e| match e {
            TermEvent::Cwd(p) => Some(p.clone()),
            _ => None,
        })
        .collect()
}

fn has_cwd(rig: &Rig, dir: &Path) -> bool {
    let want = norm_path(&dir.to_string_lossy());
    cwds(rig).iter().any(|p| norm_path(p) == want)
}

fn wait_cwd(rig: &Rig, dir: &Path, ms: u64) -> bool {
    let deadline = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < deadline {
        if has_cwd(rig, dir) {
            return true;
        }
        rig.wait_for(50, |_| false);
    }
    has_cwd(rig, dir)
}

fn spawn_def(def: &ShellDef, dir: &Path) -> Rig {
    let env = pty_env(&std_env(), Some(def.id.as_str()), None, None, Some("tok"));
    let args: Vec<&str> = def.args.iter().map(String::as_str).collect();
    Rig::spawn(&def.exe, &args, dir, Some(env), 100, 30)
}

fn any_row(rig: &Rig, pred: impl Fn(&str) -> bool) -> bool {
    rig.screen().iter().any(|r| pred(r))
}

#[test]
fn cmd_reports_its_folder_with_osc_9_9() {
    let dir = temp_dir_named("flare-m2-cwd");
    let env = pty_env(&std_env(), Some("cmd"), None, None, Some("tok"));
    let rig = Rig::spawn(&sys_tools().cmd, &[], dir.path(), Some(env), 100, 30);
    assert!(
        wait_cwd(&rig, dir.path(), 10_000),
        "no Cwd for {:?}; got {:?}; screen {:?}",
        dir.path(),
        cwds(&rig),
        rig.screen()
    );
}

fn shell_reports_its_folder(id: &str) {
    let Some(def) = shell_def(id) else {
        println!("skipped: no {id}");
        return;
    };
    let dir = temp_dir_named("flare-m2-cwd");
    let rig = spawn_def(&def, dir.path());
    assert!(
        wait_cwd(&rig, dir.path(), 15_000),
        "{id}: no Cwd for {:?}; got {:?}; screen {:?}",
        dir.path(),
        cwds(&rig),
        rig.screen()
    );
    assert!(
        rig.wait_for(5_000, |e| (0..e.rows()).any(|r| e
            .row_text(i32::try_from(r).unwrap_or(0))
            .starts_with("PS "))),
        "{id}: no `PS ` prompt row; screen {:?}",
        rig.screen()
    );
}

#[test]
fn powershell_reports_its_folder_with_osc_9_9() {
    shell_reports_its_folder("powershell");
}

#[test]
fn pwsh_reports_its_folder_with_osc_9_9() {
    shell_reports_its_folder("pwsh");
}

#[test]
fn the_environment_reaches_the_shell() {
    let dir = temp_dir_named("flare-m2-env");
    let get = |k: &str| std::env::var(k).unwrap_or_default();
    let from = vec![
        ("CLAUDECODE".to_string(), "1".to_string()),
        ("FLARE_TAB_ID".to_string(), "old".to_string()),
        ("TERM".to_string(), "dumb".to_string()),
        ("PATH".to_string(), get("PATH")),
        ("SystemRoot".to_string(), get("SystemRoot")),
        ("ComSpec".to_string(), get("ComSpec")),
    ];
    let env = pty_env(&from, Some("cmd"), None, None, Some("tok-7"));
    let rig = Rig::spawn(&sys_tools().cmd, &[], dir.path(), Some(env), 100, 30);
    assert!(
        wait_cwd(&rig, dir.path(), 10_000),
        "cmd never reached its prompt; screen {:?}",
        rig.screen()
    );
    rig.type_str("echo [%TERM%][%COLORTERM%][%FLARE_TAB_ID%][%CLAUDECODE%]\r");
    let want = "[xterm-256color][truecolor][tok-7][%CLAUDECODE%]";
    assert!(
        rig.wait_for(10_000, |e| {
            (0..e.rows()).any(|r| e.row_text(i32::try_from(r).unwrap_or(0)).trim_end() == want)
        }),
        "no output row equal to {want}; screen {:?}",
        rig.screen()
    );
}

fn cd_line_moves(id: &str) {
    let Some(def) = shell_def(id) else {
        println!("skipped: no {id}");
        return;
    };
    let a = temp_dir_named("flare-m2-cd-a");
    let b = temp_dir_named("flare-m2-cd-b");
    let rig = spawn_def(&def, a.path());
    assert!(
        wait_cwd(&rig, a.path(), 15_000),
        "{id}: no first Cwd; screen {:?}",
        rig.screen()
    );
    let line = cd_command(Some(id), &b.path().to_string_lossy()).expect("the shell has a cd line");
    rig.type_str(&line);
    assert!(
        wait_cwd(&rig, b.path(), 10_000),
        "{id}: no Cwd for the new folder; got {:?}; screen {:?}",
        cwds(&rig),
        rig.screen()
    );
}

#[test]
fn the_cd_line_moves_a_powershell() {
    cd_line_moves("powershell");
}

#[test]
fn the_cd_line_moves_cmd() {
    cd_line_moves("cmd");
}

#[test]
fn a_cmd_prompt_with_text_typed_is_not_mistaken_for_a_report() {
    let dir = temp_dir_named("flare-m2-typed");
    let env = pty_env(&std_env(), Some("cmd"), None, None, Some("tok"));
    let rig = Rig::spawn(&sys_tools().cmd, &[], dir.path(), Some(env), 100, 30);
    assert!(wait_cwd(&rig, dir.path(), 10_000), "no first Cwd");
    rig.wait_quiet(300);
    let before = cwds(&rig).len();
    rig.type_str("abc");
    assert!(
        rig.wait_for(3_000, |e| (0..e.rows())
            .any(|r| e.row_text(i32::try_from(r).unwrap_or(0)).contains("abc"))),
        "the typed text never showed; screen {:?}",
        rig.screen()
    );
    rig.wait_quiet(600);
    assert_eq!(
        cwds(&rig).len(),
        before,
        "typing produced a folder report; got {:?}",
        cwds(&rig)
    );
    assert!(any_row(&rig, |r| r.contains("abc")));
}

#[test]
fn a_shell_that_exits_ends_the_process() {
    let dir = temp_dir_named("flare-m2-exit");
    let mut rig = Rig::spawn("cmd.exe", &[], dir.path(), None, 80, 24);
    assert!(
        rig.wait_for(10_000, |e| (0..e.rows())
            .any(|r| e.row_text(i32::try_from(r).unwrap_or(0)).ends_with('>'))),
        "no prompt; screen {:?}",
        rig.screen()
    );
    rig.type_str("exit\r");
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut code = None;
    while Instant::now() < deadline {
        code = rig.exit_code();
        if code.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(code, Some(0), "screen {:?}", rig.screen());
}

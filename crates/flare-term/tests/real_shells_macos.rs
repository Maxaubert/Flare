//! Real shells on macOS, each in a real pty: zsh, bash, fish and pwsh report their folder with
//! OSC 7 at the first prompt and again after every cd, start as login shells (PATH from
//! /etc/paths even from a launchd-sized environment), read the user's own startup files, and
//! get Flare's variables. The macOS twin of `real_shells.rs` (cmd and PowerShell, OSC 9;9).
//! zsh and bash ship with every Mac; fish and pwsh are skipped, with a message, when missing.
//!
//! Every shell runs with a fresh HOME, so the tests read a known set of dotfiles and never the
//! real user's.
#![cfg(not(windows))]

mod common;

use common::{Rig, temp_dir_named};
use flare_shells::cwd::cd_command;
use flare_shells::env::pty_env;
use flare_shells::list::ShellDef;
use flare_shells::macos::detect::{RealHost, detect_with};
use flare_shells::macos::integration;
use flare_term::emulator::TermEvent;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// This process's integration folder, under the target folder (never the real Application Support).
fn integration_dir() -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("shell integration {}", std::process::id()));
    assert!(integration::activate(&dir), "the integration installs");
    dir
}

fn shell_def(id: &str) -> Option<ShellDef> {
    let dir = integration_dir();
    detect_with(&RealHost, Some(&dir))
        .into_iter()
        .find(|d| d.id == id)
}

/// The environment a Flare started by launchd (from Finder or the Dock) has: no locale, no
/// terminal, a four-folder PATH.
fn launchd_env(home: &Path) -> Vec<(String, String)> {
    let mut env = vec![
        ("HOME".to_string(), home.to_string_lossy().into_owned()),
        (
            "PATH".to_string(),
            "/usr/bin:/bin:/usr/sbin:/sbin".to_string(),
        ),
        (
            "TMPDIR".to_string(),
            std::env::temp_dir().to_string_lossy().into_owned(),
        ),
    ];
    for k in ["USER", "LOGNAME", "SHELL"] {
        if let Ok(v) = std::env::var(k) {
            env.push((k.to_string(), v));
        }
    }
    env
}

fn spawn(def: &ShellDef, dir: &Path, from: &[(String, String)]) -> Rig {
    let env = pty_env(from, Some(def.id.as_str()), None, None, Some("tok"), None);
    let args: Vec<&str> = def.args.iter().map(String::as_str).collect();
    Rig::spawn(&def.exe, &args, dir, Some(env), 200, 40)
}

/// A folder as comparable text (`/var` is `/private/var` on macOS).
fn canon(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

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

/// Waits until the LAST folder report names `dir`.
fn wait_cwd(rig: &Rig, dir: &Path, ms: u64) -> bool {
    let want = canon(dir);
    let deadline = Instant::now() + Duration::from_millis(ms);
    loop {
        if cwds(rig)
            .last()
            .is_some_and(|p| canon(Path::new(p)) == want)
        {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        rig.wait_for(50, |_| false);
    }
}

/// Waits for an output line equal to `want` (the typed command echoes differently: it holds `$`).
fn wait_line(rig: &Rig, ms: u64, pred: impl Fn(&str) -> bool) -> Option<String> {
    let deadline = Instant::now() + Duration::from_millis(ms);
    loop {
        if let Some(l) = rig.logical_lines().into_iter().find(|l| pred(l.trim())) {
            return Some(l.trim().to_string());
        }
        if Instant::now() >= deadline {
            return None;
        }
        rig.wait_for(50, |_| false);
    }
}

/// The folder report arrives at the first prompt and follows two cd lines (one typed by Flare
/// through `cd_command`, into a folder with a space, a quote and a non-ASCII letter).
fn reports_its_folder_and_follows_cd(id: &str) {
    let Some(def) = shell_def(id) else {
        eprintln!("skipped: {id} is not installed on this Mac");
        return;
    };
    let home = temp_dir_named("home");
    let work = temp_dir_named("work");
    let odd = work.path().join("a b'c æ");
    std::fs::create_dir(&odd).unwrap();
    let rig = spawn(&def, work.path(), &launchd_env(home.path()));
    assert!(
        wait_cwd(&rig, work.path(), 20_000),
        "{id}: no OSC 7 for {:?}; got {:?}; screen {:?}",
        work.path(),
        cwds(&rig),
        rig.screen()
    );
    let line = cd_command(Some(id), &odd.to_string_lossy()).expect("a cd line");
    rig.type_str(&line);
    assert!(
        wait_cwd(&rig, &odd, 10_000),
        "{id}: the report did not follow cd; got {:?}; screen {:?}",
        cwds(&rig),
        rig.screen()
    );
    rig.type_str("cd ..\r");
    assert!(
        wait_cwd(&rig, work.path(), 10_000),
        "{id}: the report did not follow cd ..; got {:?}",
        cwds(&rig)
    );
}

/// From a launchd environment the shell still has the login PATH (/etc/paths through
/// path_helper) and Flare's terminal variables.
fn starts_as_a_login_shell_with_flares_variables(id: &str, echo: &str) {
    let Some(def) = shell_def(id) else {
        eprintln!("skipped: {id} is not installed on this Mac");
        return;
    };
    let home = temp_dir_named("home");
    let work = temp_dir_named("work");
    let rig = spawn(&def, work.path(), &launchd_env(home.path()));
    assert!(wait_cwd(&rig, work.path(), 20_000), "{id}: no first prompt");
    rig.type_str(echo);
    let got = wait_line(&rig, 10_000, |l| {
        l.starts_with("FLARE=") && !l.contains('$')
    })
    .unwrap_or_else(|| panic!("{id}: no answer; screen {:?}", rig.screen()));
    let fields: Vec<&str> = got.split('|').collect();
    assert_eq!(fields[0], "FLARE=Flare", "{got}");
    assert_eq!(fields[1], "xterm-256color", "{got}");
    assert_eq!(fields[2], "truecolor", "{got}");
    assert_eq!(fields[3], "en_US.UTF-8", "{got}");
    let path: Vec<&str> = fields[4].split(':').collect();
    assert!(
        path.contains(&"/usr/local/bin"),
        "{id}: /etc/paths never reached PATH: {}",
        fields[4]
    );
    // Apple's bash would open every tab with its "default interactive shell is now zsh" notice.
    assert!(
        !rig.screen()
            .iter()
            .any(|l| l.contains("default interactive shell")),
        "{id}: {:?}",
        rig.screen()
    );
}

#[test]
fn zsh_reports_its_folder_and_follows_cd() {
    reports_its_folder_and_follows_cd("zsh");
}

#[test]
fn bash_reports_its_folder_and_follows_cd() {
    reports_its_folder_and_follows_cd("bash");
}

#[test]
fn fish_reports_its_folder_and_follows_cd() {
    reports_its_folder_and_follows_cd("fish");
}

#[test]
fn pwsh_reports_its_folder_and_follows_cd() {
    reports_its_folder_and_follows_cd("pwsh");
}

const POSIX_ECHO: &str = "printf 'FLARE=%s|%s|%s|%s|%s\\n' \"$TERM_PROGRAM\" \"$TERM\" \"$COLORTERM\" \"$LANG\" \"$PATH\"\r";

#[test]
fn zsh_starts_as_a_login_shell_with_flares_variables() {
    starts_as_a_login_shell_with_flares_variables("zsh", POSIX_ECHO);
}

#[test]
fn bash_starts_as_a_login_shell_with_flares_variables() {
    starts_as_a_login_shell_with_flares_variables("bash", POSIX_ECHO);
}

#[test]
fn fish_starts_as_a_login_shell_with_flares_variables() {
    starts_as_a_login_shell_with_flares_variables(
        "fish",
        "printf 'FLARE=%s|%s|%s|%s|%s\\n' \"$TERM_PROGRAM\" \"$TERM\" \"$COLORTERM\" \"$LANG\" (string join : $PATH)\r",
    );
}

#[test]
fn pwsh_starts_as_a_login_shell_with_flares_variables() {
    starts_as_a_login_shell_with_flares_variables(
        "pwsh",
        "Write-Output ('FLARE=' + (@($env:TERM_PROGRAM, $env:TERM, $env:COLORTERM, $env:LANG, $env:PATH) -join '|'))\r",
    );
}

/// zsh through the ZDOTDIR shim still reads the user's four startup files, in zsh's order, from
/// the user's own ZDOTDIR, and hands that ZDOTDIR back to the shell.
#[test]
fn zsh_reads_the_users_startup_files_from_their_own_zdotdir() {
    let def = shell_def("zsh").expect("zsh ships with macOS");
    let home = temp_dir_named("home");
    let zdot = home.path().join(".config/zsh");
    std::fs::create_dir_all(&zdot).unwrap();
    let log = home.path().join("order");
    for f in [".zshenv", ".zprofile", ".zshrc", ".zlogin"] {
        std::fs::write(
            zdot.join(f),
            format!("print -r -- {f} >> '{}'\n", log.display()),
        )
        .unwrap();
    }
    // The user's own precmd hook keeps running beside Flare's.
    std::fs::write(
        zdot.join(".zshrc"),
        format!(
            "print -r -- .zshrc >> '{}'\nexport FLARE_TEST_RC=yes\nmine() {{ FLARE_TEST_PC=ran }}\nprecmd_functions+=(mine)\n",
            log.display()
        ),
    )
    .unwrap();
    // A HOME .zshrc that must NOT be read (the user's ZDOTDIR wins).
    std::fs::write(home.path().join(".zshrc"), "export FLARE_TEST_RC=wrong\n").unwrap();
    let work = temp_dir_named("work");
    let mut env = launchd_env(home.path());
    env.push(("ZDOTDIR".to_string(), zdot.to_string_lossy().into_owned()));
    let rig = spawn(&def, work.path(), &env);
    assert!(
        wait_cwd(&rig, work.path(), 20_000),
        "no OSC 7 with the user's own startup files; screen {:?}",
        rig.screen()
    );
    rig.type_str(
        "print -r -- \"Z=${ZDOTDIR-unset}|$FLARE_TEST_RC|${FLARE_USER_ZDOTDIR-gone}|$FLARE_TEST_PC\"\r",
    );
    let got = wait_line(&rig, 10_000, |l| l.starts_with("Z=") && !l.contains('$'))
        .unwrap_or_else(|| panic!("no answer; screen {:?}", rig.screen()));
    assert_eq!(got, format!("Z={}|yes|gone|ran", zdot.display()));
    let order = std::fs::read_to_string(&log).unwrap();
    assert_eq!(
        order.lines().collect::<Vec<_>>(),
        [".zshenv", ".zprofile", ".zshrc", ".zlogin"]
    );
}

/// Without a ZDOTDIR of the user's, zsh reads HOME's files and the shim leaves no ZDOTDIR behind.
#[test]
fn zsh_without_a_zdotdir_reads_home_and_leaves_none_set() {
    let def = shell_def("zsh").expect("zsh ships with macOS");
    let home = temp_dir_named("home");
    std::fs::write(home.path().join(".zshrc"), "export FLARE_TEST_RC=home\n").unwrap();
    let work = temp_dir_named("work");
    let rig = spawn(&def, work.path(), &launchd_env(home.path()));
    assert!(wait_cwd(&rig, work.path(), 20_000), "no first prompt");
    rig.type_str("print -r -- \"Z=${ZDOTDIR-unset}|$FLARE_TEST_RC\"\r");
    let got = wait_line(&rig, 10_000, |l| l.starts_with("Z=") && !l.contains('$'))
        .unwrap_or_else(|| panic!("no answer; screen {:?}", rig.screen()));
    assert_eq!(got, "Z=unset|home");
}

/// bash through the rcfile reads what a login bash reads: the first of ~/.bash_profile,
/// ~/.bash_login and ~/.profile, and not ~/.bashrc on its own.
#[test]
fn bash_reads_the_users_login_files() {
    let def = shell_def("bash").expect("bash ships with macOS");
    let home = temp_dir_named("home");
    std::fs::write(
        home.path().join(".bash_profile"),
        "export FLARE_TEST_RC=profile\nPROMPT_COMMAND='FLARE_TEST_PC=ran'\n",
    )
    .unwrap();
    std::fs::write(home.path().join(".profile"), "export FLARE_TEST_RC=wrong\n").unwrap();
    std::fs::write(
        home.path().join(".bashrc"),
        "export FLARE_TEST_BASHRC=read\n",
    )
    .unwrap();
    let work = temp_dir_named("work");
    let rig = spawn(&def, work.path(), &launchd_env(home.path()));
    assert!(wait_cwd(&rig, work.path(), 20_000), "no first prompt");
    rig.type_str("echo \"B=$FLARE_TEST_RC|${FLARE_TEST_BASHRC-unread}|$FLARE_TEST_PC\"\r");
    let got = wait_line(&rig, 10_000, |l| l.starts_with("B=") && !l.contains('$'))
        .unwrap_or_else(|| panic!("no answer; screen {:?}", rig.screen()));
    // The user's PROMPT_COMMAND still runs beside Flare's hook.
    assert_eq!(got, "B=profile|unread|ran");
}

/// The hook keeps `$?`: a prompt that shows the last exit status still sees the command's.
#[test]
fn the_bash_and_zsh_hooks_keep_the_exit_status() {
    for id in ["bash", "zsh"] {
        let def = shell_def(id).expect("ships with macOS");
        let home = temp_dir_named("home");
        let work = temp_dir_named("work");
        let rig = spawn(&def, work.path(), &launchd_env(home.path()));
        assert!(wait_cwd(&rig, work.path(), 20_000), "{id}: no first prompt");
        let before = cwds(&rig).len();
        rig.type_str("(exit 7)\r");
        let deadline = Instant::now() + Duration::from_secs(10);
        while cwds(&rig).len() == before && Instant::now() < deadline {
            rig.wait_for(50, |_| false);
        }
        rig.type_str("echo \"S=$?\"\r");
        let got = wait_line(&rig, 10_000, |l| l.starts_with("S=") && !l.contains('$'))
            .unwrap_or_else(|| panic!("{id}: no answer; screen {:?}", rig.screen()));
        assert_eq!(got, "S=7", "{id}");
    }
}

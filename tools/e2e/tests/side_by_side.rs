//! M9 Task 37: Flare beside Prism Terminal. No shared lock, profile, verb, plugin or signal.
//!
//! The other product's names are DATA, read from `names.json` (the fixture Task 2 made), never
//! spelled in this file.
//!
//! Two layers. Layer A runs everywhere: the sandbox plays the owner's machine, a stand-in for the
//! neighbour is written into it with the neighbour's real names, and the built `flare.exe` is
//! launched beside it. Layer B (`#[ignore]`, and a no-op unless the install switch is on and the
//! run is on a CI runner) does the two real installs on a clean, throwaway runner.
//!
//! The exe comes from `FLARE_EXE` (absolute, or relative to any folder above this crate), else the
//! `flare.exe` next to the test binary's target folder.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use flare_e2e::proc::{Child, pid_alive, reap_by_profile};
use flare_e2e::regsandbox::{RealVerbs, RegProbe};
use flare_e2e::sandbox::Sandbox;
use flare_win::names;
use flare_win::registry::{Registry, WinRegistry};
use flare_win::single_instance::instance_names;
use serde_json::Value;
use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::GetCurrentProcessId;

const WAIT: Duration = Duration::from_secs(40);

// ---- the neighbour's names (data) -----------------------------------------------------------------

fn pt_names() -> Value {
    let text = include_str!("../../../crates/flare-win/tests/fixtures/names.json");
    let v: Value = serde_json::from_str(text).expect("names.json is JSON");
    v["pt"].clone()
}

fn pt(v: &Value, key: &str) -> String {
    v[key]
        .as_str()
        .unwrap_or_else(|| panic!("names.json has no {key}"))
        .to_string()
}

// ---- small helpers --------------------------------------------------------------------------------

fn flare_exe() -> PathBuf {
    if let Ok(p) = std::env::var("FLARE_EXE") {
        let p = PathBuf::from(p);
        if p.is_absolute() && p.is_file() {
            return p;
        }
        let mut bases: Vec<PathBuf> = vec![std::env::current_dir().unwrap_or_default()];
        bases.extend(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .ancestors()
                .map(Path::to_path_buf),
        );
        for b in bases {
            let c = b.join(&p);
            if c.is_file() {
                return c;
            }
        }
        panic!("FLARE_EXE={} is not a file", p.display());
    }
    let me = std::env::current_exe().expect("current exe");
    for a in me.ancestors() {
        for c in [a.join("flare.exe"), a.join("debug").join("flare.exe")] {
            if c.is_file() {
                return c;
            }
        }
    }
    panic!("flare.exe not found: build it first (cargo build -p flare)");
}

fn wait_until(timeout: Duration, f: impl Fn() -> bool) -> bool {
    let end = Instant::now() + timeout;
    loop {
        if f() {
            return true;
        }
        if Instant::now() >= end {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn base_command(exe: &Path, args: &[&str], env: &[(String, String)]) -> Command {
    let mut cmd = Command::new(exe);
    cmd.args(args)
        .envs(env.iter().cloned())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    cmd
}

/// Runs to the end within `timeout` and returns the exit code; None (and a kill) on a timeout.
fn run_to_end(mut cmd: Command, timeout: Duration) -> Option<i32> {
    let mut p = cmd.spawn().expect("spawn");
    let end = Instant::now() + timeout;
    loop {
        if let Ok(Some(s)) = p.try_wait() {
            return Some(s.code().unwrap_or(-1));
        }
        if Instant::now() >= end {
            let _ = p.kill();
            let _ = p.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Ids of the live processes named `exe_name` whose parent is `parent`.
fn children_named(parent: u32, exe_name: &str) -> Vec<u32> {
    let mut out = Vec::new();
    // SAFETY: a plain process snapshot; the handle is closed below.
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap == INVALID_HANDLE_VALUE {
        return out;
    }
    // SAFETY: PROCESSENTRY32W is plain data.
    let mut e: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    e.dwSize = size_of::<PROCESSENTRY32W>() as u32;
    // SAFETY: e is sized and snap is valid.
    let mut ok = unsafe { Process32FirstW(snap, &mut e) };
    while ok != 0 {
        let len = e.szExeFile.iter().position(|c| *c == 0).unwrap_or(0);
        let name = String::from_utf16_lossy(&e.szExeFile[..len]);
        if e.th32ParentProcessID == parent && name.eq_ignore_ascii_case(exe_name) {
            out.push(e.th32ProcessID);
        }
        // SAFETY: as above.
        ok = unsafe { Process32NextW(snap, &mut e) };
    }
    // SAFETY: closes the snapshot once.
    unsafe {
        CloseHandle(snap);
    }
    out
}

fn my_pid() -> u32 {
    // SAFETY: no arguments.
    unsafe { GetCurrentProcessId() }
}

fn hkcu() -> WinRegistry {
    WinRegistry::current_user()
}

/// Every file and folder below `root`, relative, forward slashes.
fn walk(root: &Path) -> BTreeSet<String> {
    fn go(base: &Path, dir: &Path, out: &mut BTreeSet<String>) {
        let Ok(rd) = fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            let rel = p
                .strip_prefix(base)
                .unwrap_or(&p)
                .to_string_lossy()
                .replace('\\', "/");
            out.insert(rel);
            if p.is_dir() {
                go(base, &p, out);
            }
        }
    }
    let mut out = BTreeSet::new();
    go(root, root, &mut out);
    out
}

// ---- the neighbour stand-in -----------------------------------------------------------------------

struct StandIn {
    exe: PathBuf,
    /// The three trees to hash: profile, dictation folder, install folder.
    trees: Vec<PathBuf>,
}

fn verb_key_paths(reg_root: &str, verb: &str) -> [String; 2] {
    [
        format!("{reg_root}\\Classes\\Directory\\shell\\{verb}"),
        format!("{reg_root}\\Classes\\Directory\\Background\\shell\\{verb}"),
    ]
}

fn write_standin(sb: &Sandbox, v: &Value) -> StandIn {
    let profile = sb.roaming.join(pt(v, "profileFolder"));
    fs::create_dir_all(profile.join("logs")).unwrap();
    fs::write(profile.join("tabs.json"), b"{\"tabs\":[1,2,3]}").unwrap();
    fs::write(profile.join("window.json"), b"{\"w\":800,\"h\":600}").unwrap();
    fs::write(profile.join("logs").join("diag.jsonl"), b"{\"t\":1}\n").unwrap();

    let dict = sb.local.join(pt(v, "dictationFolder"));
    fs::create_dir_all(&dict).unwrap();
    fs::write(dict.join("marker.bin"), [0u8, 1, 2, 3, 4, 5]).unwrap();

    let install = sb.local.join("Programs").join(pt(v, "profileFolder"));
    fs::create_dir_all(&install).unwrap();
    let exe = install.join(pt(v, "exeName"));
    let system = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
    fs::copy(Path::new(&system).join("System32").join("cmd.exe"), &exe).unwrap();

    let label = pt(v, "verbLabel");
    let keys = verb_key_paths(&sb.reg_root, &pt(v, "verbKey"));
    for (key, arg) in keys.iter().zip(["%1", "%V"]) {
        // Written below the sandbox root through the sandbox's own writer.
        let sub = key.strip_prefix(&format!("{}\\", sb.reg_root)).unwrap();
        sb.registry().set(sub, None, &label);
        sb.registry()
            .set(sub, Some("Icon"), &format!("{},0", exe.display()));
        sb.registry().set(
            &format!("{sub}\\command"),
            None,
            &format!("\"{}\" \"{arg}\"", exe.display()),
        );
    }
    StandIn {
        trees: vec![profile, dict, install],
        exe,
    }
}

/// (key, value name, value) for a verb's two keys: default, Icon, and the command.
fn verb_values(reg_root: &str, verb: &str) -> Vec<(String, String, Option<String>)> {
    let reg = hkcu();
    let mut out = Vec::new();
    for key in verb_key_paths(reg_root, verb) {
        for (sub, name) in [("", None), ("", Some("Icon")), ("\\command", None)] {
            let k = format!("{key}{sub}");
            let val = reg.get_string(&k, name).ok().flatten();
            out.push((k, name.unwrap_or("(default)").to_string(), val));
        }
    }
    out
}

fn real_verbs_for(real: &RealVerbs, verb: &str) -> Vec<(String, Option<String>)> {
    real.entries
        .iter()
        .filter(|(k, _)| k.ends_with(&format!("\\{verb}")) || k.contains(&format!("\\{verb}\\")))
        .cloned()
        .collect()
}

fn same_path(a: &str, b: &str) -> bool {
    a.replace('/', "\\").to_lowercase() == b.replace('/', "\\").to_lowercase()
}

// ---- Layer A --------------------------------------------------------------------------------------

#[test]
fn flare_runs_beside_the_neighbour_and_shares_nothing() {
    let exe = flare_exe();
    let v = pt_names();
    let sb = Sandbox::new();
    let neighbour = write_standin(&sb, &v);
    let neighbour_verb = pt(&v, "verbKey");

    // Folders the sandbox itself owns: a home for the child and a second profile.
    let e2e_dir = sb.root.join("e2e");
    let home = e2e_dir.join("home");
    let profile2 = e2e_dir.join("profile2");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&profile2).unwrap();
    let mut env = sb.env();
    env.push(("USERPROFILE".into(), home.to_string_lossy().into_owned()));

    // Before.
    let real_before = RegProbe::real_verb_values();
    let trees_before: Vec<_> = neighbour.trees.iter().map(|t| sb.snapshot(t)).collect();
    let neighbour_reg_before = verb_values(&sb.reg_root, &neighbour_verb);
    let files_before = walk(&sb.root);
    assert!(neighbour.exe.is_file(), "the stand-in's exe is in place");
    assert!(
        neighbour_reg_before.iter().all(|(_, _, v)| v.is_some()),
        "the stand-in's verb keys are written: {neighbour_reg_before:?}"
    );

    // 1. The default profile: no --user-data-dir, APPDATA is the sandbox's roaming.
    let a = Child::spawn(base_command(&exe, &["--e2e"], &env), &sb.profile);
    let flare_keys = verb_key_paths(&sb.reg_root, names::VERB_KEY);
    let command_of = |k: &str| {
        hkcu()
            .get_string(&format!("{k}\\command"), None)
            .ok()
            .flatten()
    };
    assert!(
        a.wait_for(
            || sb.profile.is_dir() && command_of(&flare_keys[0]).is_some(),
            WAIT
        ),
        "Flare wrote its verb into the sandbox root (exit: {:?})",
        a.exited()
    );

    // 2. A second Flare on another profile, while the first runs.
    let profile2_arg = format!("--user-data-dir={}", profile2.display());
    let b = Child::spawn(
        base_command(&exe, &["--e2e", profile2_arg.as_str()], &env),
        &profile2,
    );
    assert!(
        b.wait_for(
            || profile2.join("update-state.json").is_file()
                && sb.profile.join("update-state.json").is_file(),
            WAIT
        ),
        "each Flare answers update-state.json in its own profile (a: {:?}, b: {:?})",
        a.exited(),
        b.exited()
    );
    assert!(
        a.exited().is_none() && b.exited().is_none(),
        "both run at once"
    );

    // 3. A second launch on the first profile hands off and starts nothing new.
    let exe_name = exe.file_name().unwrap().to_string_lossy().into_owned();
    let before = children_named(my_pid(), &exe_name);
    assert!(
        before.contains(&a.pid()) && before.contains(&b.pid()),
        "{before:?}"
    );
    let code = run_to_end(base_command(&exe, &["--e2e"], &env), Duration::from_secs(5));
    assert_eq!(code, Some(0), "the second launch hands off and exits 0");
    let after = children_named(my_pid(), &exe_name);
    assert_eq!(after.len(), 2, "no third process: {after:?}");
    assert!(
        a.exited().is_none() && b.exited().is_none(),
        "the hand-off went to a running instance and ended neither"
    );

    // (a) Flare's verb keys, label and command; the neighbour's untouched.
    let exe_str = exe.to_string_lossy().into_owned();
    for (key, arg) in flare_keys.iter().zip(["%1", "%V"]) {
        let label = hkcu().get_string(key, None).ok().flatten();
        assert_eq!(label.as_deref(), Some(names::VERB_LABEL), "{key}");
        let cmd = command_of(key).expect("command");
        let want = format!("\"{exe_str}\" \"{arg}\"");
        assert!(same_path(&cmd, &want), "{cmd} vs {want}");
    }
    assert_eq!(
        verb_values(&sb.reg_root, &neighbour_verb),
        neighbour_reg_before,
        "the stand-in's verb keys kept their label, command and icon"
    );

    // (e) names: two profiles, two locks; none is the neighbour's.
    let n1 = instance_names(&sb.profile);
    let n2 = instance_names(&profile2);
    assert_ne!(n1.mutex, n2.mutex);
    assert_ne!(n1.pipe, n2.pipe);
    for lock in [&n1.mutex, &n1.pipe, &n2.mutex, &n2.pipe] {
        let l = lock.to_lowercase();
        for key in ["verbKey", "appId", "profileFolder", "productName"] {
            assert!(
                !l.contains(&pt(&v, key).to_lowercase()),
                "{lock} names {key}"
            );
        }
    }
    for (flare, key) in [
        (names::PLUGIN_NAME, "pluginName"),
        (names::HOOK_SIGNAL, "hookSignal"),
        (names::TAB_ID_VAR, "tabIdVar"),
        (names::VERB_KEY, "verbKey"),
        (names::AUMID, "aumid"),
        (names::PROFILE_FOLDER, "profileFolder"),
        (names::DICTATION_FOLDER, "dictationFolder"),
        (names::EXE_NAME, "exeName"),
    ] {
        assert_ne!(flare, pt(&v, key), "{key}");
    }

    // Stop both before the file checks, so nothing is mid-write.
    drop(a);
    drop(b);
    assert!(wait_until(Duration::from_secs(10), || {
        children_named(my_pid(), &exe_name).is_empty()
    }));

    // (b) Flare has its own folder; the neighbour's trees are byte for byte as before.
    assert!(sb.roaming.join(names::PROFILE_FOLDER).is_dir());
    for (t, h) in neighbour.trees.iter().zip(&trees_before) {
        let now = sb.snapshot(t);
        assert_eq!(&now, h, "{} changed: {:?}", t.display(), now.listing());
    }

    // (c) everything Flare created is Flare's own (or the sandbox's own e2e or temp folders).
    let files_after = walk(&sb.root);
    let stray: Vec<_> = files_after
        .difference(&files_before)
        .filter(|rel| {
            let mut parts = rel.split('/');
            let top = parts.next().unwrap_or("");
            let second = parts.next();
            match (top, second) {
                ("Roaming" | "Local", Some(s)) => {
                    let s = s.to_lowercase();
                    !(s.starts_with("flare") || s == "e2e" || s == "temp")
                }
                ("Roaming" | "Local", None) => false,
                _ => false, // the sandbox's own e2e folder and temp files
            }
        })
        .collect();
    assert!(
        stray.is_empty(),
        "paths Flare made outside its own: {stray:?}"
    );

    // (f) the uninstall hook. First without a sandbox root: a no-op everywhere.
    let snap_all = |sb: &Sandbox| {
        (
            sb.snapshot(&sb.root),
            verb_values(&sb.reg_root, names::VERB_KEY),
            verb_values(&sb.reg_root, &neighbour_verb),
        )
    };
    let before_noop = snap_all(&sb);
    let env_no_root: Vec<(String, String)> = env
        .iter()
        .filter(|(k, _)| k != "FLARE_E2E_VERB_ROOT")
        .cloned()
        .collect();
    let mut cmd = base_command(
        &exe,
        &["--veloapp-uninstall", "0.1.0", "--e2e"],
        &env_no_root,
    );
    cmd.env_remove(OsString::from("FLARE_E2E_VERB_ROOT"));
    assert_eq!(run_to_end(cmd, Duration::from_secs(30)), Some(0));
    let after_noop = snap_all(&sb);
    assert_eq!(
        after_noop, before_noop,
        "the hook without a sandbox root changes nothing"
    );
    assert_eq!(RegProbe::real_verb_values(), real_before);

    // Then with it: Flare's keys go, the neighbour's stay, user data stays.
    let code = run_to_end(
        base_command(&exe, &["--veloapp-uninstall", "0.1.0", "--e2e"], &env),
        Duration::from_secs(30),
    );
    assert_eq!(code, Some(0), "the uninstall hook exits 0");
    for key in &flare_keys {
        assert!(
            !hkcu().key_exists(key).unwrap_or(true),
            "{key} is gone after the uninstall hook"
        );
    }
    assert_eq!(
        verb_values(&sb.reg_root, &neighbour_verb),
        neighbour_reg_before,
        "the stand-in's keys are byte-identical"
    );
    assert!(
        sb.roaming.join(names::PROFILE_FOLDER).is_dir(),
        "uninstall keeps user data"
    );
    for (t, h) in neighbour.trees.iter().zip(&trees_before) {
        assert_eq!(&sb.snapshot(t), h, "{}", t.display());
    }

    // (g) the real HKCU verb values are what they were: this test never wrote the real registry.
    assert_eq!(
        RegProbe::real_verb_values(),
        real_before,
        "the real verb values changed during the test"
    );
}

// ---- Layer B (a clean CI runner only) ------------------------------------------------------------

fn install_switch_on() -> bool {
    let name = ["FLARE_E2E", "_INSTALL"].concat();
    std::env::var(name).is_ok_and(|v| v == "1")
        && std::env::var("GITHUB_ACTIONS").is_ok_and(|v| v == "true")
}

fn reg_export(key: &str, dir: &Path, tag: &str) -> String {
    let file = dir.join(format!("{tag}.reg"));
    let _ = Command::new("reg")
        .args(["export", key, &file.to_string_lossy(), "/y"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    fs::read(&file)
        .map(|b| {
            let units: Vec<u16> = b
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_le_bytes(*c))
                .collect();
            String::from_utf16_lossy(&units)
        })
        .unwrap_or_default()
}

const DIRECTORY_KEY: &str = r"HKCU\Software\Classes\Directory";
const UNINSTALL_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall";

#[test]
#[ignore = "installs for real: CI runner only, with the install switch on"]
fn flare_installs_beside_the_neighbour_and_both_leave_clean() {
    if !install_switch_on() {
        eprintln!("skipped: the install switch is off or this is not a CI runner");
        return;
    }
    let v = pt_names();
    let local = PathBuf::from(std::env::var("LOCALAPPDATA").expect("LOCALAPPDATA"));
    let roaming = PathBuf::from(std::env::var("APPDATA").expect("APPDATA"));
    let tmp = tempfile::tempdir().unwrap();
    let sb = Sandbox::new(); // only for its tree hashing

    let pt_dir = local.join("Programs").join(pt(&v, "profileFolder"));
    let pt_exe = pt_dir.join(pt(&v, "exeName"));
    let flare_root = local.join(names::INSTALL_FOLDER);
    let flare_exe_installed = flare_root.join("current").join(names::EXE_NAME);
    let programs = roaming
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs");
    let flare_lnk = programs.join(format!("{}.lnk", names::PRODUCT_TITLE));
    let dict_dir = local.join(names::DICTATION_FOLDER);
    fs::create_dir_all(&dict_dir).unwrap();
    fs::write(dict_dir.join("model.bin"), b"models").unwrap();

    // The neighbour first, from its latest public release.
    let dl = tmp.path().join("pt-dl");
    fs::create_dir_all(&dl).unwrap();
    let pattern = format!("{}*.exe", pt(&v, "installerPrefix"));
    let st = Command::new("gh")
        .args([
            "release",
            "download",
            "--repo",
            "Maxaubert/PrismTerminal",
            "--pattern",
            &pattern,
            "--dir",
        ])
        .arg(&dl)
        .status()
        .expect("gh");
    assert!(st.success(), "gh release download");
    let installer = fs::read_dir(&dl)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|e| e == "exe"))
        .expect("the neighbour's installer");
    let mut inst = Command::new(&installer)
        .arg("/S")
        .spawn()
        .expect("installer");
    assert!(
        wait_until(Duration::from_secs(180), || pt_exe.is_file()),
        "the neighbour installed"
    );
    let _ = inst.wait();
    let pt_hash = sb.snapshot(&pt_dir);
    let programs_before: BTreeSet<_> = walk(&programs);
    let real_before = RegProbe::real_verb_values();
    let dir_before = reg_export(DIRECTORY_KEY, tmp.path(), "dir-before");
    let uninstall_before = reg_export(UNINSTALL_KEY, tmp.path(), "un-before");

    // Flare's installer.
    let releases =
        PathBuf::from(std::env::var("FLARE_RELEASES").unwrap_or_else(|_| "Releases".into()));
    let setup = ["", "..", "../..", "../../.."]
        .iter()
        .map(|up| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(up)
                .join(&releases)
                .join(names::SETUP_NAME)
        })
        .find(|p| p.is_file())
        .or_else(|| {
            let p = releases.join(names::SETUP_NAME);
            p.is_file().then_some(p)
        })
        .expect("Flare-win-Setup.exe in the releases folder");
    let run_setup = |what: &str| {
        let c = run_to_end(
            base_command(&setup, &["--silent"], &[]),
            Duration::from_secs(240),
        );
        assert_eq!(c, Some(0), "{what}");
    };
    run_setup("first install");
    assert!(
        wait_until(Duration::from_secs(120), || {
            flare_exe_installed.is_file() && flare_root.join("Update.exe").is_file()
        }),
        "Flare installed"
    );
    assert!(flare_lnk.is_file(), "a Start menu shortcut named Flare");
    let programs_now = walk(&programs);
    assert!(
        programs_before.is_subset(&programs_now),
        "no shortcut removed or renamed for the neighbour"
    );
    assert!(
        hkcu()
            .key_exists(&format!(
                r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{}",
                names::APP_ID
            ))
            .unwrap_or(false),
        "Flare's uninstall entry"
    );

    // Setup starts the app on its own profile: end it, then launch the pair under temp profiles.
    reap_by_profile(&flare_root.join("current"));
    let flare_profile = tmp.path().join("flare-profile");
    let pt_profile = tmp.path().join("pt-profile");
    let flare_child = Child::launch(&flare_exe_installed, &[], &[], &flare_profile);
    let pt_child = Child::launch(&pt_exe, &[], &[], &pt_profile);
    std::thread::sleep(Duration::from_secs(10));
    assert!(
        flare_child.exited().is_none() && pt_child.exited().is_none(),
        "both stay up for 10 s"
    );
    drop(flare_child);

    // The real verb, written for real on this throwaway runner (no --e2e).
    let verb_profile = tmp.path().join("verb-profile");
    let arg = format!("--user-data-dir={}", verb_profile.display());
    let running = Child::spawn(
        base_command(&flare_exe_installed, &[arg.as_str()], &[]),
        &verb_profile,
    );
    let keys = [
        r"Software\Classes\Directory\shell\Flare",
        r"Software\Classes\Directory\Background\shell\Flare",
    ];
    assert!(
        wait_until(WAIT, || keys.iter().all(|k| hkcu()
            .get_string(k, None)
            .ok()
            .flatten()
            .is_some())),
        "the verb keys appear"
    );
    let root_str = flare_root.to_string_lossy().into_owned();
    for (k, a) in keys.iter().zip(["%1", "%V"]) {
        assert_eq!(
            hkcu().get_string(k, None).unwrap().as_deref(),
            Some(names::VERB_LABEL)
        );
        let icon = hkcu()
            .get_string(k, Some("Icon"))
            .unwrap()
            .unwrap_or_default();
        assert!(
            same_path(
                &icon,
                &format!("{root_str}\\current\\{},0", names::EXE_NAME)
            ),
            "{icon}"
        );
        let cmd = hkcu()
            .get_string(&format!("{k}\\command"), None)
            .unwrap()
            .unwrap_or_default();
        assert!(
            same_path(
                &cmd,
                &format!("\"{root_str}\\current\\{}\" \"{a}\"", names::EXE_NAME)
            ),
            "{cmd}"
        );
    }
    let neighbour_verb = pt(&v, "verbKey");
    assert_eq!(
        real_verbs_for(&RegProbe::real_verb_values(), &neighbour_verb),
        real_verbs_for(&real_before, &neighbour_verb),
        "the neighbour's real verb keys are unchanged"
    );

    // The installer ends the running app before touching files.
    let old_pid = running.pid();
    run_setup("reinstall over the running app");
    assert!(
        wait_until(Duration::from_secs(20), || !pid_alive(old_pid)),
        "the running Flare was ended"
    );
    assert!(
        flare_exe_installed.is_file(),
        "current\\Flare.exe is intact"
    );
    assert!(
        pt_child.exited().is_none(),
        "the neighbour is still running"
    );
    reap_by_profile(&flare_root.join("current"));

    // The uninstall ends a running app too.
    let running2 = Child::spawn(
        base_command(&flare_exe_installed, &[arg.as_str()], &[]),
        &verb_profile,
    );
    std::thread::sleep(Duration::from_secs(3));
    let pid2 = running2.pid();
    let c = run_to_end(
        base_command(
            &flare_root.join("Update.exe"),
            &["uninstall", "--silent"],
            &[],
        ),
        Duration::from_secs(240),
    );
    assert_eq!(c, Some(0), "Flare's uninstall");
    assert!(
        wait_until(Duration::from_secs(30), || !pid_alive(pid2)),
        "the running Flare was ended by the uninstall"
    );
    assert!(
        pt_child.exited().is_none(),
        "the neighbour is still running"
    );
    drop(pt_child);
    drop(running);
    drop(running2);

    // Clean after the uninstall.
    assert!(
        wait_until(Duration::from_secs(60), || !flare_root.exists()),
        "%LOCALAPPDATA%\\Flare is gone"
    );
    assert!(!flare_lnk.exists(), "the Start menu shortcut is gone");
    for k in keys {
        assert!(!hkcu().key_exists(k).unwrap_or(true), "{k}");
    }
    assert!(
        !hkcu()
            .key_exists(&format!(
                r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{}",
                names::APP_ID
            ))
            .unwrap_or(true),
        "Flare's uninstall entry is gone"
    );
    assert_eq!(
        reg_export(DIRECTORY_KEY, tmp.path(), "dir-after"),
        dir_before,
        "Directory classes are as before"
    );
    assert_eq!(
        reg_export(UNINSTALL_KEY, tmp.path(), "un-after"),
        uninstall_before,
        "uninstall entries are as before"
    );
    assert_eq!(
        sb.snapshot(&pt_dir),
        pt_hash,
        "the neighbour's install is unchanged"
    );
    assert!(
        roaming.join(names::PROFILE_FOLDER).exists() || verb_profile.exists(),
        "user data stays"
    );
    assert!(dict_dir.is_dir(), "the dictation folder stays");

    // Both leave clean: the neighbour's own uninstaller.
    let un = pt_dir.join(format!(
        "Uninstall {}.exe",
        pt(&v, "productName").replace(' ', "")
    ));
    let _ = Command::new(&un).arg("/S").status();
    assert!(
        wait_until(Duration::from_secs(120), || !pt_exe.exists()),
        "the neighbour uninstalled"
    );
}

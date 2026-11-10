//! Windows-only: the e2e harness drives the Windows app (job objects, the hidden desktop, named pipes).
//! The Mac E2E is the port plan's "Ongoing" row.
#![cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use flare_e2e::proc::{launch_args, pid_alive, reap_by_profile, Child, Job};
use flare_e2e::regsandbox::RegProbe;
use flare_e2e::sandbox::Sandbox;
use flare_win::registry::{Registry, WinRegistry};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A cmd that blocks reading an open pipe and carries `tag` in its command line.
fn dummy(tag: &str) -> Command {
    let mut c = Command::new("cmd.exe");
    c.raw_arg(format!("/c \"set /p x= & rem {tag}\""))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW);
    c
}

#[test]
fn a_sandbox_has_its_own_roaming_local_and_registry_root() {
    let (root, roaming, local, reg_root);
    {
        let sb = Sandbox::new();
        sb.registry().set("probe", Some("v"), "1");
        assert!(sb.roaming.is_dir() && sb.local.is_dir() && sb.profile.is_dir());
        assert!(sb.profile.starts_with(&sb.roaming));
        assert!(sb.reg_root.starts_with("Software\\FlareE2E\\"));
        assert!(sb.registry().exists());
        let env = sb.env();
        for k in ["APPDATA", "LOCALAPPDATA", "FLARE_E2E_VERB_ROOT"] {
            assert!(env.iter().any(|(n, _)| n == k), "{k}");
        }
        root = sb.root.clone();
        roaming = sb.roaming.clone();
        local = sb.local.clone();
        reg_root = sb.reg_root.clone();
    }
    assert!(!root.exists() && !roaming.exists() && !local.exists());
    let still = WinRegistry::current_user().key_exists(&reg_root).unwrap();
    assert!(!still);
}

#[test]
fn a_tree_hash_changes_when_a_byte_changes_and_not_otherwise() {
    let sb = Sandbox::new();
    let dir = sb.profile.join("t");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), b"abc").unwrap();
    std::fs::write(dir.join("b.txt"), b"xyz").unwrap();
    let h1 = sb.snapshot(&dir);
    let h2 = sb.snapshot(&dir);
    assert_eq!(h1, h2);
    std::fs::write(dir.join("b.txt"), b"xyZ").unwrap();
    let h3 = sb.snapshot(&dir);
    assert_ne!(h1, h3, "{:?} vs {:?}", h1.listing(), h3.listing());
}

#[test]
fn a_child_is_killed_with_its_job_when_the_test_ends() {
    let mut cmd = Command::new("ping");
    cmd.args(["-t", "127.0.0.1"])
        .stdout(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW);
    let mut proc = cmd.spawn().unwrap();
    let job = Job::new();
    assert!(job.assign(&proc));
    assert!(proc.try_wait().unwrap().is_none());
    let pid = proc.id();
    assert!(pid_alive(pid));
    // Only the job closes: no kill, no reap, so a broken job would leave ping running.
    drop(job);
    let end = std::time::Instant::now() + Duration::from_secs(2);
    while pid_alive(pid) && std::time::Instant::now() < end {
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(!pid_alive(pid), "ping survived its job");
    let _ = proc.wait();
}

#[test]
fn reaping_matches_by_profile_path_only() {
    let sb = Sandbox::new();
    let marked = sb.profile.join("mark-a");
    let other = sb.profile.join("mark-b");
    let a = Child::spawn(
        dummy(&marked.to_string_lossy()),
        Path::new("Z:\\unrelated-a"),
    );
    let b = Child::spawn(
        dummy(&other.to_string_lossy()),
        Path::new("Z:\\unrelated-b"),
    );
    assert!(a.wait_for(|| pid_alive(a.pid()), Duration::from_secs(2)));
    assert!(b.wait_for(|| pid_alive(b.pid()), Duration::from_secs(2)));
    // Let both cmd processes finish starting so their command lines are readable.
    std::thread::sleep(Duration::from_millis(300));
    let killed = reap_by_profile(&marked);
    assert_eq!(killed, 1);
    assert!(a.wait_for(|| !pid_alive(a.pid()), Duration::from_secs(3)));
    assert!(pid_alive(b.pid()), "the other profile's process must live");
}

#[test]
fn the_real_verb_probe_is_read_only() {
    let src =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/regsandbox.rs")).unwrap();
    let marker = ["pub struct ", "RegProbe;"].concat();
    let (sandbox_part, probe_part) = src.split_once(&marker).unwrap();
    for bad in [
        "set_string",
        "delete_tree",
        "delete_if_empty",
        "RegSetValue",
        "RegDelete",
        "RegCreateKey",
    ] {
        assert!(!probe_part.contains(bad), "RegProbe uses {bad}");
    }
    assert!(sandbox_part.contains("SandboxRoot"));
    // Two reads in a row agree.
    assert_eq!(RegProbe::real_verb_values(), RegProbe::real_verb_values());
}

#[test]
fn launch_adds_the_e2e_flag_and_never_foregrounds() {
    let p = Path::new("C:\\p\\Flare");
    let v = launch_args(&["--x"], p);
    assert_eq!(v, ["--x", "--e2e", "--user-data-dir=C:\\p\\Flare"]);
    let own = launch_args(&["--e2e", "--user-data-dir=D:\\q"], p);
    assert_eq!(own, ["--e2e", "--user-data-dir=D:\\q"]);
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/proc.rs")).unwrap();
    for bad in [
        "SetForegroundWindow",
        "SetFocus",
        "ShowWindow",
        "AllowSetForeground",
    ] {
        let call = [bad, "("].concat();
        assert!(!src.contains(&call), "{bad}");
    }
}

#[test]
fn a_spawned_child_is_in_its_job_and_runs_after_the_resume() {
    let sb = Sandbox::new();
    let marker = sb.profile.join("ran.txt");
    let mut c = Command::new("cmd.exe");
    c.raw_arg(format!("/c \"echo x> \"{}\"\"", marker.display()))
        .stdout(Stdio::null());
    let child = Child::spawn(c, &sb.profile);
    // The job holds it from before its first instruction (spawned suspended, assigned, resumed).
    assert!(child.in_job(), "the child is not in its job");
    assert!(
        child.wait_for(|| marker.is_file(), Duration::from_secs(5)),
        "the child never ran: it was left suspended"
    );
}

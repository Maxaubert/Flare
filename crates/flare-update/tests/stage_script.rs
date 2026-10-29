//! `tools/stage-install.ps1` writes a stage the app accepts (install when idle, 2026-10-29). Runs the real script
//! against a temp install and a temp bin folder; the real installs are never named. Skipped when `pwsh` is missing.

use std::path::{Path, PathBuf};
use std::process::Command;

use flare_update::staged::{self, Layout, NotReady};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("two levels under the repository root")
        .to_path_buf()
}

fn pwsh() -> Option<&'static str> {
    ["pwsh", "powershell"].into_iter().find(|exe| {
        Command::new(exe)
            .args(["-NoProfile", "-Command", "exit 0"])
            .status()
            .is_ok_and(|s| s.success())
    })
}

fn stage(shell: &str, root: &Path, bin: &Path, exe: &str) -> std::process::Output {
    let script = repo_root().join("tools").join("stage-install.ps1");
    Command::new(shell)
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(&script)
        .arg("-Root")
        .arg(root)
        .arg("-Bin")
        .arg(bin)
        .arg("-Repo")
        .arg(repo_root())
        .arg("-ExeName")
        .arg(exe)
        .output()
        .expect("the script runs")
}

#[test]
fn the_stage_script_writes_a_stage_the_app_accepts_and_a_second_replaces_it() {
    let Some(shell) = pwsh() else {
        eprintln!("skipped: no PowerShell");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("inst");
    let bin = dir.path().join("bin");
    std::fs::create_dir_all(root.join("current")).unwrap();
    std::fs::create_dir_all(&bin).unwrap();
    for (f, body) in [
        ("flare.exe", "exe one"),
        ("flare-mcp.exe", "mcp"),
        ("flare-parakeet-host.exe", "host"),
    ] {
        std::fs::write(bin.join(f), body).unwrap();
    }
    let out = stage(shell, &root, &bin, "FlareStable.exe");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let layout = Layout::new(&root, "FlareStable.exe");
    let m = staged::check(&layout).expect("the app accepts the stage");
    assert!(m.files.contains_key("FlareStable.exe"));
    assert!(m.files.contains_key("flare-mcp.exe"));
    assert!(m.files.contains_key("LICENSE"));
    assert!(m.files.keys().any(|k| k.starts_with("claude-plugin/")));
    for f in ["conpty.dll", "OpenConsole.exe"] {
        assert!(
            m.files.contains_key(f),
            "the pinned ConPTY pair ships beside the exe, as in pack.ps1: {f}"
        );
    }
    assert!(m.commit.len() >= 7);
    assert_eq!(
        std::fs::read(root.join("staged").join("FlareStable.exe")).unwrap(),
        b"exe one"
    );
    assert!(
        !root.join("current").join("FlareStable.exe").exists(),
        "staging never touches current"
    );

    // The plain copy's exe name works the same way, and a second stage replaces the first.
    std::fs::write(bin.join("flare.exe"), "exe two").unwrap();
    let out = stage(shell, &root, &bin, "Flare.exe");
    assert!(out.status.success());
    assert_eq!(
        staged::check(&Layout::new(&root, "FlareStable.exe")),
        Err(NotReady::NoExe("FlareStable.exe".into()))
    );
    let m2 = staged::check(&Layout::new(&root, "Flare.exe")).expect("the second stage");
    assert_ne!(m.signature(), m2.signature());
    assert!(!root.join("staged").join("FlareStable.exe").exists());
}

#[test]
fn the_stage_script_refuses_a_folder_that_is_no_install_and_a_missing_build() {
    let Some(shell) = pwsh() else {
        eprintln!("skipped: no PowerShell");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("inst");
    let bin = dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let out = stage(shell, &root, &bin, "Flare.exe");
    assert!(!out.status.success(), "no current folder: refused");
    std::fs::create_dir_all(root.join("current")).unwrap();
    let out = stage(shell, &root, &bin, "Flare.exe");
    assert!(!out.status.success(), "no build: refused");
    assert!(!root.join("staged").join("staged.json").exists());
}

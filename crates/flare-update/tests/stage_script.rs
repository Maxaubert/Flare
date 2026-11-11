//! `tools/stage-install.ps1` writes a stage the app accepts (install when idle, 2026-10-29). Runs the real script
//! against a temp install and a temp bin folder; the real installs are never named. Skipped when `pwsh` is missing.
//! macOS (rule 1132): `tools/release/mac/stage-install.sh` against a fake built bundle (`FLARE_STAGE_APP`, so no
//! cargo build) and a temp root (`FLARE_STAGE_ROOT`); `~/Library/Application Support/Flare` is never named.

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

#[cfg(target_os = "macos")]
mod mac {
    use super::*;

    /// Runs the Mac stage script on `app` into `root`. Nothing is built and the real root is never named.
    fn stage_mac(root: &Path, app: &Path) -> std::process::Output {
        let script = repo_root()
            .join("tools")
            .join("release")
            .join("mac")
            .join("stage-install.sh");
        Command::new("/bin/bash")
            .arg(&script)
            .env("FLARE_STAGE_ROOT", root)
            .env("FLARE_STAGE_APP", app)
            .env("FLARE_STAGE_REPO", repo_root())
            .env_remove("FLARE_MAC_SIGN_IDENTITY")
            .output()
            .expect("the script runs")
    }

    fn fake_bundle(app: &Path, exe: &str) {
        for (name, body) in [
            ("Contents/MacOS/Flare", exe),
            ("Contents/MacOS/flare-mcp", "mcp"),
            ("Contents/Info.plist", "plist"),
            (
                "Contents/Frameworks/Chromium Embedded Framework.framework/lib \"q\".dylib",
                "lib",
            ),
        ] {
            let p = app.join(name);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
    }

    fn say(out: &std::process::Output) -> String {
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    }

    #[test]
    fn the_mac_stage_script_writes_a_bundle_stage_the_app_accepts_and_a_second_replaces_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Support").join("Flare");
        let built = dir.path().join("build").join("Flare.app");
        fake_bundle(&built, "exe one");
        // The installed copy the app would run: the script must never touch it.
        let installed = dir.path().join("Applications").join("Flare.app");
        fake_bundle(&installed, "installed");

        let out = stage_mac(&root, &built);
        assert!(out.status.success(), "{}", say(&out));
        let layout = Layout::mac(&root, &installed, "Flare");
        let m = staged::check(&layout).expect("the app accepts the stage");
        assert!(m.files.contains_key("Contents/MacOS/Flare"));
        assert!(m.files.contains_key("Contents/Info.plist"));
        assert!(
            m.files.keys().any(|k| k.ends_with("lib \"q\".dylib")),
            "a name with a space and quotes survives the JSON: {:?}",
            m.files.keys()
        );
        assert!(m.commit.len() >= 7);
        assert!(!m.built_at.is_empty());
        assert_eq!(
            std::fs::read(root.join("staged/Flare.app/Contents/MacOS/Flare")).unwrap(),
            b"exe one"
        );
        assert!(
            !root.join("staged.tmp").exists(),
            "the temp stage was renamed in"
        );
        assert_eq!(
            std::fs::read(installed.join("Contents/MacOS/Flare")).unwrap(),
            b"installed",
            "staging never touches the installed bundle"
        );
        assert!(
            !dir.path()
                .join("Applications")
                .join("Flare.app.old")
                .exists()
        );

        // A second stage replaces the first.
        std::fs::write(built.join("Contents/MacOS/Flare"), "exe two").unwrap();
        std::fs::remove_file(built.join("Contents/Info.plist")).unwrap();
        let out = stage_mac(&root, &built);
        assert!(out.status.success(), "{}", say(&out));
        let m2 = staged::check(&layout).expect("the second stage");
        assert_ne!(m.signature(), m2.signature());
        assert!(!m2.files.contains_key("Contents/Info.plist"));
        assert!(!root.join("staged/Flare.app/Contents/Info.plist").exists());
    }

    #[test]
    fn the_mac_stage_script_refuses_a_missing_bundle_and_an_applications_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Support").join("Flare");
        let out = stage_mac(&root, &dir.path().join("nothing").join("Flare.app"));
        assert!(!out.status.success(), "no bundle: refused");
        assert!(!root.join("staged").join("staged.json").exists());

        let built = dir.path().join("build").join("Flare.app");
        fake_bundle(&built, "exe");
        for bad in [
            dir.path().join("Applications"),
            dir.path().join("Applications").join("Flare.app"),
            dir.path().join("x").join("Flare.app").join("Contents"),
        ] {
            let out = stage_mac(&bad, &built);
            assert!(!out.status.success(), "{} refused", bad.display());
            assert!(!bad.join("staged").exists());
        }
    }
}

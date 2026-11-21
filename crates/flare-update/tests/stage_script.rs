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

    fn mac_script() -> PathBuf {
        repo_root()
            .join("tools")
            .join("release")
            .join("mac")
            .join("stage-install.sh")
    }

    /// Runs the Mac stage script on `app` into `root`. Nothing is built and the real root is never named. The fake
    /// bundles are unsigned, so these runs allow an ad hoc stage (rule 1356).
    fn stage_mac(root: &Path, app: &Path) -> std::process::Output {
        stage_mac_with(root, app, true)
    }

    fn stage_mac_with(root: &Path, app: &Path, allow_adhoc: bool) -> std::process::Output {
        let mut cmd = Command::new("/bin/bash");
        cmd.arg(mac_script())
            .env("FLARE_STAGE_ROOT", root)
            .env("FLARE_STAGE_APP", app)
            .env("FLARE_STAGE_REPO", repo_root())
            .env_remove("FLARE_MAC_SIGN_IDENTITY")
            .env_remove("FLARE_STAGE_ALLOW_ADHOC");
        if allow_adhoc {
            cmd.env("FLARE_STAGE_ALLOW_ADHOC", "1");
        }
        cmd.output().expect("the script runs")
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

    /// Rule 1356: an unsigned (ad hoc) bundle is never staged without `FLARE_STAGE_ALLOW_ADHOC=1`, and the refusal
    /// leaves the stage already there as it was. `codesign -dv` only reads the bundle.
    #[test]
    fn the_mac_stage_script_refuses_a_bundle_that_is_not_developer_id_signed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Support").join("Flare");
        let built = dir.path().join("build").join("Flare.app");
        fake_bundle(&built, "exe one");
        let out = stage_mac(&root, &built);
        assert!(out.status.success(), "{}", say(&out));
        let before = std::fs::read(root.join("staged/staged.json")).unwrap();

        std::fs::write(built.join("Contents/MacOS/Flare"), "exe two").unwrap();
        let out = stage_mac_with(&root, &built, false);
        assert!(!out.status.success(), "an unsigned bundle is refused");
        assert!(
            say(&out).contains("not signed with Developer ID"),
            "{}",
            say(&out)
        );
        assert_eq!(
            std::fs::read(root.join("staged/staged.json")).unwrap(),
            before
        );
        assert_eq!(
            std::fs::read(root.join("staged/Flare.app/Contents/MacOS/Flare")).unwrap(),
            b"exe one"
        );
        assert!(!root.join("staged.tmp").exists());
    }

    /// A fake checkout whose make-app.sh records the identity it got, plus fake `security` and `codesign` first on
    /// PATH: the build path runs with no Keychain and no real signing (rule 1356).
    struct FakeBuild {
        _dir: tempfile::TempDir,
        repo: PathBuf,
        bin: PathBuf,
        root: PathBuf,
        seen: PathBuf,
    }

    fn write_exe(path: &Path, body: &str) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn fake_build(identities: &str) -> FakeBuild {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let bin = dir.path().join("bin");
        let root = dir.path().join("Support").join("Flare");
        let seen = dir.path().join("identity.txt");
        write_exe(
            &repo.join("tools/release/mac/make-app.sh"),
            &format!(
                "#!/bin/bash\nset -e\nprintf '%s' \"${{FLARE_MAC_SIGN_IDENTITY:-}}\" >'{}'\n\
                 mkdir -p \"$FLARE_MAC_OUT/Flare.app/Contents/MacOS\"\n\
                 echo exe >\"$FLARE_MAC_OUT/Flare.app/Contents/MacOS/Flare\"\n",
                seen.display()
            ),
        );
        let git = |args: &[&str]| {
            let ok = Command::new("git")
                .args([
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "-c",
                    "commit.gpgsign=false",
                ])
                .args(args)
                .current_dir(&repo)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?}");
        };
        git(&["init", "-q"]);
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "fake"]);
        let list = dir.path().join("identities.txt");
        std::fs::write(&list, identities).unwrap();
        write_exe(
            &bin.join("security"),
            &format!("#!/bin/bash\ncat '{}'\n", list.display()),
        );
        // Developer ID when make-app.sh got an identity, ad hoc otherwise.
        write_exe(
            &bin.join("codesign"),
            &format!(
                "#!/bin/bash\nif [ -s '{}' ]; then\n\
                 printf 'Authority=Developer ID Application: MAX AUBERT (WV7BY8CUM8)\\nTeamIdentifier=WV7BY8CUM8\\n' >&2\n\
                 else\nprintf 'Signature=adhoc\\nTeamIdentifier=not set\\n' >&2\nfi\n",
                seen.display()
            ),
        );
        FakeBuild {
            _dir: dir,
            repo,
            bin,
            root,
            seen,
        }
    }

    fn build_and_stage(f: &FakeBuild, allow_adhoc: bool) -> std::process::Output {
        let path = format!(
            "{}:{}",
            f.bin.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let mut cmd = Command::new("/bin/bash");
        cmd.arg(mac_script())
            .env("PATH", path)
            .env("FLARE_STAGE_ROOT", &f.root)
            .env("FLARE_STAGE_REPO", &f.repo)
            .env_remove("FLARE_STAGE_APP")
            .env_remove("FLARE_MAC_SIGN_IDENTITY")
            .env_remove("FLARE_STAGE_ALLOW_ADHOC");
        if allow_adhoc {
            cmd.env("FLARE_STAGE_ALLOW_ADHOC", "1");
        }
        cmd.output().expect("the script runs")
    }

    #[test]
    fn the_mac_stage_script_signs_with_the_keychains_developer_id_and_prefers_the_owners_team() {
        let f = fake_build(
            "  1) AAAA \"Apple Development: Someone (ZZZZZZZZZZ)\"\n\
               2) BBBB \"Developer ID Application: Other Team (QQQQQQQQQQ)\"\n\
               3) CCCC \"Developer ID Application: MAX AUBERT (WV7BY8CUM8)\"\n\
                  3 valid identities found\n",
        );
        let out = build_and_stage(&f, false);
        assert!(out.status.success(), "{}", say(&out));
        assert_eq!(
            std::fs::read_to_string(&f.seen).unwrap(),
            "Developer ID Application: MAX AUBERT (WV7BY8CUM8)"
        );
        assert!(say(&out).contains("signed with Developer ID: an agent may install it"));
        assert!(f.root.join("staged/staged.json").exists());

        // Another team's Developer ID is used when it is the only one.
        let f = fake_build(
            "  1) BBBB \"Developer ID Application: Other Team (QQQQQQQQQQ)\"\n     1 valid identities found\n",
        );
        let out = build_and_stage(&f, false);
        assert!(out.status.success(), "{}", say(&out));
        assert_eq!(
            std::fs::read_to_string(&f.seen).unwrap(),
            "Developer ID Application: Other Team (QQQQQQQQQQ)"
        );
    }

    #[test]
    fn the_mac_stage_script_refuses_to_build_ad_hoc_unless_allowed() {
        let none = "     0 valid identities found\n";
        let f = fake_build(none);
        let out = build_and_stage(&f, false);
        assert!(!out.status.success(), "no Developer ID identity: refused");
        assert!(
            say(&out).contains("FLARE_STAGE_ALLOW_ADHOC=1"),
            "{}",
            say(&out)
        );
        assert!(!f.seen.exists(), "nothing was built");
        assert!(!f.root.join("staged").exists());

        let f = fake_build(none);
        let out = build_and_stage(&f, true);
        assert!(out.status.success(), "{}", say(&out));
        assert_eq!(std::fs::read_to_string(&f.seen).unwrap(), "");
        assert!(say(&out).contains("not signed with Developer ID: only a click"));
        assert!(f.root.join("staged/staged.json").exists());
    }
}

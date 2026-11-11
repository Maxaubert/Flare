//! Flare's shell integration on macOS: the hooks that make every shell report its folder (OSC 7)
//! before each prompt, the Mac counterpart of the PowerShell prompt hook's OSC 9;9 on Windows.
//! Nothing is written into the user's dotfiles; each shell is pointed at Flare's own files, which
//! then load the user's:
//!
//! - zsh: `ZDOTDIR` names Flare's folder; its `.zshenv` hands `ZDOTDIR` back at once (so zsh
//!   reads `.zprofile`, `.zshrc` and `.zlogin` from the user's folder), sources the user's
//!   `.zshenv`, and adds a hook that installs itself at the first prompt, after `.zshrc`.
//! - bash: `--rcfile` names Flare's file, which reads what a login shell reads and then adds the
//!   hook to `PROMPT_COMMAND` (bash ignores `--rcfile` in a login shell, so the file is the login).
//! - fish: `-l --init-command`, run after the user's config.
//! - pwsh: `-Login` and the prompt wrapper in the `-Command` bootstrap, as on Windows.
//!
//! The files live in `~/Library/Application Support/Flare/shell-integration/<version>` and are
//! rewritten whenever their text differs from this build's.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

/// The folder of the zsh shim inside the integration folder (`ZDOTDIR` points here).
pub const ZSH_DIR: &str = "zsh";
/// The bash `--rcfile`.
pub const BASH_RC: &str = "bash/flare.bash";
/// The fish file `--init-command` sources.
pub const FISH_INIT: &str = "fish/flare.fish";
/// Where the user's own `ZDOTDIR` waits while zsh starts from Flare's (the shim restores it).
pub const USER_ZDOTDIR_VAR: &str = "FLARE_USER_ZDOTDIR";

/// Every file of the integration: (path inside the folder, text).
pub const FILES: [(&str, &str); 4] = [
    ("zsh/.zshenv", include_str!("scripts/zshenv.zsh")),
    ("flare.zsh", include_str!("scripts/flare.zsh")),
    (BASH_RC, include_str!("scripts/flare.bash")),
    (FISH_INIT, include_str!("scripts/flare.fish")),
];

/// The PowerShell prompt wrapper for macOS: the Windows hook's shape (wraps whatever `prompt` the
/// profile installed, reports only the FileSystem provider), reporting OSC 7 instead of OSC 9;9.
/// ONE line, no CR or LF (it is a `-Command` argument).
pub const PS_PROMPT_HOOK_OSC7: &str = concat!(
    "if (-not (Test-Path Variable:global:__flarePrompt)) { $global:__flarePrompt = $function:prompt }; ",
    "function global:prompt { ",
    "$o = if ($global:__flarePrompt) { (& $global:__flarePrompt) -join '' } else { 'PS> ' }; ",
    "if ($PWD.Provider.Name -eq 'FileSystem') { ",
    "$u = (($PWD.ProviderPath -split '/') | ForEach-Object { [uri]::EscapeDataString($_) }) -join '/'; ",
    "\"$([char]27)]7;file://$u$([char]27)\\\" + $o } else { $o } }"
);

static INSTALLED: RwLock<Option<PathBuf>> = RwLock::new(None);

/// This build's integration folder: `~/Library/Application Support/Flare/shell-integration/<version>`.
pub fn default_dir() -> Option<PathBuf> {
    let home = crate::macos::sys::home_dir()?;
    Some(
        Path::new(&home)
            .join("Library/Application Support/Flare/shell-integration")
            .join(env!("CARGO_PKG_VERSION")),
    )
}

/// Writes every file into `dir` (created owner-only), each only when its text differs, through a
/// temporary file and a rename so a shell starting meanwhile never reads half a file.
pub fn install(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    let mk = |d: &Path| {
        std::fs::DirBuilder::new() // sync-ok: detect worker, once per run
            .recursive(true)
            .mode(0o700)
            .create(d)
    };
    mk(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?; // sync-ok: detect worker
    for (rel, text) in FILES {
        let path = dir.join(rel);
        if std::fs::read(&path).is_ok_and(|b| b == text.as_bytes()) {
            // sync-ok: detect worker, a few small files
            continue;
        }
        if let Some(parent) = path.parent() {
            mk(parent)?;
        }
        let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
        {
            let mut f = std::fs::File::create(&tmp)?; // sync-ok: detect worker
            f.write_all(text.as_bytes())?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, &path)?; // sync-ok: detect worker
    }
    Ok(())
}

/// Installs into `dir` and makes it the folder new shells use; false (and a warning) when the
/// files cannot be written, and then shells start without the folder report.
pub fn activate(dir: &Path) -> bool {
    match install(dir) {
        Ok(()) => {
            *INSTALLED.write().unwrap_or_else(|p| p.into_inner()) = Some(dir.to_path_buf());
            true
        }
        Err(e) => {
            log::warn!("shell integration not installed: {e}");
            false
        }
    }
}

/// The active integration folder, when its zsh shim is still on disk. A zsh pointed at a missing
/// shim would read none of the user's startup files, so a vanished folder is never handed out.
pub fn installed_dir() -> Option<PathBuf> {
    let dir = INSTALLED
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .clone()?;
    dir.join(ZSH_DIR).join(".zshenv").is_file().then_some(dir) // sync-ok: one stat, spawn worker
}

/// The bash `--rcfile` arguments for an integration folder.
pub fn bash_args(dir: &Path) -> Vec<String> {
    vec![
        "--rcfile".into(),
        dir.join(BASH_RC).to_string_lossy().into_owned(),
    ]
}

/// The fish arguments: a login shell, then Flare's file sourced after the user's config.
pub fn fish_args(dir: &Path) -> Vec<String> {
    let file = dir.join(FISH_INIT).to_string_lossy().into_owned();
    vec![
        "-l".into(),
        "--init-command".into(),
        format!("source {}", crate::macos::cd::fish_quote(&file)),
    ]
}

/// The pwsh arguments (`-Login` must come first): the Windows bootstrap with the OSC 7 hook.
pub fn pwsh_args() -> Vec<String> {
    vec![
        "-Login".into(),
        "-NoLogo".into(),
        "-NoExit".into(),
        "-Command".into(),
        format!(
            "Set-PSReadLineOption -PredictionSource History; try {{ Set-PSReadLineOption -EnableScreenReaderMode:$false }} catch {{}}; {}; {PS_PROMPT_HOOK_OSC7}",
            crate::prompt::PS_FILE_STYLE
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn install_writes_every_file_owner_only_and_rewrites_a_changed_one() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("si").join("1.0");
        install(&dir).unwrap();
        for (rel, text) in FILES {
            assert_eq!(
                std::fs::read_to_string(dir.join(rel)).unwrap(),
                text,
                "{rel}"
            );
        }
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
        std::fs::write(dir.join(BASH_RC), "echo tampered").unwrap();
        std::fs::remove_file(dir.join("zsh/.zshenv")).unwrap();
        install(&dir).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join(BASH_RC)).unwrap(),
            FILES[2].1
        );
        assert!(dir.join("zsh/.zshenv").is_file());
        let leftovers: Vec<_> = std::fs::read_dir(dir.join("bash"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().contains("tmp-"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn a_vanished_folder_is_never_handed_to_zsh() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("si");
        assert!(activate(&dir));
        assert_eq!(installed_dir().as_deref(), Some(dir.as_path()));
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(installed_dir().is_none());
    }

    #[test]
    fn the_default_folder_is_per_version_under_application_support() {
        let d = default_dir().unwrap();
        let s = d.to_string_lossy();
        assert!(
            s.ends_with(&format!(
                "Library/Application Support/Flare/shell-integration/{}",
                env!("CARGO_PKG_VERSION")
            )),
            "{s}"
        );
    }

    #[test]
    fn the_pwsh_hook_wraps_once_and_reports_osc7_on_one_line() {
        for needle in [
            "$global:__flarePrompt = $function:prompt",
            "Test-Path Variable:global:__flarePrompt",
            "$PWD.Provider.Name -eq 'FileSystem'",
            "]7;file://$u",
            "[uri]::EscapeDataString($_)",
            "& $global:__flarePrompt",
        ] {
            assert!(PS_PROMPT_HOOK_OSC7.contains(needle), "missing {needle:?}");
        }
        assert!(!PS_PROMPT_HOOK_OSC7.contains(['\r', '\n']));
        let args = pwsh_args();
        assert_eq!(
            args[0], "-Login",
            "pwsh takes -Login only as its first argument"
        );
        assert_eq!(&args[1..4], ["-NoLogo", "-NoExit", "-Command"]);
        assert!(args[4].ends_with(PS_PROMPT_HOOK_OSC7));
        assert!(!args[4].contains(['\r', '\n']));
    }

    #[test]
    fn the_hooks_never_write_the_users_dotfiles() {
        for (rel, text) in FILES {
            for line in text.lines().map(str::trim) {
                assert!(
                    !line.contains(">>") && !(line.contains('>') && line.contains("~/")),
                    "{rel}: {line}"
                );
            }
        }
    }

    #[test]
    fn the_hooks_report_the_folder_with_no_host_so_a_renamed_mac_still_counts() {
        // zsh's $HOST and bash's $HOSTNAME are read once, when the shell starts; a Mac whose name
        // comes from the network can be renamed after that. Flare's hooks send no host at all.
        let tmp = tempfile::tempdir().unwrap();
        install(tmp.path()).unwrap();
        for (shell, file) in [("/bin/zsh", "flare.zsh"), ("/bin/bash", BASH_RC)] {
            let out = std::process::Command::new(shell)
                .args(["-c", ". \"$1\"; cd /tmp/ && __flare_osc7", "flare-test"])
                .arg(tmp.path().join(file))
                .env_clear()
                .env("HOME", tmp.path())
                .env("PATH", "/usr/bin:/bin")
                .env("HOST", "stale-name")
                .env("HOSTNAME", "stale-name")
                .stdin(std::process::Stdio::null())
                .output()
                .unwrap();
            let text = String::from_utf8_lossy(&out.stdout);
            let report = text.rsplit_once("\x1b]7;").map(|(_, r)| r).unwrap_or("");
            assert_eq!(report, "file:///tmp\x1b\\", "{shell}: {text:?}");
            let data = report.trim_end_matches("\x1b\\");
            assert_eq!(
                crate::macos::osc7::parse_osc7_on(data, Some("new-name")).as_deref(),
                Some("/tmp"),
                "{shell}"
            );
        }
        for (rel, text) in FILES {
            assert!(
                !text.contains("$HOST") && !text.contains("$hostname"),
                "{rel} names the host"
            );
        }
    }

    /// Starts `shell` as a restored Flare tab does (the integration, `FLARE_RESUME_ID` in the
    /// environment, a PATH without the agent) with "exit" on stdin, and answers what the fake agents
    /// on the PATH that the user's own startup file adds were started with.
    fn resume_in(shell: &str, id: &str) -> String {
        let tmp = tempfile::tempdir().unwrap();
        let si = tmp.path().join("si");
        install(&si).unwrap();
        let bin = tmp.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let log = tmp.path().join("agent.log");
        for agent in ["claude", "codex", "agy"] {
            let p = bin.join(agent);
            std::fs::write(
                &p,
                format!(
                    "#!/bin/sh\nprintf '{agent} %s|env=%s\\n' \"$*\" \"${{FLARE_RESUME_ID-unset}}\" >> '{}'\n",
                    log.display()
                ),
            )
            .unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        // The agent is on the PATH only through the user's startup file, as a Mac's claude is.
        let add = format!("PATH='{}':\"$PATH\"\n", bin.display());
        std::fs::write(tmp.path().join(".zshrc"), &add).unwrap();
        std::fs::write(tmp.path().join(".bash_profile"), &add).unwrap();
        let (args, zdotdir): (Vec<String>, Option<PathBuf>) = match shell {
            "/bin/zsh" => (vec!["-l".into(), "-i".into()], Some(si.join(ZSH_DIR))),
            _ => {
                let mut a = bash_args(&si);
                a.push("-i".into());
                (a, None)
            }
        };
        let mut cmd = std::process::Command::new(shell);
        cmd.args(&args)
            .current_dir(tmp.path())
            .env_clear()
            .env("HOME", tmp.path())
            .env("PATH", "/usr/bin:/bin")
            .env("TERM", "dumb")
            .env("BASH_SILENCE_DEPRECATION_WARNING", "1")
            .env(crate::resume::RESUME_ENV, id)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        if let Some(z) = zdotdir {
            cmd.env("ZDOTDIR", z);
        }
        let mut child = cmd.spawn().unwrap();
        child.stdin.take().unwrap().write_all(b"exit\n").unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while child.try_wait().unwrap().is_none() {
            if std::time::Instant::now() > deadline {
                let _ = child.kill();
                panic!("{shell} did not exit");
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        std::fs::read_to_string(&log).unwrap_or_default()
    }

    /// Rule 1133: a restored Mac tab that hosted Claude starts `claude --resume <id>` in its login
    /// shell, after the user's startup files have set PATH, and the id is out of the environment.
    #[test]
    fn a_restored_zsh_or_bash_tab_resumes_claude_through_the_login_shell() {
        const ID: &str = "3f2a9c1e-77aa-4c0d-9b1e-0a1b2c3d4e5f";
        for shell in ["/bin/zsh", "/bin/bash"] {
            assert_eq!(
                resume_in(shell, ID),
                format!("claude --resume {ID}|env=unset\n"),
                "{shell}"
            );
            assert_eq!(
                resume_in(shell, "codex:last"),
                "codex resume --last|env=unset\n",
                "{shell}"
            );
            assert_eq!(
                resume_in(shell, "agy:last"),
                "agy -c|env=unset\n",
                "{shell}"
            );
            // Anything that is not an id is never run.
            assert_eq!(resume_in(shell, "x; touch pwned"), "", "{shell}");
            assert_eq!(resume_in(shell, "--help"), "", "{shell}");
        }
    }

    #[test]
    fn a_shell_with_no_resume_starts_no_agent() {
        for shell in ["/bin/zsh", "/bin/bash"] {
            assert_eq!(resume_in(shell, ""), "", "{shell}");
        }
    }

    #[test]
    fn fish_sources_the_file_quoted() {
        let a = fish_args(Path::new("/Users/max/Library/Application Support/Flare/si"));
        assert_eq!(
            a,
            [
                "-l",
                "--init-command",
                "source '/Users/max/Library/Application Support/Flare/si/fish/flare.fish'"
            ]
        );
    }
}

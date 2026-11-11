//! with_resume, RESUME_ENV.

use crate::list::ShellDef;

/// The variable that carries the resume id into the shell (never written into the command line).
pub const RESUME_ENV: &str = "FLARE_RESUME_ID";

/// cmd.exe's switch that keeps the current folder out of the program search.
const NO_CWD_EXE: &str = "NoDefaultCurrentDirectoryInExePath";

/// How a shell is started for a resumed tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    pub exe: String,
    pub args: Vec<String>,
    pub env: Option<(String, String)>,
}

/// The launch for a resumed tab; the id travels in the environment, so two resumes share one command line.
pub fn with_resume(def: &ShellDef, resume: &str) -> Launch {
    let prior = std::env::var(NO_CWD_EXE).ok();
    with_resume_prior(def, resume, prior.as_deref())
}

/// A user's own value of the search switch can go back after the resume only if cmd reads it as plain text:
/// no character that ends or reshapes a command line.
fn plain_switch_value(v: &str) -> bool {
    !v.is_empty()
        && v.chars().all(|c| c.is_ascii_graphic() || c == ' ')
        && !v.contains(['&', '|', '<', '>', '^', '%', '"', '!', '(', ')'])
}

/// `with_resume` with the shell's own value of `NoDefaultCurrentDirectoryInExePath` (what the tab inherits)
/// given: the resume line puts it back when the agent ends, where it used to erase it.
pub fn with_resume_prior(def: &ShellDef, resume: &str, prior: Option<&str>) -> Launch {
    let restore = match prior {
        Some(v) if plain_switch_value(v) => format!("set {NO_CWD_EXE}={v}"),
        _ => format!("set {NO_CWD_EXE}="),
    };
    // A marker resumes the agent's most recent session in the folder: no id travels.
    let marker = match resume {
        "codex:last" => Some("codex resume --last"),
        "agy:last" => Some("agy -c"),
        _ => None,
    };
    let env = if marker.is_some() {
        None
    } else {
        Some((RESUME_ENV.to_string(), resume.to_string()))
    };
    let (args, env) = match def.id.as_str() {
        "pwsh" | "powershell" if !def.args.is_empty() => {
            let step = if let Some(line) = marker {
                line.to_string()
            } else {
                format!(
                    "& {{ $r = $env:{RESUME_ENV}; Remove-Item Env:{RESUME_ENV}; claude --resume $r }}"
                )
            };
            let mut args = def.args.clone();
            if let Some(last) = args.last_mut() {
                last.push_str("; ");
                last.push_str(&step);
            }
            (args, env)
        }
        "cmd" => {
            // cmd looks in the current folder (the tab's project) before PATH; a claude.cmd a
            // repo carries must never run on a restore.
            let step = if let Some(line) = marker {
                format!("set {NO_CWD_EXE}=1& {line}& {restore}")
            } else {
                format!(
                    "set {NO_CWD_EXE}=1& set {RESUME_ENV}=& claude --resume %{RESUME_ENV}%& {restore}"
                )
            };
            (vec!["/K".to_string(), step], env)
        }
        // The macOS shells: the command line stays the plain login shell, and Flare's shell integration
        // runs the resume once the user's startup files have set PATH (`macos/scripts`). The id (or the
        // marker) rides in the environment, so a Mac tab comes back resumed as a Windows one does.
        "zsh" | "bash" | "fish" => (
            def.args.clone(),
            Some((RESUME_ENV.to_string(), resume.to_string())),
        ),
        _ => (def.args.clone(), None),
    };
    Launch {
        exe: def.exe.clone(),
        args,
        env,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "5a1d0c2e-1111-4222-8333-94445555a666";
    const B: &str = "0f0e0d0c-2222-4333-8444-955566667777";

    fn def(id: &str, exe: &str, args: &[&str]) -> ShellDef {
        ShellDef {
            id: id.to_string(),
            name: id.to_string(),
            exe: exe.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn pwsh() -> ShellDef {
        def(
            "pwsh",
            "pwsh.exe",
            &["-NoLogo", "-NoExit", "-Command", "boot"],
        )
    }

    fn powershell() -> ShellDef {
        def(
            "powershell",
            "powershell.exe",
            &["-NoLogo", "-NoExit", "-Command", "boot"],
        )
    }

    fn cmd() -> ShellDef {
        def("cmd", "cmd.exe", &[])
    }

    #[test]
    fn two_resumes_share_one_command_line() {
        for d in [pwsh(), powershell(), cmd()] {
            let ra = with_resume(&d, A);
            let rb = with_resume(&d, B);
            assert_eq!(ra.args, rb.args);
            assert!(!ra.args.join(" ").contains(A));
            assert_eq!(ra.env, Some(("FLARE_RESUME_ID".to_string(), A.to_string())));
            assert_eq!(rb.env, Some(("FLARE_RESUME_ID".to_string(), B.to_string())));
        }
    }

    #[test]
    fn powershell_resume_clears_the_variable() {
        let r = with_resume(&pwsh(), A);
        assert_eq!(r.exe, "pwsh.exe");
        assert_eq!(r.args[..3], ["-NoLogo", "-NoExit", "-Command"]);
        assert_eq!(
            r.args[3],
            "boot; & { $r = $env:FLARE_RESUME_ID; Remove-Item Env:FLARE_RESUME_ID; claude --resume $r }"
        );
    }

    #[test]
    fn cmd_resume_has_no_quote() {
        let r = with_resume_prior(&cmd(), A, None);
        assert_eq!(
            r.args,
            [
                "/K",
                "set NoDefaultCurrentDirectoryInExePath=1& set FLARE_RESUME_ID=& claude --resume %FLARE_RESUME_ID%& set NoDefaultCurrentDirectoryInExePath="
            ]
        );
        assert!(!r.args[1].contains('"'));
    }

    #[test]
    fn a_cmd_resume_clears_the_search_switch_once_the_agent_ends() {
        for id in [A, "codex:last", "agy:last"] {
            let r = with_resume_prior(&cmd(), id, None);
            assert!(
                r.args[1].ends_with("& set NoDefaultCurrentDirectoryInExePath="),
                "{}",
                r.args[1]
            );
        }
    }

    /// A user's own value of the switch is put back, not erased (new-4 residual).
    #[test]
    fn a_cmd_resume_puts_the_users_own_switch_value_back() {
        for id in [A, "codex:last", "agy:last"] {
            let r = with_resume_prior(&cmd(), id, Some("1"));
            assert!(
                r.args[1].ends_with("& set NoDefaultCurrentDirectoryInExePath=1"),
                "{}",
                r.args[1]
            );
            assert!(
                r.args[1].starts_with("set NoDefaultCurrentDirectoryInExePath=1& "),
                "{}",
                r.args[1]
            );
            // A value cmd would read as more than text is cleared, never spliced into the line.
            for bad in ["a&calc", "x|y", "%PATH%", "q\"z", ""] {
                let r = with_resume_prior(&cmd(), id, Some(bad));
                assert!(
                    r.args[1].ends_with("& set NoDefaultCurrentDirectoryInExePath="),
                    "{bad}: {}",
                    r.args[1]
                );
                assert!(!r.args[1].contains(bad) || bad.is_empty());
            }
        }
    }

    #[test]
    fn a_cmd_resume_never_searches_the_current_folder() {
        for id in [A, "codex:last", "agy:last"] {
            let r = with_resume_prior(&cmd(), id, None);
            assert!(
                r.args[1].starts_with("set NoDefaultCurrentDirectoryInExePath=1& "),
                "{:?}",
                r.args
            );
        }
    }

    #[test]
    fn codex_resume_has_no_id() {
        assert_eq!(
            with_resume(&pwsh(), "codex:last"),
            Launch {
                exe: "pwsh.exe".to_string(),
                args: [
                    "-NoLogo",
                    "-NoExit",
                    "-Command",
                    "boot; codex resume --last"
                ]
                .iter()
                .map(|s| s.to_string())
                .collect(),
                env: None,
            }
        );
        let r = with_resume_prior(&cmd(), "codex:last", None);
        assert_eq!(
            r.args,
            [
                "/K",
                "set NoDefaultCurrentDirectoryInExePath=1& codex resume --last& set NoDefaultCurrentDirectoryInExePath="
            ]
        );
        assert_eq!(r.env, None);
    }

    #[test]
    fn agy_resumes_its_last_conversation_with_no_id() {
        let r = with_resume(&pwsh(), "agy:last");
        assert_eq!(r.args[3], "boot; agy -c");
        assert_eq!(r.env, None);
        let r = with_resume_prior(&cmd(), "agy:last", None);
        assert_eq!(
            r.args,
            [
                "/K",
                "set NoDefaultCurrentDirectoryInExePath=1& agy -c& set NoDefaultCurrentDirectoryInExePath="
            ]
        );
        assert_eq!(r.env, None);
    }

    /// Rule 1133: a Mac tab that hosted Claude hands the id to its zsh, bash or fish, which the
    /// integration turns into `claude --resume <id>`; the command line stays the plain login shell.
    #[test]
    fn a_mac_shell_carries_the_resume_in_its_environment() {
        let zsh = def("zsh", "/bin/zsh", &["-l"]);
        let bash = def("bash", "/bin/bash", &["--rcfile", "/si/bash/flare.bash"]);
        let fish = def(
            "fish",
            "/opt/homebrew/bin/fish",
            &["-l", "--init-command", "source '/si/fish/flare.fish'"],
        );
        for d in [zsh, bash, fish] {
            for id in [A, "codex:last", "agy:last"] {
                let r = with_resume(&d, id);
                assert_eq!(r.exe, d.exe, "{}", d.id);
                assert_eq!(
                    r.args, d.args,
                    "{}: the command line is the plain shell",
                    d.id
                );
                assert_eq!(
                    r.env,
                    Some((RESUME_ENV.to_string(), id.to_string())),
                    "{} {id}",
                    d.id
                );
            }
            assert_eq!(with_resume(&d, A).args, with_resume(&d, B).args);
        }
    }

    #[test]
    fn other_shells_are_left_alone() {
        let wsl = def("wsl-Ubuntu", "wsl.exe", &["-d", "Ubuntu"]);
        assert_eq!(
            with_resume(&wsl, A),
            Launch {
                exe: "wsl.exe".to_string(),
                args: vec!["-d".to_string(), "Ubuntu".to_string()],
                env: None
            }
        );
    }
}

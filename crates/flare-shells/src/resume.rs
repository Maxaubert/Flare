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
    let codex = resume == "codex:last";
    let env = if codex {
        None
    } else {
        Some((RESUME_ENV.to_string(), resume.to_string()))
    };
    let (args, env) = match def.id.as_str() {
        "pwsh" | "powershell" if !def.args.is_empty() => {
            let step = if codex {
                "codex resume --last".to_string()
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
            let step = if codex {
                format!("set {NO_CWD_EXE}=1& codex resume --last& set {NO_CWD_EXE}=")
            } else {
                format!(
                    "set {NO_CWD_EXE}=1& set {RESUME_ENV}=& claude --resume %{RESUME_ENV}%& set {NO_CWD_EXE}="
                )
            };
            (vec!["/K".to_string(), step], env)
        }
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
        let r = with_resume(&cmd(), A);
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
        for id in [A, "codex:last"] {
            let r = with_resume(&cmd(), id);
            assert!(
                r.args[1].ends_with("& set NoDefaultCurrentDirectoryInExePath="),
                "{}",
                r.args[1]
            );
        }
    }

    #[test]
    fn a_cmd_resume_never_searches_the_current_folder() {
        for id in [A, "codex:last"] {
            let r = with_resume(&cmd(), id);
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
        let r = with_resume(&cmd(), "codex:last");
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

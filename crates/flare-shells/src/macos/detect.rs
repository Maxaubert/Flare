//! Which shells a Mac tab can start: zsh, bash, fish and PowerShell (pwsh), each found by an
//! absolute path, never through PATH (a Flare started from Finder has a minimal PATH, and a
//! PATH search could land in the tab's own folder). The user's login shell comes first, so it is
//! the default (`list::shell_by_id` takes the first on macOS).

use crate::list::ShellDef;
use crate::macos::integration;
use std::path::Path;
use std::sync::OnceLock;

/// The shells Flare starts with its integration, in menu order after the login shell.
pub const KINDS: [&str; 4] = ["zsh", "bash", "fish", "pwsh"];

/// Homebrew's folders (Apple Silicon, then Intel), searched before `/etc/shells`: a shell
/// installed there is the newer one the user chose.
pub const HOMEBREW_BINS: [&str; 2] = ["/opt/homebrew/bin", "/usr/local/bin"];

/// Where the PowerShell package installs pwsh when it is not on a Homebrew path.
pub const PWSH_PKG: &str = "/usr/local/microsoft/powershell/7/pwsh";

/// What detection reads from the machine (fakeable).
pub trait MacHost: Send + Sync {
    /// The user's login shell (the user record's `UserShell`).
    fn login_shell(&self) -> Option<String>;
    /// The text of `/etc/shells`.
    fn etc_shells(&self) -> String;
    /// Whether `path` is an executable file.
    fn is_exec(&self, path: &str) -> bool;
}

pub struct RealHost;

impl MacHost for RealHost {
    fn login_shell(&self) -> Option<String> {
        crate::macos::sys::user_record()
            .0
            .or_else(|| std::env::var("SHELL").ok())
            .filter(|s| s.starts_with('/'))
    }

    fn etc_shells(&self) -> String {
        std::fs::read_to_string("/etc/shells").unwrap_or_default() // sync-ok: detect worker only
    }

    fn is_exec(&self, path: &str) -> bool {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path) // sync-ok: detect worker only
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
}

/// The absolute paths `/etc/shells` lists, in order (comments and blank lines skipped).
pub fn parse_etc_shells(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.split('#').next().unwrap_or("").trim())
        .filter(|l| l.starts_with('/'))
        .map(str::to_string)
        .collect()
}

/// The shell kind of a path: its file name (`/opt/homebrew/bin/fish` is "fish").
pub fn kind_of(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Where a shell of `kind` is: the login shell when it is that kind, else Homebrew, else
/// `/etc/shells`, else `/bin` (and the PowerShell package path for pwsh). The first executable one.
pub fn find(
    host: &dyn MacHost,
    kind: &str,
    login: Option<&str>,
    listed: &[String],
) -> Option<String> {
    let mut candidates: Vec<String> = Vec::new();
    if let Some(l) = login.filter(|l| kind_of(l) == kind) {
        candidates.push(l.to_string());
    }
    candidates.extend(HOMEBREW_BINS.iter().map(|b| format!("{b}/{kind}")));
    candidates.extend(listed.iter().filter(|p| kind_of(p) == kind).cloned());
    candidates.push(format!("/bin/{kind}"));
    if kind == "pwsh" {
        candidates.push(PWSH_PKG.to_string());
    }
    candidates.into_iter().find(|c| host.is_exec(c))
}

/// One shell's menu entry. With an integration folder the shell reports its folder; without one
/// (the files could not be written) it still starts as a login shell.
pub fn def_for(kind: &str, exe: String, integration_dir: Option<&Path>) -> ShellDef {
    let (name, args) = match (kind, integration_dir) {
        ("zsh", _) => ("zsh", vec!["-l".to_string()]),
        ("bash", Some(d)) => ("bash", integration::bash_args(d)),
        ("bash", None) => ("bash", vec!["-l".to_string()]),
        ("fish", Some(d)) => ("fish", integration::fish_args(d)),
        ("fish", None) => ("fish", vec!["-l".to_string()]),
        ("pwsh", _) => ("PowerShell", integration::pwsh_args()),
        (other, _) => (other, vec!["-l".to_string()]),
    };
    ShellDef {
        id: kind.to_string(),
        name: name.to_string(),
        exe,
        args,
    }
}

/// The login shell first (the default), then zsh, bash, fish and pwsh where installed. A login
/// shell of another kind (tcsh, nu) is listed first under its own name, as a plain login shell.
pub fn detect_with(host: &dyn MacHost, integration_dir: Option<&Path>) -> Vec<ShellDef> {
    let login = host.login_shell().filter(|l| host.is_exec(l));
    let listed = parse_etc_shells(&host.etc_shells());
    let mut list: Vec<ShellDef> = KINDS
        .iter()
        .filter_map(|k| {
            find(host, k, login.as_deref(), &listed).map(|exe| def_for(k, exe, integration_dir))
        })
        .collect();
    if let Some(l) = &login {
        let kind = kind_of(l);
        if let Some(i) = list.iter().position(|d| d.id == kind) {
            let d = list.remove(i);
            list.insert(0, d);
        } else if !kind.is_empty() {
            list.insert(0, def_for(kind, l.clone(), integration_dir));
        }
    }
    list
}

/// Detected once per run: installs the integration files, then lists the shells.
pub fn cached() -> &'static [ShellDef] {
    static CELL: OnceLock<Vec<ShellDef>> = OnceLock::new();
    CELL.get_or_init(|| {
        // sync-ok: runs only on the spawn worker, cached for the run
        let dir = integration::default_dir().filter(|d| integration::activate(d));
        detect_with(&RealHost, dir.as_deref())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    struct Fake {
        login: Option<&'static str>,
        etc: &'static str,
        files: HashSet<&'static str>,
    }

    impl MacHost for Fake {
        fn login_shell(&self) -> Option<String> {
            self.login.map(str::to_string)
        }
        fn etc_shells(&self) -> String {
            self.etc.to_string()
        }
        fn is_exec(&self, path: &str) -> bool {
            self.files.contains(path)
        }
    }

    const ETC: &str = "# List of acceptable shells\n\n/bin/bash\n/bin/csh\n/bin/sh\n/bin/zsh\n/opt/homebrew/bin/fish # brew\n";

    fn stock(login: &'static str) -> Fake {
        Fake {
            login: Some(login),
            etc: ETC,
            files: ["/bin/bash", "/bin/zsh", "/bin/sh", "/bin/csh"].into(),
        }
    }

    fn ids(l: &[ShellDef]) -> Vec<&str> {
        l.iter().map(|d| d.id.as_str()).collect()
    }

    #[test]
    fn a_stock_mac_lists_zsh_then_bash() {
        let l = detect_with(&stock("/bin/zsh"), None);
        assert_eq!(ids(&l), ["zsh", "bash"]);
        assert_eq!(l[0].exe, "/bin/zsh");
        assert_eq!(l[1].exe, "/bin/bash");
    }

    #[test]
    fn the_login_shell_comes_first_and_is_the_default() {
        let mut f = stock("/opt/homebrew/bin/fish");
        f.files.insert("/opt/homebrew/bin/fish");
        let l = detect_with(&f, None);
        assert_eq!(ids(&l), ["fish", "zsh", "bash"]);
        assert_eq!(l[0].exe, "/opt/homebrew/bin/fish");
        assert_eq!(crate::list::shell_by_id(None, &l).unwrap().id, "fish");
        assert_eq!(
            crate::list::shell_by_id(Some("gone"), &l).unwrap().id,
            "fish"
        );
    }

    #[test]
    fn homebrew_wins_over_the_system_copy_unless_the_login_shell_says_otherwise() {
        let mut f = stock("/bin/zsh");
        f.files.insert("/opt/homebrew/bin/bash");
        let l = detect_with(&f, None);
        assert_eq!(l[1].exe, "/opt/homebrew/bin/bash");
        let mut f = stock("/bin/bash");
        f.files.insert("/opt/homebrew/bin/bash");
        let l = detect_with(&f, None);
        assert_eq!(l[0].exe, "/bin/bash");
    }

    #[test]
    fn pwsh_is_found_on_homebrew_or_the_package_path() {
        let mut f = stock("/bin/zsh");
        f.files.insert(PWSH_PKG);
        let l = detect_with(&f, None);
        assert_eq!(ids(&l), ["zsh", "bash", "pwsh"]);
        assert_eq!(l[2].exe, PWSH_PKG);
        assert_eq!(l[2].name, "PowerShell");
        f.files.insert("/usr/local/bin/pwsh");
        assert_eq!(detect_with(&f, None)[2].exe, "/usr/local/bin/pwsh");
    }

    #[test]
    fn a_listed_shell_that_is_not_installed_is_left_out() {
        let mut f = stock("/bin/zsh");
        f.files.remove("/bin/bash");
        assert_eq!(ids(&detect_with(&f, None)), ["zsh"]);
    }

    #[test]
    fn another_login_shell_is_listed_first_as_a_plain_login_shell() {
        let mut f = stock("/bin/tcsh");
        f.files.insert("/bin/tcsh");
        let l = detect_with(&f, None);
        assert_eq!(ids(&l), ["tcsh", "zsh", "bash"]);
        assert_eq!(l[0].args, ["-l"]);
    }

    #[test]
    fn a_missing_login_shell_falls_back_to_zsh() {
        let f = Fake {
            login: Some("/usr/local/bin/gone"),
            ..stock("/bin/zsh")
        };
        assert_eq!(ids(&detect_with(&f, None)), ["zsh", "bash"]);
        let f = Fake {
            login: None,
            ..stock("/bin/zsh")
        };
        assert_eq!(ids(&detect_with(&f, None)), ["zsh", "bash"]);
    }

    #[test]
    fn every_shell_starts_as_a_login_shell() {
        let mut f = stock("/bin/zsh");
        f.files.insert("/opt/homebrew/bin/fish");
        f.files.insert("/opt/homebrew/bin/pwsh");
        let dir = Path::new("/Si");
        for (with, l) in [
            (true, detect_with(&f, Some(dir))),
            (false, detect_with(&f, None)),
        ] {
            for d in &l {
                let login = match d.id.as_str() {
                    "zsh" | "fish" => d.args[0] == "-l",
                    "pwsh" => d.args[0] == "-Login",
                    // With the integration the rcfile reads the login files; without it, -l.
                    "bash" => {
                        if with {
                            d.args == ["--rcfile", "/Si/bash/flare.bash"]
                        } else {
                            d.args == ["-l"]
                        }
                    }
                    other => panic!("unexpected {other}"),
                };
                assert!(login, "{d:?}");
            }
        }
    }

    #[test]
    fn parse_etc_shells_keeps_absolute_paths_only() {
        assert_eq!(
            parse_etc_shells(ETC),
            [
                "/bin/bash",
                "/bin/csh",
                "/bin/sh",
                "/bin/zsh",
                "/opt/homebrew/bin/fish"
            ]
        );
    }

    #[test]
    fn the_real_mac_has_zsh_and_bash_and_lists_the_login_shell_first() {
        let l = detect_with(&RealHost, None);
        let got = ids(&l);
        assert!(got.contains(&"zsh") && got.contains(&"bash"), "{got:?}");
        let login = RealHost.login_shell().unwrap();
        assert_eq!(l[0].id, kind_of(&login));
        for d in &l {
            assert!(d.exe.starts_with('/'), "{d:?}");
        }
    }
}

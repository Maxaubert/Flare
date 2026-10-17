//! ShellDef, parse_wsl_list, shell_by_id, pwsh_path.

/// One shell Flare can start: its saved id, the name shown in the menu, the executable and its arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellDef {
    pub id: String,
    pub name: String,
    pub exe: String,
    pub args: Vec<String>,
}

/// Distro names out of decoded `wsl -l -q` output: split on '\n', trim each line (this also removes '\r'),
/// keep a line only when it is non-empty and every char is [A-Za-z0-9_.-].
pub fn parse_wsl_list(out: &str) -> Vec<String> {
    out.split('\n')
        .map(str::trim)
        .filter(|line| {
            !line.is_empty()
                && line
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
        })
        .map(str::to_string)
        .collect()
}

/// The saved shell if present, else id "pwsh", else "powershell", else the first; None only for an empty list.
pub fn shell_by_id<'a>(id: Option<&str>, list: &'a [ShellDef]) -> Option<&'a ShellDef> {
    let find = |wanted: &str| list.iter().find(|s| s.id == wanted);
    id.and_then(find)
        .or_else(|| find("pwsh"))
        .or_else(|| find("powershell"))
        .or_else(|| list.first())
}

/// PowerShell 7's full path out of `where pwsh.exe` output: the first non-empty trimmed line that is an
/// absolute Windows path (drive form `X:\` or `X:/`, or starting with `\\`, `//`, `\` or `/`) and whose
/// parent folder is NOT `cwd` (compare lower-cased, trailing backslashes removed; the parent is the text before the last
/// '\' or '/'). None when there is no such line.
pub fn pwsh_path(where_out: &str, cwd: &str) -> Option<String> {
    let norm = |s: &str| s.to_lowercase().trim_end_matches('\\').to_string();
    let cwd = norm(cwd);
    where_out
        .lines()
        .map(str::trim)
        .filter(|line| is_absolute_windows_path(line))
        .find(|line| {
            let parent = match line.rfind(['\\', '/']) {
                Some(i) => &line[..i],
                None => "",
            };
            norm(parent) != cwd
        })
        .map(str::to_string)
}

fn is_absolute_windows_path(line: &str) -> bool {
    let b = line.as_bytes();
    let drive = b.len() >= 3
        && b[0].is_ascii_alphabetic()
        && b[1] == b':'
        && (b[2] == b'\\' || b[2] == b'/');
    drive
        || line.starts_with("\\\\")
        || line.starts_with("//")
        || line.starts_with('\\')
        || line.starts_with('/')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list() -> Vec<ShellDef> {
        vec![
            ShellDef {
                id: "pwsh".into(),
                name: "PowerShell 7".into(),
                exe: "pwsh.exe".into(),
                args: vec!["-NoLogo".into()],
            },
            ShellDef {
                id: "powershell".into(),
                name: "Windows PowerShell".into(),
                exe: "powershell.exe".into(),
                args: vec!["-NoLogo".into()],
            },
            ShellDef {
                id: "cmd".into(),
                name: "Command Prompt".into(),
                exe: "cmd.exe".into(),
                args: vec![],
            },
        ]
    }

    #[test]
    fn parse_wsl_list_reads_blank_lines_and_crlf() {
        assert_eq!(
            parse_wsl_list("Ubuntu\r\n\r\nDebian\r\n"),
            vec!["Ubuntu".to_string(), "Debian".to_string()]
        );
    }

    #[test]
    fn parse_wsl_list_is_empty_for_nothing_or_a_banner() {
        assert!(parse_wsl_list("").is_empty());
        assert!(
            parse_wsl_list("Windows Subsystem for Linux has no installed distributions.")
                .is_empty()
        );
    }

    #[test]
    fn parse_wsl_list_keeps_dots_and_dashes() {
        assert_eq!(
            parse_wsl_list("Ubuntu-22.04\r\nopenSUSE-Leap-15.6\r\n"),
            vec!["Ubuntu-22.04".to_string(), "openSUSE-Leap-15.6".to_string()]
        );
    }

    #[test]
    fn shell_by_id_finds_the_saved_shell() {
        let l = list();
        assert_eq!(shell_by_id(Some("cmd"), &l).unwrap().id, "cmd");
    }

    #[test]
    fn shell_by_id_falls_back_to_pwsh() {
        let l = list();
        assert_eq!(shell_by_id(Some("wsl-Arch"), &l).unwrap().id, "pwsh");
        assert_eq!(shell_by_id(None, &l).unwrap().id, "pwsh");
    }

    #[test]
    fn shell_by_id_falls_back_to_powershell() {
        let l = list();
        assert_eq!(shell_by_id(Some("x"), &l[1..]).unwrap().id, "powershell");
    }

    #[test]
    fn shell_by_id_of_an_empty_list_is_none() {
        assert!(shell_by_id(None, &[]).is_none());
    }

    #[test]
    fn pwsh_path_skips_the_current_directory() {
        let out = "C:\\Downloads\\evil\\pwsh.exe\r\nC:\\Program Files\\PowerShell\\7\\pwsh.exe\r\n";
        assert_eq!(
            pwsh_path(out, "C:\\Downloads\\evil"),
            Some("C:\\Program Files\\PowerShell\\7\\pwsh.exe".to_string())
        );
        assert_eq!(
            pwsh_path(
                "C:\\Program Files\\PowerShell\\7\\pwsh.exe\n",
                "C:\\Users\\me"
            ),
            Some("C:\\Program Files\\PowerShell\\7\\pwsh.exe".to_string())
        );
    }

    #[test]
    fn pwsh_path_is_none_when_where_finds_nothing() {
        assert!(pwsh_path("", "C:\\x").is_none());
        assert!(
            pwsh_path(
                "INFO: Could not find files for the given pattern(s).",
                "C:\\x"
            )
            .is_none()
        );
    }
}

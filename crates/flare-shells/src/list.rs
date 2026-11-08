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
/// absolute Windows path (drive form `X:\` or `X:/`, or starting with `\\`, `//`, `\` or `/`) whose
/// CANONICAL form (via `canon`: 8.3 names, trailing dots and `..` resolved) is not inside the canonical
/// `cwd` or any subfolder of it. A line that cannot be canonicalised is rejected. Returns the canonical
/// path. None when there is no such line.
pub fn pwsh_path(
    where_out: &str,
    cwd: &str,
    canon: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    let cwd_c = canon(cwd).unwrap_or_else(|| cwd.to_string());
    where_out
        .lines()
        .map(str::trim)
        .filter(|line| is_absolute_windows_path(line))
        .filter_map(canon)
        .find(|cand| !is_inside(cand, &cwd_c))
}

/// True when `path` is `dir` or lies under it (case-insensitive, either slash, trailing slashes ignored).
fn is_inside(path: &str, dir: &str) -> bool {
    let norm = |s: &str| s.replace('/', "\\").trim_end_matches('\\').to_lowercase();
    let (p, d) = (norm(path), norm(dir));
    if d.is_empty() {
        return false;
    }
    p == d || p.starts_with(&format!("{d}\\"))
}

/// The real canonical path: `std::fs::canonicalize` with the `\\?\` verbatim prefix removed.
pub fn canonical_path(p: &str) -> Option<String> {
    let c = std::fs::canonicalize(p) // sync-ok: detect worker only
        .ok()?
        .to_string_lossy()
        .into_owned();
    Some(match c.strip_prefix("\\\\?\\UNC\\") {
        Some(rest) => format!("\\\\{rest}"),
        None => c.strip_prefix("\\\\?\\").unwrap_or(&c).to_string(),
    })
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

    /// A fake canonicaliser: strips trailing dots and spaces per segment, maps 8.3 names.
    fn canon(p: &str) -> Option<String> {
        let p = p.replace('/', "\\").replace("DOWNLO~1", "Downloads");
        let segs: Vec<&str> = p
            .split('\\')
            .map(|s| {
                if s.chars().all(|c| c == '.') {
                    s
                } else {
                    s.trim_end_matches(['.', ' '])
                }
            })
            .collect();
        Some(segs.join("\\"))
    }

    const GOOD: &str = "C:\\Program Files\\PowerShell\\7\\pwsh.exe";

    #[test]
    fn pwsh_path_skips_the_current_directory() {
        let out = format!("C:\\Downloads\\evil\\pwsh.exe\r\n{GOOD}\r\n");
        assert_eq!(
            pwsh_path(&out, "C:\\Downloads\\evil", &canon),
            Some(GOOD.to_string())
        );
        assert_eq!(
            pwsh_path(&format!("{GOOD}\n"), "C:\\Users\\me", &canon),
            Some(GOOD.to_string())
        );
    }

    #[test]
    fn pwsh_path_rejects_a_cwd_subfolder() {
        let out = format!("C:\\Downloads\\evil\\bin\\deep\\pwsh.exe\n{GOOD}\n");
        assert_eq!(
            pwsh_path(&out, "C:\\Downloads\\evil", &canon),
            Some(GOOD.to_string())
        );
        assert!(
            pwsh_path(
                "C:\\Downloads\\evil\\bin\\pwsh.exe\n",
                "C:\\Downloads\\evil",
                &canon
            )
            .is_none()
        );
    }

    #[test]
    fn pwsh_path_rejects_8dot3_and_trailing_dot_variants_of_cwd() {
        for cand in [
            "C:\\DOWNLO~1\\evil\\pwsh.exe",
            "C:\\Downloads.\\evil\\pwsh.exe",
            "C:\\Downloads\\evil.\\pwsh.exe",
            "C:/Downloads/EVIL/pwsh.exe",
        ] {
            assert!(
                pwsh_path(cand, "C:\\Downloads\\evil", &canon).is_none(),
                "{cand}"
            );
        }
        // the cwd given in 8.3 form is canonicalised too
        assert!(
            pwsh_path(
                "C:\\Downloads\\evil\\pwsh.exe",
                "C:\\DOWNLO~1\\evil",
                &canon
            )
            .is_none()
        );
    }

    #[test]
    fn pwsh_path_rejects_what_cannot_be_canonicalised() {
        assert!(pwsh_path(GOOD, "C:\\x", &|_| None).is_none());
    }

    #[test]
    fn a_sibling_folder_sharing_a_name_prefix_is_not_inside() {
        assert!(!is_inside(
            "C:\\Downloads\\evil2\\pwsh.exe",
            "C:\\Downloads\\evil"
        ));
        assert!(is_inside(
            "c:\\downloads\\EVIL\\a\\pwsh.exe",
            "C:\\Downloads\\evil\\"
        ));
    }

    #[test]
    fn pwsh_path_is_none_when_where_finds_nothing() {
        assert!(pwsh_path("", "C:\\x", &canon).is_none());
        assert!(
            pwsh_path(
                "INFO: Could not find files for the given pattern(s).",
                "C:\\x",
                &canon
            )
            .is_none()
        );
    }

    #[cfg(windows)]
    #[test]
    fn canonical_path_strips_the_verbatim_prefix() {
        let c =
            canonical_path(&std::env::var("SystemRoot").unwrap_or("C:\\Windows".into())).unwrap();
        assert!(!c.starts_with("\\\\?\\"), "{c}");
        assert!(canonical_path("C:\\definitely\\not\\here").is_none());
    }
}

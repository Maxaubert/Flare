//! parse_osc9, decide_follow, cd_command.

use crate::ps_quote::ps_quote;

/// The path out of an OSC 9 payload as handed over by the scanner: the text after the first ';' of the
/// OSC, so a Windows Terminal report arrives as "9;C:\\path". Only payloads starting "9;" count. The rest is
/// trimmed (whitespace), one pair of surrounding double quotes is removed, and the result must be an absolute
/// drive path (`X:\` or `X:/`) or a UNC path (`\\` followed by a char that is not '\' or '/'); else None.
pub fn parse_osc9(data: &str) -> Option<String> {
    let rest = data.strip_prefix("9;")?;
    let mut p = rest.trim();
    if p.len() > 1 && p.starts_with('"') && p.ends_with('"') {
        p = &p[1..p.len() - 1];
    }
    if is_absolute(p) && !has_control(p) {
        Some(p.to_string())
    } else {
        None
    }
}

/// The folder out of a folder report: OSC 9;9 (the Windows Terminal convention, what Flare's
/// PowerShell and cmd hooks send) on every platform, and on macOS also OSC 7 (`file://host/path`,
/// what Flare's zsh, bash, fish and pwsh hooks send there). `ident` is the OSC number, `data`
/// the text after its ';'. None for anything else.
pub fn parse_cwd_report(ident: u32, data: &str) -> Option<String> {
    match ident {
        9 => parse_osc9(data),
        #[cfg(not(windows))]
        7 => crate::macos::osc7::parse_osc7(data),
        _ => None,
    }
}

/// True when the text holds a C0 control, DEL or a C1 control. A real Windows folder name never does, and in
/// a typed `cd` line a CR or LF would end the command and run the rest.
pub fn has_control(p: &str) -> bool {
    p.chars()
        .any(|c| c < ' ' || ('\u{7f}'..='\u{9f}').contains(&c))
}

/// True for a path that may leave this machine: a UNC path (`\\host\share`, also with forward
/// slashes) and every device form (`\\?\`, `\\.\`) except one naming a local drive (`\\?\C:\`) without a `.` or `..` segment.
/// `\\?\UNC\`, `\\.\UNC\` and `\\?\GLOBALROOT\Device\Mup\` all reach SMB, so they count.
pub fn is_unc(p: &str) -> bool {
    let n = p.replace('/', "\\");
    if !n.starts_with("\\\\") {
        return false;
    }
    match n.get(..4) {
        Some("\\\\?\\") | Some("\\\\.\\") => {
            let b = &n.as_bytes()[4..];
            let local = b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\';
            // Windows folds `.` and `..` in a `\\.\` path, so `\\.\C:\..\UNC\h\s` drops the drive and reaches SMB.
            let dotted = n[4..].split('\\').any(|s| s == "." || s == "..");
            !local || dotted
        }
        _ => true,
    }
}

/// True for a folder that may leave this machine (the platform's rule): `is_unc` on Windows.
#[cfg(windows)]
pub fn is_remote(p: &str) -> bool {
    is_unc(p)
}

/// True for a folder that may leave this machine (the platform's rule): on macOS a `//` path, a
/// path under an autofs mount point (`/net`, `/Network`) or one with a `.` or `..` segment
/// (`macos::paths::is_remote`).
#[cfg(not(windows))]
pub fn is_remote(p: &str) -> bool {
    crate::macos::paths::is_remote(p)
}

/// The folder a tab may spawn, restore or stat after a folder report: a remote report is never trusted
/// (Windows would connect over SMB and can leak the NTLM hash; a Mac would mount the server), so the tab
/// keeps its last local folder. Only an explicit user action may use a remote folder.
pub fn folder_after_report<'a>(last_local: &'a str, reported: &'a str) -> &'a str {
    if is_remote(reported) {
        last_local
    } else {
        reported
    }
}

/// `X:\` or `X:/` (a drive path), or `\\` followed by a char that is not a slash (UNC).
fn is_absolute(p: &str) -> bool {
    let b = p.as_bytes();
    let drive = b.len() >= 3
        && b[0].is_ascii_alphabetic()
        && b[1] == b':'
        && (b[2] == b'\\' || b[2] == b'/');
    let unc = b.len() >= 3 && b[0] == b'\\' && b[1] == b'\\' && b[2] != b'\\' && b[2] != b'/';
    drive || unc
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Follow {
    Same,
    Inside,
    Outside,
}

/// norm = replace every run of '\' or '/' by one '\', drop one trailing '\', lower-case.
/// Either empty after norm -> Outside; equal -> Same; cwd starts with root + "\" -> Inside; else Outside.
pub fn decide_follow(root: &str, cwd: &str) -> Follow {
    #[cfg(not(windows))]
    return crate::macos::paths::decide_follow(root, cwd);
    #[cfg(windows)]
    decide_follow_win(root, cwd)
}

/// The Windows rule above (macOS keeps case and `/` only: `macos::paths::decide_follow`).
#[cfg(windows)]
fn decide_follow_win(root: &str, cwd: &str) -> Follow {
    let r = norm(root);
    let c = norm(cwd);
    if r.is_empty() || c.is_empty() {
        Follow::Outside
    } else if r == c {
        Follow::Same
    } else if c.starts_with(&format!("{r}\\")) {
        Follow::Inside
    } else {
        Follow::Outside
    }
}

#[cfg(windows)]
fn norm(p: &str) -> String {
    let mut out = String::with_capacity(p.len());
    let mut last_sep = false;
    for ch in p.chars() {
        if ch == '\\' || ch == '/' {
            if !last_sep {
                out.push('\\');
            }
            last_sep = true;
        } else {
            out.push(ch);
            last_sep = false;
        }
    }
    if out.ends_with('\\') {
        out.pop();
    }
    out.to_lowercase()
}

/// pwsh and powershell: format!("Set-Location -LiteralPath '{}'\r", ps_escape(path)) where every one of
/// ' U+2018 U+2019 U+201A U+201B is doubled. cmd: None if the path has '%', '"', '!' or '^', else
/// format!("cd /d \"{path}\"\r"). Anything else (wsl-*, None), an empty path or one holding a control
/// character (C0, DEL, C1): None (regression rule 133).
pub fn cd_command(shell_id: Option<&str>, path: &str) -> Option<String> {
    if path.is_empty() || has_control(path) {
        return None;
    }
    match shell_id {
        Some("pwsh") | Some("powershell") => {
            Some(format!("Set-Location -LiteralPath {}\r", ps_quote(path)))
        }
        Some("cmd") if !path.contains(['%', '"', '!', '^']) => Some(format!("cd /d \"{path}\"\r")),
        // macOS: zsh, bash and fish (`macos::cd`).
        #[cfg(not(windows))]
        Some(id) => crate::macos::cd::cd_line(id, path),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_osc9_reads_the_report_quoted_or_not() {
        assert_eq!(
            parse_osc9("9;C:\\Users\\me\\proj"),
            Some("C:\\Users\\me\\proj".to_string())
        );
        assert_eq!(
            parse_osc9("9;\"C:\\Users\\me\\proj\""),
            Some("C:\\Users\\me\\proj".to_string())
        );
        assert_eq!(
            parse_osc9("9;\\\\srv\\share\\x"),
            Some("\\\\srv\\share\\x".to_string())
        );
    }

    #[test]
    fn parse_osc9_refuses_everything_else() {
        assert_eq!(parse_osc9("4;100;50"), None);
        assert_eq!(parse_osc9("9;"), None);
        assert_eq!(parse_osc9("9;Cert:\\LocalMachine"), None);
        assert_eq!(parse_osc9("9;/home/me"), None);
    }

    #[cfg(windows)]
    #[test]
    fn decide_follow_tells_same_inside_outside() {
        let root = "C:\\work\\Atlas";
        assert_eq!(decide_follow(root, "c:\\WORK\\atlas"), Follow::Same);
        assert_eq!(decide_follow(root, "C:\\work\\Atlas\\"), Follow::Same);
        assert_eq!(
            decide_follow(root, "C:\\work\\Atlas\\src\\main"),
            Follow::Inside
        );
        assert_eq!(decide_follow(root, "C:\\work\\Atlas2"), Follow::Outside);
        assert_eq!(decide_follow(root, "C:\\work"), Follow::Outside);
        assert_eq!(decide_follow(root, "D:\\"), Follow::Outside);
    }

    #[test]
    fn cd_command_for_the_powershells() {
        assert_eq!(
            cd_command(Some("pwsh"), "C:\\it's\\here"),
            Some("Set-Location -LiteralPath 'C:\\it''s\\here'\r".to_string())
        );
        assert_eq!(
            cd_command(Some("powershell"), "C:\\a b"),
            Some("Set-Location -LiteralPath 'C:\\a b'\r".to_string())
        );
    }

    #[test]
    fn cd_command_doubles_the_curly_quotes() {
        assert_eq!(
            cd_command(Some("pwsh"), "C:\\x\u{2019}; calc; \u{2019}y"),
            Some(
                "Set-Location -LiteralPath 'C:\\x\u{2019}\u{2019}; calc; \u{2019}\u{2019}y'\r"
                    .to_string()
            )
        );
        assert_eq!(
            cd_command(Some("pwsh"), "C:\\a\u{2018}b\u{201A}c\u{201B}d"),
            Some(
                "Set-Location -LiteralPath 'C:\\a\u{2018}\u{2018}b\u{201A}\u{201A}c\u{201B}\u{201B}d'\r"
                    .to_string()
            )
        );
    }

    #[test]
    fn cd_command_for_cmd_uses_cd_d() {
        assert_eq!(
            cd_command(Some("cmd"), "D:\\a b"),
            Some("cd /d \"D:\\a b\"\r".to_string())
        );
    }

    #[test]
    fn cd_command_for_cmd_refuses_percent_and_quote() {
        assert_eq!(cd_command(Some("cmd"), "D:\\%PATH%\\x"), None);
        assert_eq!(cd_command(Some("cmd"), "D:\\a\"b"), None);
    }

    #[test]
    fn parse_osc9_refuses_control_characters() {
        for bad in [
            "9;C:\\x\rcalc",
            "9;C:\\x\ncalc",
            "9;C:\\x\x1b[0m",
            "9;C:\\x\x7f",
            "9;C:\\x\u{85}y",
            "9;C:\\x\u{9b}y",
            "9;C:\\x\0y",
            "9;\\\\h\\s\rcalc",
        ] {
            assert_eq!(parse_osc9(bad), None, "{bad:?}");
        }
        assert_eq!(
            parse_osc9("9;C:\\Users\\Zo\u{eb} \u{65e5}\u{672c}\\a b"),
            Some("C:\\Users\\Zo\u{eb} \u{65e5}\u{672c}\\a b".to_string())
        );
    }

    #[test]
    fn cd_command_refuses_control_characters_in_every_shell() {
        for shell in ["pwsh", "powershell", "cmd"] {
            for bad in [
                "C:\\x\rcalc",
                "C:\\x\ncalc",
                "C:\\x\x1b",
                "C:\\x\x7f",
                "C:\\x\u{85}",
                "C:\\x\u{9b}",
            ] {
                assert_eq!(cd_command(Some(shell), bad), None, "{shell} {bad:?}");
            }
        }
        assert!(cd_command(Some("pwsh"), "C:\\a b\\\u{e9}").is_some());
    }

    #[test]
    fn cd_command_for_cmd_refuses_bang_and_caret() {
        assert_eq!(cd_command(Some("cmd"), "D:\\a!b"), None);
        assert_eq!(cd_command(Some("cmd"), "D:\\a^b"), None);
    }

    #[cfg(windows)]
    #[test]
    fn a_unc_report_never_replaces_the_local_folder() {
        for unc in [
            "\\\\evil\\share",
            "//evil/share",
            "\\\\?\\UNC\\evil\\share",
            "\\\\.\\UNC\\evil\\share",
            "\\\\.\\unc\\evil\\share",
            "\\\\?\\GLOBALROOT\\Device\\Mup\\evil\\share",
            "//?/UNC/evil/share",
        ] {
            assert!(is_unc(unc), "{unc}");
            assert_eq!(folder_after_report("C:\\work", unc), "C:\\work");
        }
        assert!(!is_unc("C:\\work"));
        assert!(!is_unc("\\\\?\\C:\\work"));
        assert!(!is_unc("\\\\.\\D:\\work"));
        // A device form with a dot segment is normalised by Windows and can drop the drive into SMB.
        assert!(is_unc("\\\\.\\C:\\..\\UNC\\h\\s"));
        assert!(is_unc("//./C:/../UNC/h/s"));
        assert!(is_unc("\\\\.\\C:\\a\\.\\b"));
        assert!(is_unc("\\\\?\\C:\\..\\x"));
        assert_eq!(folder_after_report("C:\\work", "D:\\other"), "D:\\other");
    }

    #[test]
    fn a_folder_report_is_osc_9_9_on_every_platform() {
        assert_eq!(parse_cwd_report(9, "9;C:\\w").as_deref(), Some("C:\\w"));
        assert!(parse_cwd_report(9, "file:///tmp").is_none());
        assert!(parse_cwd_report(8, "9;C:\\w").is_none());
    }

    #[cfg(windows)]
    #[test]
    fn osc_7_is_no_folder_report_on_windows() {
        assert!(parse_cwd_report(7, "file:///C:/w").is_none());
        assert!(parse_cwd_report(7, "file:///tmp").is_none());
    }

    #[cfg(not(windows))]
    #[test]
    fn osc_7_is_the_folder_report_on_macos() {
        assert_eq!(
            parse_cwd_report(7, "file:///tmp/a%20b").as_deref(),
            Some("/tmp/a b")
        );
        assert!(parse_cwd_report(7, "file://far-away/tmp").is_none());
        assert!(parse_cwd_report(7, "9;C:\\w").is_none());
    }

    #[cfg(not(windows))]
    #[test]
    fn cd_command_for_the_mac_shells() {
        assert_eq!(
            cd_command(Some("zsh"), "/Users/me/a b").as_deref(),
            Some("cd -- '/Users/me/a b'\r")
        );
        assert_eq!(
            cd_command(Some("bash"), "/tmp/it's").as_deref(),
            Some("cd -- '/tmp/it'\\''s'\r")
        );
        assert_eq!(
            cd_command(Some("fish"), "/tmp").as_deref(),
            Some("cd '/tmp'\r")
        );
        assert_eq!(
            cd_command(Some("pwsh"), "/tmp/a b").as_deref(),
            Some("Set-Location -LiteralPath '/tmp/a b'\r")
        );
        for shell in ["zsh", "bash", "fish"] {
            for bad in [
                "/tmp/a\rcalc",
                "/tmp/a\ncalc",
                "/tmp/\u{1b}x",
                "/tmp/\u{85}x",
                "",
            ] {
                assert_eq!(cd_command(Some(shell), bad), None, "{shell} {bad:?}");
            }
        }
        assert_eq!(cd_command(Some("cmd"), "D:\\%PATH%"), None);
    }

    #[test]
    fn cd_command_for_other_shells_is_none() {
        assert_eq!(cd_command(Some("wsl-Ubuntu"), "C:\\x"), None);
        assert_eq!(cd_command(None, "C:\\x"), None);
        assert_eq!(cd_command(Some("pwsh"), ""), None);
    }

    #[cfg(not(windows))]
    #[test]
    fn a_remote_report_never_replaces_the_local_folder_on_macos() {
        for remote in [
            "//evil/share",
            "/net/evil/share",
            "/Network/Servers/evil",
            "/a/../net/h",
        ] {
            assert!(is_remote(remote), "{remote}");
            assert_eq!(folder_after_report("/Users/me", remote), "/Users/me");
        }
        assert_eq!(folder_after_report("/Users/me", "/tmp/x"), "/tmp/x");
    }
}

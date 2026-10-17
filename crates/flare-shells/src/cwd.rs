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
    if is_absolute(p) {
        Some(p.to_string())
    } else {
        None
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
/// ' U+2018 U+2019 U+201A U+201B is doubled. cmd: None if the path has '%' or '"', else format!("cd /d \"{path}\"\r").
/// Anything else (wsl-*, None) or an empty path: None.
pub fn cd_command(shell_id: Option<&str>, path: &str) -> Option<String> {
    if path.is_empty() {
        return None;
    }
    match shell_id {
        Some("pwsh") | Some("powershell") => {
            Some(format!("Set-Location -LiteralPath {}\r", ps_quote(path)))
        }
        Some("cmd") if !path.contains(['%', '"']) => Some(format!("cd /d \"{path}\"\r")),
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

    #[test]
    fn decide_follow_tells_same_inside_outside() {
        let root = "C:\\work\\Prism";
        assert_eq!(decide_follow(root, "c:\\WORK\\prism"), Follow::Same);
        assert_eq!(decide_follow(root, "C:\\work\\Prism\\"), Follow::Same);
        assert_eq!(
            decide_follow(root, "C:\\work\\Prism\\src\\main"),
            Follow::Inside
        );
        assert_eq!(decide_follow(root, "C:\\work\\Prism2"), Follow::Outside);
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
    fn cd_command_for_other_shells_is_none() {
        assert_eq!(cd_command(Some("wsl-Ubuntu"), "C:\\x"), None);
        assert_eq!(cd_command(None, "C:\\x"), None);
        assert_eq!(cd_command(Some("pwsh"), ""), None);
    }
}

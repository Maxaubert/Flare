//! Paste rules: sanitising pasted text, quoting copied paths for each shell, and deciding what a paste sends (a key, a bracketed paste, or nothing).

use crate::ps_quote::ps_quote;

/// What the clipboard holds, as the window reads it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClipboardState {
    pub image: bool,
    pub text: String,
    pub files: Vec<String>,
}

/// What a paste does in the terminal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PasteDecision {
    /// Forward the Ctrl+V key so the program reads the image itself.
    Key,
    /// Send this text as a bracketed paste.
    Text(String),
    /// Send nothing.
    None,
}

/// The shell a copied path is quoted for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathShell {
    PowerShell,
    Cmd,
    Bash,
    /// A WSL distro's bash: a Windows path is written as the Linux path it has inside (`/mnt/c/...`).
    Wsl,
    /// fish (macOS): single quotes in which only `\` and `'` are special, each escaped with a `\`.
    Fish,
    /// tcsh and csh (macOS): the POSIX single quotes, plus `!` written as `\!`, because history
    /// expansion still acts inside single quotes there.
    Csh,
}

/// Removes every char in U+0000..=U+0008, U+000B, U+000C, U+000E..=U+001F, U+007F and U+009B.
/// Keeps TAB, LF, CR and all other text. (A paste must not be able to end the bracketed paste:
/// regression rule 1.)
pub fn sanitize_paste(text: &str) -> String {
    text.chars()
        .filter(|&c| {
            let n = c as u32;
            !matches!(n, 0x00..=0x08 | 0x0B | 0x0C | 0x0E..=0x1F | 0x7F | 0x9B)
        })
        .collect()
}

/// True for C0, DEL and C1 characters. A real Windows path never holds one.
fn is_control(c: char) -> bool {
    let n = c as u32;
    n < 0x20 || (0x7F..=0x9F).contains(&n)
}

/// A Windows path as the WSL distro sees it: `C:\a\b` is `/mnt/c/a/b`, `\\wsl$\Ubuntu\home\me` (and
/// `\\wsl.localhost\...`) is `/home/me`. A path already in Linux form passes as is. `None` for what WSL
/// cannot name (a UNC share, a relative path with backslashes): it is refused, never guessed.
pub fn wsl_path(p: &str) -> Option<String> {
    let b = p.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        let rest = &p[2..];
        if !rest.is_empty() && !rest.starts_with(['\\', '/']) {
            return None; // `C:foo` is relative to the drive's own folder
        }
        let drive = (b[0] as char).to_ascii_lowercase();
        let tail = rest.replace('\\', "/");
        return Some(format!("/mnt/{drive}{tail}"));
    }
    if let Some(unc) = p.strip_prefix("\\\\") {
        let mut parts = unc.splitn(3, '\\');
        let host = parts.next()?.to_ascii_lowercase();
        if host == "wsl$" || host == "wsl.localhost" {
            let _distro = parts.next()?;
            let tail = parts.next().unwrap_or("").replace('\\', "/");
            return Some(format!("/{tail}"));
        }
        return None;
    }
    if p.contains('\\') {
        return None;
    }
    Some(p.to_string())
}

/// Quotes one path so the shell reads it as a single inert word. `None` means the path cannot be
/// typed safely and must not be: any control character (every shell), and for cmd any of `%`, `!`
/// or `"`, which still act inside double quotes.
/// Unknown shell and Bash: single quotes with each `'` written as `'\''`; PowerShell: `ps_quote`;
/// Cmd: `"p"`.
pub fn quote_path(p: &str, shell: Option<PathShell>) -> Option<String> {
    if p.chars().any(is_control) {
        return None;
    }
    match shell {
        Some(PathShell::Cmd) => {
            if p.contains(['%', '!', '"']) {
                None
            } else {
                Some(format!("\"{p}\""))
            }
        }
        None | Some(PathShell::Bash) => Some(format!("'{}'", p.replace('\'', "'\\''"))),
        Some(PathShell::Wsl) => {
            let linux = wsl_path(p)?;
            Some(format!("'{}'", linux.replace('\'', "'\\''")))
        }
        Some(PathShell::PowerShell) => Some(ps_quote(p)),
        Some(PathShell::Fish) => Some(format!(
            "'{}'",
            p.replace('\\', "\\\\").replace('\'', "\\'")
        )),
        Some(PathShell::Csh) => Some(format!(
            "'{}'",
            p.replace('\'', "'\\''").replace('!', "\\!")
        )),
    }
}

/// Each path quoted, joined with one space. `None` when any path is refused.
pub fn quote_paths(paths: &[String], shell: Option<PathShell>) -> Option<String> {
    let quoted: Option<Vec<String>> = paths.iter().map(|p| quote_path(p, shell)).collect();
    quoted.map(|q| q.join(" "))
}

/// files non-empty -> Text(quote_paths(..)), or None when a path is refused; else image -> Key;
/// else sanitised text non-empty -> Text(it); else None. Every Text payload passes the control
/// filter, so nothing can end the bracketed paste early (regression rule 135).
pub fn decide_paste(clip: &ClipboardState, shell: Option<PathShell>) -> PasteDecision {
    if !clip.files.is_empty() {
        return match quote_paths(&clip.files, shell) {
            Some(t) if sanitize_paste(&t) == t => PasteDecision::Text(t),
            _ => PasteDecision::None,
        };
    }
    if clip.image {
        return PasteDecision::Key;
    }
    let text = sanitize_paste(&clip.text);
    if text.is_empty() {
        PasteDecision::None
    } else {
        PasteDecision::Text(text)
    }
}

/// What `Terminal.paste` writes: every `\r\n` and every lone `\n` becomes `\r`, then, when the program
/// asked for bracketed paste, `ESC [ 2 0 0 ~` before and `ESC [ 2 0 1 ~` after. The caller has already
/// sanitised the text (`sanitize_paste`); this does not filter again.
pub fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    let body = text.replace("\r\n", "\r").replace('\n', "\r");
    // A program that never asked for bracketed paste (cmd, PSReadLine) runs each line at its CR: the last
    // line is left at the prompt, for the owner to read and send, never run unseen.
    let body = if bracketed {
        body.as_str()
    } else {
        body.trim_end_matches('\r')
    };
    let mut out = Vec::with_capacity(body.len() + 12);
    if bracketed {
        out.extend_from_slice(b"\x1b[200~");
    }
    out.extend_from_slice(body.as_bytes());
    if bracketed {
        out.extend_from_slice(b"\x1b[201~");
    }
    out
}

/// The quoting a tab's shell reads a path in, from its shell id (`pwsh`, `powershell`, `cmd`,
/// `wsl-<distro>`): `cmd` -> Cmd; starts with `wsl` or contains `bash` (case-insensitive) -> Bash;
/// anything else, including `None` and "", -> PowerShell.
///
/// macOS: the same for the ids Windows has (none of them is a Mac shell), then `fish` -> Fish,
/// `pwsh` or `powershell` -> PowerShell, and anything else (zsh, sh, `None` and "") -> the POSIX
/// single quotes (Bash): the login shell is a POSIX one.
pub fn path_shell_of(shell_id: Option<&str>) -> PathShell {
    let Some(id) = shell_id else {
        return other_shell("");
    };
    let lower = id.to_ascii_lowercase();
    if lower == "cmd" {
        PathShell::Cmd
    } else if lower.starts_with("wsl") {
        PathShell::Wsl
    } else if lower.contains("bash") {
        PathShell::Bash
    } else {
        other_shell(&lower)
    }
}

/// Windows: every other id is PowerShell.
#[cfg(windows)]
fn other_shell(_lower: &str) -> PathShell {
    PathShell::PowerShell
}

/// macOS: fish, PowerShell, tcsh and csh, or else the POSIX quotes.
#[cfg(not(windows))]
fn other_shell(lower: &str) -> PathShell {
    match lower {
        "fish" => PathShell::Fish,
        "pwsh" | "powershell" => PathShell::PowerShell,
        l if l.starts_with("tcsh") || l.starts_with("csh") => PathShell::Csh,
        _ => PathShell::Bash,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(image: bool, text: &str, files: &[&str]) -> ClipboardState {
        ClipboardState {
            image,
            text: text.to_string(),
            files: files.iter().map(|f| f.to_string()).collect(),
        }
    }

    // decidePaste

    #[test]
    fn decide_paste_image_forwards_the_key() {
        assert_eq!(decide_paste(&clip(true, "", &[]), None), PasteDecision::Key);
    }

    #[test]
    fn decide_paste_image_beats_text() {
        assert_eq!(
            decide_paste(&clip(true, "x", &[]), None),
            PasteDecision::Key
        );
    }

    #[test]
    fn decide_paste_text_is_text() {
        assert_eq!(
            decide_paste(&clip(false, "a\nb", &[]), None),
            PasteDecision::Text("a\nb".to_string())
        );
    }

    #[test]
    fn decide_paste_files_use_the_shell_quoting() {
        let c = clip(false, "", &["C:\\a b\\s.png"]);
        assert_eq!(
            decide_paste(&c, Some(PathShell::PowerShell)),
            PasteDecision::Text("'C:\\a b\\s.png'".to_string())
        );
        assert_eq!(
            decide_paste(&c, Some(PathShell::Cmd)),
            PasteDecision::Text("\"C:\\a b\\s.png\"".to_string())
        );
    }

    #[test]
    fn decide_paste_image_file_pastes_its_path() {
        let c = clip(true, "", &["C:\\a b\\s.png"]);
        assert_eq!(
            decide_paste(&c, Some(PathShell::PowerShell)),
            PasteDecision::Text("'C:\\a b\\s.png'".to_string())
        );
    }

    #[test]
    fn decide_paste_files_beat_their_text_form() {
        let c = clip(false, "s.png", &["C:\\x\\s.png"]);
        assert_eq!(
            decide_paste(&c, Some(PathShell::PowerShell)),
            PasteDecision::Text("'C:\\x\\s.png'".to_string())
        );
    }

    #[test]
    fn decide_paste_empty_is_none() {
        assert_eq!(
            decide_paste(&ClipboardState::default(), None),
            PasteDecision::None
        );
    }

    #[test]
    fn decide_paste_controls_only_is_none() {
        assert_eq!(
            decide_paste(&clip(false, "\u{1b}[201~", &[]), None),
            PasteDecision::Text("[201~".to_string())
        );
        assert_eq!(
            decide_paste(&clip(false, "\u{1b}\u{7}", &[]), None),
            PasteDecision::None
        );
    }

    // sanitizePaste

    #[test]
    fn sanitize_paste_removes_every_esc_and_c1_csi() {
        let out = sanitize_paste("echo hi\u{1b}[201~rm -rf ~\r");
        assert_eq!(out, "echo hi[201~rm -rf ~\r");
        assert!(!out.contains('\u{1b}'));
        assert_eq!(sanitize_paste("a\u{9b}201~b"), "a201~b");
    }

    #[test]
    fn sanitize_paste_keeps_tab_lf_cr_and_text() {
        let s = "a\tb\nc\r\nd \u{fc} \u{5b57} \u{1f389}";
        assert_eq!(sanitize_paste(s), s);
    }

    #[test]
    fn sanitize_paste_drops_other_controls_and_del() {
        assert_eq!(sanitize_paste("a\u{0}b\u{3}c\u{7f}d"), "abcd");
    }

    // quotePath

    #[test]
    fn quote_path_powershell_single_quotes() {
        assert_eq!(
            quote_path(
                "C:\\d\\report$(Start-Process calc).pdf",
                Some(PathShell::PowerShell)
            )
            .as_deref(),
            Some("'C:\\d\\report$(Start-Process calc).pdf'")
        );
        assert_eq!(
            quote_path("C:\\$HOME\\x", Some(PathShell::PowerShell)).as_deref(),
            Some("'C:\\$HOME\\x'")
        );
    }

    #[test]
    fn quote_path_doubles_quotes() {
        assert_eq!(
            quote_path("C:\\Max's notes", Some(PathShell::PowerShell)).as_deref(),
            Some("'C:\\Max''s notes'")
        );
        assert_eq!(
            quote_path("C:\\Max\u{2019}s notes", Some(PathShell::PowerShell)).as_deref(),
            Some("'C:\\Max\u{2019}\u{2019}s notes'")
        );
    }

    #[test]
    fn quote_path_bash_escape() {
        assert_eq!(
            quote_path("/mnt/c/it's", Some(PathShell::Bash)).as_deref(),
            Some("'/mnt/c/it'\\''s'")
        );
    }

    #[test]
    fn quote_path_cmd_keeps_double_quotes() {
        assert_eq!(
            quote_path("C:\\a b", Some(PathShell::Cmd)).as_deref(),
            Some("\"C:\\a b\"")
        );
    }

    #[test]
    fn quote_path_unknown_shell_uses_single_quotes() {
        assert_eq!(quote_path("C:\\a b", None).as_deref(), Some("'C:\\a b'"));
        assert_eq!(
            quote_path("/x/$(calc).txt", None).as_deref(),
            Some("'/x/$(calc).txt'")
        );
        assert_eq!(
            quote_path("/x/`calc`.txt", None).as_deref(),
            Some("'/x/`calc`.txt'")
        );
        assert_eq!(quote_path("/it's", None).as_deref(), Some("'/it'\\''s'"));
    }

    #[test]
    fn quote_path_bash_inert_forms() {
        assert_eq!(
            quote_path("/x/$(calc).txt", Some(PathShell::Bash)).as_deref(),
            Some("'/x/$(calc).txt'")
        );
        assert_eq!(
            quote_path("/x/\"q\"!", Some(PathShell::Bash)).as_deref(),
            Some("'/x/\"q\"!'")
        );
    }

    #[test]
    fn quote_path_cmd_refuses_percent_bang_and_quote() {
        for p in ["C:\\%COMSPEC%.txt", "C:\\a!b", "C:\\a\"b", "C:\\%"] {
            assert_eq!(quote_path(p, Some(PathShell::Cmd)), None, "{p}");
        }
    }

    #[test]
    fn quote_path_refuses_control_characters_in_every_shell() {
        let shells = [
            None,
            Some(PathShell::Cmd),
            Some(PathShell::Bash),
            Some(PathShell::PowerShell),
        ];
        for sh in shells {
            for bad in [
                "C:\\x\r calc",
                "C:\\x\ncalc",
                "C:\\x\u{1b}[201~calc",
                "C:\\x\u{7f}",
                "C:\\x\u{9b}201~",
                "C:\\x\u{85}",
                "C:\\x\u{0}",
                "C:\\x\ty",
            ] {
                assert_eq!(quote_path(bad, sh), None, "{bad:?} {sh:?}");
            }
        }
    }

    // quotePaths

    #[test]
    fn quote_paths_joins_with_spaces() {
        let paths = vec!["C:\\a.png".to_string(), "C:\\b c.png".to_string()];
        assert_eq!(
            quote_paths(&paths, Some(PathShell::PowerShell)).as_deref(),
            Some("'C:\\a.png' 'C:\\b c.png'")
        );
        assert_eq!(
            quote_paths(&["C:\\a.png".to_string()], None).as_deref(),
            Some("'C:\\a.png'")
        );
    }

    #[test]
    fn quote_paths_refuses_all_when_one_is_refused() {
        let paths = vec!["C:\\ok.png".to_string(), "C:\\%X%.png".to_string()];
        assert_eq!(quote_paths(&paths, Some(PathShell::Cmd)), None);
    }

    #[test]
    fn decide_paste_refused_path_sends_nothing() {
        let c = clip(false, "", &["C:\\%COMSPEC%.txt"]);
        assert_eq!(decide_paste(&c, Some(PathShell::Cmd)), PasteDecision::None);
        let c = clip(false, "fallback", &["C:\\x\u{1b}[201~calc"]);
        assert_eq!(
            decide_paste(&c, Some(PathShell::PowerShell)),
            PasteDecision::None
        );
    }

    #[test]
    fn decide_paste_path_never_carries_the_bracketed_paste_end() {
        let shells = [
            None,
            Some(PathShell::Cmd),
            Some(PathShell::Bash),
            Some(PathShell::PowerShell),
        ];
        for sh in shells {
            let c = clip(false, "", &["C:\\x\u{1b}[201~calc"]);
            match decide_paste(&c, sh) {
                PasteDecision::Text(t) => {
                    assert!(!t.contains('\u{1b}') && !t.contains("[201~"))
                }
                PasteDecision::None => {}
                PasteDecision::Key => panic!("key"),
            }
        }
    }

    #[test]
    fn a_path_with_an_escape_never_reaches_the_pty() {
        // A file name carrying a raw ESC and an ESC[201~ end marker: every shell refuses the path,
        // so decide_paste types nothing at all (rule 135).
        let shells = [
            None,
            Some(PathShell::Cmd),
            Some(PathShell::Bash),
            Some(PathShell::PowerShell),
        ];
        for sh in shells {
            let c = clip(false, "", &["C:\\x\u{1b}[201~calc\u{1b}"]);
            assert_eq!(decide_paste(&c, sh), PasteDecision::None, "shell {sh:?}");
        }
    }
}

#[cfg(test)]
mod paste_bytes_tests {
    use super::*;

    #[test]
    fn paste_bytes_turns_every_newline_into_a_carriage_return() {
        assert_eq!(paste_bytes("a\r\nb\nc\rd", false), b"a\rb\rc\rd".to_vec());
    }

    #[test]
    fn an_unbracketed_multi_line_paste_never_sends_its_final_enter() {
        assert_eq!(
            paste_bytes("a\nRemove-Item x\n", false),
            b"a\rRemove-Item x".to_vec()
        );
        assert_eq!(paste_bytes("a\r\nb\r\n\r\n", false), b"a\rb".to_vec());
        assert_eq!(paste_bytes("\n", false), Vec::<u8>::new());
        // Bracketed, the program holds the whole block until the owner sends it.
        assert_eq!(paste_bytes("a\n", true), b"\x1b[200~a\r\x1b[201~".to_vec());
    }

    #[test]
    fn a_wsl_tab_gets_the_linux_path() {
        let q = |p: &str| quote_path(p, Some(PathShell::Wsl));
        assert_eq!(
            q(r"C:\Users\me\shot.png").as_deref(),
            Some("'/mnt/c/Users/me/shot.png'")
        );
        assert_eq!(
            q(r"d:\a b\it's.png").as_deref(),
            Some(r"'/mnt/d/a b/it'\''s.png'")
        );
        assert_eq!(q(r"C:\").as_deref(), Some("'/mnt/c/'"));
        assert_eq!(
            q(r"\\wsl$\Ubuntu\home\me\a.txt").as_deref(),
            Some("'/home/me/a.txt'")
        );
        assert_eq!(
            q(r"\\wsl.localhost\Ubuntu\tmp\a.txt").as_deref(),
            Some("'/tmp/a.txt'")
        );
        assert_eq!(q("/mnt/c/x").as_deref(), Some("'/mnt/c/x'"));
        // What WSL cannot name is refused, not guessed.
        assert_eq!(q(r"\\server\share\a.txt"), None);
        assert_eq!(q(r"C:foo"), None);
        assert_eq!(q(r"sub\a.txt"), None);
        assert_eq!(q("C:\\a\u{1b}b"), None);
    }

    #[test]
    fn paste_bytes_brackets_when_asked() {
        assert_eq!(paste_bytes("x", true), b"\x1b[200~x\x1b[201~".to_vec());
    }

    #[test]
    fn a_multi_line_paste_is_one_bracketed_block() {
        assert_eq!(
            paste_bytes("one\ntwo", true),
            b"\x1b[200~one\rtwo\x1b[201~".to_vec()
        );
        let bytes = paste_bytes("one\ntwo", true);
        let end: &[u8] = b"\x1b[201~";
        let count = bytes.windows(end.len()).filter(|w| *w == end).count();
        assert_eq!(count, 1);
    }

    #[test]
    fn a_sanitised_hostile_paste_cannot_end_the_bracket() {
        let bytes = paste_bytes(&sanitize_paste("x\x1b[201~rm -rf /\r"), true);
        let end: &[u8] = b"\x1b[201~";
        let positions: Vec<usize> = bytes
            .windows(end.len())
            .enumerate()
            .filter(|(_, w)| *w == end)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(positions, vec![bytes.len() - end.len()]);
    }

    #[test]
    fn an_empty_paste_is_an_empty_frame_or_nothing() {
        assert!(paste_bytes("", false).is_empty());
        assert_eq!(paste_bytes("", true), b"\x1b[200~\x1b[201~".to_vec());
    }

    #[test]
    fn shell_ids_read_as_the_language_they_speak() {
        assert_eq!(path_shell_of(Some("pwsh")), PathShell::PowerShell);
        assert_eq!(path_shell_of(Some("powershell")), PathShell::PowerShell);
        assert_eq!(path_shell_of(Some("cmd")), PathShell::Cmd);
        assert_eq!(path_shell_of(Some("CMD")), PathShell::Cmd);
        assert_eq!(path_shell_of(Some("wsl-Ubuntu")), PathShell::Wsl);
        assert_eq!(path_shell_of(Some("wsl-Debian-12")), PathShell::Wsl);
        assert_eq!(path_shell_of(Some("git-bash")), PathShell::Bash);
        #[cfg(windows)]
        {
            assert_eq!(path_shell_of(Some("")), PathShell::PowerShell);
            assert_eq!(path_shell_of(None), PathShell::PowerShell);
            assert_eq!(path_shell_of(Some("fish")), PathShell::PowerShell);
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn mac_shell_ids_read_as_the_language_they_speak() {
        assert_eq!(path_shell_of(Some("zsh")), PathShell::Bash);
        assert_eq!(path_shell_of(Some("bash")), PathShell::Bash);
        assert_eq!(path_shell_of(Some("fish")), PathShell::Fish);
        assert_eq!(path_shell_of(Some("pwsh")), PathShell::PowerShell);
        assert_eq!(path_shell_of(None), PathShell::Bash);
        assert_eq!(path_shell_of(Some("")), PathShell::Bash);
        assert_eq!(path_shell_of(Some("tcsh")), PathShell::Csh);
        assert_eq!(path_shell_of(Some("csh")), PathShell::Csh);
    }

    #[test]
    fn csh_quotes_escape_the_history_bang_inside_the_single_quotes() {
        assert_eq!(
            quote_path("/tmp/a!ls b", Some(PathShell::Csh)).as_deref(),
            Some("'/tmp/a\\!ls b'")
        );
        assert_eq!(
            quote_path("/tmp/it's $(x) `y`", Some(PathShell::Csh)).as_deref(),
            Some("'/tmp/it'\\''s $(x) `y`'")
        );
        assert_eq!(quote_path("/a\nb", Some(PathShell::Csh)), None);
    }

    #[test]
    fn fish_quotes_escape_only_the_backslash_and_the_quote() {
        assert_eq!(
            quote_path("/tmp/it's \\ $x *", Some(PathShell::Fish)).as_deref(),
            Some("'/tmp/it\\'s \\\\ $x *'")
        );
        assert_eq!(quote_path("/a\nb", Some(PathShell::Fish)), None);
    }
}

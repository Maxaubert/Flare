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

/// None or Cmd: `"p"`; Bash: single quotes with each `'` written as `'\''`; PowerShell: `ps_quote`.
pub fn quote_path(p: &str, shell: Option<PathShell>) -> String {
    match shell {
        None | Some(PathShell::Cmd) => format!("\"{p}\""),
        Some(PathShell::Bash) => format!("'{}'", p.replace('\'', "'\\''")),
        Some(PathShell::PowerShell) => ps_quote(p),
    }
}

/// Each path quoted, joined with one space.
pub fn quote_paths(paths: &[String], shell: Option<PathShell>) -> String {
    paths
        .iter()
        .map(|p| quote_path(p, shell))
        .collect::<Vec<_>>()
        .join(" ")
}

/// files non-empty -> Text(quote_paths(..)); else image -> Key; else sanitised text non-empty -> Text(it); else None.
pub fn decide_paste(clip: &ClipboardState, shell: Option<PathShell>) -> PasteDecision {
    if !clip.files.is_empty() {
        return PasteDecision::Text(quote_paths(&clip.files, shell));
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
            ),
            "'C:\\d\\report$(Start-Process calc).pdf'"
        );
        assert_eq!(
            quote_path("C:\\$HOME\\x", Some(PathShell::PowerShell)),
            "'C:\\$HOME\\x'"
        );
    }

    #[test]
    fn quote_path_doubles_quotes() {
        assert_eq!(
            quote_path("C:\\Max's notes", Some(PathShell::PowerShell)),
            "'C:\\Max''s notes'"
        );
        assert_eq!(
            quote_path("C:\\Max\u{2019}s notes", Some(PathShell::PowerShell)),
            "'C:\\Max\u{2019}\u{2019}s notes'"
        );
    }

    #[test]
    fn quote_path_bash_escape() {
        assert_eq!(
            quote_path("/mnt/c/it's", Some(PathShell::Bash)),
            "'/mnt/c/it'\\''s'"
        );
    }

    #[test]
    fn quote_path_cmd_and_unnamed_keep_double_quotes() {
        assert_eq!(quote_path("C:\\a b", Some(PathShell::Cmd)), "\"C:\\a b\"");
        assert_eq!(quote_path("C:\\a b", None), "\"C:\\a b\"");
    }

    // quotePaths

    #[test]
    fn quote_paths_joins_with_spaces() {
        let paths = vec!["C:\\a.png".to_string(), "C:\\b c.png".to_string()];
        assert_eq!(
            quote_paths(&paths, Some(PathShell::PowerShell)),
            "'C:\\a.png' 'C:\\b c.png'"
        );
        assert_eq!(
            quote_paths(&["C:\\a.png".to_string()], None),
            "\"C:\\a.png\""
        );
    }
}

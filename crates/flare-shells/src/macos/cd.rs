//! The `cd` line a tab types into zsh, bash or fish (the macOS arms of `cwd::cd_command`).

/// zsh and bash: `cd -- '<path>'`; fish: `cd '<path>'` in fish's quoting. Only an absolute path
/// (every folder report is one), so the line can never be read as an option. Other shells: None.
/// The caller has already refused an empty path and one holding a control character.
pub fn cd_line(shell_id: &str, path: &str) -> Option<String> {
    if !path.starts_with('/') {
        return None;
    }
    match shell_id {
        "zsh" | "bash" => Some(format!("cd -- {}\r", sh_quote(path))),
        "fish" => Some(format!("cd {}\r", fish_quote(path))),
        _ => None,
    }
}

/// POSIX single quotes: everything inside is literal, and a `'` is written `'\''`.
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// fish single quotes: only `\` and `'` are special inside, each escaped with a `\`.
pub fn fish_quote(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn posix_shells_get_a_quoted_cd_that_ends_options() {
        assert_eq!(
            cd_line("zsh", "/Users/max/a b").as_deref(),
            Some("cd -- '/Users/max/a b'\r")
        );
        assert_eq!(
            cd_line("bash", "/tmp/it's $(x) `y`").as_deref(),
            Some("cd -- '/tmp/it'\\''s $(x) `y`'\r")
        );
    }

    #[test]
    fn fish_gets_its_own_quoting() {
        assert_eq!(
            cd_line("fish", "/tmp/it's \\ $x").as_deref(),
            Some("cd '/tmp/it\\'s \\\\ $x'\r")
        );
    }

    #[test]
    fn a_relative_path_or_an_unknown_shell_gets_no_line() {
        assert!(cd_line("zsh", "-P").is_none());
        assert!(cd_line("zsh", "proj").is_none());
        assert!(cd_line("tcsh", "/tmp").is_none());
    }
}

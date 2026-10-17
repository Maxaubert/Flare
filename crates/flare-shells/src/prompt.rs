//! PS_PROMPT_HOOK, PS_FILE_STYLE, cmd_prompt.
//!
//! The prompt reports where it is (OSC 9;9, the Windows Terminal convention). The PowerShell
//! hook wraps whatever `prompt` the profile installed, so oh-my-posh and hand-written prompts
//! survive with an invisible prefix. Only the FileSystem provider is reported.

/// Appended to the PowerShell bootstrap (`-Command`), pwsh and 5.1 alike. ONE line, no CR or LF.
pub const PS_PROMPT_HOOK: &str = concat!(
    "if (-not (Test-Path Variable:global:__flarePrompt)) { $global:__flarePrompt = $function:prompt }; ",
    "function global:prompt { ",
    "$o = if ($global:__flarePrompt) { (& $global:__flarePrompt) -join '' } else { 'PS> ' }; ",
    "if ($PWD.Provider.Name -eq 'FileSystem') { \"$([char]27)]9;9;$($PWD.ProviderPath)$([char]27)\\\" + $o } else { $o } }"
);

/// Directory names bold blue TEXT, nothing else coloured; guarded because Windows PowerShell has no $PSStyle.
pub const PS_FILE_STYLE: &str = concat!(
    "if (Get-Variable PSStyle -ErrorAction SilentlyContinue) { ",
    "$PSStyle.FileInfo.Directory = \"$([char]27)[34;1m\"; ",
    "$PSStyle.FileInfo.Executable = ''; $PSStyle.FileInfo.SymbolicLink = ''; ",
    "$PSStyle.FileInfo.Extension.Clear(); ",
    "$PSStyle.Formatting.TableHeader = ''; $PSStyle.Formatting.CustomTableHeaderLabel = ''; ",
    "$PSStyle.Formatting.FormatAccent = '' }"
);

/// cmd's PROMPT with the OSC 9;9 report in front of whatever it was ("" or None means "$P$G").
pub fn cmd_prompt(existing: Option<&str>) -> String {
    let rest = match existing {
        Some(s) if !s.is_empty() => s,
        _ => "$P$G",
    };
    format!("$e]9;9;$P$e\\{rest}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ps_prompt_hook_wraps_once_and_reports_the_filesystem_folder() {
        for needle in [
            "$global:__flarePrompt = $function:prompt",
            "Test-Path Variable:global:__flarePrompt",
            "$PWD.Provider.Name -eq 'FileSystem'",
            "]9;9;$($PWD.ProviderPath)",
            "& $global:__flarePrompt",
        ] {
            assert!(PS_PROMPT_HOOK.contains(needle), "missing {needle:?}");
        }
        // One line: it is a -Command argument, and a newline in argv is a trap.
        assert!(!PS_PROMPT_HOOK.contains('\r'));
        assert!(!PS_PROMPT_HOOK.contains('\n'));
    }

    #[test]
    fn cmd_prompt_goes_in_front_of_the_existing_prompt() {
        assert_eq!(cmd_prompt(None), "$e]9;9;$P$e\\$P$G");
        assert_eq!(cmd_prompt(Some("")), "$e]9;9;$P$e\\$P$G");
        assert_eq!(cmd_prompt(Some("$D $P$G")), "$e]9;9;$P$e\\$D $P$G");
    }

    #[test]
    fn ps_file_style_paints_directories_bold_blue_text() {
        assert!(PS_FILE_STYLE.contains("Get-Variable PSStyle -ErrorAction SilentlyContinue"));
        assert!(PS_FILE_STYLE.contains(r#"$PSStyle.FileInfo.Directory = "$([char]27)[34;1m""#));
        assert!(!PS_FILE_STYLE.contains("[44"));
        for plain in [
            "FileInfo.Executable",
            "FileInfo.SymbolicLink",
            "Formatting.TableHeader",
            "Formatting.CustomTableHeaderLabel",
            "Formatting.FormatAccent",
        ] {
            assert!(
                PS_FILE_STYLE.contains(&format!("$PSStyle.{plain} = ''")),
                "missing {plain}"
            );
        }
        assert!(PS_FILE_STYLE.contains("$PSStyle.FileInfo.Extension.Clear()"));
    }
}

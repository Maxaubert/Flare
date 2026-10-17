//! The Win32 calls behind the path openers (no flare-term dependency).
//!
//! Every opener receives an absolute path that `open_term_path` already resolved and checked. The
//! functions here only refuse a path they cannot pass to Windows as one argument; they never decide
//! what may open.

use std::os::windows::process::CommandExt;
use std::process::Command;
use std::ptr;

use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

use crate::clipboard::utf16_with_nul;

/// `explorer.exe` under the system root, by absolute path (never a bare name).
pub fn explorer_path(sys_root: &str) -> String {
    format!("{}\\explorer.exe", sys_root.trim_end_matches('\\'))
}

/// True for a drive root such as `C:\`, which keeps its trailing backslash.
fn is_drive_root(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\'
}

/// The path as one quoted argument's body: refused when it holds a quote or a control character,
/// with trailing backslashes trimmed (except on a drive root) so `\"` can never escape the quote.
fn quotable(abs: &str) -> Option<&str> {
    let refused = abs.chars().any(|c| {
        c == '"' || (c as u32) < 0x20 || c == '\u{7f}' || ('\u{80}'..='\u{9f}').contains(&c)
    });
    if refused {
        return None;
    }
    let mut t = abs;
    while t.ends_with('\\') && !is_drive_root(t) {
        t = &t[..t.len() - 1];
    }
    if t.is_empty() { None } else { Some(t) }
}

/// The one argument for a reveal: `/select,"<abs>"`. None when `abs` holds `"` or any control
/// character (0x00 to 0x1F, 0x7F, 0x80 to 0x9F).
pub fn reveal_args(abs: &str) -> Option<String> {
    quotable(abs).map(|t| format!("/select,\"{t}\""))
}

/// The one argument for opening a folder: `"<abs>"`, same refusals and trimming as `reveal_args`.
pub fn folder_args(abs: &str) -> Option<String> {
    quotable(abs).map(|t| format!("\"{t}\""))
}

/// `ShellExecuteW(null, "open", abs, null, null, SW_SHOWNORMAL)`: a file that Windows opens with
/// its default verb. Only reached for an allowlisted file (see `open_term_path`).
pub fn shell_open_file(abs: &str) -> Result<(), String> {
    if abs.is_empty() {
        return Err("empty path".to_string());
    }
    let verb = utf16_with_nul("open");
    let target = utf16_with_nul(abs);
    // SAFETY: both strings are zero-terminated UTF-16 buffers that live across the call; the null
    // window, parameters and directory are all allowed by ShellExecuteW.
    let code = unsafe {
        ShellExecuteW(
            ptr::null_mut(),
            verb.as_ptr(),
            target.as_ptr(),
            ptr::null(),
            ptr::null(),
            SW_SHOWNORMAL,
        )
    } as isize;
    if code > 32 {
        Ok(())
    } else {
        Err(format!("ShellExecuteW failed with {code}"))
    }
}

/// Starts `explorer.exe` with `args` as its one raw command line (not waited on).
fn spawn_explorer(args: &str) -> Result<(), String> {
    let sys_root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string());
    Command::new(explorer_path(&sys_root)) // sync-ok: called from the open worker only (TermUi::open_path_async)
        .raw_arg(args)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("explorer did not start: {}", e.kind()))
}

/// Opens a folder in Explorer by absolute path. A folder is never handed to ShellExecute.
pub fn explorer_open_folder(abs: &str) -> Result<(), String> {
    let args = folder_args(abs).ok_or_else(|| "path refused".to_string())?;
    spawn_explorer(&args)
}

/// Shows a file selected in Explorer.
pub fn explorer_reveal(abs: &str) -> Result<(), String> {
    let args = reveal_args(abs).ok_or_else(|| "path refused".to_string())?;
    spawn_explorer(&args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reveal_argument_is_one_select_argument() {
        assert_eq!(
            reveal_args("C:\\a b\\c.txt").as_deref(),
            Some("/select,\"C:\\a b\\c.txt\"")
        );
    }

    #[test]
    fn a_reveal_path_with_a_quote_or_control_is_refused() {
        for bad in ["C:\\a\"b", "C:\\a\u{1b}", "C:\\a\u{9b}", "C:\\a\u{7f}"] {
            assert_eq!(reveal_args(bad), None, "{bad:?}");
            assert_eq!(folder_args(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_trailing_backslash_cannot_escape_the_quote() {
        assert_eq!(folder_args("C:\\dir\\").as_deref(), Some("\"C:\\dir\""));
        assert_eq!(folder_args("C:\\").as_deref(), Some("\"C:\\\""));
    }

    #[test]
    fn explorer_is_found_under_the_system_root_by_absolute_path() {
        assert_eq!(explorer_path("C:\\Windows"), "C:\\Windows\\explorer.exe");
        assert_eq!(explorer_path("C:\\Windows\\"), "C:\\Windows\\explorer.exe");
    }
}

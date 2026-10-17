//! The Win32 calls behind the path openers (no flare-term dependency).
//!
//! Every opener receives an absolute path that `open_term_path` already resolved and checked. The
//! functions here only refuse a path they cannot pass to Windows as one argument; they never decide
//! what may open.

use std::cell::Cell;
use std::os::windows::process::CommandExt;
use std::process::Command;
use std::ptr;

use windows_sys::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx,
};
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
/// A drive root is passed bare (`C:\`): a quote after its backslash reads as an escaped quote.
pub fn folder_args(abs: &str) -> Option<String> {
    quotable(abs).map(|t| {
        if is_drive_root(t) {
            t.to_string()
        } else {
            format!("\"{t}\"")
        }
    })
}

/// `\\?\C:\x` becomes `C:\x` and `\\?\UNC\h\s` becomes `\\h\s`; any other form is kept as is.
fn strip_verbatim(s: &str) -> String {
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        let b = rest.as_bytes();
        if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
            rest.to_string()
        } else {
            s.to_string()
        }
    } else {
        s.to_string()
    }
}

/// The path as the file system names it (links and 8.3 names resolved), None when it is gone.
fn canonical(abs: &str) -> Option<String> {
    let c = std::fs::canonicalize(abs).ok()?; // sync-ok: called from the open worker only
    Some(strip_verbatim(&c.to_string_lossy()))
}

/// Whether `p` is a directory now; None when it cannot be read.
fn is_dir(p: &str) -> Option<bool> {
    std::fs::metadata(p).ok().map(|m| m.is_dir()) // sync-ok: called from the open worker only
}

thread_local! {
    static COM_READY: Cell<bool> = const { Cell::new(false) };
}

/// Initialises COM (apartment threaded) once on this thread, as ShellExecute documents.
fn ensure_com() {
    COM_READY.with(|ready| {
        if !ready.get() {
            // SAFETY: a null reserved pointer is required; a changed-mode answer only means COM
            // is already set up on this thread, which is fine for ShellExecute.
            unsafe {
                CoInitializeEx(
                    ptr::null(),
                    (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
                );
            }
            ready.set(true);
        }
    });
}

/// Opens a file with its default verb after the last checks, on the canonical path: a directory
/// is refused, `allow` (the allowlist) is asked again, and only then `exec` runs with that path.
fn open_file_with(
    abs: &str,
    allow: &dyn Fn(&str) -> bool,
    exec: impl FnOnce(&str) -> Result<(), String>,
) -> Result<(), String> {
    if abs.is_empty() {
        return Err("empty path".to_string());
    }
    let canon = canonical(abs).ok_or_else(|| "path not found".to_string())?;
    if is_dir(&canon) != Some(false) {
        return Err("not a file".to_string());
    }
    if !allow(&canon) {
        return Err("type not allowed".to_string());
    }
    exec(&canon)
}

/// `ShellExecuteW(null, "open", canonical, null, null, SW_SHOWNORMAL)` for an allowlisted file.
/// `allow` is the caller's allowlist, asked on the canonical path right before the call.
pub fn shell_open_file(abs: &str, allow: &dyn Fn(&str) -> bool) -> Result<(), String> {
    open_file_with(abs, allow, |canon| {
        ensure_com();
        let verb = utf16_with_nul("open");
        let target = utf16_with_nul(canon);
        // SAFETY: both strings are zero-terminated UTF-16 buffers that live across the call; the
        // null window, parameters and directory are all allowed by ShellExecuteW.
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
    })
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

/// Re-verifies, on the canonical path, that `abs` is a folder, then hands that path to `spawn`.
fn open_folder_with(
    abs: &str,
    spawn: impl FnOnce(&str) -> Result<(), String>,
) -> Result<(), String> {
    let canon = canonical(abs).ok_or_else(|| "path not found".to_string())?;
    // sync-ok: called from the open worker only
    if !std::fs::metadata(&canon)
        .map(|m| m.is_dir())
        .unwrap_or(false)
    {
        return Err("not a folder".to_string());
    }
    let args = folder_args(&canon).ok_or_else(|| "path refused".to_string())?;
    spawn(&args)
}

/// Opens a folder in Explorer. The canonical path is checked to be a folder right before the
/// spawn and is what Explorer receives, so a file swapped in meanwhile is never run.
pub fn explorer_open_folder(abs: &str) -> Result<(), String> {
    open_folder_with(abs, spawn_explorer)
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
        assert_eq!(folder_args("C:\\").as_deref(), Some("C:\\"));
    }

    #[test]
    fn explorer_is_found_under_the_system_root_by_absolute_path() {
        assert_eq!(explorer_path("C:\\Windows"), "C:\\Windows\\explorer.exe");
        assert_eq!(explorer_path("C:\\Windows\\"), "C:\\Windows\\explorer.exe");
    }

    #[test]
    fn the_verbatim_prefix_is_stripped_without_changing_a_unc_into_something_else() {
        assert_eq!(strip_verbatim(r"\\?\C:\a b\c"), r"C:\a b\c");
        assert_eq!(strip_verbatim(r"\\?\UNC\host\share\d"), r"\\host\share\d");
        assert_eq!(strip_verbatim(r"\\?\Volume{1}\x"), r"\\?\Volume{1}\x");
        assert_eq!(strip_verbatim(r"C:\plain"), r"C:\plain");
        assert_eq!(strip_verbatim(r"\\host\share"), r"\\host\share");
    }

    #[test]
    fn a_file_open_refuses_a_directory_and_never_executes() {
        let dir = tempfile::tempdir().unwrap();
        let mut ran = false;
        let r = open_file_with(dir.path().to_str().unwrap(), &|_| true, |_| {
            ran = true;
            Ok(())
        });
        assert!(r.is_err());
        assert!(!ran);
    }

    #[test]
    fn a_file_open_asks_the_allowlist_on_the_canonical_path_and_runs_that_path() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.txt");
        std::fs::write(&f, "x").unwrap();
        let asked = std::cell::RefCell::new(String::new());
        let mut ran = String::new();
        let r = open_file_with(
            f.to_str().unwrap(),
            &|p| {
                *asked.borrow_mut() = p.to_string();
                true
            },
            |p| {
                ran = p.to_string();
                Ok(())
            },
        );
        assert!(r.is_ok());
        assert!(!asked.borrow().starts_with(r"\\?\"), "{}", asked.borrow());
        assert_eq!(*asked.borrow(), ran);
    }

    #[test]
    fn a_file_the_allowlist_refuses_or_that_is_missing_never_executes() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.txt");
        std::fs::write(&f, "x").unwrap();
        let mut ran = false;
        assert!(
            open_file_with(f.to_str().unwrap(), &|_| false, |_| {
                ran = true;
                Ok(())
            })
            .is_err()
        );
        let gone = dir.path().join("gone.txt");
        assert!(
            open_file_with(gone.to_str().unwrap(), &|_| true, |_| {
                ran = true;
                Ok(())
            })
            .is_err()
        );
        assert!(open_file_with("", &|_| true, |_| Ok(())).is_err());
        assert!(!ran);
    }

    #[test]
    fn a_folder_open_refuses_a_file_swapped_in_and_spawns_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("evil.exe");
        std::fs::write(&f, "x").unwrap();
        let mut spawned = false;
        let r = open_folder_with(f.to_str().unwrap(), |_| {
            spawned = true;
            Ok(())
        });
        assert!(r.is_err());
        assert!(!spawned);
    }

    #[test]
    fn a_folder_open_hands_explorer_the_canonical_quoted_path() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("a b");
        std::fs::create_dir(&sub).unwrap();
        let mut got = String::new();
        open_folder_with(sub.to_str().unwrap(), |a| {
            got = a.to_string();
            Ok(())
        })
        .unwrap();
        assert!(got.starts_with('"') && got.ends_with('"'), "{got}");
        assert!(!got.contains(r"\\?\"), "{got}");
        assert!(got.contains("a b"), "{got}");
    }

    #[test]
    fn a_missing_folder_spawns_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let gone = dir.path().join("gone");
        let mut spawned = false;
        assert!(
            open_folder_with(gone.to_str().unwrap(), |_| {
                spawned = true;
                Ok(())
            })
            .is_err()
        );
        assert!(!spawned);
    }

    #[test]
    fn com_is_initialised_once_per_thread() {
        ensure_com();
        ensure_com();
        assert!(COM_READY.with(Cell::get));
    }
}

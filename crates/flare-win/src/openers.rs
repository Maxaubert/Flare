//! The Win32 calls behind the path openers (no flare-term dependency).
//!
//! Every opener receives an absolute path that `open_term_path` already resolved and checked. The
//! functions here only refuse a path they cannot pass to Windows as one argument; they never decide
//! what may open.
//!
//! macOS: the same checks, then `NSWorkspace` (`workspace.rs`). A file opens in its default app
//! only after the canonical path is asked of the allowlist again; a folder (and anything Settings
//! hands over) only ever opens in a Finder window, which shows and never runs. That is why the
//! Mac needs no folder hold: the Windows hold stops a file swapped in under a folder's name from
//! being what Explorer RUNS, and Finder's file viewer runs nothing (a bundle such as `.app` is a
//! folder, and is shown, not launched). The Windows-only argument rules (quoting for a command
//! line) are kept for their tests.
#![cfg_attr(not(windows), allow(dead_code))]

#[cfg(windows)]
use std::cell::Cell;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
use std::process::Command;
#[cfg(windows)]
use std::ptr;
#[cfg(windows)]
use std::time::Duration;

#[cfg(windows)]
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CreateFileW, FILE_ATTRIBUTE_DIRECTORY,
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE,
    GetFileInformationByHandle, OPEN_EXISTING,
};
#[cfg(windows)]
use windows_sys::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx,
};
#[cfg(windows)]
use windows_sys::Win32::UI::Shell::ShellExecuteW;
#[cfg(windows)]
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

#[cfg(windows)]
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

#[cfg(windows)]
thread_local! {
    static COM_READY: Cell<bool> = const { Cell::new(false) };
}

/// Initialises COM (apartment threaded) once on this thread, as ShellExecute documents.
#[cfg(windows)]
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
#[cfg(windows)]
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
#[cfg(windows)]
fn spawn_explorer(args: &str) -> Result<(), String> {
    let sys_root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string());
    Command::new(explorer_path(&sys_root)) // sync-ok: called from the open worker only (TermUi::open_path_async)
        .raw_arg(args)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("explorer did not start: {}", e.kind()))
}

/// How long a folder stays held after Explorer is started: long enough for Explorer to resolve
/// the path it was handed.
#[cfg(windows)]
const FOLDER_HOLD: Duration = Duration::from_secs(10);

/// An open handle on a real directory, shared for read and write but NOT delete: while it lives
/// the directory cannot be renamed or removed, so its name cannot become a runnable file.
#[cfg(windows)]
pub(crate) struct FolderHold(HANDLE);

// SAFETY: a kernel handle may be closed from any thread; nothing else touches it.
#[cfg(windows)]
unsafe impl Send for FolderHold {}

#[cfg(windows)]
impl Drop for FolderHold {
    fn drop(&mut self) {
        // SAFETY: built only from a valid handle CreateFileW returned; closed exactly once.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

/// Holds `canon` open when it is a real directory now (not a file, not a link or junction).
#[cfg(windows)]
fn hold_folder(canon: &str) -> Option<FolderHold> {
    let wide = utf16_with_nul(canon);
    // SAFETY: `wide` is a zero-terminated UTF-16 path that outlives the call; null security
    // attributes and template are allowed. FILE_LIST_DIRECTORY makes the share mode binding (an
    // attribute-only open would not stop a rename).
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE || handle.is_null() {
        return None;
    }
    let hold = FolderHold(handle);
    // SAFETY: an all-zero BY_HANDLE_FILE_INFORMATION is a valid value of this plain C struct.
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: `handle` is live (owned by `hold`) and `info` is a writable struct of the right type.
    if unsafe { GetFileInformationByHandle(handle, &mut info) } == 0 {
        return None;
    }
    let attrs = info.dwFileAttributes;
    (attrs & FILE_ATTRIBUTE_DIRECTORY != 0 && attrs & FILE_ATTRIBUTE_REPARSE_POINT == 0)
        .then_some(hold)
}

/// Holds the canonical path as a directory (refused otherwise), then hands that path to `spawn`.
/// The hold is returned so it outlives the spawn: the name stays a folder while Explorer reads it.
#[cfg(windows)]
fn open_folder_with(
    abs: &str,
    spawn: impl FnOnce(&str) -> Result<(), String>,
) -> Result<FolderHold, String> {
    let canon = canonical(abs).ok_or_else(|| "path not found".to_string())?;
    let hold = hold_folder(&canon).ok_or_else(|| "not a folder".to_string())?;
    let args = folder_args(&canon).ok_or_else(|| "path refused".to_string())?;
    spawn(&args)?;
    Ok(hold)
}

/// Opens a folder in Explorer. The canonical path is held open as a directory (no rename, no
/// delete) from before the spawn until `FOLDER_HOLD` after it, so a file swapped in under that
/// name is never what Explorer runs.
#[cfg(windows)]
pub fn explorer_open_folder(abs: &str) -> Result<(), String> {
    let hold = open_folder_with(abs, spawn_explorer)?;
    std::thread::spawn(move || {
        std::thread::sleep(FOLDER_HOLD); // sync-ok: a detached thread that only keeps the hold
        drop(hold);
    });
    Ok(())
}

/// The one argument for opening a path in Explorer: `"<abs>"`. None when `abs` holds `"` or any
/// control character.
pub fn open_args(abs: &str) -> Option<String> {
    quotable(abs).map(|t| format!("\"{t}\""))
}

/// Hands a path to Explorer by its absolute exe path (the Settings page's "Open folder"). Call it
/// from a worker: the process start can wait on a slow, scanned disk.
#[cfg(windows)]
pub fn explorer_open_path(abs: &str) -> Result<(), String> {
    let args = open_args(abs).ok_or_else(|| "path refused".to_string())?;
    spawn_explorer(&args)
}

/// Shows a file selected in Explorer.
#[cfg(windows)]
pub fn explorer_reveal(abs: &str) -> Result<(), String> {
    let args = reveal_args(abs).ok_or_else(|| "path refused".to_string())?;
    spawn_explorer(&args)
}

/// macOS: opens an allowlisted file in its default app (`NSWorkspace openURL:`), after the same
/// last checks as on Windows: canonical, not a folder, the allowlist asked again on that path.
#[cfg(target_os = "macos")]
pub fn shell_open_file(abs: &str, allow: &dyn Fn(&str) -> bool) -> Result<(), String> {
    open_file_with(abs, allow, crate::workspace::open_file)
}

/// Checks that the canonical path is a folder now, then hands that path to `show`.
#[cfg(target_os = "macos")]
fn show_folder_with(
    abs: &str,
    show: impl FnOnce(&str) -> Result<(), String>,
) -> Result<(), String> {
    let canon = canonical(abs).ok_or_else(|| "path not found".to_string())?;
    if is_dir(&canon) != Some(true) {
        return Err("not a folder".to_string());
    }
    show(&canon)
}

/// macOS: opens a folder in a Finder window (shown, never run).
#[cfg(target_os = "macos")]
pub fn explorer_open_folder(abs: &str) -> Result<(), String> {
    show_folder_with(abs, crate::workspace::show_folder)
}

/// What Settings' "Open folder" does with a path on macOS: a folder opens in Finder, anything else
/// is shown selected in its folder. Nothing is launched either way.
#[cfg(target_os = "macos")]
fn open_path_with(
    abs: &str,
    show: impl FnOnce(&str) -> Result<(), String>,
    reveal: impl FnOnce(&str) -> Result<(), String>,
) -> Result<(), String> {
    match canonical(abs) {
        Some(canon) if is_dir(&canon) == Some(true) => show(&canon),
        Some(canon) => reveal(&canon),
        None => Err("path not found".to_string()),
    }
}

/// macOS: hands a path to Finder (the Settings page's "Open folder"). Call it from a worker.
#[cfg(target_os = "macos")]
pub fn explorer_open_path(abs: &str) -> Result<(), String> {
    open_path_with(abs, crate::workspace::show_folder, crate::workspace::reveal)
}

/// macOS: shows a file selected in a Finder window.
#[cfg(target_os = "macos")]
pub fn explorer_reveal(abs: &str) -> Result<(), String> {
    crate::workspace::reveal(abs)
}

#[cfg(test)]
mod tests {
    #[test]
    fn open_args_quote_the_path_and_refuse_a_quote_or_a_control_character() {
        assert_eq!(
            super::open_args("C:\\Users\\me\\logs\\").as_deref(),
            Some("\"C:\\Users\\me\\logs\"")
        );
        assert_eq!(super::open_args("C:\\a\"b"), None);
        assert_eq!(super::open_args("C:\\a\nb"), None);
    }

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

    #[cfg(windows)]
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

    #[cfg(windows)]
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

    #[cfg(windows)]
    #[test]
    fn a_folder_is_held_against_rename_and_delete_while_explorer_starts() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("p");
        let sub = parent.join("tool.exe");
        std::fs::create_dir_all(&sub).unwrap();
        let moved = parent.join("moved");
        let parent_moved = dir.path().join("q");
        let mut checked = false;
        let hold = open_folder_with(sub.to_str().unwrap(), |_| {
            // The name cannot be freed for a runnable file while Explorer resolves it, neither
            // by moving the folder nor by moving the folder above it.
            assert!(std::fs::rename(&sub, &moved).is_err());
            assert!(std::fs::remove_dir(&sub).is_err());
            assert!(std::fs::rename(&parent, &parent_moved).is_err());
            checked = true;
            Ok(())
        })
        .unwrap();
        assert!(checked);
        assert!(
            std::fs::rename(&sub, &moved).is_err(),
            "still held after the spawn"
        );
        drop(hold);
        std::fs::rename(&sub, &moved).unwrap();
    }

    #[cfg(windows)]
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

    #[cfg(windows)]
    #[test]
    fn com_is_initialised_once_per_thread() {
        ensure_com();
        ensure_com();
        assert!(COM_READY.with(Cell::get));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_mac_folder_open_refuses_a_file_and_a_missing_path_and_shows_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("evil.command");
        std::fs::write(&f, "x").unwrap();
        let mut shown = false;
        assert!(
            show_folder_with(f.to_str().unwrap(), |_| {
                shown = true;
                Ok(())
            })
            .is_err()
        );
        let gone = dir.path().join("gone");
        assert!(
            show_folder_with(gone.to_str().unwrap(), |_| {
                shown = true;
                Ok(())
            })
            .is_err()
        );
        assert!(!shown);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_mac_folder_open_shows_the_canonical_folder_and_a_bundle_is_only_shown() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("Tool.app");
        std::fs::create_dir(&app).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&app, &link).unwrap();
        let mut got = String::new();
        show_folder_with(link.to_str().unwrap(), |p| {
            got = p.to_string();
            Ok(())
        })
        .unwrap();
        let canon = std::fs::canonicalize(&app).unwrap();
        assert_eq!(got, canon.to_str().unwrap());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_mac_open_path_shows_a_folder_and_reveals_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.log");
        std::fs::write(&f, "x").unwrap();
        let (mut shown, mut revealed) = (String::new(), String::new());
        open_path_with(
            dir.path().to_str().unwrap(),
            |p| {
                shown = p.to_string();
                Ok(())
            },
            |_| panic!("a folder is not revealed"),
        )
        .unwrap();
        open_path_with(
            f.to_str().unwrap(),
            |_| panic!("a file is not shown as a folder"),
            |p| {
                revealed = p.to_string();
                Ok(())
            },
        )
        .unwrap();
        assert!(
            !shown.is_empty() && revealed.ends_with("/a.log"),
            "{shown} {revealed}"
        );
        assert!(
            open_path_with(
                dir.path().join("gone").to_str().unwrap(),
                |_| Ok(()),
                |_| Ok(())
            )
            .is_err()
        );
    }
}

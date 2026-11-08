//! The Win32 calls behind the path openers (no flare-term dependency).
//!
//! Every opener receives an absolute path that `open_term_path` already resolved and checked. The
//! functions here only refuse a path they cannot pass to Windows as one argument; they never decide
//! what may open.

use std::cell::Cell;
use std::os::windows::process::CommandExt;
use std::process::Command;
use std::ptr;
use std::time::Duration;

use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Shell::{IShellWindows, IWebBrowser2, ShellWindows};
use windows::core::Interface;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, HWND, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CreateFileW, FILE_ATTRIBUTE_DIRECTORY,
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE,
    GetFileInformationByHandle, OPEN_EXISTING,
};
use windows_sys::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx,
};
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    IsIconic, IsWindowVisible, SW_RESTORE, SW_SHOWNORMAL, SetForegroundWindow, ShowWindowAsync,
};

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

/// How long a folder stays held after Explorer is started: long enough for Explorer to resolve
/// the path it was handed.
const FOLDER_HOLD: Duration = Duration::from_secs(10);

/// An open handle on a real directory, shared for read and write but NOT delete: while it lives
/// the directory cannot be renamed or removed, so its name cannot become a runnable file.
pub(crate) struct FolderHold(HANDLE);

// SAFETY: a kernel handle may be closed from any thread; nothing else touches it.
unsafe impl Send for FolderHold {}

impl Drop for FolderHold {
    fn drop(&mut self) {
        // SAFETY: built only from a valid handle CreateFileW returned; closed exactly once.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

/// Holds `canon` open when it is a real directory now (not a file, not a link or junction).
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

/// The Windows path a `file:` address names (`%XX` read as UTF-8), None for any other address.
fn file_url_path(url: &str) -> Option<String> {
    let rest = url.strip_prefix("file://")?;
    let body = rest
        .strip_prefix('/')
        .map_or_else(|| format!("//{rest}"), str::to_string);
    let raw = body.as_bytes();
    let mut bytes = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        let hex = (raw[i] == b'%')
            .then(|| raw.get(i + 1..i + 3))
            .flatten()
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        bytes.push(hex.unwrap_or(raw[i]));
        i += if hex.is_some() { 3 } else { 1 };
    }
    Some(String::from_utf8(bytes).ok()?.replace('/', "\\"))
}

/// Whether two absolute paths name the same folder, as Windows compares names.
fn same_folder(a: &str, b: &str) -> bool {
    a.trim_end_matches('\\').to_lowercase() == b.trim_end_matches('\\').to_lowercase()
}

/// The visible Explorer window showing `canon` now. None when there is none, or when the shell
/// cannot be asked: the caller then starts Explorer, as it would for a folder that is not open.
/// Call it from a worker: a busy Explorer can hold the answer up.
fn explorer_window_showing(canon: &str) -> Option<HWND> {
    ensure_com();
    // SAFETY: COM is initialised on this thread by `ensure_com`; every interface is a live one
    // the shell returned and is released on drop, and the index VARIANT outlives its call.
    unsafe {
        let windows: IShellWindows = CoCreateInstance(&ShellWindows, None, CLSCTX_ALL).ok()?;
        for i in 0..windows.Count().ok()? {
            let Some(browser) = windows
                .Item(&VARIANT::from(i))
                .ok()
                .and_then(|item| item.cast::<IWebBrowser2>().ok())
            else {
                continue;
            };
            let Some(url) = browser.LocationURL().ok().map(|u| u.to_string()) else {
                continue;
            };
            if !file_url_path(&url).is_some_and(|p| same_folder(&p, canon)) {
                continue;
            }
            let hwnd = browser.HWND().ok()?.0 as HWND;
            if IsWindowVisible(hwnd) != 0 {
                return Some(hwnd);
            }
        }
    }
    None
}

/// Brings the Explorer window already showing `canon` to the front; false when no window shows it.
/// A second `explorer.exe` hands the folder to the Explorer that owns the window and leaves the
/// raise to it, and the foreground lock can refuse that process (issue #55). This process is the
/// foreground one during the click, so the raise is made from here. Should the user have moved on
/// since, Windows refuses it and nothing takes the focus.
fn bring_open_folder_forward(canon: &str) -> bool {
    let Some(hwnd) = explorer_window_showing(canon) else {
        return false;
    };
    // SAFETY: plain Win32 calls that accept any window handle, a stale one included; the async
    // show cannot block on a busy Explorer.
    unsafe {
        if IsIconic(hwnd) != 0 {
            ShowWindowAsync(hwnd, SW_RESTORE);
        }
        SetForegroundWindow(hwnd);
    }
    true
}

/// Holds the canonical path as a directory (refused otherwise), asks `bring_forward` to raise a
/// window already showing it, and otherwise hands that path to `spawn`, so a folder that is open
/// is never opened twice. The hold is returned so it outlives the spawn: the name stays a folder
/// while Explorer reads it.
fn open_folder_with(
    abs: &str,
    bring_forward: impl FnOnce(&str) -> bool,
    spawn: impl FnOnce(&str) -> Result<(), String>,
) -> Result<FolderHold, String> {
    let canon = canonical(abs).ok_or_else(|| "path not found".to_string())?;
    let hold = hold_folder(&canon).ok_or_else(|| "not a folder".to_string())?;
    if bring_forward(&canon) {
        return Ok(hold);
    }
    let args = folder_args(&canon).ok_or_else(|| "path refused".to_string())?;
    spawn(&args)?;
    Ok(hold)
}

/// Opens a folder in Explorer, or brings forward the window already showing it. The canonical path is held open as a directory (no rename, no
/// delete) from before the spawn until `FOLDER_HOLD` after it, so a file swapped in under that
/// name is never what Explorer runs.
pub fn explorer_open_folder(abs: &str) -> Result<(), String> {
    let hold = open_folder_with(abs, bring_open_folder_forward, spawn_explorer)?;
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
pub fn explorer_open_path(abs: &str) -> Result<(), String> {
    let args = open_args(abs).ok_or_else(|| "path refused".to_string())?;
    spawn_explorer(&args)
}

/// Shows a file selected in Explorer.
pub fn explorer_reveal(abs: &str) -> Result<(), String> {
    let args = reveal_args(abs).ok_or_else(|| "path refused".to_string())?;
    spawn_explorer(&args)
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

    #[test]
    fn a_folder_open_refuses_a_file_swapped_in_and_spawns_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("evil.exe");
        std::fs::write(&f, "x").unwrap();
        let mut spawned = false;
        let r = open_folder_with(
            f.to_str().unwrap(),
            |_| panic!("a file is never looked for"),
            |_| {
                spawned = true;
                Ok(())
            },
        );
        assert!(r.is_err());
        assert!(!spawned);
    }

    #[test]
    fn a_folder_open_hands_explorer_the_canonical_quoted_path() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("a b");
        std::fs::create_dir(&sub).unwrap();
        let mut got = String::new();
        open_folder_with(
            sub.to_str().unwrap(),
            |_| false,
            |a| {
                got = a.to_string();
                Ok(())
            },
        )
        .unwrap();
        assert!(got.starts_with('"') && got.ends_with('"'), "{got}");
        assert!(!got.contains(r"\\?\"), "{got}");
        assert!(got.contains("a b"), "{got}");
    }

    #[test]
    fn a_folder_is_held_against_rename_and_delete_while_explorer_starts() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("p");
        let sub = parent.join("tool.exe");
        std::fs::create_dir_all(&sub).unwrap();
        let moved = parent.join("moved");
        let parent_moved = dir.path().join("q");
        let mut checked = false;
        let hold = open_folder_with(
            sub.to_str().unwrap(),
            |_| false,
            |_| {
                // The name cannot be freed for a runnable file while Explorer resolves it, neither
                // by moving the folder nor by moving the folder above it.
                assert!(std::fs::rename(&sub, &moved).is_err());
                assert!(std::fs::remove_dir(&sub).is_err());
                assert!(std::fs::rename(&parent, &parent_moved).is_err());
                checked = true;
                Ok(())
            },
        )
        .unwrap();
        assert!(checked);
        assert!(
            std::fs::rename(&sub, &moved).is_err(),
            "still held after the spawn"
        );
        drop(hold);
        std::fs::rename(&sub, &moved).unwrap();
    }

    #[test]
    fn a_missing_folder_spawns_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let gone = dir.path().join("gone");
        let mut spawned = false;
        assert!(
            open_folder_with(
                gone.to_str().unwrap(),
                |_| false,
                |_| {
                    spawned = true;
                    Ok(())
                }
            )
            .is_err()
        );
        assert!(!spawned);
    }

    #[test]
    fn a_folder_already_showing_is_brought_forward_and_explorer_is_not_started() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("a b");
        std::fs::create_dir(&sub).unwrap();
        let mut asked = String::new();
        let mut spawned = false;
        let hold = open_folder_with(
            sub.to_str().unwrap(),
            |canon| {
                asked = canon.to_string();
                true
            },
            |_| {
                spawned = true;
                Ok(())
            },
        );
        assert!(hold.is_ok());
        assert!(!spawned, "a second Explorer would duplicate the window");
        assert!(!asked.starts_with(r"\\?\"), "{asked}");
        assert!(asked.ends_with("a b"), "{asked}");
    }

    #[test]
    fn a_folder_no_window_shows_starts_explorer() {
        let dir = tempfile::tempdir().unwrap();
        let mut spawned = false;
        open_folder_with(
            dir.path().to_str().unwrap(),
            |_| false,
            |_| {
                spawned = true;
                Ok(())
            },
        )
        .unwrap();
        assert!(spawned);
    }

    #[test]
    fn an_explorer_address_reads_back_as_its_windows_path() {
        assert_eq!(
            file_url_path("file:///C:/a%20b/m%C3%B8te").as_deref(),
            Some("C:\\a b\\m\u{f8}te")
        );
        assert_eq!(file_url_path("file:///C:/").as_deref(), Some("C:\\"));
        assert_eq!(
            file_url_path("file://host/share/d").as_deref(),
            Some(r"\\host\share\d")
        );
        assert_eq!(
            file_url_path("file:///C:/100%").as_deref(),
            Some("C:\\100%")
        );
        assert_eq!(
            file_url_path("file:///C:/m\u{f8}te").as_deref(),
            Some("C:\\m\u{f8}te")
        );
        for other in [
            "",
            "about:blank",
            "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}",
            "file:///C:/%FF",
        ] {
            assert_eq!(file_url_path(other), None, "{other:?}");
        }
    }

    #[test]
    fn the_same_folder_is_matched_across_case_and_a_trailing_backslash_only() {
        assert!(same_folder("C:\\Users\\Me", "c:\\users\\me\\"));
        assert!(same_folder("C:\\", "c:\\"));
        assert!(same_folder(r"\\host\share", r"\\HOST\share"));
        assert!(!same_folder("C:\\a", "C:\\a b"));
        assert!(!same_folder("C:\\a", "C:\\a\\b"));
    }

    #[test]
    fn a_folder_no_explorer_shows_is_not_found_and_nothing_is_raised() {
        // Only reads the shell's window list: nothing opens and nothing comes forward.
        let dir = tempfile::tempdir().unwrap();
        assert!(!bring_open_folder_forward(dir.path().to_str().unwrap()));
    }

    #[test]
    fn com_is_initialised_once_per_thread() {
        ensure_com();
        ensure_com();
        assert!(COM_READY.with(Cell::get));
    }
}

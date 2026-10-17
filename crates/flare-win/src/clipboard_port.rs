//! The clipboard port: the Windows implementation, the in-memory stand-in and the read side.
//! Everything here BLOCKS and is called from worker threads only.

use std::ptr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread::sleep; // sync-ok: the clipboard retry below, at most 80 ms
use std::time::Duration;

use flare_shells::paste::ClipboardState;
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, GetClipboardData, GetClipboardSequenceNumber, IsClipboardFormatAvailable,
    OpenClipboard, RegisterClipboardFormatW,
};
use windows_sys::Win32::System::Memory::{GlobalLock, GlobalUnlock};
use windows_sys::Win32::System::Ole::{CF_DIB, CF_DIBV5, CF_HDROP, CF_UNICODETEXT};
use windows_sys::Win32::UI::Shell::DragQueryFileW;

/// UI copies (Copy folder path, the command help): refuse, never trim.
pub const CLIPBOARD_WRITE_MAX: usize = 4000;
/// The terminal's own copies (Ctrl+C, menu Copy): a guard, refuse, never trim.
pub const COPY_MAX: usize = 16 * 1024 * 1024;
/// Equals `flare_term::osc52::OSC52_MAX` (flare-win cannot depend on flare-term); Task 21 pins it.
pub const OSC52_MAX: usize = 1_048_576;

/// The clipboard as the terminal sees it.
pub trait ClipboardPort: Send + Sync {
    /// Writes exact text. Err for empty text, text over `max` (counted in chars), or a busy clipboard.
    fn write_text(&self, text: &str, max: usize) -> Result<(), String>;
    /// The text now, or None when there is none or it cannot be read.
    fn read_text(&self) -> Option<String>;
    /// Everything a paste decision needs: text, copied files (absolute paths), an image present.
    fn read_state(&self) -> ClipboardState;
    /// A number that changes with every clipboard change.
    fn sequence(&self) -> u32;
}

fn check_write(text: &str, max: usize) -> Result<(), String> {
    if text.is_empty() {
        return Err("nothing to copy".to_string());
    }
    if text.chars().count() > max {
        return Err("too much text to copy".to_string());
    }
    Ok(())
}

/// The real clipboard.
pub struct WinClipboard;

/// Opens the clipboard with up to 5 tries, 20 ms apart.
fn open() -> bool {
    for attempt in 0..5 {
        // SAFETY: a null owner opens the clipboard for this thread; the result is checked.
        if unsafe { OpenClipboard(ptr::null_mut()) } != 0 {
            return true;
        }
        if attempt < 4 {
            sleep(Duration::from_millis(20)); // sync-ok: clipboard retry, at most 80 ms
        }
    }
    false
}

/// Text from CF_UNICODETEXT; the clipboard must be open on this thread.
fn unicode_text_open() -> Option<String> {
    // SAFETY: the caller has the clipboard open; the handle is locked only while non-null and
    // read up to its terminating zero, within the block the owner allocated.
    unsafe {
        if IsClipboardFormatAvailable(u32::from(CF_UNICODETEXT)) == 0 {
            return None;
        }
        let handle = GetClipboardData(u32::from(CF_UNICODETEXT));
        if handle.is_null() {
            return None;
        }
        let locked = GlobalLock(handle).cast::<u16>();
        if locked.is_null() {
            return None;
        }
        let mut len = 0;
        while *locked.add(len) != 0 {
            len += 1;
        }
        let s = String::from_utf16_lossy(std::slice::from_raw_parts(locked, len));
        GlobalUnlock(handle);
        Some(s)
    }
}

/// Copied files from CF_HDROP; the clipboard must be open on this thread.
fn files_open() -> Vec<String> {
    let mut out = Vec::new();
    // SAFETY: the caller has the clipboard open; the HDROP handle is used only while non-null and
    // only for DragQueryFileW, whose buffer length is passed with every call.
    unsafe {
        if IsClipboardFormatAvailable(u32::from(CF_HDROP)) == 0 {
            return out;
        }
        let drop = GetClipboardData(u32::from(CF_HDROP));
        if drop.is_null() {
            return out;
        }
        let count = DragQueryFileW(drop, 0xFFFF_FFFF, ptr::null_mut(), 0);
        for i in 0..count {
            let need = DragQueryFileW(drop, i, ptr::null_mut(), 0);
            if need == 0 {
                continue;
            }
            let mut buf = vec![0u16; need as usize + 1];
            let got = DragQueryFileW(drop, i, buf.as_mut_ptr(), buf.len() as u32);
            if got > 0 {
                out.push(String::from_utf16_lossy(&buf[..got as usize]));
            }
        }
    }
    out
}

fn image_open() -> bool {
    let name: Vec<u16> = "PNG".encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: the caller has the clipboard open; `name` is a NUL-terminated UTF-16 string that
    // outlives the call.
    unsafe {
        let png = RegisterClipboardFormatW(name.as_ptr());
        IsClipboardFormatAvailable(u32::from(CF_DIB)) != 0
            || IsClipboardFormatAvailable(u32::from(CF_DIBV5)) != 0
            || (png != 0 && IsClipboardFormatAvailable(png) != 0)
    }
}

impl ClipboardPort for WinClipboard {
    fn write_text(&self, text: &str, max: usize) -> Result<(), String> {
        check_write(text, max)?;
        crate::clipboard::set_text(text)
    }

    fn read_text(&self) -> Option<String> {
        if !open() {
            return None;
        }
        let t = unicode_text_open();
        // SAFETY: opened above on this thread, closed exactly once.
        unsafe {
            CloseClipboard();
        }
        t
    }

    fn read_state(&self) -> ClipboardState {
        if !open() {
            return ClipboardState::default();
        }
        let state = ClipboardState {
            files: files_open(),
            text: unicode_text_open().unwrap_or_default(),
            image: image_open(),
        };
        // SAFETY: opened above on this thread, closed exactly once.
        unsafe {
            CloseClipboard();
        }
        state
    }

    fn sequence(&self) -> u32 {
        // SAFETY: takes no arguments and only reads a system counter.
        unsafe { GetClipboardSequenceNumber() }
    }
}

#[derive(Default)]
struct Inner {
    text: Option<String>,
    files: Vec<String>,
    image: bool,
}

/// The `--e2e` and test stand-in.
#[derive(Default)]
pub struct MemoryClipboard {
    inner: Mutex<Inner>,
    seq: AtomicU32,
}

impl MemoryClipboard {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn set_files(&self, files: Vec<String>) {
        self.lock().files = files;
        self.seq.fetch_add(1, Ordering::SeqCst);
    }

    pub fn set_image(&self, present: bool) {
        self.lock().image = present;
        self.seq.fetch_add(1, Ordering::SeqCst);
    }

    /// As another program copying: bumps the sequence.
    pub fn set_text(&self, text: &str) {
        self.lock().text = Some(text.to_string());
        self.seq.fetch_add(1, Ordering::SeqCst);
    }
}

impl ClipboardPort for MemoryClipboard {
    fn write_text(&self, text: &str, max: usize) -> Result<(), String> {
        check_write(text, max)?;
        self.set_text(text);
        Ok(())
    }

    fn read_text(&self) -> Option<String> {
        self.lock().text.clone()
    }

    fn read_state(&self) -> ClipboardState {
        let g = self.lock();
        ClipboardState {
            image: g.image,
            text: g.text.clone().unwrap_or_default(),
            files: g.files.clone(),
        }
    }

    fn sequence(&self) -> u32 {
        self.seq.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_the_exact_text_it_was_handed() {
        let c = MemoryClipboard::new();
        let t = format!(
            "Get-ChildItem -Recurse | Where-Object {{ $_.Length -gt 1MB }} | {}",
            "x".repeat(30)
        );
        c.write_text(&t, CLIPBOARD_WRITE_MAX).unwrap();
        assert_eq!(c.read_text().as_deref(), Some(t.as_str()));
    }

    #[test]
    fn refuses_empty_and_too_long_and_writes_none_of_it() {
        let c = MemoryClipboard::new();
        c.write_text("before", CLIPBOARD_WRITE_MAX).unwrap();
        assert!(c.write_text("", CLIPBOARD_WRITE_MAX).is_err());
        assert!(c.write_text(&"x".repeat(4001), 4000).is_err());
        assert_eq!(c.read_text().as_deref(), Some("before"));
        assert!(c.write_text(&"x".repeat(4000), 4000).is_ok());
    }

    #[test]
    fn the_terminal_cap_is_in_chars_not_bytes() {
        let c = MemoryClipboard::new();
        assert!(c.write_text(&"é".repeat(OSC52_MAX), OSC52_MAX).is_ok());
        assert!(c.write_text(&"é".repeat(OSC52_MAX + 1), OSC52_MAX).is_err());
    }

    #[test]
    fn the_sequence_changes_on_every_write_and_on_set_text() {
        let c = MemoryClipboard::new();
        let a = c.sequence();
        c.write_text("a", 10).unwrap();
        let b = c.sequence();
        c.write_text("b", 10).unwrap();
        let d = c.sequence();
        c.set_text("c");
        let e = c.sequence();
        assert!(a < b && b < d && d < e);
    }

    #[test]
    fn read_state_reports_files_before_text_and_the_image_flag() {
        let c = MemoryClipboard::new();
        c.set_text("hello");
        c.set_files(vec!["C:\\a.txt".to_string()]);
        c.set_image(true);
        let s = c.read_state();
        assert_eq!(s.text, "hello");
        assert_eq!(s.files, vec!["C:\\a.txt".to_string()]);
        assert!(s.image);
    }

    #[test]
    fn a_real_round_trip_is_not_attempted_in_unit_tests() {
        fn _assert_send_sync<T: Send + Sync>() {}
        _assert_send_sync::<WinClipboard>();
    }
}

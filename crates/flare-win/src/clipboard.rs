//! set_text: the clipboard write for Copy folder path (Task 17).

use std::ptr;
use std::thread::sleep; // sync-ok: the clipboard retry below, at most 80 ms
use std::time::Duration;

use windows_sys::Win32::Foundation::GlobalFree;
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};

/// CF_UNICODETEXT.
const CF_UNICODETEXT: u32 = 13;

/// UTF-16 code units of `s`, with a terminating zero.
pub fn utf16_with_nul(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Puts `text` on the clipboard as Unicode text. Opening the clipboard can fail while another
/// program holds it, so it is retried five times, 20 ms apart.
pub fn set_text(text: &str) -> Result<(), String> {
    let wide = utf16_with_nul(text);
    let bytes = wide.len() * std::mem::size_of::<u16>();

    let mut opened = false;
    for attempt in 0..5 {
        // SAFETY: a null owner opens the clipboard for this thread; the result is checked.
        if unsafe { OpenClipboard(ptr::null_mut()) } != 0 {
            opened = true;
            break;
        }
        if attempt < 4 {
            sleep(Duration::from_millis(20)); // sync-ok: clipboard retry, at most 80 ms on a busy clipboard
        }
    }
    if !opened {
        return Err("the clipboard is busy".to_string());
    }

    // SAFETY: the clipboard is open on this thread (opened above), and every handle used below
    // is checked for null before it is read; the memory block is ours until SetClipboardData
    // takes it, and it is freed here when that call fails.
    let result = unsafe {
        EmptyClipboard();
        let block = GlobalAlloc(GMEM_MOVEABLE, bytes);
        if block.is_null() {
            Err("could not allocate clipboard memory".to_string())
        } else {
            let target = GlobalLock(block);
            if target.is_null() {
                GlobalFree(block);
                Err("could not lock clipboard memory".to_string())
            } else {
                ptr::copy_nonoverlapping(wide.as_ptr().cast::<u8>(), target.cast::<u8>(), bytes);
                GlobalUnlock(block);
                if SetClipboardData(CF_UNICODETEXT, block).is_null() {
                    GlobalFree(block);
                    Err("the clipboard refused the text".to_string())
                } else {
                    Ok(())
                }
            }
        }
    };
    // SAFETY: the clipboard was opened on this thread above and is closed exactly once here.
    unsafe {
        CloseClipboard();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_has_a_terminating_zero_and_surrogates() {
        assert_eq!(utf16_with_nul("a\u{1F600}"), vec![0x61, 0xD83D, 0xDE00, 0]);
    }

    /// Reads the clipboard's Unicode text back (test only).
    fn read_text() -> Option<String> {
        use windows_sys::Win32::System::DataExchange::GetClipboardData;
        // SAFETY: the clipboard is opened, read and closed on this thread; the handle from
        // GetClipboardData is locked only while it is non-null, and the text is read up to its
        // terminating zero, which set_text always writes.
        unsafe {
            if OpenClipboard(ptr::null_mut()) == 0 {
                return None;
            }
            let handle = GetClipboardData(CF_UNICODETEXT);
            let text = if handle.is_null() {
                None
            } else {
                let locked = GlobalLock(handle).cast::<u16>();
                if locked.is_null() {
                    None
                } else {
                    let mut len = 0;
                    while *locked.add(len) != 0 {
                        len += 1;
                    }
                    let units = std::slice::from_raw_parts(locked, len);
                    let s = String::from_utf16_lossy(units);
                    GlobalUnlock(handle);
                    Some(s)
                }
            };
            CloseClipboard();
            text
        }
    }

    #[test]
    #[ignore = "touches the real clipboard"]
    fn set_text_round_trips() {
        let old = read_text();
        set_text("flare-m3").expect("the clipboard accepts text");
        assert_eq!(read_text().as_deref(), Some("flare-m3"));
        if let Some(old) = old {
            set_text(&old).expect("the old text goes back");
        }
    }
}

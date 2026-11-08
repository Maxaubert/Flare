//! set_text: the clipboard write for Copy folder path (Task 17).

#[cfg(windows)]
use std::ptr;
#[cfg(windows)]
use std::thread::sleep; // sync-ok: the clipboard retry below, at most 80 ms
#[cfg(windows)]
use std::time::Duration;

#[cfg(windows)]
use windows_sys::Win32::Foundation::GlobalFree;
#[cfg(windows)]
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
#[cfg(windows)]
use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};

/// CF_UNICODETEXT.
#[cfg(windows)]
const CF_UNICODETEXT: u32 = 13;

/// UTF-16 code units of `s`, with a terminating zero.
pub fn utf16_with_nul(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// A movable global block holding `wide` (already zero-terminated), or why it could not be made.
/// `size` is the block's byte size (the text's, except in the test that asks for an impossible one).
#[cfg(windows)]
fn alloc_text_block(wide: &[u16], size: usize) -> Result<*mut core::ffi::c_void, String> {
    let bytes = std::mem::size_of_val(wide);
    // SAFETY: the block is ours until SetClipboardData takes it or the caller frees it; every
    // handle is checked for null before it is used; the copy stays inside both buffers.
    unsafe {
        let block = GlobalAlloc(GMEM_MOVEABLE, size);
        if block.is_null() {
            return Err("could not allocate clipboard memory".to_string());
        }
        let target = GlobalLock(block);
        if target.is_null() {
            GlobalFree(block);
            return Err("could not lock clipboard memory".to_string());
        }
        ptr::copy_nonoverlapping(
            wide.as_ptr().cast::<u8>(),
            target.cast::<u8>(),
            bytes.min(size),
        );
        GlobalUnlock(block);
        Ok(block)
    }
}

/// Puts `text` on the clipboard as Unicode text. The block is made BEFORE the clipboard is
/// touched, so a failed allocation never wipes what the user already copied. Opening the
/// clipboard can fail while another program holds it, so it is retried five times, 20 ms apart.
#[cfg(windows)]
pub fn set_text(text: &str) -> Result<(), String> {
    let wide = utf16_with_nul(text);
    let bytes = wide.len() * std::mem::size_of::<u16>();
    let block = alloc_text_block(&wide, bytes)?;

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
        // SAFETY: the block was never handed over; it is ours to free.
        unsafe { GlobalFree(block) };
        return Err("the clipboard is busy".to_string());
    }

    // SAFETY: the clipboard is open on this thread (opened above); the block is ours until
    // SetClipboardData takes it, and it is freed here when that call fails.
    let result = unsafe {
        EmptyClipboard();
        if SetClipboardData(CF_UNICODETEXT, block).is_null() {
            GlobalFree(block);
            Err("the clipboard refused the text".to_string())
        } else {
            Ok(())
        }
    };
    // SAFETY: the clipboard was opened on this thread above and is closed exactly once here.
    unsafe {
        CloseClipboard();
    }
    result
}

/// Puts `text` on the clipboard as text (`NSPasteboard`, macOS). The pasteboard keeps the
/// `NSString`, so the text reads back as the same UTF-8 whatever the locale.
#[cfg(target_os = "macos")]
pub fn set_text(text: &str) -> Result<(), String> {
    crate::pasteboard::write_text(&crate::pasteboard::general(), text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn the_block_is_made_before_the_clipboard_is_touched_and_can_fail_alone() {
        let wide = utf16_with_nul("kept");
        let ok = alloc_text_block(&wide, wide.len() * 2).expect("a small block");
        // SAFETY: made just above, never handed to the clipboard.
        unsafe { GlobalFree(ok) };
        // An impossible size fails here, with no clipboard call anywhere near it.
        assert!(alloc_text_block(&wide, usize::MAX / 2).is_err());
    }

    #[test]
    fn utf16_has_a_terminating_zero_and_surrogates() {
        assert_eq!(utf16_with_nul("a\u{1F600}"), vec![0x61, 0xD83D, 0xDE00, 0]);
    }

    /// Reads the clipboard's Unicode text back (test only).
    #[cfg(windows)]
    fn read_text() -> Option<String> {
        use windows_sys::Win32::System::DataExchange::GetClipboardData;
        // SAFETY: the clipboard is opened, read and closed on this thread, so the handle from
        // GetClipboardData stays live for the bounded read (`read_utf16_block` stops at the
        // block's own size and unlocks on every path).
        unsafe {
            if OpenClipboard(ptr::null_mut()) == 0 {
                return None;
            }
            let text = crate::clipboard_port::read_utf16_block(GetClipboardData(CF_UNICODETEXT));
            CloseClipboard();
            text
        }
    }

    #[cfg(windows)]
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

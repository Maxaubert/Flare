//! The clipboard port: the Windows implementation, the in-memory stand-in and the read side.
//! Everything here BLOCKS and is called from worker threads only.

#[cfg(windows)]
use std::ptr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
#[cfg(windows)]
use std::thread::sleep; // sync-ok: the clipboard retry below, at most 80 ms
#[cfg(windows)]
use std::time::Duration;

use flare_shells::paste::ClipboardState;
#[cfg(windows)]
use windows_sys::Win32::Foundation::HGLOBAL;
#[cfg(windows)]
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, GetClipboardData, GetClipboardSequenceNumber, IsClipboardFormatAvailable,
    OpenClipboard, RegisterClipboardFormatW,
};
#[cfg(windows)]
use windows_sys::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
#[cfg(windows)]
use windows_sys::Win32::System::Ole::{CF_DIB, CF_DIBV5, CF_HDROP, CF_UNICODETEXT};
#[cfg(windows)]
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
    /// `--e2e` only: puts a PNG (base64) on the clipboard. The real clipboard refuses.
    fn set_image_png_b64(&self, _png_b64: &str) -> Result<(), String> {
        Err("the real clipboard never takes an image from the harness".to_string())
    }
    /// The image on the clipboard as file bytes (remote sessions PR 6: a paste on a remote tab copies it to the host):
    /// a PNG when one is there, else the bitmap as a `.bmp` file (Windows' `CF_DIB`). None when there is none, it
    /// cannot be read, or it is over [`IMAGE_READ_MAX`]. A worker thread only.
    fn read_image(&self) -> Option<Vec<u8>> {
        None
    }
}

/// The biggest clipboard image `read_image` reads (a 4K screenshot as a bitmap is about 33 MB).
pub const IMAGE_READ_MAX: usize = 128 * 1024 * 1024;

/// Pure: a `.bmp` file of a packed DIB (`CF_DIB`, `CF_DIBV5`): the 14-byte file header in front, its pixel offset past
/// the info header, the colour masks a `BITMAPINFOHEADER` with `BI_BITFIELDS` carries, and the colour table. None for a
/// DIB too short for its own header.
pub fn bmp_of_dib(dib: &[u8]) -> Option<Vec<u8>> {
    let u32_at =
        |i: usize| -> Option<u32> { Some(u32::from_le_bytes(dib.get(i..i + 4)?.try_into().ok()?)) };
    let header = u32_at(0)? as usize;
    if header < 12 || dib.len() < header {
        return None;
    }
    let (bits, compression, used) = if header == 12 {
        // BITMAPCOREHEADER: 3-byte colour table entries; Windows never puts one on the clipboard, but read it right.
        let bits = u16::from_le_bytes(dib.get(10..12)?.try_into().ok()?);
        (bits, 0, 0)
    } else {
        let bits = u16::from_le_bytes(dib.get(14..16)?.try_into().ok()?);
        (bits, u32_at(16)?, u32_at(32)?)
    };
    let entry = if header == 12 { 3 } else { 4 };
    let colours = if used != 0 {
        used as usize
    } else if bits <= 8 {
        1usize << bits
    } else {
        0
    };
    // BI_BITFIELDS (3) and BI_ALPHABITFIELDS (6) put their masks after a 40-byte header; a V4 or V5 header holds them.
    let masks = match (header, compression) {
        (40, 3) => 12,
        (40, 6) => 16,
        _ => 0,
    };
    let offset = 14 + header + masks + colours * entry;
    let size = 14 + dib.len();
    let mut out = Vec::with_capacity(size);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&u32::try_from(size).ok()?.to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&u32::try_from(offset).ok()?.to_le_bytes());
    out.extend_from_slice(dib);
    Some(out)
}

/// The bytes of a global block, at most `max`. None for a null handle, a failed lock or a bigger block.
///
/// # Safety
/// `handle` must be null or a live movable global memory handle for the whole call.
#[cfg(windows)]
unsafe fn read_global_bytes(handle: HGLOBAL, max: usize) -> Option<Vec<u8>> {
    if handle.is_null() {
        return None;
    }
    // SAFETY: the caller guarantees a live handle; the pointer is null-checked below.
    let (locked, size) = unsafe { (GlobalLock(handle).cast::<u8>(), GlobalSize(handle)) };
    if locked.is_null() {
        return None;
    }
    let _unlock = LockGuard(handle);
    if size == 0 || size > max {
        return None;
    }
    // SAFETY: the lock holds `size` bytes readable at `locked`.
    Some(unsafe { std::slice::from_raw_parts(locked, size) }.to_vec())
}

/// The clipboard's image as file bytes: the registered "PNG" format first, then `CF_DIB` and `CF_DIBV5` as a `.bmp`.
/// The clipboard must be open on this thread.
#[cfg(windows)]
fn image_bytes_open() -> Option<Vec<u8>> {
    let name: Vec<u16> = "PNG".encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: the caller has the clipboard open, so a handle it gives stays valid for the read; `name` is a
    // NUL-terminated UTF-16 string that outlives the call.
    unsafe {
        let png = RegisterClipboardFormatW(name.as_ptr());
        if png != 0
            && IsClipboardFormatAvailable(png) != 0
            && let Some(b) = read_global_bytes(GetClipboardData(png), IMAGE_READ_MAX)
        {
            return Some(b);
        }
        for f in [CF_DIB, CF_DIBV5] {
            if IsClipboardFormatAvailable(u32::from(f)) != 0
                && let Some(dib) = read_global_bytes(GetClipboardData(u32::from(f)), IMAGE_READ_MAX)
            {
                return bmp_of_dib(&dib);
            }
        }
    }
    None
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
#[cfg(windows)]
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

/// Closes the clipboard on every path out of a read.
#[cfg(windows)]
struct ClipboardGuard;

#[cfg(windows)]
impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        // SAFETY: only built after `open()` succeeded on this thread; dropped exactly once.
        unsafe {
            CloseClipboard();
        }
    }
}

/// Unlocks a locked global block on every path out of a read.
#[cfg(windows)]
struct LockGuard(HGLOBAL);

#[cfg(windows)]
impl Drop for LockGuard {
    fn drop(&mut self) {
        // SAFETY: only built after a successful GlobalLock of this handle; dropped exactly once.
        unsafe {
            GlobalUnlock(self.0);
        }
    }
}

/// The UTF-16 text in a global block, up to the first zero unit but never past the block's own
/// size (another program may have left no terminator). None for a null handle or a failed lock.
///
/// # Safety
/// `handle` must be null or a live movable global memory handle for the whole call.
#[cfg(windows)]
pub(crate) unsafe fn read_utf16_block(handle: HGLOBAL) -> Option<String> {
    if handle.is_null() {
        return None;
    }
    // SAFETY: the caller guarantees a live handle; the pointer is null-checked below.
    let (locked, size) = unsafe { (GlobalLock(handle).cast::<u16>(), GlobalSize(handle)) };
    if locked.is_null() {
        return None;
    }
    let _unlock = LockGuard(handle);
    let units = size / 2;
    // SAFETY: the lock holds `size` bytes readable at `locked`; `units` u16s fit in them.
    let all = unsafe { std::slice::from_raw_parts(locked, units) };
    let end = all.iter().position(|&u| u == 0).unwrap_or(units);
    Some(String::from_utf16_lossy(&all[..end]))
}

/// Text from CF_UNICODETEXT; the clipboard must be open on this thread.
#[cfg(windows)]
fn unicode_text_open() -> Option<String> {
    // SAFETY: the caller has the clipboard open, so the handle stays valid for the read.
    unsafe {
        if IsClipboardFormatAvailable(u32::from(CF_UNICODETEXT)) == 0 {
            return None;
        }
        read_utf16_block(GetClipboardData(u32::from(CF_UNICODETEXT)))
    }
}

/// Copied files from CF_HDROP; the clipboard must be open on this thread.
#[cfg(windows)]
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

#[cfg(windows)]
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

#[cfg(windows)]
impl ClipboardPort for WinClipboard {
    fn write_text(&self, text: &str, max: usize) -> Result<(), String> {
        check_write(text, max)?;
        crate::clipboard::set_text(text)
    }

    fn read_text(&self) -> Option<String> {
        if !open() {
            return None;
        }
        let _close = ClipboardGuard;
        unicode_text_open()
    }

    fn read_state(&self) -> ClipboardState {
        if !open() {
            return ClipboardState::default();
        }
        let _close = ClipboardGuard;
        ClipboardState {
            files: files_open(),
            text: unicode_text_open().unwrap_or_default(),
            image: image_open(),
        }
    }

    fn sequence(&self) -> u32 {
        // SAFETY: takes no arguments and only reads a system counter.
        unsafe { GetClipboardSequenceNumber() }
    }

    fn read_image(&self) -> Option<Vec<u8>> {
        if !open() {
            return None;
        }
        let _close = ClipboardGuard;
        image_bytes_open()
    }
}

/// macOS: `NSPasteboard` (see `pasteboard.rs`): text, copied files as POSIX paths, an image seen,
/// and the change count as the sequence.
#[cfg(target_os = "macos")]
impl ClipboardPort for WinClipboard {
    fn write_text(&self, text: &str, max: usize) -> Result<(), String> {
        check_write(text, max)?;
        crate::clipboard::set_text(text)
    }

    fn read_text(&self) -> Option<String> {
        crate::pasteboard::read_text(&crate::pasteboard::general())
    }

    fn read_state(&self) -> ClipboardState {
        let pb = crate::pasteboard::general();
        ClipboardState {
            files: crate::pasteboard::read_files(&pb),
            text: crate::pasteboard::read_text(&pb).unwrap_or_default(),
            image: crate::pasteboard::has_image(&pb),
        }
    }

    fn sequence(&self) -> u32 {
        crate::pasteboard::change_count(&crate::pasteboard::general())
    }

    fn read_image(&self) -> Option<Vec<u8>> {
        crate::pasteboard::read_png(&crate::pasteboard::general(), IMAGE_READ_MAX)
    }
}

#[derive(Default)]
struct Inner {
    text: Option<String>,
    files: Vec<String>,
    image: bool,
    png_b64: Option<String>,
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

    fn read_image(&self) -> Option<Vec<u8>> {
        use base64::Engine as _;
        let g = self.lock();
        let b64 = g.png_b64.as_deref()?;
        base64::engine::general_purpose::STANDARD.decode(b64).ok()
    }

    fn set_image_png_b64(&self, png_b64: &str) -> Result<(), String> {
        if png_b64.is_empty() {
            return Err("an empty image".to_string());
        }
        {
            let mut g = self.lock();
            g.image = true;
            g.png_b64 = Some(png_b64.to_string());
        }
        self.seq.fetch_add(1, Ordering::SeqCst);
        Ok(())
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
    fn an_image_set_from_the_harness_shows_in_the_state() {
        let c = MemoryClipboard::new();
        let a = c.sequence();
        assert!(c.set_image_png_b64("").is_err());
        c.set_image_png_b64("iVBORw0KGgo=").unwrap();
        assert!(c.read_state().image);
        assert!(c.sequence() > a);
    }

    /// Remote sessions PR 6: the harness's image reads back as its bytes, and a bare "an image is there" reads none.
    #[test]
    fn the_stand_in_reads_back_the_png_it_was_given() {
        let c = MemoryClipboard::new();
        assert_eq!(c.read_image(), None);
        c.set_image(true);
        assert_eq!(c.read_image(), None);
        c.set_image_png_b64("iVBORw0KGgo=").unwrap();
        assert_eq!(c.read_image().unwrap(), b"\x89PNG\r\n\x1a\n");
    }

    /// A clipboard bitmap becomes a `.bmp` file whose pixel offset skips the header, the masks and the colour table.
    #[test]
    fn a_dib_becomes_a_bmp_file_with_its_pixels_where_the_header_says() {
        fn dib(header: u32, bits: u16, compression: u32, used: u32, rest: usize) -> Vec<u8> {
            let mut d = vec![0u8; header as usize];
            d[0..4].copy_from_slice(&header.to_le_bytes());
            d[4..8].copy_from_slice(&1i32.to_le_bytes());
            d[8..12].copy_from_slice(&1i32.to_le_bytes());
            d[14..16].copy_from_slice(&bits.to_le_bytes());
            d[16..20].copy_from_slice(&compression.to_le_bytes());
            d[32..36].copy_from_slice(&used.to_le_bytes());
            d.extend(std::iter::repeat_n(7u8, rest));
            d
        }
        let offset = |b: &[u8]| u32::from_le_bytes(b[10..14].try_into().unwrap());
        // 24-bit, no table: pixels right after the 40-byte header.
        let b = bmp_of_dib(&dib(40, 24, 0, 0, 4)).unwrap();
        assert_eq!(&b[0..2], b"BM");
        assert_eq!(
            u32::from_le_bytes(b[2..6].try_into().unwrap()) as usize,
            b.len()
        );
        assert_eq!(offset(&b), 14 + 40);
        // 32-bit BI_BITFIELDS: three masks after the header.
        assert_eq!(
            offset(&bmp_of_dib(&dib(40, 32, 3, 0, 16)).unwrap()),
            14 + 40 + 12
        );
        // 8-bit: a 256-entry table; a table of the size it says.
        assert_eq!(
            offset(&bmp_of_dib(&dib(40, 8, 0, 0, 1024)).unwrap()),
            14 + 40 + 1024
        );
        assert_eq!(
            offset(&bmp_of_dib(&dib(40, 8, 0, 16, 64)).unwrap()),
            14 + 40 + 64
        );
        // A V5 header holds its own masks.
        assert_eq!(
            offset(&bmp_of_dib(&dib(124, 32, 3, 0, 4)).unwrap()),
            14 + 124
        );
        // Too short for its own header.
        assert_eq!(bmp_of_dib(&[40, 0, 0, 0]), None);
        assert_eq!(bmp_of_dib(&[]), None);
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

    /// A moveable block filled with `fill` over its whole reported size.
    #[cfg(windows)]
    fn full_block(bytes: usize, fill: u16) -> (HGLOBAL, usize) {
        use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc};
        // SAFETY: allocates and fills a block this test owns; the caller frees it.
        unsafe {
            let h = GlobalAlloc(GMEM_MOVEABLE, bytes);
            assert!(!h.is_null());
            let size = GlobalSize(h);
            let p = GlobalLock(h).cast::<u16>();
            assert!(!p.is_null());
            for i in 0..size / 2 {
                *p.add(i) = fill;
            }
            GlobalUnlock(h);
            (h, size)
        }
    }

    #[cfg(windows)]
    #[test]
    fn an_unterminated_block_is_read_exactly_to_its_end() {
        use windows_sys::Win32::Foundation::GlobalFree;
        let (h, size) = full_block(40, u16::from(b'a'));
        // SAFETY: `h` is a live global handle owned by this test.
        let got = unsafe { read_utf16_block(h) };
        // SAFETY: freeing the block this test allocated.
        unsafe {
            GlobalFree(h);
        }
        assert_eq!(got.unwrap(), "a".repeat(size / 2));
    }

    #[cfg(windows)]
    #[test]
    fn a_terminated_block_stops_at_its_zero() {
        use windows_sys::Win32::Foundation::GlobalFree;
        let (h, _) = full_block(40, u16::from(b'x'));
        // SAFETY: the block is live and owned by this test; two units are overwritten.
        unsafe {
            let p = GlobalLock(h).cast::<u16>();
            *p = u16::from(b'h');
            *p.add(1) = 0;
            GlobalUnlock(h);
        }
        // SAFETY: `h` is a live global handle owned by this test.
        let got = unsafe { read_utf16_block(h) };
        // SAFETY: freeing the block this test allocated.
        unsafe {
            GlobalFree(h);
        }
        assert_eq!(got.as_deref(), Some("h"));
    }

    #[cfg(windows)]
    #[test]
    fn a_null_handle_reads_nothing() {
        // SAFETY: null is allowed.
        assert_eq!(unsafe { read_utf16_block(std::ptr::null_mut()) }, None);
    }

    #[test]
    fn a_real_round_trip_is_not_attempted_in_unit_tests() {
        fn _assert_send_sync<T: Send + Sync>() {}
        _assert_send_sync::<WinClipboard>();
    }
}

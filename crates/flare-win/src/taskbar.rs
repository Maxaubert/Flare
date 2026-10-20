//! The taskbar overlay badge: a disc Flare draws itself (decision D12), set as the window's
//! overlay icon through `ITaskbarList3`, never as a window icon.
//!
//! Part A is pure pixels (a circle with analytic anti-aliasing composited with a coverage mask).
//! Part B rasterises the digits with GDI as a coverage mask and talks to the shell.

#[cfg(windows)]
use std::ffi::c_void;

/// Fill of the disc, `#25242c`, as (r, g, b).
const FILL: (u8, u8, u8) = (0x25, 0x24, 0x2c);

/// A rendered disc: premultiplied BGRA, top-down, `size * size * 4` bytes.
pub struct Disc {
    pub size: u32,
    pub bgra: Vec<u8>,
}

/// Analytic anti-aliasing: coverage of a pixel = clamp(r - dist(centre of pixel, (c, c)) + 0.5, 0, 1)
/// with r = size / 2.
pub fn disc_coverage(size: u32) -> Vec<f32> {
    let r = size as f32 / 2.0;
    let c = r;
    let mut out = Vec::with_capacity((size * size) as usize);
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 + 0.5 - c;
            let dy = y as f32 + 0.5 - c;
            let d = (dx * dx + dy * dy).sqrt();
            out.push((r - d + 0.5).clamp(0.0, 1.0));
        }
    }
    out
}

/// Composite: fill `#25242c` at the disc coverage, then white at `mask[i]/255 * coverage`; the
/// result premultiplied BGRA.
pub fn compose_disc(size: u32, mask: &[u8]) -> Disc {
    let cov = disc_coverage(size);
    let mut bgra = Vec::with_capacity(cov.len() * 4);
    for (i, &c) in cov.iter().enumerate() {
        let m = mask.get(i).copied().unwrap_or(0) as f32 / 255.0;
        let w = m * c; // white's share of the pixel, already scaled by coverage
        let ch = |f: u8| -> u8 {
            // premultiplied: fill * (c - w) + 255 * w
            let v = f as f32 * (c - w) + 255.0 * w;
            v.round().clamp(0.0, 255.0) as u8
        };
        bgra.push(ch(FILL.2));
        bgra.push(ch(FILL.1));
        bgra.push(ch(FILL.0));
        bgra.push((c * 255.0).round().clamp(0.0, 255.0) as u8);
    }
    Disc { size, bgra }
}

/// The font height for the number: 0.8x of the disc for one character, 0.56x for more.
pub fn font_px(size: u32, text_len: usize) -> u32 {
    let k = if text_len > 1 { 0.56 } else { 0.8 };
    (size as f64 * k).round() as u32
}

/// Moves a `size * size` coverage mask up or down so its ink (the rows with a pixel past half
/// coverage) is centred on the disc's middle, within half a pixel. Prism Terminal draws on the
/// canvas's em-middle line nudged 4% down, which centres Segoe's figures; GDI's `DT_VCENTER`
/// centres the whole cell, ascent and descent, so the figures came out 2 to 5 px low (M6 Task 34).
pub fn centre_ink_vertically(mask: &[u8], size: u32) -> Vec<u8> {
    let n = size as usize;
    let inked = |y: usize| mask[y * n..(y + 1) * n].iter().any(|&v| v > 128);
    let Some(top) = (0..n).find(|&y| inked(y)) else {
        return mask.to_vec();
    };
    let bottom = (0..n).rev().find(|&y| inked(y)).unwrap_or(top);
    // Centre (top + bottom + 1) / 2 onto n / 2: the shift is half their doubled distance, floored
    // (an odd distance leaves the half pixel no whole shift can remove).
    let shift = (n as i64 - (top + bottom + 1) as i64).div_euclid(2);
    let mut out = vec![0; n * n];
    for y in 0..n {
        let from = y as i64 - shift;
        if (0..n as i64).contains(&from) {
            let from = from as usize;
            out[y * n..(y + 1) * n].copy_from_slice(&mask[from * n..(from + 1) * n]);
        }
    }
    out
}

/// Disc size in device pixels: round(16 * s), s = scale if finite and > 0 (capped 4) else 1.
/// (The same rule as `flare-agent`'s `badge_pixels`; `flare-win` stays free of that crate.)
pub fn badge_px(scale: f64) -> u32 {
    let s = if scale.is_finite() && scale > 0.0 {
        scale.min(4.0)
    } else {
        1.0
    };
    (16.0 * s).round() as u32
}

/// The number on the disc: "9+" past nine.
fn count_text(count: usize) -> String {
    if count > 9 {
        "9+".to_string()
    } else {
        count.to_string()
    }
}

fn description(count: usize) -> String {
    if count == 1 {
        "1 tab needs a look".to_string()
    } else {
        format!("{count} tabs need a look")
    }
}

// ---------------------------------------------------------------- Part B: GDI and the shell

#[cfg(windows)]
mod win {
    use super::*;
    use windows::Win32::Foundation::{COLORREF, HWND, RECT, RPC_E_CHANGED_MODE};
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    };
    use windows::Win32::UI::Shell::{ITaskbarList3, TaskbarList};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateIconIndirect, DestroyIcon, HICON, ICONINFO,
    };
    use windows::core::PCWSTR;

    const FACES: [&str; 2] = ["Segoe UI Variable Display", "Segoe UI"];

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn dib_info(size: u32) -> BITMAPINFO {
        BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size as i32,
                biHeight: -(size as i32), // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Creates `face` at `px` high and reports the face GDI actually picked (the font mapper
    /// substitutes silently).
    fn make_font(hdc: HDC, face: &str, px: u32) -> (HFONT, String) {
        let name = wide(face);
        // SAFETY: `name` is a NUL-terminated UTF-16 buffer that outlives the call.
        let font = unsafe {
            CreateFontW(
                -(px as i32),
                0,
                0,
                0,
                400,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                ANTIALIASED_QUALITY,
                0,
                PCWSTR(name.as_ptr()),
            )
        };
        // SAFETY: `hdc` is a live memory DC and `font` was just created.
        let old = unsafe { SelectObject(hdc, font.into()) };
        let mut buf = [0u16; 64];
        // SAFETY: `buf` is a writable slice and `hdc` is live.
        let n = unsafe { GetTextFaceW(hdc, Some(&mut buf)) };
        let got = if n > 1 {
            String::from_utf16_lossy(&buf[..(n as usize - 1).min(63)])
        } else {
            String::new()
        };
        // SAFETY: re-selecting the previous object into the same DC.
        unsafe { SelectObject(hdc, old) };
        (font, got)
    }

    /// The font to use: the first face GDI really gave us, else Segoe UI whatever it mapped to.
    fn pick_font(hdc: HDC, px: u32) -> (HFONT, String) {
        for face in FACES {
            let (f, got) = make_font(hdc, face, px);
            if got.eq_ignore_ascii_case(face) {
                return (f, got);
            }
            // SAFETY: not selected into any DC now.
            unsafe {
                let _ = DeleteObject(f.into());
            }
        }
        make_font(hdc, FACES[1], px)
    }

    /// The face name GDI reports for the font `text_mask` would use.
    pub fn chosen_face(size: u32) -> String {
        // SAFETY: a memory DC compatible with the screen, deleted below.
        let hdc = unsafe { CreateCompatibleDC(None) };
        let (font, got) = pick_font(hdc, font_px(size, 1).max(1));
        // SAFETY: font and DC were created above and the font is not selected.
        unsafe {
            let _ = DeleteObject(font.into());
            let _ = DeleteDC(hdc);
        }
        got
    }

    pub fn text_mask(size: u32, text: &str) -> Vec<u8> {
        let n = (size * size) as usize;
        if text.is_empty() || size == 0 {
            return vec![0; n];
        }
        let info = dib_info(size);
        let mut bits: *mut c_void = std::ptr::null_mut();
        // SAFETY: a memory DC and a 32-bit DIB section whose pixel pointer GDI fills in; both
        // are released before returning.
        let (hdc, bmp) = unsafe {
            let hdc = CreateCompatibleDC(None);
            let bmp = CreateDIBSection(Some(hdc), &info, DIB_RGB_COLORS, &mut bits, None, 0);
            (hdc, bmp)
        };
        let bmp = match bmp {
            Ok(b) if !bits.is_null() => b,
            other => {
                // SAFETY: created above.
                unsafe {
                    if let Ok(b) = other {
                        let _ = DeleteObject(b.into());
                    }
                    let _ = DeleteDC(hdc);
                }
                return vec![0; n];
            }
        };
        // SAFETY: selecting our own bitmap into our own DC.
        let old_bmp = unsafe { SelectObject(hdc, bmp.into()) };
        // SAFETY: `bits` points at size*size*4 writable bytes owned by the DIB section.
        unsafe { std::ptr::write_bytes(bits as *mut u8, 0, n * 4) };
        let (font, _) = pick_font(hdc, font_px(size, text.chars().count()).max(1));
        let mut w: Vec<u16> = text.encode_utf16().collect();
        let mut rc = RECT {
            left: 0,
            top: 0,
            right: size as i32,
            bottom: size as i32,
        };
        // SAFETY: font and DC are ours; `w` and `rc` live across the call.
        unsafe {
            let old_font = SelectObject(hdc, font.into());
            SetBkMode(hdc, TRANSPARENT);
            SetTextColor(hdc, COLORREF(0x00FF_FFFF));
            DrawTextW(
                hdc,
                &mut w,
                &mut rc,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
            SelectObject(hdc, old_font);
        }
        // SAFETY: the DIB is still alive; read exactly n * 4 bytes.
        let src = unsafe { std::slice::from_raw_parts(bits as *const u8, n * 4) };
        // BGRA: the red channel is byte 2.
        let out: Vec<u8> = src.as_chunks::<4>().0.iter().map(|p| p[2]).collect();
        // SAFETY: release everything created here, after the last read of `bits`.
        unsafe {
            SelectObject(hdc, old_bmp);
            let _ = DeleteObject(font.into());
            let _ = DeleteObject(bmp.into());
            let _ = DeleteDC(hdc);
        }
        super::centre_ink_vertically(&out, size)
    }

    /// The Windows shell.
    pub struct WinShell {
        hwnd: isize,
        list: Option<ITaskbarList3>,
        tried: bool,
    }

    impl WinShell {
        pub fn new(hwnd: isize) -> Self {
            Self {
                hwnd,
                list: None,
                tried: false,
            }
        }

        fn list(&mut self) -> Option<&ITaskbarList3> {
            if self.list.is_none() && !self.tried {
                self.tried = true;
                // SAFETY: COM init on the calling (UI) thread; S_FALSE and a changed mode are fine.
                let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
                if hr.is_ok() || hr == RPC_E_CHANGED_MODE {
                    // SAFETY: plain COM activation of the shell's taskbar list.
                    let made: windows::core::Result<ITaskbarList3> =
                        unsafe { CoCreateInstance(&TaskbarList, None, CLSCTX_INPROC_SERVER) };
                    if let Ok(l) = made {
                        // SAFETY: `l` is a live interface.
                        if unsafe { l.HrInit() }.is_ok() {
                            self.list = Some(l);
                        }
                    }
                }
            }
            self.list.as_ref()
        }
    }

    fn make_icon(d: &Disc) -> Option<HICON> {
        let info = dib_info(d.size);
        let mut bits: *mut c_void = std::ptr::null_mut();
        // SAFETY: a DIB section the size of the disc; GDI fills `bits`.
        let color =
            unsafe { CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0) }.ok()?;
        if bits.is_null() {
            // SAFETY: created above.
            unsafe {
                let _ = DeleteObject(color.into());
            }
            return None;
        }
        // SAFETY: `bits` holds size*size*4 writable bytes and `d.bgra` is exactly that long.
        unsafe { std::ptr::copy_nonoverlapping(d.bgra.as_ptr(), bits as *mut u8, d.bgra.len()) };
        // SAFETY: a 1-bpp mask, all zero (the alpha channel carries the shape).
        let mask = unsafe { CreateBitmap(d.size as i32, d.size as i32, 1, 1, None) };
        let ii = ICONINFO {
            fIcon: true.into(),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: color,
        };
        // SAFETY: both bitmaps are valid; the icon copies them.
        let icon = unsafe { CreateIconIndirect(&ii) }.ok();
        // SAFETY: the icon owns copies now.
        unsafe {
            let _ = DeleteObject(color.into());
            let _ = DeleteObject(mask.into());
        }
        icon
    }

    impl super::OverlayShell for WinShell {
        fn reset(&mut self) {
            self.list = None;
            self.tried = false;
        }

        fn set_overlay(&mut self, icon: Option<&Disc>, text: &str) -> bool {
            if self.hwnd == 0 {
                return false;
            }
            let hwnd = HWND(self.hwnd as *mut c_void);
            let hicon = match icon {
                Some(d) => match make_icon(d) {
                    Some(h) => h,
                    None => return false,
                },
                None => HICON::default(),
            };
            let desc = wide(text);
            let ok = match self.list() {
                Some(l) => {
                    let p = if icon.is_some() {
                        PCWSTR(desc.as_ptr())
                    } else {
                        PCWSTR::null()
                    };
                    // SAFETY: valid HWND value, icon (or null to clear) and NUL-terminated text.
                    unsafe { l.SetOverlayIcon(hwnd, hicon, p) }.is_ok()
                }
                None => false,
            };
            if icon.is_some() {
                // SAFETY: the taskbar copied the icon.
                unsafe {
                    let _ = DestroyIcon(hicon);
                }
            }
            ok
        }
    }
}

/// White text on black rasterised by GDI, read back as 0..255 coverage; `size * size` bytes.
#[cfg(windows)]
pub fn text_mask(size: u32, text: &str) -> Vec<u8> {
    win::text_mask(size, text)
}

/// Non-Windows builds have no GDI: no ink.
#[cfg(not(windows))]
pub fn text_mask(size: u32, _text: &str) -> Vec<u8> {
    vec![0; (size * size) as usize]
}

/// The face name GDI reports for the font the badge uses.
#[cfg(windows)]
pub fn chosen_face(size: u32) -> String {
    win::chosen_face(size)
}

/// The shell side of the overlay, so a test can stand in for it.
pub trait OverlayShell {
    /// `None` clears the overlay. True when the shell took it.
    fn set_overlay(&mut self, icon: Option<&Disc>, text: &str) -> bool;

    /// The taskbar was recreated (Explorer restarted): forget the COM object made for the old one.
    fn reset(&mut self) {}
}

/// Whether `msg` is the registered `TaskbarButtonCreated` message (`registered` 0 means the
/// registration failed, which matches nothing).
pub fn is_button_created(msg: u32, registered: u32) -> bool {
    registered != 0 && msg == registered
}

/// Listens on `hwnd` for the shell's `TaskbarButtonCreated` broadcast (sent when Explorer makes the
/// window's button, again after it restarts). The window procedure only sets a flag; the UI tick
/// redraws the badge. Safe to call again.
#[cfg(windows)]
pub fn watch_button_created(hwnd: isize) -> bool {
    use windows_sys::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
    use windows_sys::Win32::UI::WindowsAndMessaging::RegisterWindowMessageW;
    const ID: usize = 0x464C_5442;
    static REGISTERED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    unsafe extern "system" fn proc(
        hwnd: windows_sys::Win32::Foundation::HWND,
        msg: u32,
        wparam: windows_sys::Win32::Foundation::WPARAM,
        lparam: windows_sys::Win32::Foundation::LPARAM,
        _id: usize,
        _data: usize,
    ) -> windows_sys::Win32::Foundation::LRESULT {
        if is_button_created(msg, REGISTERED.load(std::sync::atomic::Ordering::SeqCst)) {
            note_button_created();
        }
        // SAFETY: the next procedure in the chain, with the arguments this one was given.
        unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
    }
    let name: Vec<u16> = "TaskbarButtonCreated"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: `name` is NUL-terminated and outlives the call.
    let id = unsafe { RegisterWindowMessageW(name.as_ptr()) };
    REGISTERED.store(id, std::sync::atomic::Ordering::SeqCst);
    // SAFETY: subclasses a window of this process from its own thread; the procedure is a plain function.
    id != 0 && unsafe { SetWindowSubclass(hwnd as _, Some(proc), ID, 0) != 0 }
}

static BUTTON_CREATED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Notes that the taskbar button was (re)created; the UI tick takes it with [`take_button_created`].
pub fn note_button_created() {
    BUTTON_CREATED.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// True once per `TaskbarButtonCreated`: the overlay must be drawn again.
pub fn take_button_created() -> bool {
    BUTTON_CREATED.swap(false, std::sync::atomic::Ordering::SeqCst)
}

/// A shell that takes every overlay and shows it nowhere: an `--e2e` run has a badge to read
/// (`last_description`) and never touches the real taskbar.
pub struct RecordShell;

impl OverlayShell for RecordShell {
    fn set_overlay(&mut self, _icon: Option<&Disc>, _text: &str) -> bool {
        true
    }
}

/// The shell used where there is no taskbar.
#[cfg(not(windows))]
struct NoShell;
#[cfg(not(windows))]
impl OverlayShell for NoShell {
    fn set_overlay(&mut self, _icon: Option<&Disc>, _text: &str) -> bool {
        false
    }
}

std::thread_local! {
    /// What the shell was last told on this thread: the description, or empty once cleared. The
    /// test hook's agent probe reads it (the window and its probes share the UI thread).
    static LAST_DESCRIPTION: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

std::thread_local! {
    /// The width in device pixels of the disc the shell last took on this thread; 0 once cleared.
    static LAST_OVERLAY_PX: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// The width of the overlay picture the shell last took on this thread (`badge_px` of the scale it
/// was drawn at), 0 when the overlay is clear. The test hook reads it: the picture itself is never
/// kept.
pub fn last_overlay_px() -> u32 {
    LAST_OVERLAY_PX.with(std::cell::Cell::get)
}

fn remember_description(text: &str) {
    LAST_DESCRIPTION.with(|d| *d.borrow_mut() = text.to_string());
}

/// The description the shell last took on this thread ("1 tab needs a look"), empty when the
/// overlay is clear or was never set.
pub fn last_description() -> String {
    LAST_DESCRIPTION.with(|d| d.borrow().clone())
}

/// The overlay icon on the window's taskbar button (lower right).
pub struct TaskbarBadge {
    shell: Box<dyn OverlayShell>,
    last: Option<(usize, u32)>,
    cleared: bool,
}

impl TaskbarBadge {
    pub fn new(hwnd: isize) -> Self {
        #[cfg(windows)]
        let shell: Box<dyn OverlayShell> = Box::new(win::WinShell::new(hwnd));
        #[cfg(not(windows))]
        let shell: Box<dyn OverlayShell> = {
            let _ = hwnd;
            Box::new(NoShell)
        };
        Self::with_shell(shell)
    }

    pub fn with_shell(shell: Box<dyn OverlayShell>) -> Self {
        Self {
            shell,
            last: None,
            cleared: false,
        }
    }

    /// `count` 0 clears the overlay; otherwise a disc of `badge_px(scale)` with the count text.
    /// A repeat of the same (count, scale) does nothing. True when the shell took it.
    pub fn set(&mut self, count: usize, scale: f64) -> bool {
        if count == 0 {
            return self.clear();
        }
        let size = badge_px(scale);
        if self.last == Some((count, size)) {
            return true;
        }
        let disc = compose_disc(size, &text_mask(size, &count_text(count)));
        let text = description(count);
        let ok = self.shell.set_overlay(Some(&disc), &text);
        if ok {
            remember_description(&text);
            LAST_OVERLAY_PX.with(|p| p.set(size));
            self.last = Some((count, size));
            self.cleared = false;
        }
        ok
    }

    /// The taskbar button was created again (Explorer restarted, audit L14): the shell lost the
    /// overlay and its COM object is stale. The shell starts afresh and the next `set` of the same
    /// count draws again.
    pub fn refresh(&mut self) {
        self.shell.reset();
        self.last = None;
        self.cleared = false;
    }

    pub fn clear(&mut self) -> bool {
        if self.cleared && self.last.is_none() {
            return true;
        }
        let ok = self.shell.set_overlay(None, "");
        if ok {
            remember_description("");
            LAST_OVERLAY_PX.with(|p| p.set(0));
            self.last = None;
            self.cleared = true;
        }
        ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    const SCALES: [f64; 8] = [1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 3.0];

    #[test]
    fn the_corners_are_clear_and_the_middle_is_the_fill() {
        let d = compose_disc(36, &[0; 36 * 36]);
        assert_eq!(d.bgra[3], 0);
        let i = (18 * 36 + 18) * 4;
        assert_eq!(&d.bgra[i..i + 4], &[0x2c, 0x24, 0x25, 255]);
        let sum: f32 = disc_coverage(36).iter().sum();
        let want = std::f32::consts::PI * 18.0 * 18.0;
        assert!((sum - want).abs() / want < 0.02, "{sum} vs {want}");
    }

    #[test]
    fn the_edge_is_anti_aliased_and_symmetric() {
        let cov = disc_coverage(36);
        let row = &cov[18 * 36..19 * 36];
        for x in 0..36 {
            assert!((row[x] - row[35 - x]).abs() < 1e-4);
        }
        let d = compose_disc(36, &[0; 36 * 36]);
        assert!(
            d.bgra
                .as_chunks::<4>()
                .0
                .iter()
                .any(|p| p[3] > 0 && p[3] < 255)
        );
    }

    #[test]
    fn a_full_mask_pixel_is_white_inside_the_disc() {
        let mut m = vec![0u8; 36 * 36];
        m[18 * 36 + 18] = 255;
        let d = compose_disc(36, &m);
        let i = (18 * 36 + 18) * 4;
        assert_eq!(&d.bgra[i..i + 4], &[255, 255, 255, 255]);
    }

    #[test]
    fn font_sizes_follow_the_ported_badge() {
        assert_eq!(font_px(16, 1), 13);
        assert_eq!(font_px(36, 1), 29);
        assert_eq!(font_px(36, 2), 20);
        assert_eq!(font_px(16, 2), 9);
    }

    #[test]
    fn it_works_at_every_scale() {
        let want = [16, 20, 24, 28, 32, 36, 40, 48];
        for (s, w) in SCALES.iter().zip(want) {
            let px = badge_px(*s);
            assert_eq!(px, w);
            let d = compose_disc(px, &vec![0; (px * px) as usize]);
            assert_eq!(d.bgra.len(), (px * px * 4) as usize);
            let c = ((px / 2) * px + px / 2) as usize * 4;
            assert_eq!(d.bgra[c + 3], 255);
        }
        assert_eq!(badge_px(f64::NAN), 16);
        assert_eq!(badge_px(9.0), 64);
    }

    #[cfg(windows)]
    fn ink_box(mask: &[u8], size: usize) -> Option<(usize, usize, usize, usize)> {
        let mut b: Option<(usize, usize, usize, usize)> = None;
        for y in 0..size {
            for x in 0..size {
                if mask[y * size + x] > 128 {
                    b = Some(match b {
                        None => (x, y, x, y),
                        Some((a, c, d, e)) => (a.min(x), c.min(y), d.max(x), e.max(y)),
                    });
                }
            }
        }
        b
    }

    /// M6 Task 34 (the badge sheet showed every digit 2 to 5 px below the disc's middle): at the
    /// eight scalings' sizes the ink of each count sits centred up and down, within half a pixel
    /// (an ink box of the other parity cannot do better), as Prism Terminal's nudged middle line
    /// centres it.
    #[cfg(windows)]
    #[test]
    fn the_digits_sit_centred_on_the_disc_at_every_size() {
        for size in [16usize, 20, 24, 28, 32, 36, 40, 48] {
            for text in ["1", "3", "5", "8", "9", "9+"] {
                let m = text_mask(size as u32, text);
                let (_, y0, _, y1) = ink_box(&m, size).expect("ink");
                let off = (y0 + y1 + 1) as f64 / 2.0 - size as f64 / 2.0;
                assert!(
                    off.abs() <= 0.5,
                    "{text} at {size}: ink centre {off:+} px off the middle"
                );
            }
        }
    }

    #[cfg(windows)]
    #[test]
    fn text_mask_has_ink_for_a_digit_and_none_for_empty() {
        let m = text_mask(36, "3");
        assert_eq!(m.len(), 36 * 36);
        let ink = m.iter().filter(|&&v| v > 128).count();
        assert!(
            ink * 100 >= 36 * 36 * 4 && ink * 100 <= 36 * 36 * 40,
            "ink {ink}"
        );
        let (x0, y0, x1, y1) = ink_box(&m, 36).unwrap();
        let cx = (x0 + x1) as f64 / 2.0;
        let cy = (y0 + y1) as f64 / 2.0;
        assert!(
            (cx - 17.5).abs() <= 3.0 && (cy - 17.5).abs() <= 3.0,
            "{cx},{cy}"
        );
        assert!(text_mask(36, "").iter().all(|&v| v == 0));
    }

    #[cfg(windows)]
    #[test]
    fn two_characters_are_narrower_each() {
        let w = |t: &str| {
            let (x0, _, x1, _) = ink_box(&text_mask(36, t), 36).unwrap();
            x1 - x0 + 1
        };
        let one = w("9");
        let two = w("9+");
        assert!(two < one * 2 && two > one, "{one} {two}");
    }

    #[cfg(windows)]
    #[test]
    fn the_mask_is_not_the_wrong_font_on_this_machine() {
        let face = chosen_face(36);
        println!("GDI chose: {face}");
        assert!(
            face == "Segoe UI Variable Display" || face == "Segoe UI",
            "got {face}"
        );
    }

    #[test]
    fn the_overlay_does_not_panic_without_a_window() {
        let mut b = TaskbarBadge::new(0);
        assert!(!b.set(3, 1.0));
        assert!(!b.clear());
    }

    type Log = Rc<RefCell<Vec<(Option<u32>, String)>>>;
    struct Fake(Log);
    impl OverlayShell for Fake {
        fn set_overlay(&mut self, icon: Option<&Disc>, text: &str) -> bool {
            self.0
                .borrow_mut()
                .push((icon.map(|d| d.size), text.to_string()));
            true
        }
    }

    #[test]
    fn the_last_description_is_what_the_shell_was_last_told() {
        let log: Log = Rc::new(RefCell::new(Vec::new()));
        let mut b = TaskbarBadge::with_shell(Box::new(Fake(log)));
        assert_eq!(last_description(), "");
        assert!(b.set(1, 1.0));
        assert_eq!(last_description(), "1 tab needs a look");
        assert!(b.set(2, 1.0));
        assert_eq!(last_description(), "2 tabs need a look");
        assert!(b.clear());
        assert_eq!(last_description(), "", "a cleared badge says nothing");
    }

    #[test]
    fn the_last_overlay_width_is_the_disc_the_shell_was_last_given() {
        let mut b = TaskbarBadge::with_shell(Box::new(RecordShell));
        assert!(b.set(1, 1.0));
        assert_eq!(last_overlay_px(), 16);
        assert!(b.set(1, 2.25));
        assert_eq!(last_overlay_px(), 36);
        assert!(b.clear());
        assert_eq!(last_overlay_px(), 0, "a cleared badge has no picture");
    }

    #[test]
    fn the_recording_shell_keeps_the_badge_readable_without_a_taskbar() {
        let mut b = TaskbarBadge::with_shell(Box::new(RecordShell));
        assert!(b.set(2, 2.25));
        assert_eq!(last_description(), "2 tabs need a look");
        assert!(b.clear());
        assert_eq!(last_description(), "");
    }

    #[test]
    fn a_repeat_is_free() {
        let log: Log = Rc::new(RefCell::new(Vec::new()));
        let mut b = TaskbarBadge::with_shell(Box::new(Fake(log.clone())));
        assert!(b.set(3, 1.0));
        assert!(b.set(3, 1.0));
        assert_eq!(log.borrow().len(), 1);
        assert!(b.set(3, 2.25));
        assert_eq!(log.borrow().len(), 2);
        assert_eq!(
            log.borrow()[1],
            (Some(36), "3 tabs need a look".to_string())
        );
        assert!(b.set(0, 1.0));
        assert_eq!(log.borrow().len(), 3);
        assert_eq!(log.borrow()[2].0, None);
        assert!(b.set(0, 1.0));
        assert_eq!(log.borrow().len(), 3);
        assert!(b.set(1, 1.0));
        assert_eq!(log.borrow()[3].1, "1 tab needs a look");
    }

    /// L14 (audit 2026-10-20): Explorer restarting recreates the taskbar buttons and the overlay is
    /// gone. The refresh makes the shell start afresh and the next `set` of the SAME count draw again.
    #[test]
    fn a_refresh_after_an_explorer_restart_draws_the_same_badge_again() {
        let log: Log = Rc::new(RefCell::new(Vec::new()));
        let resets = Rc::new(RefCell::new(0u32));
        struct Counting(Log, Rc<RefCell<u32>>);
        impl OverlayShell for Counting {
            fn set_overlay(&mut self, icon: Option<&Disc>, text: &str) -> bool {
                self.0
                    .borrow_mut()
                    .push((icon.map(|d| d.size), text.to_string()));
                true
            }
            fn reset(&mut self) {
                *self.1.borrow_mut() += 1;
            }
        }
        let mut b = TaskbarBadge::with_shell(Box::new(Counting(log.clone(), resets.clone())));
        for scale in [1.0, 2.25, 3.0] {
            log.borrow_mut().clear();
            assert!(b.set(2, scale));
            assert!(b.set(2, scale));
            assert_eq!(log.borrow().len(), 1, "a repeat is free");
            b.refresh();
            assert!(b.set(2, scale));
            assert_eq!(
                log.borrow().len(),
                2,
                "the badge is drawn again after the refresh"
            );
            assert_eq!(
                log.borrow()[1],
                (Some(badge_px(scale)), "2 tabs need a look".to_string())
            );
        }
        assert_eq!(
            *resets.borrow(),
            3,
            "the shell was told to start afresh each time"
        );
    }

    #[test]
    fn the_button_created_message_is_matched_only_by_its_registered_id() {
        assert!(is_button_created(0xC123, 0xC123));
        assert!(!is_button_created(0x0001, 0xC123));
        assert!(
            !is_button_created(0xC123, 0),
            "an id of 0 (registration failed) matches nothing"
        );
        assert!(!take_button_created());
        note_button_created();
        assert!(take_button_created());
        assert!(!take_button_created(), "taken once");
    }
}

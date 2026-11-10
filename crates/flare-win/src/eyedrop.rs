//! The eyedropper: the Win32 screen pick (M7 Task 30, decision D12).
//!
//! `pick_colour` answers the e2e seam first (no window). Otherwise one worker thread freezes
//! the virtual screen into a topmost popup, shows a 9 by 9 magnifier with the hex under it
//! beside the pointer, and reports `Picked`, `Cancelled` or `Failed` through `done`.
//!
//! By hand (`the_window_picks_a_pixel`, ignored): run
//! `cargo test -p flare-win the_window_picks_a_pixel -- --ignored --nocapture --target-dir <T>`,
//! move the pointer over a known colour and left-click. Expected output: `Picked("#rrggbb")`
//! with that colour. A right-click or Escape prints `Cancelled`.

#[cfg(windows)]
use std::cell::{Cell, RefCell};
#[cfg(windows)]
use std::ptr::null;
#[cfg(windows)]
use std::sync::Once;
#[cfg(any(windows, test))]
use std::sync::atomic::AtomicUsize;
#[cfg(any(windows, test))]
use std::sync::atomic::Ordering;
use std::thread;

#[cfg(windows)]
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
#[cfg(windows)]
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BeginPaint, BitBlt, CAPTUREBLT, COLORONCOLOR,
    CreateCompatibleBitmap, CreateCompatibleDC, CreateDIBSection, CreateSolidBrush, DIB_RGB_COLORS,
    DeleteDC, DeleteObject, DrawTextW, EndPaint, FillRect, FrameRect, GetDC, GetDIBits, HBITMAP,
    HDC, HGDIOBJ, InvalidateRect, PAINTSTRUCT, ReleaseDC, SRCCOPY, SelectObject, SetBkMode,
    SetStretchBltMode, SetTextColor, StretchBlt, TRANSPARENT, UpdateWindow,
};
#[cfg(windows)]
use windows_sys::Win32::Graphics::Gdi::{DT_CENTER, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER};
#[cfg(windows)]
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
#[cfg(windows)]
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext,
};
#[cfg(windows)]
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, SetFocus, VK_ESCAPE,
};
#[cfg(windows)]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWLP_USERDATA,
    GetMessageW, GetSystemMetrics, IDC_CROSS, LoadCursorW, MSG, PostQuitMessage, RegisterClassW,
    SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SW_SHOW,
    SetForegroundWindow, SetWindowLongPtrW, ShowWindow, TranslateMessage, WM_CAPTURECHANGED,
    WM_DESTROY, WM_ERASEBKGND, WM_KEYDOWN, WM_KILLFOCUS, WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCCREATE,
    WM_PAINT, WM_RBUTTONUP, WNDCLASSW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

#[cfg(windows)]
pub use crate::eyedrop_math::Snapshot;
#[cfg(windows)]
use crate::eyedrop_math::{
    LOUPE_CELL_PX, LOUPE_CELLS, caption_ink, hex_of, loupe_origin, loupe_source,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickResult {
    Picked(String),
    Cancelled,
    Failed(String),
}

/// The value of `FLARE_E2E_PICK_COLOUR` (read by the binary, never here): `#rrggbb` picks,
/// `cancel` cancels, anything else (and unset) is no seam.
pub fn pick_seam(env: Option<&str>) -> Option<PickResult> {
    let v = env?;
    if v == "cancel" {
        return Some(PickResult::Cancelled);
    }
    let digits = v.strip_prefix('#')?;
    if digits.len() == 6 && digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Some(PickResult::Picked(v.to_ascii_lowercase()));
    }
    None
}

/// How many pick windows this process has created (tests assert the seam makes none).
#[cfg(any(windows, test))]
static WINDOWS_CREATED: AtomicUsize = AtomicUsize::new(0);

/// Asks for a colour. The seam answers at once with no window; else a worker thread owns the
/// window and its message loop and calls `done` once.
pub fn pick_colour(seam: Option<String>, done: impl FnOnce(PickResult) + Send + 'static) {
    if let Some(result) = pick_seam(seam.as_deref()) {
        done(result);
        return;
    }
    let spawned = thread::Builder::new()
        .name("flare-eyedrop".into())
        .spawn(move || {
            let result = run_window();
            done(result);
        });
    if let Err(e) = spawned {
        log::warn!("eyedrop: no worker thread: {e}");
    }
}

#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

#[cfg(windows)]
fn bitmap_info(w: i32, h: i32) -> BITMAPINFO {
    BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h, // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        },
        bmiColors: [Default::default()],
    }
}

/// Copies the whole virtual screen (physical pixels) into a snapshot.
#[cfg(windows)]
pub fn capture_virtual_screen() -> Result<Snapshot, String> {
    // SAFETY: plain metric reads.
    let (x, y, w, h) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    };
    if w <= 0 || h <= 0 {
        return Err("the virtual screen has no size".into());
    }
    // SAFETY: every GDI object created below is released on every path before returning; the
    // pixel buffer is exactly w * h * 4 bytes, which GetDIBits fills for h scan lines.
    unsafe {
        let screen = GetDC(std::ptr::null_mut());
        if screen.is_null() {
            return Err("GetDC failed".into());
        }
        let mem = CreateCompatibleDC(screen);
        if mem.is_null() {
            ReleaseDC(std::ptr::null_mut(), screen);
            return Err("CreateCompatibleDC failed".into());
        }
        let bmp: HBITMAP = CreateCompatibleBitmap(screen, w, h);
        if bmp.is_null() {
            DeleteDC(mem);
            ReleaseDC(std::ptr::null_mut(), screen);
            return Err("CreateCompatibleBitmap failed".into());
        }
        let old = SelectObject(mem, bmp as HGDIOBJ);
        let copied = BitBlt(mem, 0, 0, w, h, screen, x, y, SRCCOPY | CAPTUREBLT);
        // The bitmap must not be selected into a DC while GetDIBits reads it.
        SelectObject(mem, old);
        let mut bgra = vec![0u8; (w as usize) * (h as usize) * 4];
        let mut info = bitmap_info(w, h);
        let lines = if copied != 0 {
            GetDIBits(
                mem,
                bmp,
                0,
                h as u32,
                bgra.as_mut_ptr().cast(),
                &mut info,
                DIB_RGB_COLORS,
            )
        } else {
            0
        };
        DeleteObject(bmp as HGDIOBJ);
        DeleteDC(mem);
        ReleaseDC(std::ptr::null_mut(), screen);
        if copied == 0 {
            return Err("BitBlt failed".into());
        }
        if lines != h {
            return Err("GetDIBits failed".into());
        }
        Ok(Snapshot {
            origin: (x, y),
            size: (w as u32, h as u32),
            bgra,
        })
    }
}

#[cfg(windows)]
const LOUPE_SIDE: u32 = LOUPE_CELLS * LOUPE_CELL_PX + 2;
#[cfg(windows)]
const CAPTION_H: u32 = 22;
#[cfg(windows)]
const LOUPE_BOX: (u32, u32) = (LOUPE_SIDE, LOUPE_SIDE + CAPTION_H);

#[cfg(windows)]
struct State {
    snap: Snapshot,
    mem: HDC,
    dib: HBITMAP,
    old: HGDIOBJ,
    /// The pointer in window (snapshot) coordinates; None before the first move.
    pointer: Option<(i32, i32)>,
}

#[cfg(windows)]
thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    static RESULT: Cell<Option<PickResult>> = const { Cell::new(None) };
}

#[cfg(windows)]
fn rgb_ref(c: [u8; 3]) -> u32 {
    u32::from(c[0]) | (u32::from(c[1]) << 8) | (u32::from(c[2]) << 16)
}

/// Records the first result and closes the window; later calls (the capture-lost message that
/// the close itself raises) change nothing.
#[cfg(windows)]
fn finish(hwnd: HWND, result: PickResult) {
    let first = RESULT.with(|r| {
        let cur = r.take();
        let first = cur.is_none();
        r.set(cur.or(Some(result)));
        first
    });
    if first {
        // SAFETY: the window belongs to this thread; no state borrow is held here.
        unsafe {
            ReleaseCapture();
            DestroyWindow(hwnd);
        }
    }
}

#[cfg(windows)]
fn loupe_rect(st: &State, p: (i32, i32)) -> RECT {
    let (w, h) = st.snap.size;
    let (ox, oy) = loupe_origin(p, LOUPE_BOX, (0, 0, w, h));
    RECT {
        left: ox,
        top: oy,
        right: ox + LOUPE_BOX.0 as i32,
        bottom: oy + LOUPE_BOX.1 as i32,
    }
}

#[cfg(windows)]
fn lparam_point(lp: LPARAM) -> (i32, i32) {
    (
        i32::from((lp & 0xffff) as u16 as i16),
        i32::from(((lp >> 16) & 0xffff) as u16 as i16),
    )
}

#[cfg(windows)]
fn paint(hwnd: HWND) {
    // SAFETY: BeginPaint/EndPaint pair on this window's own thread; all GDI objects made here
    // are deleted before EndPaint.
    unsafe {
        let mut ps: PAINTSTRUCT = std::mem::zeroed();
        let hdc = BeginPaint(hwnd, &mut ps);
        if hdc.is_null() {
            return;
        }
        STATE.with(|s| {
            let s = s.borrow();
            let Some(st) = s.as_ref() else { return };
            let rc = ps.rcPaint;
            let (pw, ph) = (rc.right - rc.left, rc.bottom - rc.top);
            if pw <= 0 || ph <= 0 {
                return;
            }
            // Double buffer: draw into a bitmap the size of the dirty rectangle.
            let back = CreateCompatibleDC(hdc);
            let bbmp = CreateCompatibleBitmap(hdc, pw, ph);
            let bold = SelectObject(back, bbmp as HGDIOBJ);
            BitBlt(back, 0, 0, pw, ph, st.mem, rc.left, rc.top, SRCCOPY);
            if let Some(p) = st.pointer {
                draw_loupe(back, st, p, rc.left, rc.top);
            }
            BitBlt(hdc, rc.left, rc.top, pw, ph, back, 0, 0, SRCCOPY);
            SelectObject(back, bold);
            DeleteObject(bbmp as HGDIOBJ);
            DeleteDC(back);
        });
        EndPaint(hwnd, &ps);
    }
}

#[cfg(windows)]
fn draw_loupe(dc: HDC, st: &State, p: (i32, i32), dx: i32, dy: i32) {
    // SAFETY: GDI calls on a DC that the caller owns; each brush created is deleted here.
    unsafe {
        let r = loupe_rect(st, p);
        let (l, t) = (r.left - dx, r.top - dy);
        let src = loupe_source(p, (0, 0), st.snap.size);
        let colour = st
            .snap
            .colour_at(p.0 + st.snap.origin.0, p.1 + st.snap.origin.1)
            .unwrap_or([0, 0, 0]);
        let frame = CreateSolidBrush(rgb_ref([40, 40, 40]));
        let white = CreateSolidBrush(rgb_ref([255, 255, 255]));
        let black = CreateSolidBrush(rgb_ref([0, 0, 0]));
        let side = LOUPE_SIDE as i32;
        let outer = RECT {
            left: l,
            top: t,
            right: l + side,
            bottom: t + side,
        };
        FillRect(dc, &outer, frame);
        SetStretchBltMode(dc, COLORONCOLOR);
        let n = (LOUPE_CELLS * LOUPE_CELL_PX) as i32;
        StretchBlt(
            dc,
            l + 1,
            t + 1,
            n,
            n,
            st.mem,
            src.0,
            src.1,
            LOUPE_CELLS as i32,
            LOUPE_CELLS as i32,
            SRCCOPY,
        );
        // The centre cell is the pointer pixel except at an edge.
        let cx = (p.0 - src.0).clamp(0, LOUPE_CELLS as i32 - 1);
        let cy = (p.1 - src.1).clamp(0, LOUPE_CELLS as i32 - 1);
        let cp = LOUPE_CELL_PX as i32;
        let cell = RECT {
            left: l + 1 + cx * cp,
            top: t + 1 + cy * cp,
            right: l + 1 + (cx + 1) * cp,
            bottom: t + 1 + (cy + 1) * cp,
        };
        FrameRect(dc, &cell, black);
        let inner = RECT {
            left: cell.left + 1,
            top: cell.top + 1,
            right: cell.right - 1,
            bottom: cell.bottom - 1,
        };
        FrameRect(dc, &inner, white);
        // The caption under the magnifier, painted in the picked colour.
        let cap = RECT {
            left: l,
            top: t + side,
            right: l + side,
            bottom: t + side + CAPTION_H as i32,
        };
        let cap_brush = CreateSolidBrush(rgb_ref(colour));
        FillRect(dc, &cap, cap_brush);
        FrameRect(dc, &cap, frame);
        SetBkMode(dc, TRANSPARENT as _);
        SetTextColor(dc, rgb_ref(caption_ink(colour)));
        let text = wide(&hex_of(colour));
        let mut tr = cap;
        DrawTextW(
            dc,
            text.as_ptr(),
            -1,
            &mut tr,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
        DeleteObject(cap_brush as HGDIOBJ);
        DeleteObject(frame as HGDIOBJ);
        DeleteObject(white as HGDIOBJ);
        DeleteObject(black as HGDIOBJ);
    }
}

#[cfg(windows)]
unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // SAFETY: a window procedure called by the system on the window's own thread; the state
    // lives in a thread-local and is never borrowed across a call that can re-enter.
    unsafe {
        match msg {
            WM_NCCREATE => {
                let cs = lp as *const CREATESTRUCTW;
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, (*cs).lpCreateParams as isize);
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            WM_ERASEBKGND => 1,
            WM_PAINT => {
                paint(hwnd);
                0
            }
            WM_MOUSEMOVE => {
                let p = lparam_point(lp);
                let rects = STATE.with(|s| {
                    let mut s = s.borrow_mut();
                    let st = s.as_mut()?;
                    let old = st.pointer.map(|q| loupe_rect(st, q));
                    st.pointer = Some(p);
                    Some((old, loupe_rect(st, p)))
                });
                if let Some((old, new)) = rects {
                    if let Some(o) = old {
                        InvalidateRect(hwnd, &o, 0);
                    }
                    InvalidateRect(hwnd, &new, 0);
                }
                0
            }
            WM_LBUTTONUP => {
                let p = lparam_point(lp);
                let hex = STATE.with(|s| {
                    let s = s.borrow();
                    let st = s.as_ref()?;
                    st.snap
                        .colour_at(p.0 + st.snap.origin.0, p.1 + st.snap.origin.1)
                        .map(hex_of)
                });
                finish(
                    hwnd,
                    hex.map_or_else(
                        || PickResult::Failed("no pixel under the pointer".into()),
                        PickResult::Picked,
                    ),
                );
                0
            }
            WM_RBUTTONUP | WM_KILLFOCUS | WM_CAPTURECHANGED => {
                finish(hwnd, PickResult::Cancelled);
                0
            }
            WM_KEYDOWN => {
                if wp == VK_ESCAPE as usize {
                    finish(hwnd, PickResult::Cancelled);
                }
                0
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

#[cfg(windows)]
fn register_class() -> Result<(), String> {
    static ONCE: Once = Once::new();
    static OK: AtomicUsize = AtomicUsize::new(0);
    ONCE.call_once(|| {
        // SAFETY: the class name buffer is leaked so it outlives the class; the procedure is ours.
        let ok = unsafe {
            let name: &'static [u16] = Box::leak(wide("FlareEyedrop").into_boxed_slice());
            let wc = WNDCLASSW {
                style: 0,
                lpfnWndProc: Some(wnd_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: GetModuleHandleW(null()),
                hIcon: std::ptr::null_mut(),
                hCursor: LoadCursorW(std::ptr::null_mut(), IDC_CROSS),
                hbrBackground: std::ptr::null_mut(),
                lpszMenuName: null(),
                lpszClassName: name.as_ptr(),
            };
            RegisterClassW(&wc) != 0
        };
        OK.store(usize::from(ok), Ordering::SeqCst);
    });
    if OK.load(Ordering::SeqCst) == 1 {
        Ok(())
    } else {
        Err("RegisterClassW failed".into())
    }
}

/// The worker body: capture, show, loop, clean up.
#[cfg(windows)]
fn run_window() -> PickResult {
    // SAFETY: thread-scoped DPI context, restored at the end.
    let prev = unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let result = run_window_inner();
    if !prev.is_null() {
        // SAFETY: restores the value the call above returned.
        unsafe { SetThreadDpiAwarenessContext(prev) };
    }
    result
}

#[cfg(windows)]
fn run_window_inner() -> PickResult {
    if let Err(e) = register_class() {
        return PickResult::Failed(e);
    }
    let snap = match capture_virtual_screen() {
        Ok(s) => s,
        Err(e) => return PickResult::Failed(e),
    };
    let (ox, oy) = snap.origin;
    let (w, h) = (snap.size.0 as i32, snap.size.1 as i32);
    // SAFETY: the DIB section, its DC and the window are created, used and released here on
    // this thread; `bits` is the section's own w * h * 4 byte buffer.
    unsafe {
        let screen = GetDC(std::ptr::null_mut());
        let mem = CreateCompatibleDC(screen);
        ReleaseDC(std::ptr::null_mut(), screen);
        if mem.is_null() {
            return PickResult::Failed("CreateCompatibleDC failed".into());
        }
        let info = bitmap_info(w, h);
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let dib = CreateDIBSection(
            mem,
            &info,
            DIB_RGB_COLORS,
            &mut bits,
            std::ptr::null_mut(),
            0,
        );
        if dib.is_null() || bits.is_null() {
            DeleteDC(mem);
            return PickResult::Failed("CreateDIBSection failed".into());
        }
        std::ptr::copy_nonoverlapping(snap.bgra.as_ptr(), bits.cast::<u8>(), snap.bgra.len());
        let old = SelectObject(mem, dib as HGDIOBJ);
        STATE.with(|s| {
            *s.borrow_mut() = Some(State {
                snap,
                mem,
                dib,
                old,
                pointer: None,
            })
        });
        RESULT.with(|r| r.set(None));

        let class = wide("FlareEyedrop");
        let title = wide("Pick a colour");
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            class.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
            ox,
            oy,
            w,
            h,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            GetModuleHandleW(null()),
            null(),
        );
        if hwnd.is_null() {
            cleanup();
            return PickResult::Failed("CreateWindowExW failed".into());
        }
        WINDOWS_CREATED.fetch_add(1, Ordering::SeqCst);
        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd);
        SetFocus(hwnd);
        SetCapture(hwnd);
        UpdateWindow(hwnd);
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        cleanup();
    }
    RESULT.with(|r| r.take()).unwrap_or(PickResult::Cancelled)
}

/// Releases the snapshot's GDI objects.
#[cfg(windows)]
fn cleanup() {
    if let Some(st) = STATE.with(|s| s.borrow_mut().take()) {
        // SAFETY: the objects were created by `run_window_inner` on this thread and nothing
        // else holds them once the state is taken.
        unsafe {
            SelectObject(st.mem, st.old);
            DeleteObject(st.dib as HGDIOBJ);
            DeleteDC(st.mem);
        }
    }
}

/// macOS: phase 3 of the port plan (NSColorSampler). Until then a pick fails at once.
#[cfg(not(windows))]
fn run_window() -> PickResult {
    PickResult::Failed("the eyedropper is not built on this platform yet".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn the_seam_answers_without_a_window() {
        assert_eq!(
            pick_seam(Some("#aabbcc")),
            Some(PickResult::Picked("#aabbcc".into()))
        );
        assert_eq!(pick_seam(Some("cancel")), Some(PickResult::Cancelled));
        assert_eq!(pick_seam(Some("x")), None);
        assert_eq!(pick_seam(None), None);
    }

    #[test]
    fn pick_colour_answers_the_seam_without_a_window() {
        let before = WINDOWS_CREATED.load(Ordering::SeqCst);
        let (tx, rx) = mpsc::channel();
        pick_colour(Some("#aabbcc".into()), move |r| {
            let _ = tx.send(r);
        });
        let got = rx
            .recv_timeout(Duration::from_secs(1))
            .expect("the seam answers");
        assert_eq!(got, PickResult::Picked("#aabbcc".into()));
        assert_eq!(WINDOWS_CREATED.load(Ordering::SeqCst), before);
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "needs a desktop"]
    fn capture_has_the_virtual_screen_size() {
        let s = capture_virtual_screen().expect("capture");
        // SAFETY: plain metric reads.
        let (w, h) = unsafe {
            (
                GetSystemMetrics(SM_CXVIRTUALSCREEN),
                GetSystemMetrics(SM_CYVIRTUALSCREEN),
            )
        };
        assert_eq!(s.size, (w as u32, h as u32));
        assert_eq!(s.bgra.len(), (w as usize) * (h as usize) * 4);
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "run by hand: shows a window; see the module header"]
    fn the_window_picks_a_pixel() {
        let (tx, rx) = mpsc::channel();
        pick_colour(None, move |r| {
            let _ = tx.send(r);
        });
        let got = rx.recv_timeout(Duration::from_secs(120)).expect("a result");
        println!("{got:?}");
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "needs a desktop"]
    fn no_gdi_object_leaks() {
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        use windows_sys::Win32::System::Threading::{GR_GDIOBJECTS, GetGuiResources};
        // SAFETY: reads a count for the current process.
        let count = || unsafe { GetGuiResources(GetCurrentProcess(), GR_GDIOBJECTS) };
        let _ = capture_virtual_screen();
        let before = count();
        for _ in 0..5 {
            capture_virtual_screen().expect("capture");
        }
        assert_eq!(count(), before);
    }
}

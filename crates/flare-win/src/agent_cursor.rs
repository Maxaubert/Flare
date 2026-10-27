//! The agent's cursor over the web panel: an orange arrow with a white rim and a name tag, a ripple
//! on each press. Flare draws it in a window of its own, never in the page (docs/surfaces/decisions.md):
//! the page cannot see, move or hide it and it is in no screenshot the agent takes.
//!
//! Part A is pure pixels: the arrow as a polygon with analytic anti-aliasing (the taskbar disc's
//! approach), the tag's text as a GDI coverage mask (as `taskbar::text_mask`), composed premultiplied
//! BGRA. Part B is the window: a small layered popup owned by Flare's main window, transparent to the
//! mouse, never activated, with no taskbar button. A trait stands in for it in the tests and `--e2e`.
//!
//! The border round the page while the agent uses it (Max, 2026-10-27) is a second such window the size
//! of the page: the browser is a child window Slint cannot draw over, and a layered popup shows above it
//! while every click goes through to the page.

/// The default look's accent orange as (r, g, b), fixed: the cursor is the same in every theme.
const ORANGE: (u8, u8, u8) = (0xfe, 0x8f, 0x34);
/// The tag's text, near black.
const INK: (u8, u8, u8) = (0x1a, 0x1a, 0x1a);
/// The arrow in units of a 24 unit box drawn 22 px wide at 100 %, from its tip (mockup A's path).
const ARROW: [(f64, f64); 4] = [(0.0, 0.0), (15.0, 9.2), (8.4, 10.7), (5.0, 17.0)];
/// Pixels per arrow unit at 100 %.
const UNIT: f64 = 22.0 / 24.0;
/// The white rim, centred on the arrow's edge, in pixels at 100 %.
const RIM_PX: f64 = 1.6;
/// The ripple's disc at full size (scale 1), in pixels across at 100 %; it grows from 0.3 to 1.4 of it.
const RIPPLE_PX: f64 = 34.0;
/// The ripple's opacity when it starts (the mockup's orange at 0.55, faded from 0.9).
const RIPPLE_ALPHA: f64 = 0.5;
/// Where the tag's top left sits from the tip, its height, its side padding and its text, at 100 %.
const TAG_AT: (f64, f64) = (14.0, 14.0);
const TAG_H: f64 = 19.0;
const TAG_PAD: f64 = 7.0;
const TAG_FONT: f64 = 11.0;
/// A name longer than this is cut (the tag stays a tag).
const TAG_CHARS: usize = 24;
/// The border round the page: the cursor's orange at this opacity (light orange over the page), this
/// many pixels wide at 100 %.
const BORDER_ALPHA: f64 = 0.55;
const BORDER_PX: f64 = 2.0;
/// The largest border drawn, in pixels each way (a bigger page has none rather than a huge bitmap).
const BORDER_MAX: u32 = 16_384;

/// A rendered cursor: premultiplied BGRA, top-down, `w * h * 4` bytes, with the tip at `tip`.
#[derive(Clone, Debug, PartialEq)]
pub struct CursorBitmap {
    pub w: u32,
    pub h: u32,
    pub bgra: Vec<u8>,
    pub tip: (i32, i32),
}

/// The scale as used for drawing: finite, above 0, at most 4; else 1.
fn clean_scale(scale: f64) -> f64 {
    if scale.is_finite() && scale > 0.0 {
        scale.min(4.0)
    } else {
        1.0
    }
}

/// Signed distance from (x, y) to the polygon's edge: negative inside.
fn polygon_distance(x: f64, y: f64, pts: &[(f64, f64)]) -> f64 {
    let mut best = f64::MAX;
    let mut inside = false;
    for i in 0..pts.len() {
        let (ax, ay) = pts[i];
        let (bx, by) = pts[(i + 1) % pts.len()];
        let (ex, ey) = (bx - ax, by - ay);
        let t = (((x - ax) * ex + (y - ay) * ey) / (ex * ex + ey * ey)).clamp(0.0, 1.0);
        let (dx, dy) = (x - ax - ex * t, y - ay - ey * t);
        best = best.min((dx * dx + dy * dy).sqrt());
        // Even-odd crossing test on the edge (a, b).
        if (ay > y) != (by > y) && x < ax + (y - ay) / (by - ay) * ex {
            inside = !inside;
        }
    }
    if inside { -best } else { best }
}

/// The arrow's coverage at `scale`: the whole shape with its rim (`outer`) and the orange inside the
/// rim (`fill`), each 0..1, on a `w * h` box whose tip is at `tip`.
pub struct ArrowCoverage {
    pub w: u32,
    pub h: u32,
    pub tip: (u32, u32),
    pub outer: Vec<f32>,
    pub fill: Vec<f32>,
}

/// Analytic anti-aliasing, as the taskbar disc: coverage = clamp(edge - distance + 0.5, 0, 1), with the
/// rim's outer edge `RIM_PX / 2` outside the path and the fill's the same inside.
pub fn arrow_coverage(scale: f64) -> ArrowCoverage {
    let s = clean_scale(scale);
    let k = UNIT * s;
    let half = RIM_PX * s / 2.0;
    let pad = half.ceil() as u32 + 1;
    let pts: Vec<(f64, f64)> = ARROW
        .iter()
        .map(|(x, y)| (x * k + f64::from(pad), y * k + f64::from(pad)))
        .collect();
    let max_x = ARROW.iter().map(|p| p.0).fold(0.0, f64::max) * k;
    let max_y = ARROW.iter().map(|p| p.1).fold(0.0, f64::max) * k;
    let w = max_x.ceil() as u32 + 2 * pad;
    let h = max_y.ceil() as u32 + 2 * pad;
    let mut outer = Vec::with_capacity((w * h) as usize);
    let mut fill = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        for x in 0..w {
            let d = polygon_distance(f64::from(x) + 0.5, f64::from(y) + 0.5, &pts);
            outer.push((half - d + 0.5).clamp(0.0, 1.0) as f32);
            fill.push((-half - d + 0.5).clamp(0.0, 1.0) as f32);
        }
    }
    ArrowCoverage {
        w,
        h,
        tip: (pad, pad),
        outer,
        fill,
    }
}

/// Coverage of a pill (a rectangle with half-height round ends) at the pixel's centre.
fn pill_coverage(x: f64, y: f64, w: f64, h: f64) -> f64 {
    let r = h / 2.0;
    let cx = x.clamp(r, (w - r).max(r));
    let d = ((x - cx).powi(2) + (y - r).powi(2)).sqrt();
    (r - d + 0.5).clamp(0.0, 1.0)
}

/// The tag's words: the agent's name, cut to [`TAG_CHARS`].
fn tag_text(label: &str) -> String {
    label.trim().chars().take(TAG_CHARS).collect()
}

/// Premultiplied `colour` at `a` (0..1) over premultiplied `under` (BGRA).
fn over(under: [f32; 4], colour: (u8, u8, u8), a: f32) -> [f32; 4] {
    let k = 1.0 - a;
    [
        f32::from(colour.2) * a + under[0] * k,
        f32::from(colour.1) * a + under[1] * k,
        f32::from(colour.0) * a + under[2] * k,
        255.0 * a + under[3] * k,
    ]
}

/// The arrow and its tag drawn once for a scale and a name; [`CursorArt::frame`] adds the ripples.
pub struct CursorArt {
    w: u32,
    h: u32,
    tip: (i32, i32),
    scale: f64,
    /// Premultiplied BGRA, 0..255 as floats.
    base: Vec<[f32; 4]>,
}

impl CursorArt {
    pub fn new(scale: f64, label: &str) -> Self {
        let s = clean_scale(scale);
        // Room up and left of the tip for the ripple at its largest.
        let m = (RIPPLE_PX * 1.4 / 2.0 * s).ceil() as i32 + 1;
        let arrow = arrow_coverage(s);
        let text = tag_text(label);
        let tag_h = (TAG_H * s).round().max(1.0) as u32;
        let (text_w, mask) = label_mask((TAG_FONT * s).round().max(1.0) as u32, tag_h, &text);
        let pad = (TAG_PAD * s).round() as u32;
        let tag_w = if text.is_empty() { 0 } else { text_w + 2 * pad };
        let tag_x = m + (TAG_AT.0 * s).round() as i32;
        let tag_y = m + (TAG_AT.1 * s).round() as i32;
        let ax = m - arrow.tip.0 as i32;
        let ay = m - arrow.tip.1 as i32;
        let w = (2 * m + 1)
            .max(tag_x + tag_w as i32 + 1)
            .max(ax + arrow.w as i32) as u32;
        let h = (2 * m + 1)
            .max(tag_y + tag_h as i32 + 1)
            .max(ay + arrow.h as i32) as u32;
        let mut base = vec![[0.0f32; 4]; (w * h) as usize];
        // The tag first (under the arrow): an orange pill with the name in ink.
        if tag_w > 0 {
            for ty in 0..tag_h {
                for tx in 0..tag_w {
                    let a = pill_coverage(
                        f64::from(tx) + 0.5,
                        f64::from(ty) + 0.5,
                        f64::from(tag_w),
                        f64::from(tag_h),
                    ) as f32;
                    if a <= 0.0 {
                        continue;
                    }
                    let i = ((tag_y as u32 + ty) * w + tag_x as u32 + tx) as usize;
                    let mut p = over(base[i], ORANGE, a);
                    if tx >= pad && tx - pad < text_w {
                        let ink = f32::from(mask[(ty * text_w + tx - pad) as usize]) / 255.0;
                        p = over(p, INK, ink * a);
                    }
                    base[i] = p;
                }
            }
        }
        // The arrow: white where the rim is, orange inside it.
        for y in 0..arrow.h {
            for x in 0..arrow.w {
                let j = (y * arrow.w + x) as usize;
                let (o, f) = (arrow.outer[j], arrow.fill[j]);
                if o <= 0.0 {
                    continue;
                }
                let i = ((ay as u32 + y) * w + ax as u32 + x) as usize;
                // The rim's white over what is below, then the orange over the rim.
                let p = over(base[i], (255, 255, 255), o);
                base[i] = over(p, ORANGE, f);
            }
        }
        Self {
            w,
            h,
            tip: (m, m),
            scale: s,
            base,
        }
    }

    /// The bitmap with a ripple for each progress in `ripples` (0..1) under the arrow.
    pub fn frame(&self, ripples: &[f32]) -> CursorBitmap {
        let (cx, cy) = (f64::from(self.tip.0), f64::from(self.tip.1));
        let discs: Vec<(f64, f64)> = ripples
            .iter()
            .map(|t| {
                let t = f64::from(t.clamp(0.0, 1.0));
                let ease = 1.0 - (1.0 - t).powi(3);
                let r = RIPPLE_PX / 2.0 * self.scale * (0.3 + 1.1 * ease);
                (r, RIPPLE_ALPHA * (1.0 - t))
            })
            .collect();
        let mut bgra = Vec::with_capacity(self.base.len() * 4);
        for (i, p) in self.base.iter().enumerate() {
            let mut under = [0.0f32; 4];
            if !discs.is_empty() {
                let x = (i as u32 % self.w) as f64 + 0.5 - cx;
                let y = (i as u32 / self.w) as f64 + 0.5 - cy;
                let d = (x * x + y * y).sqrt();
                for (r, a) in &discs {
                    let c = (r - d + 0.5).clamp(0.0, 1.0) * a;
                    under = over(under, ORANGE, c as f32);
                }
            }
            // The art over the ripples.
            let k = 1.0 - p[3] / 255.0;
            for ch in 0..4 {
                bgra.push((p[ch] + under[ch] * k).round().clamp(0.0, 255.0) as u8);
            }
        }
        CursorBitmap {
            w: self.w,
            h: self.h,
            bgra,
            tip: self.tip,
        }
    }
}

/// The cursor at `scale` with its tag naming `label` and a ripple for each progress in `ripples`.
pub fn compose_cursor(scale: f64, label: &str, ripples: &[f32]) -> CursorBitmap {
    CursorArt::new(scale, label).frame(ripples)
}

/// The border for a page of `w * h` physical pixels at `scale`: a ring [`BORDER_PX`] wide just inside
/// the page's edge in the cursor's orange at [`BORDER_ALPHA`], clear inside, its top left at `tip` (0, 0).
/// The page's corners are square (the browser's window is a plain rectangle under the panel's head), so
/// the ring's are too. Empty for an empty or too large page.
pub fn border_bitmap(w: u32, h: u32, scale: f64) -> CursorBitmap {
    if w == 0 || h == 0 || w > BORDER_MAX || h > BORDER_MAX {
        return CursorBitmap {
            w: 0,
            h: 0,
            bgra: Vec::new(),
            tip: (0, 0),
        };
    }
    let t = ((BORDER_PX * clean_scale(scale)).round() as u32).clamp(1, w.min(h).div_ceil(2));
    let a = BORDER_ALPHA;
    let px = [
        (f64::from(ORANGE.2) * a).round() as u8,
        (f64::from(ORANGE.1) * a).round() as u8,
        (f64::from(ORANGE.0) * a).round() as u8,
        (255.0 * a).round() as u8,
    ];
    let mut bgra = vec![0u8; (w * h * 4) as usize];
    for (y, row) in bgra.chunks_exact_mut((w * 4) as usize).enumerate() {
        let y = y as u32;
        let edge_row = y < t || y >= h - t;
        for (x, p) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let x = x as u32;
            if edge_row || x < t || x >= w - t {
                p.copy_from_slice(&px);
            }
        }
    }
    CursorBitmap {
        w,
        h,
        bgra,
        tip: (0, 0),
    }
}

// ---------------------------------------------------------------- Part B: GDI and the window

#[cfg(windows)]
mod win {
    use super::CursorBitmap;
    use std::ffi::c_void;
    use std::ptr::{null, null_mut};
    use std::sync::Once;
    use std::sync::atomic::{AtomicBool, Ordering};
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::{
        AC_SRC_ALPHA, AC_SRC_OVER, ANTIALIASED_QUALITY, BI_RGB, BITMAPINFO, BITMAPINFOHEADER,
        BLENDFUNCTION, CLIP_DEFAULT_PRECIS, CreateCompatibleDC, CreateDIBSection, CreateFontW,
        DEFAULT_CHARSET, DIB_RGB_COLORS, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DeleteDC,
        DeleteObject, DrawTextW, GetTextExtentPoint32W, OUT_DEFAULT_PRECIS, SelectObject,
        SetBkMode, SetTextColor, TRANSPARENT,
    };
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, HTTRANSPARENT, MA_NOACTIVATE,
        RegisterClassW, SW_HIDE, SW_SHOWNOACTIVATE, ShowWindow, ULW_ALPHA, UpdateLayeredWindow,
        WM_MOUSEACTIVATE, WM_NCHITTEST, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
        WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT, WS_POPUP,
    };

    const CLASS: &str = "FlareAgentCursor";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    fn dib_info(w: u32, h: u32) -> BITMAPINFO {
        BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                biHeight: -(h as i32), // top-down
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

    /// The tag's text, white on black, read back as 0..255 coverage: (width, `width * h` bytes).
    pub fn label_mask(font_px: u32, h: u32, text: &str) -> (u32, Vec<u8>) {
        if text.is_empty() || h == 0 {
            return (0, Vec::new());
        }
        let face = wide("Segoe UI");
        let w16: Vec<u16> = text.encode_utf16().collect();
        // SAFETY: a memory DC and a font made here and released below; `face` outlives the call.
        let (hdc, font) = unsafe {
            let hdc = CreateCompatibleDC(null_mut());
            let font = CreateFontW(
                -(font_px as i32),
                0,
                0,
                0,
                600,
                0,
                0,
                0,
                DEFAULT_CHARSET as u32,
                OUT_DEFAULT_PRECIS as u32,
                CLIP_DEFAULT_PRECIS as u32,
                ANTIALIASED_QUALITY as u32,
                0,
                face.as_ptr(),
            );
            (hdc, font)
        };
        let mut ext = SIZE { cx: 0, cy: 0 };
        // SAFETY: the font is ours and selected into our DC; `w16` and `ext` live across the call.
        let old_font = unsafe {
            let old = SelectObject(hdc, font);
            GetTextExtentPoint32W(hdc, w16.as_ptr(), w16.len() as i32, &mut ext);
            old
        };
        let w = (ext.cx.max(0) as u32 + 1).min(4096);
        let n = (w * h) as usize;
        let info = dib_info(w, h);
        let mut bits: *mut c_void = null_mut();
        // SAFETY: a 32-bit DIB section on our DC; GDI fills `bits`.
        let bmp = unsafe { CreateDIBSection(hdc, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0) };
        let mut out = vec![0u8; n];
        if !bmp.is_null() && !bits.is_null() {
            let mut w16 = w16.clone();
            let mut rc = RECT {
                left: 0,
                top: 0,
                right: w as i32,
                bottom: h as i32,
            };
            // SAFETY: our bitmap into our DC; `bits` holds n * 4 writable bytes; `w16` and `rc` live
            // across the call.
            unsafe {
                let old_bmp = SelectObject(hdc, bmp);
                std::ptr::write_bytes(bits as *mut u8, 0, n * 4);
                SetBkMode(hdc, TRANSPARENT as i32);
                SetTextColor(hdc, 0x00FF_FFFF);
                DrawTextW(
                    hdc,
                    w16.as_mut_ptr(),
                    w16.len() as i32,
                    &mut rc,
                    DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
                );
                let src = std::slice::from_raw_parts(bits as *const u8, n * 4);
                // BGRA: the red channel is byte 2.
                for (o, p) in out.iter_mut().zip(src.as_chunks::<4>().0.iter()) {
                    *o = p[2];
                }
                SelectObject(hdc, old_bmp);
            }
        }
        // SAFETY: release everything made here, after the last read of `bits`.
        unsafe {
            SelectObject(hdc, old_font);
            if !bmp.is_null() {
                DeleteObject(bmp);
            }
            DeleteObject(font);
            DeleteDC(hdc);
        }
        (w, out)
    }

    unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        match msg {
            // Never the mouse's, never activated.
            WM_NCHITTEST => HTTRANSPARENT as LRESULT,
            WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
            // SAFETY: the default handling of the window's own message.
            _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
        }
    }

    fn register() -> bool {
        static ONCE: Once = Once::new();
        static OK: AtomicBool = AtomicBool::new(false);
        ONCE.call_once(|| {
            // SAFETY: the class name is leaked so it outlives the class; the procedure is ours.
            let ok = unsafe {
                let name: &'static [u16] = Box::leak(wide(CLASS).into_boxed_slice());
                let wc = WNDCLASSW {
                    style: 0,
                    lpfnWndProc: Some(proc),
                    cbClsExtra: 0,
                    cbWndExtra: 0,
                    hInstance: GetModuleHandleW(null()),
                    hIcon: null_mut(),
                    hCursor: null_mut(),
                    hbrBackground: null_mut(),
                    lpszMenuName: null(),
                    lpszClassName: name.as_ptr(),
                };
                RegisterClassW(&wc) != 0
            };
            OK.store(ok, Ordering::SeqCst);
        });
        OK.load(Ordering::SeqCst)
    }

    /// The real window: made the first time the cursor is shown, destroyed with this value.
    pub struct CursorWindow {
        owner: isize,
        hwnd: isize,
        visible: bool,
    }

    impl CursorWindow {
        pub fn new(owner: isize) -> Self {
            Self {
                owner,
                hwnd: 0,
                visible: false,
            }
        }

        fn window(&mut self) -> Option<HWND> {
            if self.hwnd == 0 && self.owner != 0 && register() {
                let class = wide(CLASS);
                // SAFETY: a popup owned by Flare's own window, of the class registered above.
                let h = unsafe {
                    CreateWindowExW(
                        WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                        class.as_ptr(),
                        class.as_ptr(),
                        WS_POPUP,
                        0,
                        0,
                        1,
                        1,
                        self.owner as HWND,
                        null_mut(),
                        GetModuleHandleW(null()),
                        null(),
                    )
                };
                if h.is_null() {
                    log::warn!("agent cursor: the window was not made");
                    // Not asked again: the cursor is a nicety, not worth a retry per frame.
                    self.owner = 0;
                } else {
                    self.hwnd = h as isize;
                }
            }
            (self.hwnd != 0).then_some(self.hwnd as HWND)
        }
    }

    impl super::CursorShell for CursorWindow {
        fn show_at(&mut self, x: i32, y: i32, bmp: &CursorBitmap, alpha: u8) -> bool {
            if bmp.w == 0 || bmp.h == 0 || bmp.bgra.len() != (bmp.w * bmp.h * 4) as usize {
                return false;
            }
            let Some(hwnd) = self.window() else {
                return false;
            };
            let info = dib_info(bmp.w, bmp.h);
            let mut bits: *mut c_void = null_mut();
            // SAFETY: a memory DC and a DIB section the size of the bitmap, released below.
            let (hdc, dib) = unsafe {
                let hdc = CreateCompatibleDC(null_mut());
                let dib = CreateDIBSection(hdc, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
                (hdc, dib)
            };
            let mut ok = false;
            if !dib.is_null() && !bits.is_null() {
                let dst = POINT {
                    x: x - bmp.tip.0,
                    y: y - bmp.tip.1,
                };
                let size = SIZE {
                    cx: bmp.w as i32,
                    cy: bmp.h as i32,
                };
                let src = POINT { x: 0, y: 0 };
                let blend = BLENDFUNCTION {
                    BlendOp: AC_SRC_OVER as u8,
                    BlendFlags: 0,
                    SourceConstantAlpha: alpha,
                    AlphaFormat: AC_SRC_ALPHA as u8,
                };
                // SAFETY: `bits` holds w * h * 4 writable bytes, exactly the bitmap's length; the window is
                // ours and layered; every pointer lives across the call.
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        bmp.bgra.as_ptr(),
                        bits as *mut u8,
                        bmp.bgra.len(),
                    );
                    let old = SelectObject(hdc, dib);
                    ok = UpdateLayeredWindow(
                        hwnd,
                        null_mut(),
                        &dst,
                        &size,
                        hdc,
                        &src,
                        0,
                        &blend,
                        ULW_ALPHA,
                    ) != 0;
                    SelectObject(hdc, old);
                }
            }
            // SAFETY: made above.
            unsafe {
                if !dib.is_null() {
                    DeleteObject(dib);
                }
                DeleteDC(hdc);
            }
            if ok && !self.visible {
                // SAFETY: our own window; shown without taking the focus.
                unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
                self.visible = true;
            }
            ok
        }

        fn set_alpha(&mut self, alpha: u8) -> bool {
            if !self.visible || self.hwnd == 0 {
                return false;
            }
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: alpha,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            // SAFETY: our own layered window; with no source DC it keeps its picture and place and takes
            // only the new blend, which lives across the call.
            unsafe {
                UpdateLayeredWindow(
                    self.hwnd as HWND,
                    null_mut(),
                    null(),
                    null(),
                    null_mut(),
                    null(),
                    0,
                    &blend,
                    ULW_ALPHA,
                ) != 0
            }
        }

        fn hide(&mut self) {
            if self.visible && self.hwnd != 0 {
                // SAFETY: our own window.
                unsafe { ShowWindow(self.hwnd as HWND, SW_HIDE) };
            }
            self.visible = false;
        }
    }

    impl Drop for CursorWindow {
        fn drop(&mut self) {
            if self.hwnd != 0 {
                // SAFETY: our own window, on the thread that made it.
                unsafe { DestroyWindow(self.hwnd as HWND) };
                self.hwnd = 0;
            }
        }
    }
}

#[cfg(windows)]
pub use win::CursorWindow;

/// The tag's text rasterised by GDI: (width, `width * h` bytes of 0..255 coverage).
#[cfg(windows)]
fn label_mask(font_px: u32, h: u32, text: &str) -> (u32, Vec<u8>) {
    win::label_mask(font_px, h, text)
}

/// Non-Windows builds have no GDI: a tag of about the right width with no ink.
#[cfg(not(windows))]
fn label_mask(font_px: u32, h: u32, text: &str) -> (u32, Vec<u8>) {
    let w = (text.chars().count() as u32 * font_px * 3 / 5).max(1);
    (w, vec![0; (w * h) as usize])
}

/// The window side of the cursor, so a test (and `--e2e`) can stand in for it.
pub trait CursorShell {
    /// Shows `bmp` with its tip at (x, y) in screen pixels, at `alpha` (255 opaque) over all. True when
    /// it was shown.
    fn show_at(&mut self, x: i32, y: i32, bmp: &CursorBitmap, alpha: u8) -> bool;
    /// The picture last shown, where it is, at a new `alpha` (a fade), with no new bitmap. False when
    /// that cannot be done (nothing shown); the caller shows it again whole.
    fn set_alpha(&mut self, alpha: u8) -> bool {
        let _ = alpha;
        false
    }
    /// Gone at once.
    fn hide(&mut self);
}

/// What the cursor's shell was last told on this thread: the e2e probe and the tests read it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CursorRecord {
    pub shown: bool,
    /// The tip, in screen pixels.
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub alpha: u8,
    /// How many times it was drawn.
    pub draws: u64,
}

type Record = std::thread::LocalKey<std::cell::RefCell<CursorRecord>>;

std::thread_local! {
    static RECORD: std::cell::RefCell<CursorRecord> = std::cell::RefCell::new(CursorRecord::default());
    static BORDER_RECORD: std::cell::RefCell<CursorRecord> =
        std::cell::RefCell::new(CursorRecord::default());
}

/// What a [`RecordCursor`] on this thread was last told.
pub fn recorded() -> CursorRecord {
    RECORD.with(|r| r.borrow().clone())
}

/// What a [`RecordBorder`] on this thread was last told (`x`, `y` its top left).
pub fn recorded_border() -> CursorRecord {
    BORDER_RECORD.with(|r| r.borrow().clone())
}

fn record_show(rec: &'static Record, x: i32, y: i32, bmp: &CursorBitmap, alpha: u8) -> bool {
    rec.with(|r| {
        let mut r = r.borrow_mut();
        r.shown = true;
        (r.x, r.y, r.w, r.h, r.alpha) = (x, y, bmp.w, bmp.h, alpha);
        r.draws += 1;
    });
    true
}

fn record_alpha(rec: &'static Record, alpha: u8) -> bool {
    rec.with(|r| {
        let mut r = r.borrow_mut();
        if r.shown {
            r.alpha = alpha;
        }
        r.shown
    })
}

/// A shell that keeps what it is told and shows it nowhere: tests and `--e2e` runs never make the window.
pub struct RecordCursor;

impl CursorShell for RecordCursor {
    fn show_at(&mut self, x: i32, y: i32, bmp: &CursorBitmap, alpha: u8) -> bool {
        record_show(&RECORD, x, y, bmp, alpha)
    }

    fn set_alpha(&mut self, alpha: u8) -> bool {
        record_alpha(&RECORD, alpha)
    }

    fn hide(&mut self) {
        RECORD.with(|r| r.borrow_mut().shown = false);
    }
}

/// The border's stand-in, as [`RecordCursor`] is the arrow's.
pub struct RecordBorder;

impl CursorShell for RecordBorder {
    fn show_at(&mut self, x: i32, y: i32, bmp: &CursorBitmap, alpha: u8) -> bool {
        record_show(&BORDER_RECORD, x, y, bmp, alpha)
    }

    fn set_alpha(&mut self, alpha: u8) -> bool {
        record_alpha(&BORDER_RECORD, alpha)
    }

    fn hide(&mut self) {
        BORDER_RECORD.with(|r| r.borrow_mut().shown = false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCALES: [f64; 8] = [1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 3.0];

    fn px(b: &CursorBitmap, x: i32, y: i32) -> [u8; 4] {
        let i = ((y as u32 * b.w + x as u32) * 4) as usize;
        [b.bgra[i], b.bgra[i + 1], b.bgra[i + 2], b.bgra[i + 3]]
    }

    /// The pixel `(ux, uy)` arrow units from the tip.
    fn at_units(b: &CursorBitmap, scale: f64, ux: f64, uy: f64) -> [u8; 4] {
        let k = UNIT * scale;
        px(
            b,
            b.tip.0 + (ux * k).floor() as i32,
            b.tip.1 + (uy * k).floor() as i32,
        )
    }

    #[test]
    fn inside_the_arrow_is_opaque_orange_and_up_left_of_the_tip_is_clear() {
        for s in SCALES {
            let b = compose_cursor(s, "", &[]);
            assert_eq!(b.bgra.len(), (b.w * b.h * 4) as usize);
            assert_eq!(
                at_units(&b, s, 5.0, 8.0),
                [ORANGE.2, ORANGE.1, ORANGE.0, 255],
                "at {s}"
            );
            assert!(px(&b, b.tip.0, b.tip.1)[3] > 0, "the tip is drawn at {s}");
            let off = (3.0 * s).ceil() as i32;
            assert_eq!(px(&b, b.tip.0 - off, b.tip.1 - off), [0; 4], "at {s}");
        }
    }

    #[test]
    fn the_rim_is_white_and_the_edge_is_anti_aliased() {
        let s = 3.0;
        let b = compose_cursor(s, "", &[]);
        // The middle of the top edge, on the path: the rim's centre.
        let p = at_units(&b, s, 7.5, 4.6);
        assert!(
            p[3] == 255 && p[0] > 230 && p[1] > 230 && p[2] > 230,
            "{p:?}"
        );
        let a = arrow_coverage(s);
        assert!(a.outer.iter().any(|c| *c > 0.0 && *c < 1.0));
        assert!(a.fill.iter().zip(&a.outer).all(|(f, o)| f <= o));
        // A pixel's colour never runs past its alpha (premultiplied).
        assert!(
            b.bgra
                .as_chunks::<4>()
                .0
                .iter()
                .all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3])
        );
    }

    #[test]
    fn a_ripple_is_orange_round_the_tip_and_fades_as_it_grows() {
        for s in SCALES {
            let off = (3.0 * s).round() as i32;
            let alpha = |t: f32| {
                let b = compose_cursor(s, "", &[t]);
                px(&b, b.tip.0 - off, b.tip.1 - off)[3]
            };
            assert_eq!(alpha(1.0), 0, "a finished ripple is gone at {s}");
            let (early, late) = (alpha(0.2), alpha(0.8));
            assert!(early > late && late > 0, "{early} {late} at {s}");
            let b = compose_cursor(s, "", &[0.2]);
            let p = px(&b, b.tip.0 - off, b.tip.1 - off);
            // Orange: red over green over blue.
            assert!(p[2] > p[1] && p[1] > p[0], "{p:?}");
        }
        // The ripple is under the arrow: the arrow's inside stays the same.
        let b = compose_cursor(1.0, "", &[0.1]);
        assert_eq!(
            at_units(&b, 1.0, 5.0, 8.0),
            [ORANGE.2, ORANGE.1, ORANGE.0, 255]
        );
    }

    #[test]
    fn every_scale_fits_the_largest_ripple_and_the_tag() {
        for s in SCALES {
            let b = compose_cursor(s, "Claude", &[0.999]);
            let r = (RIPPLE_PX * 1.4 / 2.0 * s).ceil() as i32;
            assert!(b.tip.0 >= r && b.tip.1 >= r, "at {s}");
            assert!(
                b.w as i32 > b.tip.0 + r && b.h as i32 > b.tip.1 + r,
                "at {s}"
            );
        }
        assert_eq!(clean_scale(f64::NAN), 1.0);
        assert_eq!(clean_scale(9.0), 4.0);
    }

    #[test]
    fn a_long_name_is_cut() {
        assert_eq!(tag_text("  Claude "), "Claude");
        assert_eq!(tag_text(&"x".repeat(100)).len(), TAG_CHARS);
    }

    #[cfg(windows)]
    #[test]
    fn the_tag_is_an_orange_pill_with_the_name_readable_on_it() {
        for s in [1.0, 1.5, 2.25, 3.0] {
            let b = compose_cursor(s, "Claude", &[]);
            let none = compose_cursor(s, "", &[]);
            assert!(b.w > none.w, "the tag widens the picture at {s}");
            // The tag's rectangle in the picture.
            let x0 = b.tip.0 + (TAG_AT.0 * s).round() as i32;
            let y0 = b.tip.1 + (TAG_AT.1 * s).round() as i32;
            let th = (TAG_H * s).round() as i32;
            let mid = y0 + th / 2;
            // Its left end is orange, the arrow well clear of it.
            let pad = (TAG_PAD * s).round() as i32;
            let mut dark = Vec::new();
            for x in x0..b.w as i32 {
                let p = px(&b, x, mid);
                if p[3] == 255 && p[2] < 0x80 {
                    dark.push(x);
                }
            }
            let ink_w = dark.last().zip(dark.first()).map_or(0, |(l, f)| l - f + 1);
            // Six letters of 11 px Segoe UI semibold are about 36 px wide.
            assert!(
                f64::from(ink_w) > 25.0 * s && f64::from(ink_w) < 55.0 * s,
                "ink {ink_w} px at {s}"
            );
            assert!(dark.first().is_some_and(|f| *f >= x0 + pad - 1), "at {s}");
            let edge = px(&b, x0 + pad / 2, mid);
            assert_eq!(edge, [ORANGE.2, ORANGE.1, ORANGE.0, 255], "at {s}");
        }
    }

    #[test]
    fn the_recording_shell_keeps_what_it_was_told() {
        let mut r = RecordCursor;
        assert!(!recorded().shown);
        let b = compose_cursor(1.0, "Claude", &[]);
        assert!(r.show_at(100, 200, &b, 255));
        let rec = recorded();
        assert!(rec.shown);
        assert_eq!(
            (rec.x, rec.y, rec.w, rec.alpha, rec.draws),
            (100, 200, b.w, 255, 1)
        );
        r.hide();
        assert!(!recorded().shown);
    }

    #[test]
    fn the_border_is_a_light_orange_ring_inside_the_page_and_clear_within() {
        for (s, t) in [(1.0, 2), (1.25, 3), (1.5, 3), (2.0, 4), (3.0, 6)] {
            let b = border_bitmap(300, 200, s);
            assert_eq!((b.w, b.h, b.tip), (300, 200, (0, 0)));
            assert_eq!(b.bgra.len(), 300 * 200 * 4);
            let ring = px(&b, 0, 0);
            // The cursor's orange, premultiplied at about half: light, and orange (red over green over blue).
            assert_eq!(ring[3], (255.0 * BORDER_ALPHA).round() as u8);
            assert!(ring[2] > ring[1] && ring[1] > ring[0], "{ring:?}");
            assert!(ring[0] <= ring[3] && ring[1] <= ring[3] && ring[2] <= ring[3]);
            for (x, y) in [
                (t - 1, 100),
                (299 - (t - 1), 100),
                (150, t - 1),
                (150, 199 - (t - 1)),
            ] {
                assert_eq!(px(&b, x, y), ring, "on the ring at {s}: ({x}, {y})");
            }
            for (x, y) in [
                (t, 100),
                (299 - t, 100),
                (150, t),
                (150, 199 - t),
                (150, 100),
            ] {
                assert_eq!(px(&b, x, y), [0; 4], "clear inside at {s}: ({x}, {y})");
            }
        }
        assert_eq!(border_bitmap(0, 10, 1.0).w, 0);
        assert_eq!(border_bitmap(BORDER_MAX + 1, 10, 1.0).w, 0);
        // A page thinner than the ring is all ring.
        let b = border_bitmap(3, 3, 3.0);
        assert!(b.bgra.as_chunks::<4>().0.iter().all(|p| p[3] > 0));
    }

    #[test]
    fn the_border_s_stand_in_keeps_its_own_record_and_a_fade_changes_only_the_alpha() {
        let mut r = RecordBorder;
        assert!(!r.set_alpha(10), "nothing shown");
        let b = border_bitmap(40, 30, 1.0);
        assert!(r.show_at(5, 6, &b, 255));
        assert!(!recorded().shown, "not the arrow's");
        assert!(r.set_alpha(128));
        let rec = recorded_border();
        assert_eq!(
            (rec.x, rec.y, rec.w, rec.h, rec.alpha, rec.draws),
            (5, 6, 40, 30, 128, 1)
        );
        r.hide();
        assert!(!recorded_border().shown);
    }

    #[cfg(windows)]
    #[test]
    fn the_window_is_not_made_without_an_owner() {
        let mut w = CursorWindow::new(0);
        assert!(!w.show_at(10, 10, &compose_cursor(1.0, "Claude", &[]), 255));
        assert!(!w.set_alpha(100));
        w.hide();
    }
}

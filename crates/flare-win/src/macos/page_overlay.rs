//! The agent's cursor, the glow round the page and the agent's arrow on macOS (rule 1155): the Mac meaning of the
//! Windows layered popups in `agent_cursor` (`CursorWindow`, `GlowWindow`, `ArrowWindow`).
//!
//! On Windows the page is a child window Slint cannot draw over, so each picture is a small layered popup owned by
//! Flare's window. On the Mac the page is CEF's picture drawn by the window itself, but the same rules hold: the
//! cursor is never in the page (the page cannot see, move or hide it, and it is in no screenshot the agent takes),
//! and it never takes a click or the keyboard. Each picture here is a view of its own class above everything in the
//! window's content view: layer-backed, its layer's contents the same premultiplied BGRA bitmap the Windows window
//! gets, `hitTest:` answering nil (every click, move and scroll goes to the window's own view, so the person can
//! take over at any time) and never first responder. Being a subview of the content view, it moves, hides and
//! miniaturizes with the window by itself; web_ctl places it on every frame where the page is now.
//!
//! Coordinates: web_ctl gives "screen" pixels as the window's client origin plus client pixels; on the Mac it has no
//! HWND, so the origin is (0, 0) and they are device pixels from the content view's top left. [`sprite_frame`] turns
//! them into the content view's points (its scale is the window's backing scale, Slint's `1px / 1phx`).
//!
//! Every call refuses (false) off the main thread, with no content view, or for an empty bitmap.

use crate::agent_cursor::{CursorBitmap, CursorShell, EdgeGlow, GlowShell};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSEvent, NSResponder, NSView, NSWindowOrderingMode};
use objc2_core_foundation::CFRetained;
use objc2_core_graphics::{
    CGBitmapContextCreate, CGBitmapContextCreateImage, CGColorSpace, CGImage, CGImageAlphaInfo,
    CGImageByteOrderInfo,
};
use objc2_foundation::{NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize};
use std::rc::Rc;

/// Where the pictures go: the window's content view (as an integer, raw-window-handle's AppKit handle) and its
/// scale (device pixels per point). None while the window has no handle.
pub type ViewSource = Rc<dyn Fn() -> Option<(isize, f64)>>;

/// Over everything else in the content view, whatever order AppKit keeps the sublayers in.
const Z: f64 = 10_000.0;

/// The scale as used for placing: finite and above 0, else 1.
fn clean_scale(scale: f64) -> f64 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

/// Pure: the frame, in the content view's points, of a `size` (w, h) bitmap of device pixels whose `tip` goes to
/// `at` (device pixels from the view's top left), at `scale`. `host_h` is the view's height in points and `flipped`
/// whether its y runs down (AppKit's default view has y up from its bottom left).
pub fn sprite_frame(
    at: (i32, i32),
    tip: (i32, i32),
    size: (u32, u32),
    scale: f64,
    host_h: f64,
    flipped: bool,
) -> NSRect {
    let s = clean_scale(scale);
    let x = f64::from(at.0 - tip.0) / s;
    let top = f64::from(at.1 - tip.1) / s;
    let (w, h) = (f64::from(size.0) / s, f64::from(size.1) / s);
    let y = if flipped { top } else { host_h - top - h };
    NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
}

/// Pure: an opacity 0..255 as AppKit's alpha value.
pub fn alpha_value(alpha: u8) -> f64 {
    f64::from(alpha) / 255.0
}

define_class!(
    // SAFETY: NSView may be subclassed; the class adds no instance variables and no `Drop`.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "FlareAgentOverlayView"]
    pub struct OverlayView;

    impl OverlayView {
        /// Never the mouse's: AppKit hit-tests past it to the window's own view.
        #[unsafe(method(hitTest:))]
        fn hit_test(&self, _point: NSPoint) -> *mut NSView {
            std::ptr::null_mut()
        }

        /// Never the keyboard's.
        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            false
        }

        /// And refuses it even when asked directly (`makeFirstResponder:` does not ask `acceptsFirstResponder`).
        #[unsafe(method(becomeFirstResponder))]
        fn become_first_responder(&self) -> bool {
            false
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: *mut NSEvent) -> bool {
            false
        }

        #[unsafe(method(isOpaque))]
        fn is_opaque(&self) -> bool {
            false
        }
    }

    unsafe impl NSObjectProtocol for OverlayView {}
);

impl OverlayView {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1.0, 1.0));
        // SAFETY: NSView's designated initializer on a freshly allocated instance of a subclass of it.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }
}

/// The view's layer, made if it has none yet.
fn layer_of(view: &NSView) -> Option<Retained<AnyObject>> {
    view.setWantsLayer(true);
    // SAFETY: `layer` is NSView's getter, answering the view's CALayer or nil.
    unsafe { msg_send![view, layer] }
}

/// The bitmap (premultiplied BGRA, top-down) as a CGImage, copied.
fn cg_image(bmp: &CursorBitmap) -> Option<CFRetained<CGImage>> {
    if bmp.w == 0 || bmp.h == 0 || bmp.bgra.len() != (bmp.w as usize) * (bmp.h as usize) * 4 {
        return None;
    }
    let mut bytes = bmp.bgra.clone();
    let space = CGColorSpace::new_device_rgb()?;
    // BGRA in memory is ARGB read as a little-endian 32-bit word, alpha premultiplied.
    let info = CGImageByteOrderInfo::Order32Little.0 | CGImageAlphaInfo::PremultipliedFirst.0;
    // SAFETY: `bytes` holds w * h * 4 bytes, rows of w * 4, and outlives the context, which is dropped before it;
    // the image is a copy made by CGBitmapContextCreateImage.
    unsafe {
        let ctx = CGBitmapContextCreate(
            bytes.as_mut_ptr().cast(),
            bmp.w as usize,
            bmp.h as usize,
            8,
            bmp.w as usize * 4,
            Some(&space),
            info,
        )?;
        CGBitmapContextCreateImage(Some(&ctx))
    }
}

/// One picture over the page: a click-through view in the window's content view, made the first time it is shown.
pub struct Sprite {
    source: ViewSource,
    view: Option<Retained<OverlayView>>,
    shown: bool,
}

impl Sprite {
    pub fn new(source: ViewSource) -> Self {
        Self {
            source,
            view: None,
            shown: false,
        }
    }

    /// The overlay view (for the AppKit tests), once made.
    pub fn view_handle(&self) -> Option<isize> {
        self.view
            .as_ref()
            .map(|v| Retained::as_ptr(v) as *const NSView as isize)
    }

    /// Shows `bmp` with its tip at `at` (device pixels from the content view's top left) at `alpha`.
    pub fn show(&mut self, at: (i32, i32), bmp: &CursorBitmap, alpha: u8) -> bool {
        let Some(mtm) = MainThreadMarker::new() else {
            return false;
        };
        let Some((host, scale)) = (self.source)().filter(|(h, _)| *h != 0) else {
            return false;
        };
        let Some(image) = cg_image(bmp) else {
            return false;
        };
        // SAFETY: the source answers the live content view of Flare's window (raw-window-handle's AppKit handle),
        // and this is the main thread, where AppKit objects may be touched.
        let host: &NSView = unsafe { &*(host as *const NSView) };
        let view = self
            .view
            .get_or_insert_with(|| OverlayView::new(mtm))
            .clone();
        let as_view: &NSView = &view;
        // On top of the content view's subviews: added there the first time, and raised again when something
        // was added above it since (or the window gave it a new content view).
        // SAFETY: reads of the view tree on the main thread.
        let parent = unsafe { as_view.superview() };
        let on_top = parent.as_deref().is_some_and(|p| std::ptr::eq(p, host))
            && host
                .subviews()
                .lastObject()
                .is_some_and(|last| std::ptr::eq(&*last, as_view));
        if !on_top {
            host.addSubview_positioned_relativeTo(as_view, NSWindowOrderingMode::Above, None);
        }
        let s = clean_scale(scale);
        as_view.setFrame(sprite_frame(
            at,
            bmp.tip,
            (bmp.w, bmp.h),
            s,
            host.bounds().size.height,
            host.isFlipped(),
        ));
        let Some(layer) = layer_of(as_view) else {
            return false;
        };
        let contents = CFRetained::as_ptr(&image).as_ptr() as *const AnyObject;
        // SAFETY: CALayer setters on the view's own layer, on the main thread; a CGImage is a valid `contents`.
        unsafe {
            let _: () = msg_send![&*layer, setContents: contents];
            let _: () = msg_send![&*layer, setContentsScale: s];
            let _: () = msg_send![&*layer, setZPosition: Z];
        }
        as_view.setAlphaValue(alpha_value(alpha));
        as_view.setHidden(false);
        self.shown = true;
        true
    }

    /// The picture shown, where it is, at a new `alpha`. False when nothing is shown.
    pub fn set_alpha(&mut self, alpha: u8) -> bool {
        match (&self.view, self.shown, MainThreadMarker::new()) {
            (Some(v), true, Some(_)) => {
                v.setAlphaValue(alpha_value(alpha));
                true
            }
            _ => false,
        }
    }

    pub fn hide(&mut self) {
        if let (Some(v), true, Some(_)) = (&self.view, self.shown, MainThreadMarker::new()) {
            v.setHidden(true);
        }
        self.shown = false;
    }

    pub fn is_shown(&self) -> bool {
        self.shown
    }
}

impl Drop for Sprite {
    fn drop(&mut self) {
        if let (Some(v), Some(_)) = (self.view.take(), MainThreadMarker::new()) {
            v.removeFromSuperview();
        }
    }
}

/// The agent's cursor (and the agent's arrow, the person's kept one): one [`Sprite`].
pub struct OverlayCursor(pub Sprite);

impl OverlayCursor {
    pub fn new(source: ViewSource) -> Self {
        Self(Sprite::new(source))
    }
}

impl CursorShell for OverlayCursor {
    fn show_at(&mut self, x: i32, y: i32, bmp: &CursorBitmap, alpha: u8) -> bool {
        self.0.show((x, y), bmp, alpha)
    }

    fn set_alpha(&mut self, alpha: u8) -> bool {
        self.0.set_alpha(alpha)
    }

    fn hide(&mut self) {
        self.0.hide();
    }
}

/// The glow round the page: one [`Sprite`] per edge band, as Windows has one window per band.
pub struct OverlayGlow {
    pub parts: Vec<Sprite>,
    used: usize,
}

impl OverlayGlow {
    pub fn new(source: ViewSource) -> Self {
        Self {
            parts: (0..4).map(|_| Sprite::new(source.clone())).collect(),
            used: 0,
        }
    }
}

impl GlowShell for OverlayGlow {
    fn show_at(&mut self, x: i32, y: i32, glow: &EdgeGlow, alpha: u8) -> bool {
        if glow.bands.is_empty() {
            return false;
        }
        self.used = glow.bands.len().min(self.parts.len());
        let mut ok = true;
        for (i, part) in self.parts.iter_mut().enumerate() {
            match glow.bands.get(i) {
                Some(((bx, by), bmp)) => ok &= part.show((x + bx, y + by), bmp, alpha),
                None => part.hide(),
            }
        }
        ok
    }

    fn set_alpha(&mut self, alpha: u8) -> bool {
        if self.used == 0 {
            return false;
        }
        let mut ok = true;
        for part in &mut self.parts[..self.used] {
            ok &= part.set_alpha(alpha);
        }
        ok
    }

    fn hide(&mut self) {
        self.used = 0;
        for part in &mut self.parts {
            part.hide();
        }
    }
}

/// The tag's text drawn by AppKit (the system font, semibold, as Segoe UI 600 on Windows), white on clear, read
/// back as 0..255 coverage: (width, `width * h` bytes), the text centred in `h`.
pub fn label_mask(font_px: u32, h: u32, text: &str) -> (u32, Vec<u8>) {
    use objc2_app_kit::{
        NSColor, NSFont, NSFontAttributeName, NSFontWeightSemibold, NSForegroundColorAttributeName,
        NSGraphicsContext, NSStringDrawing,
    };
    use objc2_foundation::{NSDictionary, NSString};
    if text.is_empty() || h == 0 || font_px == 0 {
        return (0, Vec::new());
    }
    objc2::rc::autoreleasepool(|_| {
        // SAFETY: a plain read of AppKit's weight constant.
        let weight = unsafe { NSFontWeightSemibold };
        let font = NSFont::systemFontOfSize_weight(f64::from(font_px), weight);
        let white = NSColor::whiteColor();
        // SAFETY: AppKit's attribute name constants.
        let (font_key, colour_key) =
            unsafe { (NSFontAttributeName, NSForegroundColorAttributeName) };
        let attrs: Retained<NSDictionary<_, AnyObject>> = NSDictionary::from_slices(
            &[font_key, colour_key],
            &[font.as_ref() as &AnyObject, white.as_ref() as &AnyObject],
        );
        let s = NSString::from_str(text);
        // SAFETY: the dictionary holds a font and a colour under their own keys.
        let size = unsafe { s.sizeWithAttributes(Some(&attrs)) };
        let w = ((size.width.max(0.0).ceil() as u32) + 1).min(4096);
        let mut rgba = vec![0u8; (w as usize) * (h as usize) * 4];
        let Some(space) = CGColorSpace::new_device_rgb() else {
            return (w, vec![0; (w * h) as usize]);
        };
        let info = CGImageAlphaInfo::PremultipliedLast.0;
        // SAFETY: `rgba` holds w * h * 4 bytes, rows of w * 4, and outlives the context and the drawing below.
        let ctx = unsafe {
            CGBitmapContextCreate(
                rgba.as_mut_ptr().cast(),
                w as usize,
                h as usize,
                8,
                w as usize * 4,
                Some(&space),
                info,
            )
        };
        if let Some(ctx) = ctx {
            let g = NSGraphicsContext::graphicsContextWithCGContext_flipped(&ctx, false);
            let before = NSGraphicsContext::currentContext();
            NSGraphicsContext::setCurrentContext(Some(&g));
            // Not flipped: y runs up from the bottom; the line box centred in the tag.
            let y = ((f64::from(h) - size.height) / 2.0).max(0.0);
            // SAFETY: as above; drawn into the bitmap context made current here.
            unsafe { s.drawAtPoint_withAttributes(NSPoint::new(0.0, y), Some(&attrs)) };
            g.flushGraphics();
            NSGraphicsContext::setCurrentContext(before.as_deref());
        }
        // White text on clear: the coverage is the alpha.
        let out = rgba.as_chunks::<4>().0.iter().map(|p| p[3]).collect();
        (w, out)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_pixels_from_the_top_left_become_points_in_a_flipped_or_plain_view() {
        // A cursor bitmap 60 x 50 device pixels with its tip 24 in, its tip at (400, 300) at 2x.
        let r = sprite_frame((400, 300), (24, 24), (60, 50), 2.0, 640.0, true);
        assert_eq!(
            r,
            NSRect::new(NSPoint::new(188.0, 138.0), NSSize::new(30.0, 25.0))
        );
        // The same in AppKit's default view (y up from the bottom): the bottom edge is 640 - 138 - 25 up.
        let r = sprite_frame((400, 300), (24, 24), (60, 50), 2.0, 640.0, false);
        assert_eq!(
            r,
            NSRect::new(NSPoint::new(188.0, 477.0), NSSize::new(30.0, 25.0))
        );
        // At 1x a pixel is a point.
        let r = sprite_frame((10, 20), (0, 0), (5, 6), 1.0, 100.0, true);
        assert_eq!(
            r,
            NSRect::new(NSPoint::new(10.0, 20.0), NSSize::new(5.0, 6.0))
        );
    }

    #[test]
    fn the_glow_s_bands_tile_the_page_s_rectangle_in_points() {
        // A page of 800 x 600 device pixels at (100, 80) at 2x: its bands, placed as the sprites place them, cover
        // exactly the page's 400 x 300 points from (50, 40).
        let g = crate::agent_cursor::edge_glow(800, 600, 2.0);
        let frames: Vec<NSRect> = g
            .bands
            .iter()
            .map(|((bx, by), b)| {
                sprite_frame((100 + bx, 80 + by), b.tip, (b.w, b.h), 2.0, 900.0, true)
            })
            .collect();
        let min_x = frames.iter().map(|f| f.origin.x).fold(f64::MAX, f64::min);
        let min_y = frames.iter().map(|f| f.origin.y).fold(f64::MAX, f64::min);
        let max_x = frames
            .iter()
            .map(|f| f.origin.x + f.size.width)
            .fold(0.0, f64::max);
        let max_y = frames
            .iter()
            .map(|f| f.origin.y + f.size.height)
            .fold(0.0, f64::max);
        assert_eq!((min_x, min_y, max_x, max_y), (50.0, 40.0, 450.0, 340.0));
        let area: f64 = frames.iter().map(|f| f.size.width * f.size.height).sum();
        assert!(
            area < 400.0 * 300.0 / 2.0,
            "only the bands, not the page: {area}"
        );
    }

    #[test]
    fn a_bad_scale_places_at_one_and_alpha_is_a_fraction() {
        for s in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let r = sprite_frame((10, 10), (0, 0), (4, 4), s, 100.0, true);
            assert_eq!(r.size, NSSize::new(4.0, 4.0), "at {s}");
        }
        assert_eq!(alpha_value(255), 1.0);
        assert_eq!(alpha_value(0), 0.0);
        assert!((alpha_value(128) - 0.502).abs() < 0.001);
    }

    #[test]
    fn the_tag_s_text_has_ink_of_about_the_right_width() {
        let (w, mask) = label_mask(11, 19, "Claude");
        assert_eq!(mask.len(), (w * 19) as usize);
        let cols: Vec<u32> = (0..w)
            .filter(|x| (0..19).any(|y| mask[(y * w + x) as usize] > 128))
            .collect();
        let ink = cols.last().zip(cols.first()).map_or(0, |(l, f)| l - f + 1);
        assert!(ink > 25 && ink < 55, "ink {ink} px of {w}");
        // Centred: no ink on the top or bottom row.
        assert!((0..w).all(|x| mask[x as usize] < 64 && mask[(18 * w + x) as usize] < 64));
        assert_eq!(label_mask(11, 19, ""), (0, Vec::new()));
    }

    #[test]
    fn a_shell_with_no_view_or_an_empty_picture_refuses() {
        let bmp = crate::agent_cursor::compose_cursor(1.0, "", &[]);
        let empty = CursorBitmap {
            w: 0,
            h: 0,
            bgra: Vec::new(),
            tip: (0, 0),
        };
        assert!(cg_image(&empty).is_none());
        let mut c = OverlayCursor::new(Rc::new(|| Some((0, 2.0))));
        assert!(!c.show_at(10, 10, &bmp, 255));
        assert!(!c.set_alpha(10));
        let mut none = OverlayCursor::new(Rc::new(|| None));
        assert!(!none.show_at(10, 10, &bmp, 255));
        let mut g = OverlayGlow::new(Rc::new(|| None));
        assert!(!g.show_at(0, 0, &crate::agent_cursor::edge_glow(0, 0, 1.0), 255));
        assert!(!g.set_alpha(5));
    }
}

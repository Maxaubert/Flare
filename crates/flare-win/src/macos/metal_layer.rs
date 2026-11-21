//! The window's `CAMetalLayer` as the window server sees it (rule 1324, the pulse watch): the size of
//! the drawable Flare presents against the layer's bounds and scale. When they disagree the window
//! server stretches every frame; wgpu on Metal never reports that, so only a read like this shows it.
//!
//! Read only, on the main thread; None for anything unexpected (no layer, no Metal layer, another
//! thread).

use objc2::MainThreadMarker;
use objc2::msg_send;
use objc2::runtime::{AnyClass, AnyObject, Bool};
use objc2_core_graphics::CGColorSpace;
use objc2_foundation::{NSRect, NSSize};

/// What [`geom`] read.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MetalLayerGeom {
    pub drawable_w: f64,
    pub drawable_h: f64,
    pub bounds_w: f64,
    pub bounds_h: f64,
    pub scale: f64,
    pub window_scale: f64,
    pub opaque: bool,
    /// The layer names a colour space.
    pub tagged: bool,
}

/// The first `CAMetalLayer` at or under `layer`, at most a few levels down.
fn find_metal(layer: &AnyObject, class: &AnyClass, depth: u32) -> Option<*const AnyObject> {
    // SAFETY: `layer` is a live CALayer of this process, read on the main thread; isKindOfClass:,
    // sublayers (an NSArray of CALayers, or nil), count and objectAtIndex: are plain getters, and
    // every pointer is checked for nil before it is used.
    unsafe {
        let is: Bool = msg_send![layer, isKindOfClass: class];
        if is.as_bool() {
            return Some(layer as *const AnyObject);
        }
        if depth == 0 {
            return None;
        }
        let subs: *const AnyObject = msg_send![layer, sublayers];
        if subs.is_null() {
            return None;
        }
        let subs: &AnyObject = &*subs;
        let n: usize = msg_send![subs, count];
        for i in 0..n {
            let sub: *const AnyObject = msg_send![subs, objectAtIndex: i];
            if sub.is_null() {
                continue;
            }
            if let Some(m) = find_metal(&*sub, class, depth - 1) {
                return Some(m);
            }
        }
    }
    None
}

/// The Metal layer under the window's content view (the `NSView` handle raw-window-handle gives).
pub fn geom(view: isize) -> Option<MetalLayerGeom> {
    MainThreadMarker::new()?;
    if view == 0 {
        return None;
    }
    let class = AnyClass::get(c"CAMetalLayer")?;
    // SAFETY: the caller passes the live content view of one of this process's windows, and we are
    // on the main thread, where AppKit and Core Animation objects may be read. Every message is a
    // property getter of the view, its window, CALayer or CAMetalLayer, and every pointer is checked
    // for nil before it is used.
    unsafe {
        let view: &AnyObject = &*(view as *const AnyObject);
        let root: *const AnyObject = msg_send![view, layer];
        if root.is_null() {
            return None;
        }
        let layer: &AnyObject = &*find_metal(&*root, class, 3)?;
        let drawable: NSSize = msg_send![layer, drawableSize];
        let bounds: NSRect = msg_send![layer, bounds];
        let scale: f64 = msg_send![layer, contentsScale];
        let opaque: Bool = msg_send![layer, isOpaque];
        let space: *const CGColorSpace = msg_send![layer, colorspace];
        let window: *const AnyObject = msg_send![view, window];
        let window_scale: f64 = if window.is_null() {
            0.0
        } else {
            msg_send![&*window, backingScaleFactor]
        };
        Some(MetalLayerGeom {
            drawable_w: drawable.width,
            drawable_h: drawable.height,
            bounds_w: bounds.size.width,
            bounds_h: bounds.size.height,
            scale,
            window_scale,
            opaque: opaque.as_bool(),
            tagged: !space.is_null(),
        })
    }
}

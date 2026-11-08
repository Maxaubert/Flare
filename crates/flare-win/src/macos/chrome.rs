//! The native window chrome on macOS (port phase 3, step 1): the standard traffic lights placed in
//! Flare's top row, native full screen and zoom, the titlebar double click the user chose in
//! System Settings, and the screens a saved window frame is checked against.
//!
//! The window itself is made by winit with a transparent, hidden-title, full-size-content titlebar
//! (`flare::backend`), so Flare draws its own top row under the traffic lights exactly as on
//! Windows; only its own minimise, maximise and close buttons are left out there (`app.slint`,
//! `native-caption`), and the row keeps `CAPTION_GAP` free on the left for the lights.
//!
//! AppKit lays the titlebar out again on a resize, on leaving full screen and when the title
//! changes, which puts the lights back at its own 28 pt row; `install` therefore re-places them on
//! every one of those notifications.
//!
//! The handle every call takes is the window's `NSView` (raw-window-handle's `AppKit` handle) as an
//! integer. Calls refuse (false, empty) off the main thread.

use crate::window_state::Area;
use objc2::MainThreadMarker;
use objc2::rc::{Retained, Weak};
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{
    NSScreen, NSWindow, NSWindowButton, NSWindowCollectionBehavior,
    NSWindowDidEndLiveResizeNotification, NSWindowDidExitFullScreenNotification,
    NSWindowDidResizeNotification, NSWindowStyleMask,
};
use objc2_foundation::{
    NSNotification, NSNotificationCenter, NSObjectProtocol, NSPoint, NSRect, NSString,
    NSUserDefaults,
};
use std::cell::RefCell;
use std::ptr::NonNull;

/// Flare's top row, in points (`Theme.bar-height`).
pub const BAR_HEIGHT: f64 = 36.0;
/// The close button's left edge from the window's left edge, in points.
pub const LIGHTS_LEFT: f64 = 14.0;
/// From one light's left edge to the next one's, in points (AppKit's own spacing: 14 + 6).
pub const LIGHTS_STEP: f64 = 20.0;
/// What the top row leaves free on the left for the lights, in logical pixels: the three lights
/// (`LIGHTS_LEFT` + two steps + one 14 pt light) and 10 more before the first tab.
pub const CAPTION_GAP: f32 = 78.0;

/// Pure: the origin of each light (close, minimise, zoom) inside a titlebar view `bar` points
/// high, for a light `light_h` points high: one step apart from `LIGHTS_LEFT`, centred in height
/// (AppKit's titlebar view is not flipped, so centred reads the same from either edge).
pub fn light_origins(bar: f64, light_h: f64) -> [(f64, f64); 3] {
    let y = ((bar - light_h) / 2.0).max(0.0).round();
    [0.0, 1.0, 2.0].map(|i| (LIGHTS_LEFT + i * LIGHTS_STEP, y))
}

/// What a double click on the window's move area does on this Mac.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TitleDoubleClick {
    /// Zoom (System Settings "Zoom" or "Fill", and the default).
    Zoom,
    Minimize,
    Nothing,
}

/// Pure: the `AppleActionOnDoubleClick` default (System Settings, Desktop & Dock, "Double-click a
/// window's title bar to") as an action. Unset means zoom, the system default.
pub fn title_double_click(pref: Option<&str>) -> TitleDoubleClick {
    match pref.map(str::trim) {
        Some("Minimize") => TitleDoubleClick::Minimize,
        Some("None") => TitleDoubleClick::Nothing,
        _ => TitleDoubleClick::Zoom,
    }
}

/// What this user chose for a titlebar double click (read on every double click: the setting can
/// change while Flare runs).
pub fn title_double_click_now() -> TitleDoubleClick {
    let defaults = NSUserDefaults::standardUserDefaults();
    let pref = defaults.stringForKey(&NSString::from_str("AppleActionOnDoubleClick"));
    title_double_click(pref.map(|s| s.to_string()).as_deref())
}

/// Puts the three lights where Flare's top row wants them: the titlebar container `bar` points
/// high at the top left of the window and only `CAPTION_GAP` wide, each light centred in it. A
/// full-screen window (its titlebar lives in the menu-bar overlay then) and a window without
/// lights are left alone; false then.
///
/// Why so narrow: the titlebar container is the part of the window AppKit treats as titlebar (it
/// moves the window by it, and zooms on a double click there). Flare's tabs sit in the same row,
/// so the native titlebar keeps only the lights' corner (where Flare draws nothing but its move
/// area) and the rest of the row reaches Flare, which moves the window only by the regions it
/// marks (`WindowMoveArea`, winit's `drag_window`).
pub fn place_traffic_lights(window: &NSWindow, bar: f64) -> bool {
    if window.styleMask().contains(NSWindowStyleMask::FullScreen) {
        return false;
    }
    let lights = [
        NSWindowButton::CloseButton,
        NSWindowButton::MiniaturizeButton,
        NSWindowButton::ZoomButton,
    ]
    .map(|b| window.standardWindowButton(b));
    let [Some(close), Some(mini), Some(zoom)] = lights else {
        return false;
    };
    // SAFETY: the superviews of the window's own close button, read on the main thread: the
    // titlebar view, and the titlebar container above it.
    let Some(container) = (unsafe { close.superview().and_then(|t| t.superview()) }) else {
        return false;
    };
    let mut frame = container.frame();
    frame.size.height = bar;
    frame.size.width = f64::from(CAPTION_GAP);
    frame.origin.x = 0.0;
    frame.origin.y = window.frame().size.height - bar;
    container.setFrame(frame);
    let light_h = close.frame().size.height;
    for (button, (x, y)) in [close, mini, zoom].iter().zip(light_origins(bar, light_h)) {
        button.setFrameOrigin(NSPoint::new(x, y));
    }
    true
}

/// The observers `install` registered, kept for the life of the window (UI thread only).
type Token = Retained<ProtocolObject<dyn NSObjectProtocol>>;
thread_local! {
    static OBSERVERS: RefCell<Vec<Token>> = const { RefCell::new(Vec::new()) };
}

/// The native chrome for Flare's main window: native full screen (the green light and the
/// Window menu's "Enter Full Screen"), the lights placed now and again after every resize and
/// every return from full screen. False off the main thread or without a window.
pub fn install(view: isize) -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    let Some(window) = crate::glass::window_of(view, mtm) else {
        return false;
    };
    install_on(&window)
}

/// `install` for a window in hand.
pub fn install_on(window: &NSWindow) -> bool {
    let mut behavior = window.collectionBehavior();
    behavior |= NSWindowCollectionBehavior::FullScreenPrimary;
    behavior &= !NSWindowCollectionBehavior::FullScreenAuxiliary;
    window.setCollectionBehavior(behavior);
    place_traffic_lights(window, BAR_HEIGHT);
    let center = NSNotificationCenter::defaultCenter();
    let weak = Weak::new(window);
    // SAFETY: AppKit's own constant notification names.
    let names = unsafe {
        [
            NSWindowDidResizeNotification,
            NSWindowDidEndLiveResizeNotification,
            NSWindowDidExitFullScreenNotification,
        ]
    };
    for name in names {
        let weak = weak.clone();
        let block = block2::RcBlock::new(move |_: NonNull<NSNotification>| {
            if let Some(w) = weak.load() {
                place_traffic_lights(&w, BAR_HEIGHT);
            }
        });
        // SAFETY: the observer is told about this one window only; the block holds the window
        // weakly, so it outlives nothing, and AppKit posts these on the main thread (queue nil:
        // the block runs on the posting thread).
        let token = unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(name),
                Some(window),
                None,
                &block,
            )
        };
        OBSERVERS.with(|o| o.borrow_mut().push(token));
    }
    true
}

/// Pure: an AppKit rectangle (points, bottom-left origin, y up from the primary screen's bottom)
/// as an `Area` in device pixels with the top-left origin winit and `window_state` use: flipped
/// against the primary screen's height, scaled by `scale`.
pub fn area_of(r: NSRect, primary_h: f64, scale: f64) -> Area {
    let top = primary_h - (r.origin.y + r.size.height);
    Area {
        x: (r.origin.x * scale).round() as i32,
        y: (top * scale).round() as i32,
        width: (r.size.width * scale).round() as i32,
        height: (r.size.height * scale).round() as i32,
    }
}

/// Pure: every screen's rectangle (AppKit points, `(rect, backing scale)`) as areas in ONE space,
/// the global points times `space`, each with the scale its own screen shows at. One factor for every
/// screen keeps them apart: scaling each by its own factor would make a 2x primary screen's right
/// half land on the screen next to it. winit reports a window's position as points times that
/// window's scale, so `space` is the scale of the window (or of the saved state) being placed.
pub fn areas_in(rects: &[(NSRect, f64)], primary_h: f64, space: f64) -> Vec<(Area, f64)> {
    let space = if space.is_finite() && space > 0.0 {
        space
    } else {
        1.0
    };
    rects
        .iter()
        .map(|(r, scale)| (area_of(*r, primary_h, space), *scale))
        .collect()
}

/// Every screen's visible frame and whole frame (points) with its backing scale, plus the primary
/// screen's height and scale. `None` off the main thread or without a screen.
#[allow(clippy::type_complexity)]
fn screen_rects() -> Option<(Vec<(NSRect, NSRect, f64)>, f64, f64)> {
    let mtm = MainThreadMarker::new()?;
    let all = NSScreen::screens(mtm);
    let primary = all.firstObject()?;
    let rects = all
        .iter()
        .map(|s| (s.visibleFrame(), s.frame(), s.backingScaleFactor()))
        .collect();
    Some((
        rects,
        primary.frame().size.height,
        primary.backingScaleFactor(),
    ))
}

/// The primary screen's backing scale (the one winit turns a new window's position with); 1.0 off
/// the main thread or without a screen.
pub fn primary_scale() -> f64 {
    screen_rects().map(|(_, _, s)| s).unwrap_or(1.0)
}

/// Every screen's usable area (without the menu bar and the Dock), in the one space of points times
/// `space` (`areas_in`), with the scale it shows at. Empty off the main thread.
pub fn screens_at(space: f64) -> Vec<(Area, f64)> {
    let Some((rects, primary_h, _)) = screen_rects() else {
        return Vec::new();
    };
    let visible: Vec<(NSRect, f64)> = rects.iter().map(|(v, _, s)| (*v, *s)).collect();
    areas_in(&visible, primary_h, space)
}

/// `screens_at` in the primary screen's scale.
pub fn screens() -> Vec<(Area, f64)> {
    screens_at(primary_scale())
}

/// The primary screen's height in points (what AppKit's y axis is flipped against); 0 off the main
/// thread or without a screen.
pub fn primary_height() -> f64 {
    MainThreadMarker::new()
        .and_then(|mtm| NSScreen::screens(mtm).firstObject())
        .map(|s| s.frame().size.height)
        .unwrap_or(0.0)
}

/// Every screen's whole frame (the menu bar and the Dock included), in the one space of points times
/// `space` (`areas_in`).
pub fn screen_frames_at(space: f64) -> Vec<Area> {
    let Some((rects, primary_h, _)) = screen_rects() else {
        return Vec::new();
    };
    let whole: Vec<(NSRect, f64)> = rects.iter().map(|(_, f, s)| (*f, *s)).collect();
    areas_in(&whole, primary_h, space)
        .into_iter()
        .map(|(a, _)| a)
        .collect()
}

/// Pure: the scale of the screen holding the device-pixel point, else the first screen's, else 1.
pub fn scale_in(screens: &[(Area, f64)], x: i32, y: i32) -> f64 {
    screens
        .iter()
        .find(|(a, _)| x >= a.x && x < a.x + a.width && y >= a.y && y < a.y + a.height)
        .or(screens.first())
        .map(|(_, s)| *s)
        .filter(|s| s.is_finite() && *s > 0.0)
        .unwrap_or(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2_foundation::NSSize;

    #[test]
    fn the_lights_sit_one_step_apart_and_centred_in_the_top_row() {
        let o = light_origins(BAR_HEIGHT, 16.0);
        assert_eq!(o, [(14.0, 10.0), (34.0, 10.0), (54.0, 10.0)]);
        // The gap the row keeps covers the last light and leaves room before the first tab.
        assert!(f64::from(CAPTION_GAP) >= o[2].0 + 14.0 + 8.0);
        // A titlebar lower than a light never pushes it below the window's top edge.
        assert_eq!(light_origins(10.0, 16.0)[0].1, 0.0);
    }

    #[test]
    fn the_double_click_follows_the_users_setting() {
        assert_eq!(title_double_click(None), TitleDoubleClick::Zoom);
        assert_eq!(title_double_click(Some("Maximize")), TitleDoubleClick::Zoom);
        assert_eq!(title_double_click(Some("Fill")), TitleDoubleClick::Zoom);
        assert_eq!(
            title_double_click(Some("Minimize")),
            TitleDoubleClick::Minimize
        );
        assert_eq!(title_double_click(Some("None")), TitleDoubleClick::Nothing);
        // The live read never fails, whatever this Mac says.
        let _ = title_double_click_now();
    }

    #[test]
    fn a_screen_becomes_a_top_left_device_pixel_area() {
        // A 1512x982 pt Retina screen with a 33 pt menu bar on top.
        let visible = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1512.0, 949.0));
        assert_eq!(
            area_of(visible, 982.0, 2.0),
            Area {
                x: 0,
                y: 66,
                width: 3024,
                height: 1898
            }
        );
        // A second screen above the primary one has a negative top.
        let above = NSRect::new(NSPoint::new(-200.0, 982.0), NSSize::new(1920.0, 1080.0));
        assert_eq!(area_of(above, 982.0, 1.0).y, -1080);
    }

    #[test]
    fn screens_of_different_scales_never_overlap() {
        // A 1512x982 pt Retina primary screen and a 1x 1920x1080 pt screen to its right.
        let rects = [
            (
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1512.0, 982.0)),
                2.0,
            ),
            (
                NSRect::new(NSPoint::new(1512.0, -98.0), NSSize::new(1920.0, 1080.0)),
                1.0,
            ),
        ];
        for space in [1.0, 2.0] {
            let a = areas_in(&rects, 982.0, space);
            let (p, q) = (a[0].0, a[1].0);
            assert_eq!(p.x + p.width, q.x, "side by side at {space}x: {a:?}");
            // A window 100 pt into the second screen, as winit reports it at that scale.
            let x = (1612.0 * space) as i32;
            assert_eq!(scale_in(&a, x, 10), 1.0, "at {space}x");
            assert_eq!(scale_in(&a, (100.0 * space) as i32, 10), 2.0);
        }
        assert_eq!(areas_in(&rects, 982.0, f64::NAN)[1].0.x, 1512);
    }

    #[test]
    fn the_scale_is_the_one_of_the_screen_under_the_point() {
        let s = [
            (
                Area {
                    x: 0,
                    y: 0,
                    width: 3024,
                    height: 1964,
                },
                2.0,
            ),
            (
                Area {
                    x: 3024,
                    y: 0,
                    width: 1920,
                    height: 1080,
                },
                1.0,
            ),
        ];
        assert_eq!(scale_in(&s, 100, 100), 2.0);
        assert_eq!(scale_in(&s, 3100, 100), 1.0);
        assert_eq!(scale_in(&s, -5000, -5000), 2.0);
        assert_eq!(scale_in(&[], 0, 0), 1.0);
    }

    #[test]
    fn off_the_main_thread_nothing_is_touched() {
        assert!(!install(0));
        assert!(screens().is_empty());
        assert!(screen_frames_at(2.0).is_empty());
        assert_eq!(primary_scale(), 1.0);
    }
}

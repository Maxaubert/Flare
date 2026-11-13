//! The app icon picker's artwork and how the running app wears the pick (rules 1230 and 1231, 2026-11-13).
//!
//! The shipped icon stays the original Flare icon on both systems (the bundle's `Flare.icns`, the exe's resource 1).
//! A pick in Settings > Appearance > App icon changes only what the running app shows:
//! - macOS: `NSApplication.applicationIconImage` (the Dock and the app switcher), set at start and on every change;
//!   the original clears it, so the bundle's icon shows again. The signed bundle is never written to (setting a
//!   custom icon on `Flare.app` breaks its code signature), so Finder keeps the original.
//! - Windows: the window's big and small icons (`WM_SETICON`, which the taskbar button shows too), set when the window
//!   has its handle, on every DPI change and on every pick. Shortcuts, the exe and the installer keep the original.
//!
//! The dictation widget is a tool window with no icon (off the taskbar, never in the Dock), and Settings is a tab of
//! the main window, so the main window is the only one that wears the icon.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

#[cfg(test)]
use flare_settings::app_icon::APP_ICONS;
use flare_settings::app_icon::{AppIcon, ORIGINAL, app_icon, on_app_icon_change};
use flare_settings::kv::{PrefStore, Subscription};

/// The original Flare icon (the in-app mark is the same artwork).
const ORIGINAL_SVG: &[u8] = include_bytes!("../../../assets/icon/flare-color.svg");

/// The artwork of an icon on this system: its SVG source. Liquid Glass is the owner's static vector of the Icon
/// Composer icon on macOS and #199's Windows counterpart elsewhere; an unknown id is the original.
pub fn svg(id: &str) -> &'static [u8] {
    match id {
        "liquid-glass" if cfg!(target_os = "macos") => {
            include_bytes!("../../../assets/icon/app-icons/liquid-glass.svg")
        }
        "liquid-glass" => include_bytes!("../../../assets/icon/app-icons/liquid-glass-windows.svg"),
        "high-voltage" => include_bytes!("../../../assets/icon/app-icons/high-voltage.svg"),
        "daybreak" => include_bytes!("../../../assets/icon/app-icons/daybreak.svg"),
        "hazard-reverse" => include_bytes!("../../../assets/icon/app-icons/hazard-reverse.svg"),
        "safety-orange" => include_bytes!("../../../assets/icon/app-icons/safety-orange.svg"),
        "blue-enamel" => include_bytes!("../../../assets/icon/app-icons/blue-enamel.svg"),
        "raspberry-lacquer" => {
            include_bytes!("../../../assets/icon/app-icons/raspberry-lacquer.svg")
        }
        "lemon-ink" => include_bytes!("../../../assets/icon/app-icons/lemon-ink.svg"),
        "jade-glass" => include_bytes!("../../../assets/icon/app-icons/jade-glass.svg"),
        "citrus-punch" => include_bytes!("../../../assets/icon/app-icons/citrus-punch.svg"),
        "cobalt-electric" => include_bytes!("../../../assets/icon/app-icons/cobalt-electric.svg"),
        _ => ORIGINAL_SVG,
    }
}

thread_local! {
    /// The picker's tile pictures, parsed once per thread (Slint draws them at the tile's size).
    static TILES: RefCell<HashMap<&'static str, slint::Image>> = RefCell::new(HashMap::new());
}

/// The picker tile's picture of an icon: its SVG, drawn by Slint at the tile's size.
pub fn tile_image(id: &str) -> slint::Image {
    let icon = flare_settings::app_icon::icon_of(id);
    TILES.with(|t| {
        t.borrow_mut()
            .entry(icon.id)
            .or_insert_with(|| {
                slint::Image::load_from_svg_data(svg(icon.id)).unwrap_or_else(|e| {
                    log::warn!("app icon {}: {e}", icon.id);
                    slint::Image::default()
                })
            })
            .clone()
    })
}

/// An icon drawn at `px` by `px`: straight (not premultiplied) RGBA rows, top row first. None for a size of 0 or an
/// SVG that does not parse.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn render_rgba(id: &str, px: u32) -> Option<Vec<u8>> {
    let pixmap = render(id, px)?;
    let mut out = Vec::with_capacity((px * px * 4) as usize);
    for p in pixmap.pixels() {
        let c = p.demultiply();
        out.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }
    Some(out)
}

/// An icon drawn at `px` by `px` as a PNG file's bytes (what AppKit's NSImage reads).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn render_png(id: &str, px: u32) -> Option<Vec<u8>> {
    render(id, px)?.encode_png().ok()
}

fn render(id: &str, px: u32) -> Option<resvg::tiny_skia::Pixmap> {
    let tree = resvg::usvg::Tree::from_data(svg(id), &resvg::usvg::Options::default()).ok()?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(px, px)?;
    let size = tree.size();
    let scale = (px as f32 / size.width()).min(px as f32 / size.height());
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    Some(pixmap)
}

/// What wears the icon: the system in the app, a recorder in the tests.
pub trait IconPort {
    fn wear(&self, icon: &'static AppIcon);
}

/// Wears the pick in force, once per change. Nothing is done for the original at start: the bundle and the exe
/// already show it.
pub struct IconCtl {
    port: Rc<dyn IconPort>,
    worn: &'static str,
}

impl IconCtl {
    pub fn new(port: Rc<dyn IconPort>) -> Self {
        IconCtl {
            port,
            worn: ORIGINAL,
        }
    }

    /// The pick in `prefs` is worn (an unknown stored word wears the original).
    pub fn sync(&mut self, prefs: &PrefStore) {
        let want = app_icon(prefs);
        if want.id != self.worn {
            self.port.wear(want);
            self.worn = want.id;
        }
    }

    /// The id worn now.
    #[cfg(test)]
    pub fn worn(&self) -> &'static str {
        self.worn
    }
}

/// The running app's icon port.
pub struct SystemIcon;

impl IconPort for SystemIcon {
    fn wear(&self, icon: &'static AppIcon) {
        wear_system(icon.id);
    }
}

thread_local! {
    static CTL: RefCell<Option<IconCtl>> = const { RefCell::new(None) };
}

/// Starts the app's icon on the UI thread: wears the pick in force now and after every change. The returned listener
/// lives as long as the window.
pub fn start(prefs: &PrefStore) -> Subscription {
    CTL.with(|c| {
        let mut ctl = IconCtl::new(Rc::new(SystemIcon));
        ctl.sync(prefs);
        *c.borrow_mut() = Some(ctl);
    });
    let store = prefs.clone();
    on_app_icon_change(prefs, move || {
        let store = store.clone();
        // A change is heard on the thread that stored it; AppKit and the window are the UI thread's.
        let _ = slint::invoke_from_event_loop(move || {
            CTL.with(|c| {
                if let Some(ctl) = c.borrow_mut().as_mut() {
                    ctl.sync(&store);
                }
            });
        });
    })
}

/// Every icon id the picker offers, in its order.
#[cfg(test)]
pub fn ids() -> impl Iterator<Item = &'static str> {
    APP_ICONS.iter().map(|i| i.id)
}

// ---- the systems -------------------------------------------------------------------------------------------------

/// The Dock and app switcher size AppKit scales down from (the largest Dock tile at 2x).
#[cfg(target_os = "macos")]
const DOCK_PX: u32 = 512;

/// macOS: the app's icon image, or none for the original (the bundle's icon shows again).
#[cfg(target_os = "macos")]
fn wear_system(id: &'static str) {
    use objc2::AllocAnyThread;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    let Some(mtm) = MainThreadMarker::new() else {
        log::warn!("app icon: not on the main thread");
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    if id == ORIGINAL {
        // SAFETY: AppKit's own setter, on the main thread; nil restores the bundle's icon.
        unsafe { app.setApplicationIconImage(None) };
        return;
    }
    let Some(png) = render_png(id, DOCK_PX) else {
        log::warn!("app icon {id}: not drawn");
        return;
    };
    let data = NSData::with_bytes(&png);
    let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) else {
        log::warn!("app icon {id}: AppKit read no image");
        return;
    };
    // SAFETY: as above; the image is retained by AppKit.
    unsafe { app.setApplicationIconImage(Some(&image)) };
}

/// Windows: the pick is kept for the window icon, then put on the window now.
#[cfg(windows)]
fn wear_system(id: &'static str) {
    crate::ship_ctl::set_worn_icon(id);
}

/// Elsewhere (Linux builds of the tests): nothing wears an icon.
#[cfg(not(any(windows, target_os = "macos")))]
fn wear_system(_id: &'static str) {}

#[cfg(test)]
mod tests {
    use super::*;
    use flare_settings::app_icon::{KEY, set_app_icon_word};
    use std::cell::RefCell;

    #[derive(Default)]
    struct Recorder(RefCell<Vec<&'static str>>);

    impl IconPort for Recorder {
        fn wear(&self, icon: &'static AppIcon) {
            self.0.borrow_mut().push(icon.id);
        }
    }

    fn ctl() -> (Rc<Recorder>, IconCtl) {
        let rec = Rc::new(Recorder::default());
        let port: Rc<dyn IconPort> = rec.clone();
        (rec, IconCtl::new(port))
    }

    /// Rule 1230: a fresh profile wears the original, which the app already shows, so nothing is set at start.
    #[test]
    fn the_default_is_the_original_and_needs_nothing_at_start() {
        let prefs = PrefStore::memory();
        let (rec, mut c) = ctl();
        c.sync(&prefs);
        assert_eq!(c.worn(), ORIGINAL);
        assert!(rec.0.borrow().is_empty());
    }

    /// A stored pick is worn at start, a new pick once, the same pick never twice, and the original again on the way
    /// back (which clears the custom icon).
    #[test]
    fn a_pick_is_worn_once_and_the_original_brings_the_shipped_icon_back() {
        let prefs = PrefStore::memory();
        set_app_icon_word(&prefs, "daybreak");
        let (rec, mut c) = ctl();
        c.sync(&prefs);
        c.sync(&prefs);
        set_app_icon_word(&prefs, "liquid-glass");
        c.sync(&prefs);
        set_app_icon_word(&prefs, "original");
        c.sync(&prefs);
        assert_eq!(
            *rec.0.borrow(),
            ["daybreak", "liquid-glass", "original"],
            "each change worn once"
        );
    }

    /// An unknown stored word (an older or newer build's icon) wears the original.
    #[test]
    fn an_unknown_stored_icon_falls_back_to_the_original() {
        let prefs = PrefStore::memory();
        set_app_icon_word(&prefs, "jade-glass");
        let (rec, mut c) = ctl();
        c.sync(&prefs);
        prefs.set(KEY, "neon-sign");
        c.sync(&prefs);
        assert_eq!(*rec.0.borrow(), ["jade-glass", "original"]);
        assert_eq!(c.worn(), ORIGINAL);
    }

    /// Every icon's artwork parses and draws at the sizes the systems ask for: an opaque body in the middle and a
    /// transparent corner (the rounded square never fills the frame), and each design differs from the original.
    #[test]
    fn every_icon_draws_at_the_system_sizes() {
        let original = render_rgba(ORIGINAL, 32).expect("the original draws");
        for id in ids() {
            for px in [16u32, 32, 48, 256] {
                let rgba = render_rgba(id, px).unwrap_or_else(|| panic!("{id} at {px}"));
                assert_eq!(rgba.len(), (px * px * 4) as usize);
                let at = |x: u32, y: u32| {
                    let i = ((y * px + x) * 4) as usize;
                    [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
                };
                assert_eq!(
                    at(px / 2, px / 2)[3],
                    255,
                    "{id} at {px}: the middle is opaque"
                );
                assert_eq!(at(0, 0)[3], 0, "{id} at {px}: the corner is clear");
            }
            if id != ORIGINAL {
                assert_ne!(
                    render_rgba(id, 32).unwrap(),
                    original,
                    "{id} is its own design"
                );
            }
        }
        let png = render_png("daybreak", 64).expect("a png");
        assert_eq!(&png[1..4], b"PNG");
    }

    /// Liquid Glass is the static vector of the Icon Composer icon on the Mac and #199's Windows icon elsewhere; an
    /// unknown id draws the original.
    #[test]
    fn liquid_glass_has_one_artwork_per_system_and_unknown_is_the_original() {
        let glass = std::str::from_utf8(svg("liquid-glass")).unwrap();
        if cfg!(target_os = "macos") {
            assert!(glass.contains("Liquid Glass"), "{}", &glass[..200]);
        } else {
            assert!(
                glass.contains("Flare app icon (Windows)"),
                "{}",
                &glass[..200]
            );
        }
        assert_eq!(svg("nothing"), ORIGINAL_SVG);
        assert_eq!(svg(ORIGINAL), ORIGINAL_SVG);
    }
}

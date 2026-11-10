//! The window glass on macOS (port phase 3): an `NSVisualEffectView` behind the content view, the
//! Mac counterpart of the DWM system backdrop. `dwm::set_system_backdrop` and
//! `dwm::set_immersive_dark` land here, so the material state machine (`material.rs`) and its
//! order of operations (decision D7) are the same on both systems.
//!
//! The effect view is a sibling BELOW the content view inside the window's frame view: winit owns
//! the content view (it casts `contentView` back to its own class), so the glass never replaces or
//! wraps it. With the full-size content view the frame view and the content view cover the same
//! rectangle, the titlebar included, so the glass reaches under the traffic lights too.
//!
//! The handle every call takes is the window's `NSView` (raw-window-handle's `AppKit` handle), as
//! an integer. Every call is cosmetic (rule 74): no main thread, no window or a refusal answers
//! false and the window stays solid.

use crate::dwm::Backdrop;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
    NSAutoresizingMaskOptions, NSColor, NSUserInterfaceItemIdentification, NSView,
    NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView,
    NSWindow, NSWindowOrderingMode,
};
use objc2_foundation::NSString;

/// The identifier the glass view carries, so a later call finds it again.
pub const GLASS_ID: &str = "flare.glass";

/// Pure: the Mac material for a DWM backdrop. Acrylic (the one Flare asks for, a strong blur of
/// what is behind the window) is the under-window background; Mica (a quiet tint of the
/// desktop) the window background; Mica Alt (tabbed) the titlebar's; Auto the window
/// background. None takes the glass away.
pub fn effect_material(b: Backdrop) -> Option<NSVisualEffectMaterial> {
    match b {
        Backdrop::None => None,
        Backdrop::Acrylic => Some(NSVisualEffectMaterial::UnderWindowBackground),
        Backdrop::Mica | Backdrop::Auto => Some(NSVisualEffectMaterial::WindowBackground),
        Backdrop::Tabbed => Some(NSVisualEffectMaterial::Titlebar),
    }
}

/// The window an `NSView` handle sits in. None off the main thread, for 0 or a view without one.
pub(crate) fn window_of(view: isize, mtm: MainThreadMarker) -> Option<Retained<NSWindow>> {
    let _ = mtm;
    if view == 0 {
        return None;
    }
    // SAFETY: the caller passes the live content view of one of this process's windows (the
    // AppKit handle raw-window-handle gave it), and we are on the main thread, where AppKit
    // objects may be touched.
    let view: &NSView = unsafe { &*(view as *const NSView) };
    view.window()
}

/// The glass view of `window`, when one was put in.
pub fn glass_view(window: &NSWindow) -> Option<Retained<NSVisualEffectView>> {
    let content = window.contentView()?;
    // SAFETY: reading the superview of the window's own content view on the main thread.
    let frame = unsafe { content.superview() }?;
    let id = NSString::from_str(GLASS_ID);
    frame.subviews().iter().find_map(|v| {
        let ident = v.identifier()?;
        if *ident == *id {
            v.downcast::<NSVisualEffectView>().ok()
        } else {
            None
        }
    })
}

/// Puts the glass behind `window`'s content (or changes its material), or takes it away for
/// `Backdrop::None`. False when the window has no content view inside a frame view.
pub fn set_window_backdrop(window: &NSWindow, b: Backdrop, mtm: MainThreadMarker) -> bool {
    let existing = glass_view(window);
    let Some(material) = effect_material(b) else {
        if let Some(v) = existing {
            v.removeFromSuperview();
        }
        return true;
    };
    if let Some(v) = existing {
        v.setMaterial(material);
        return true;
    }
    let Some(content) = window.contentView() else {
        return false;
    };
    // SAFETY: as in `glass_view`.
    let Some(frame) = (unsafe { content.superview() }) else {
        return false;
    };
    let glass = NSVisualEffectView::initWithFrame(mtm.alloc(), frame.bounds());
    glass.setMaterial(material);
    glass.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
    // Frosted while the window is key, flatter when it is not: what every Mac window does.
    glass.setState(NSVisualEffectState::FollowsWindowActiveState);
    glass.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    glass.setIdentifier(Some(&NSString::from_str(GLASS_ID)));
    frame.addSubview_positioned_relativeTo(&glass, NSWindowOrderingMode::Below, Some(&content));
    // The window itself must let the glass show through where the UI's ground is see-through.
    window.setOpaque(false);
    window.setBackgroundColor(Some(&NSColor::clearColor()));
    true
}

/// The frost's light or dark, and the traffic lights' with it: the window wears the dark or the
/// light Aqua appearance, so the glass follows the theme's ground the way DWM's immersive dark does.
pub fn set_window_dark(window: &NSWindow, dark: bool) -> bool {
    // SAFETY: the appearance names are AppKit's own constant strings.
    let name = unsafe {
        if dark {
            NSAppearanceNameDarkAqua
        } else {
            NSAppearanceNameAqua
        }
    };
    let Some(appearance) = NSAppearance::appearanceNamed(name) else {
        return false;
    };
    window.setAppearance(Some(&appearance));
    true
}

/// `dwm::set_system_backdrop` on macOS.
pub fn set_backdrop(view: isize, b: Backdrop) -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    match window_of(view, mtm) {
        Some(w) => set_window_backdrop(&w, b, mtm),
        None => false,
    }
}

/// `dwm::set_immersive_dark` on macOS.
pub fn set_dark(view: isize, dark: bool) -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    match window_of(view, mtm) {
        Some(w) => set_window_dark(&w, dark),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acrylic_is_the_strong_blur_and_none_takes_the_glass_away() {
        assert_eq!(
            effect_material(Backdrop::Acrylic),
            Some(NSVisualEffectMaterial::UnderWindowBackground)
        );
        assert_eq!(
            effect_material(Backdrop::Mica),
            Some(NSVisualEffectMaterial::WindowBackground)
        );
        assert_eq!(
            effect_material(Backdrop::Auto),
            Some(NSVisualEffectMaterial::WindowBackground)
        );
        assert_eq!(
            effect_material(Backdrop::Tabbed),
            Some(NSVisualEffectMaterial::Titlebar)
        );
        assert_eq!(effect_material(Backdrop::None), None);
    }

    #[test]
    fn off_the_main_thread_nothing_is_touched() {
        // Unit tests run on worker threads: every call refuses rather than touch AppKit there.
        assert!(!set_backdrop(0, Backdrop::Acrylic));
        assert!(!set_dark(0, true));
    }
}

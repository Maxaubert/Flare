//! macOS: the Dock tile as Windows' taskbar button. The badge is the tile's own badge label
//! (`NSDockTile setBadgeLabel:`, the red count every Mac app shows), the bell's flash is
//! `requestUserAttention:`, and "the window is in front" is `NSApp isActive`.
//!
//! Windows flashes the button with `FLASHW_TRAY | FLASHW_TIMERNOFG`: until the window comes to the
//! front, and never while it is. The Mac match is a CRITICAL request (the icon bounces until Flare is
//! activated), never an informational one (one bounce), and none at all while Flare is the active app
//! ([`attention_for`]). AppKit calls are made on the main thread only: off it every call here does
//! nothing and reads as "not active", "no badge", which is what a test thread sees.

use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSRequestUserAttentionType};
use objc2_foundation::NSString;

/// What the Dock is asked when a bell rings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attention {
    /// Bounces until the app is activated: the match for Windows' flash-until-in-front.
    Critical,
    /// Bounces once. Not used for the bell (it would stop before the person looked).
    Informational,
}

/// Pure: the request a bell makes. Nothing while Flare is the active app (Windows never flashes the
/// foreground window), else a critical one, which runs until the person brings Flare forward.
pub fn attention_for(active: bool) -> Option<Attention> {
    (!active).then_some(Attention::Critical)
}

/// Pure: the AppKit request type of an [`Attention`].
pub fn request_type(kind: Attention) -> NSRequestUserAttentionType {
    match kind {
        Attention::Critical => NSRequestUserAttentionType::CriticalRequest,
        Attention::Informational => NSRequestUserAttentionType::InformationalRequest,
    }
}

/// True when Flare is the active app (one of its windows has the keyboard). False off the main thread.
pub fn is_active() -> bool {
    MainThreadMarker::new().is_some_and(|mtm| NSApplication::sharedApplication(mtm).isActive())
}

/// Asks the Dock for attention. The request's id, or None when nothing was asked: off the main thread,
/// or because AppKit returns 0 for an app that is already active.
pub fn request_attention(kind: Attention) -> Option<isize> {
    let mtm = MainThreadMarker::new()?;
    let id = NSApplication::sharedApplication(mtm).requestUserAttention(request_type(kind));
    (id != 0).then_some(id)
}

/// Ends a request made by [`request_attention`] (activating the app ends it too).
pub fn cancel_attention(id: isize) {
    if let Some(mtm) = MainThreadMarker::new() {
        NSApplication::sharedApplication(mtm).cancelUserAttentionRequest(id);
    }
}

/// Sets the Dock tile's badge (`Some("3")`) or clears it (`None`). False off the main thread.
pub fn set_badge_label(label: Option<&str>) -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    let tile = NSApplication::sharedApplication(mtm).dockTile();
    let text = label.map(NSString::from_str);
    tile.setBadgeLabel(text.as_deref());
    true
}

/// The Dock tile's badge as it stands; None when there is none or off the main thread.
pub fn badge_label() -> Option<String> {
    let mtm = MainThreadMarker::new()?;
    let tile = NSApplication::sharedApplication(mtm).dockTile();
    tile.badgeLabel().map(|s| s.to_string())
}

/// The taskbar overlay's shell on macOS: the Dock tile's badge label carries the count text Windows
/// draws in its disc ("3", "9+"); the disc itself is not used (the Dock draws its own).
pub struct DockShell;

impl crate::taskbar::OverlayShell for DockShell {
    fn set_overlay(&mut self, icon: Option<&crate::taskbar::Disc>, _text: &str) -> bool {
        // Only a clear can be done without the count; `set_badge` is the real entry.
        icon.is_none() && set_badge_label(None)
    }

    fn set_badge(
        &mut self,
        label: Option<&str>,
        _icon: Option<&crate::taskbar::Disc>,
        _text: &str,
    ) -> bool {
        set_badge_label(label)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bell_asks_for_attention_until_flare_is_in_front_and_never_while_it_is() {
        assert_eq!(attention_for(true), None);
        assert_eq!(attention_for(false), Some(Attention::Critical));
        assert_eq!(
            request_type(Attention::Critical),
            NSRequestUserAttentionType::CriticalRequest
        );
        assert_eq!(
            request_type(Attention::Informational),
            NSRequestUserAttentionType::InformationalRequest
        );
    }

    #[test]
    fn off_the_main_thread_nothing_is_touched() {
        // libtest runs this on a worker: AppKit is never called from it.
        assert!(!is_active());
        assert_eq!(request_attention(Attention::Critical), None);
        assert!(!set_badge_label(Some("1")));
        assert_eq!(badge_label(), None);
        cancel_attention(1);
    }
}

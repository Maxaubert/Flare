//! The walls of a mod's or plugin's view (design 13), as data: [`plugin_seal`] says what a view of each kind may
//! do, the host applies all of it in one function (`WebHost::seal_plugin_view`) to every mod and plugin view,
//! and the fake records it, so the walls are tested here without a browser.
//!
//! Every kind: no script dialog (an `alert`, `confirm`, `prompt` or leave-page question is dismissed at once and
//! never shown), no window of its own, no permission, no external program, no developer tools or status bar, and
//! frames only of its own host. A bar or a background page also has no context menu, no browser keys (reload,
//! find, print, zoom) and no download; a tab's panel keeps those as a mod's panel always had them, and its
//! downloads wait for the person's yes like the web panel's (S6).

use flare_surfaces::slot::ViewKind;

/// What happens to a download the page starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DownloadWall {
    /// Held until the person says yes or no (`WebEvent::Download`); nothing is saved without a yes.
    Ask,
    /// Cancelled at once: nobody is there to ask.
    Cancel,
}

/// What a view may do (true) or is walled off from (false).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seal {
    pub kind: ViewKind,
    /// The browser's own dialogs for `alert`, `confirm`, `prompt` and `beforeunload`.
    pub script_dialogs: bool,
    /// The browser's right-click menu.
    pub context_menu: bool,
    /// The browser's own keys (F5, Ctrl+F, Ctrl+P, Ctrl+plus and the rest). The app's chords are taken
    /// either way.
    pub browser_keys: bool,
    pub dev_tools: bool,
    pub status_bar: bool,
    /// `window.open` and `target=_blank`: refused when false.
    pub new_windows: bool,
    /// Camera, microphone, location, notifications, clipboard and the rest: denied when false.
    pub permissions: bool,
    /// `mailto:` and other schemes that start a program: refused when false.
    pub external_uris: bool,
    /// A frame may navigate only to its own host, `about:blank` or `about:srcdoc`.
    pub frames_own_only: bool,
    pub downloads: DownloadWall,
}

/// The walls of a mod's or plugin's view of `kind`.
pub fn plugin_seal(kind: ViewKind) -> Seal {
    // A tab's panel is the person's to use: its menu and keys stay as they were for a mod's panel. A bar or a
    // background page is not something the person works in.
    let panel = kind == ViewKind::Panel;
    Seal {
        kind,
        script_dialogs: false,
        context_menu: panel,
        browser_keys: panel,
        dev_tools: false,
        status_bar: false,
        new_windows: false,
        permissions: false,
        external_uris: false,
        frames_own_only: true,
        downloads: if panel {
            DownloadWall::Ask
        } else {
            DownloadWall::Cancel
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The walls every kind holds.
    fn walled_everywhere(s: &Seal) {
        assert!(!s.script_dialogs, "{:?}: no script dialog", s.kind);
        assert!(!s.dev_tools && !s.status_bar, "{:?}", s.kind);
        assert!(!s.new_windows, "{:?}: no window of its own", s.kind);
        assert!(!s.permissions, "{:?}: every permission denied", s.kind);
        assert!(!s.external_uris, "{:?}: no external program", s.kind);
        assert!(s.frames_own_only, "{:?}: frames of its own host", s.kind);
    }

    #[test]
    fn a_panel_is_sealed_and_keeps_its_menu_keys_and_the_download_question() {
        let s = plugin_seal(ViewKind::Panel);
        walled_everywhere(&s);
        assert!(s.context_menu && s.browser_keys);
        assert_eq!(s.downloads, DownloadWall::Ask);
    }

    #[test]
    fn a_bar_is_sealed_with_no_menu_no_browser_keys_and_no_download() {
        let s = plugin_seal(ViewKind::Bar);
        walled_everywhere(&s);
        assert!(!s.context_menu && !s.browser_keys);
        assert_eq!(s.downloads, DownloadWall::Cancel);
    }

    #[test]
    fn a_background_page_is_sealed_with_no_menu_no_browser_keys_and_no_download() {
        let s = plugin_seal(ViewKind::Background);
        walled_everywhere(&s);
        assert!(!s.context_menu && !s.browser_keys);
        assert_eq!(s.downloads, DownloadWall::Cancel);
    }
}

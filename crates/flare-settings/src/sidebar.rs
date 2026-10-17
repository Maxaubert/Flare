//! Sidebar prefs (Task 3): the tab sidebar's mode (open or collapsed) and its width in logical pixels.
//! Keys `flare.window.sidebar` ("open" | "collapsed", fallback open) and `flare.window.sidebarWidth`
//! (a number, clamped to 150..=420, whole; junk reads the initial 220).

use crate::kv::{PrefStore, Subscription, on_any_change};

const MODE_KEY: &str = "flare.window.sidebar";
const WIDTH_KEY: &str = "flare.window.sidebarWidth";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarMode {
    Open,
    Collapsed,
}

pub struct SidebarWidth {
    pub initial: i32,
    pub min: i32,
    pub max: i32,
}

pub const SIDEBAR_WIDTH: SidebarWidth = SidebarWidth {
    initial: 220,
    min: 150,
    max: 420,
};

/// "collapsed" is collapsed; anything else (unset, unknown) is open.
pub fn valid_sidebar_mode(v: Option<&str>) -> SidebarMode {
    match v {
        Some("collapsed") => SidebarMode::Collapsed,
        _ => SidebarMode::Open,
    }
}

/// A number, clamped and rounded; anything that is not a number reads the initial width.
pub fn valid_sidebar_width(v: Option<&str>) -> i32 {
    match v.map(|s| s.trim().parse::<f64>()) {
        Some(Ok(n)) => valid_sidebar_width_num(n),
        _ => SIDEBAR_WIDTH.initial,
    }
}

/// NaN reads the initial width; any other number is clamped to the range and rounded to whole pixels.
pub fn valid_sidebar_width_num(n: f64) -> i32 {
    if n.is_nan() {
        return SIDEBAR_WIDTH.initial;
    }
    n.clamp(f64::from(SIDEBAR_WIDTH.min), f64::from(SIDEBAR_WIDTH.max))
        .round() as i32
}

pub fn sidebar_mode(s: &PrefStore) -> SidebarMode {
    valid_sidebar_mode(s.get(MODE_KEY).as_deref())
}

pub fn set_sidebar_mode_stored(s: &PrefStore, m: SidebarMode) {
    let word = match m {
        SidebarMode::Open => "open",
        SidebarMode::Collapsed => "collapsed",
    };
    s.set(MODE_KEY, word);
}

pub fn sidebar_width(s: &PrefStore) -> i32 {
    valid_sidebar_width(s.get(WIDTH_KEY).as_deref())
}

pub fn set_sidebar_width(s: &PrefStore, px: f64) {
    s.set(WIDTH_KEY, &valid_sidebar_width_num(px).to_string());
}

pub fn toggle_sidebar_mode(s: &PrefStore) {
    let next = match sidebar_mode(s) {
        SidebarMode::Open => SidebarMode::Collapsed,
        SidebarMode::Collapsed => SidebarMode::Open,
    };
    set_sidebar_mode_stored(s, next);
}

/// Fires for both the mode and the width.
pub fn on_sidebar_change(s: &PrefStore, cb: impl Fn() + Send + Sync + 'static) -> Subscription {
    on_any_change(s, &[MODE_KEY, WIDTH_KEY], cb)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn sidebar_is_open_at_220_until_chosen() {
        let s = PrefStore::memory();
        assert_eq!(sidebar_mode(&s), SidebarMode::Open);
        assert_eq!(sidebar_width(&s), 220);
    }

    #[test]
    fn sidebar_mode_is_remembered() {
        let s = PrefStore::memory();
        set_sidebar_mode_stored(&s, SidebarMode::Collapsed);
        assert_eq!(s.get("flare.window.sidebar"), Some("collapsed".to_string()));
        assert_eq!(sidebar_mode(&s), SidebarMode::Collapsed);
        set_sidebar_mode_stored(&s, SidebarMode::Open);
        assert_eq!(sidebar_mode(&s), SidebarMode::Open);
    }

    #[test]
    fn sidebar_unknown_mode_is_open() {
        assert_eq!(valid_sidebar_mode(Some("floating")), SidebarMode::Open);
        assert_eq!(valid_sidebar_mode(None), SidebarMode::Open);
    }

    #[test]
    fn sidebar_width_is_clamped_whole_and_junk_is_220() {
        let s = PrefStore::memory();
        set_sidebar_width(&s, 260.6);
        assert_eq!(sidebar_width(&s), 261);
        set_sidebar_width(&s, 5.0);
        assert_eq!(sidebar_width(&s), 150);
        set_sidebar_width(&s, 9999.0);
        assert_eq!(sidebar_width(&s), 420);
        s.set("flare.window.sidebarWidth", "wide");
        assert_eq!(sidebar_width(&s), 220);
        assert_eq!(valid_sidebar_width(Some("")), 220);
        assert_eq!(valid_sidebar_width(None), 220);
        assert_eq!(valid_sidebar_width(Some("300")), 300);
        assert_eq!(valid_sidebar_width_num(f64::NAN), 220);
    }

    #[test]
    fn sidebar_listeners_hear_both_keys() {
        let s = PrefStore::memory();
        let hits = Arc::new(AtomicUsize::new(0));
        let h = Arc::clone(&hits);
        let sub = on_sidebar_change(&s, move || {
            h.fetch_add(1, Ordering::SeqCst);
        });
        set_sidebar_mode_stored(&s, SidebarMode::Collapsed);
        set_sidebar_width(&s, 300.0);
        assert_eq!(hits.load(Ordering::SeqCst), 2);
        sub.cancel();
        set_sidebar_mode_stored(&s, SidebarMode::Open);
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn toggle_sidebar_mode_flips() {
        let s = PrefStore::memory();
        toggle_sidebar_mode(&s);
        assert_eq!(sidebar_mode(&s), SidebarMode::Collapsed);
        toggle_sidebar_mode(&s);
        assert_eq!(sidebar_mode(&s), SidebarMode::Open);
    }
}

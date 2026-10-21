//! The command help preferences: the switch and the shell (M7 Task 12).
//!
//! Keys: `flare.help.enabled` (on unless "0"), `flare.help.shell` (the shell picked by hand:
//! `powershell`, `cmd` or `bash`; anything else reads as never picked), `flare.help.panelWidth`
//! (the side panel's width in CSS px, clamped 420 to 1100 on the way in and out, 620 until resized).

use crate::kv::{PrefStore, Subscription, bool_pref_get, bool_pref_set, on_any_change};

pub const ENABLED_KEY: &str = "flare.help.enabled";
pub const SHELL_KEY: &str = "flare.help.shell";
pub const WIDTH_KEY: &str = "flare.help.panelWidth";
pub const HELP_KEYS: [&str; 3] = [ENABLED_KEY, SHELL_KEY, WIDTH_KEY];

/// The side panel's width bounds in CSS pixels.
pub struct HelpWidth {
    pub min: u32,
    pub max: u32,
    pub default: u32,
}

pub const HELP_WIDTH: HelpWidth = HelpWidth {
    min: 420,
    max: 1100,
    default: 620,
};

const SHELLS: [&str; 3] = ["powershell", "cmd", "bash"];

/// On unless switched off.
pub fn help_enabled(s: &PrefStore) -> bool {
    bool_pref_get(s, ENABLED_KEY, true)
}

pub fn set_help_enabled(s: &PrefStore, on: bool) {
    bool_pref_set(s, ENABLED_KEY, on);
}

/// The shell picked by hand, or None when never picked (or the stored word is not one of the three).
pub fn help_shell(s: &PrefStore) -> Option<String> {
    let v = s.get(SHELL_KEY)?;
    SHELLS.contains(&v.as_str()).then_some(v)
}

/// Stores the shell only when it is one of the three; anything else is dropped.
pub fn set_help_shell(s: &PrefStore, shell: &str) {
    if SHELLS.contains(&shell) {
        s.set(SHELL_KEY, shell);
    }
}

/// A finite number above zero is rounded and clamped to the bounds; anything else is the default.
pub fn clamp_help_width(n: f64) -> u32 {
    if n.is_finite() && n > 0.0 {
        let rounded = n
            .round()
            .clamp(f64::from(HELP_WIDTH.min), f64::from(HELP_WIDTH.max));
        // The value is already inside the u32 bounds, so the cast is exact.
        rounded as u32
    } else {
        HELP_WIDTH.default
    }
}

/// Never set, empty or junk reads as the default (never zero); a stored number is clamped.
pub fn help_width(s: &PrefStore) -> u32 {
    match s
        .get(WIDTH_KEY)
        .and_then(|raw| raw.trim().parse::<f64>().ok())
    {
        Some(n) => clamp_help_width(n),
        None => HELP_WIDTH.default,
    }
}

/// Stores the clamped whole-pixel width.
pub fn set_help_width(s: &PrefStore, px: f64) {
    s.set(WIDTH_KEY, &clamp_help_width(px).to_string());
}

/// Calls `cb` on a change of any of the three keys; dropping the subscription stops it.
pub fn on_help_change(s: &PrefStore, cb: impl Fn() + Send + Sync + 'static) -> Subscription {
    on_any_change(s, &HELP_KEYS, cb)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kv::BlockedBackend;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn help_is_on_until_switched_off_and_off_means_off() {
        let s = PrefStore::memory();
        assert!(help_enabled(&s));
        set_help_enabled(&s, false);
        assert!(!help_enabled(&s));
        assert_eq!(s.get(ENABLED_KEY), Some("0".to_string()));
        set_help_enabled(&s, true);
        assert!(help_enabled(&s));
    }

    #[test]
    fn a_hand_picked_shell_is_remembered_and_junk_reads_never_picked() {
        let s = PrefStore::memory();
        assert_eq!(help_shell(&s), None);
        set_help_shell(&s, "bash");
        assert_eq!(help_shell(&s), Some("bash".to_string()));
        s.set(SHELL_KEY, "fish");
        assert_eq!(help_shell(&s), None);
        set_help_shell(&s, "fish");
        assert_eq!(s.get(SHELL_KEY), Some("fish".to_string()));
    }

    #[test]
    fn listeners_hear_a_change_and_let_go() {
        let s = PrefStore::memory();
        let hits = Arc::new(AtomicUsize::new(0));
        let h = Arc::clone(&hits);
        let sub = on_help_change(&s, move || {
            h.fetch_add(1, Ordering::SeqCst);
        });
        set_help_enabled(&s, false);
        set_help_shell(&s, "cmd");
        set_help_width(&s, 700.0);
        assert_eq!(hits.load(Ordering::SeqCst), 3);
        s.set("flare.other", "x");
        assert_eq!(hits.load(Ordering::SeqCst), 3);
        sub.cancel();
        set_help_enabled(&s, true);
        assert_eq!(hits.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn the_keys_are_flares_own() {
        for k in HELP_KEYS {
            assert!(k.starts_with("flare.help."), "{k}");
        }
    }

    #[test]
    fn the_width_is_the_default_until_resized_never_set_is_not_zero() {
        let s = PrefStore::memory();
        assert_eq!(help_width(&s), HELP_WIDTH.default);
        s.set(WIDTH_KEY, "");
        assert_eq!(help_width(&s), HELP_WIDTH.default);
        s.set(WIDTH_KEY, "wide");
        assert_eq!(help_width(&s), HELP_WIDTH.default);
    }

    #[test]
    fn the_width_is_clamped_in_and_out() {
        let s = PrefStore::memory();
        set_help_width(&s, 900.0);
        assert_eq!(help_width(&s), 900);
        set_help_width(&s, 9000.0);
        assert_eq!(help_width(&s), HELP_WIDTH.max);
        set_help_width(&s, 10.0);
        assert_eq!(help_width(&s), HELP_WIDTH.min);
        s.set(WIDTH_KEY, "99999");
        assert_eq!(help_width(&s), HELP_WIDTH.max);
    }

    #[test]
    fn the_width_rounds_and_tells_its_listeners() {
        let s = PrefStore::memory();
        let hits = Arc::new(AtomicUsize::new(0));
        let h = Arc::clone(&hits);
        let _sub = on_help_change(&s, move || {
            h.fetch_add(1, Ordering::SeqCst);
        });
        set_help_width(&s, 812.6);
        assert_eq!(help_width(&s), 813);
        assert_eq!(s.get(WIDTH_KEY), Some("813".to_string()));
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn clamp_help_width_treats_non_finite_and_non_positive_as_the_default() {
        assert_eq!(clamp_help_width(f64::NAN), HELP_WIDTH.default);
        assert_eq!(clamp_help_width(f64::INFINITY), HELP_WIDTH.default);
        assert_eq!(clamp_help_width(0.0), HELP_WIDTH.default);
        assert_eq!(clamp_help_width(-5.0), HELP_WIDTH.default);
        assert_eq!(clamp_help_width(419.4), HELP_WIDTH.min);
        assert_eq!(clamp_help_width(1100.4), HELP_WIDTH.max);
    }

    #[test]
    fn a_blocked_store_loses_the_choice_not_the_app() {
        let s = PrefStore::new(Arc::new(BlockedBackend));
        set_help_enabled(&s, false);
        assert!(help_enabled(&s));
        set_help_shell(&s, "bash");
        assert_eq!(help_shell(&s), None);
        assert_eq!(help_width(&s), HELP_WIDTH.default);
    }
}

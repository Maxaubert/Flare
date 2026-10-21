//! The agent settings (Task 16): the attention switches (done, question, failed, rainbow), the
//! "Exact status from Claude Code" switch, the taskbar badge and Keep agent sessions open.
//!
//! Every switch is on unless its stored value is "0". The indicator word and the three colour
//! picks are M4's `look` store and are NOT here (decision D13). The rows in the settings window
//! are M7's.

use crate::kv::{PrefStore, Subscription, bool_pref_get, bool_pref_set, on_any_change};

const DONE_ON: &str = "flare.term.agentDoneOn";
const QUESTION_ON: &str = "flare.term.agentQuestionOn";
const FAILED_ON: &str = "flare.term.agentFailedOn";
const RAINBOW: &str = "flare.term.agentRainbow";
const HOOKS: &str = "flare.term.agentHooks";
const TASKBAR_BADGE: &str = "flare.window.taskbarBadge";
const AGENT_GUARD: &str = "flare.window.agentGuard";

/// The look keys the marks follow, for one subscription: M4's four plus M6's three attention
/// switches and the rainbow.
pub const AGENT_LOOK_KEYS: [&str; 8] = [
    "flare.term.agentIndicator",
    "flare.term.agentColor",
    "flare.term.agentDoneColor",
    "flare.term.agentQuestionColor",
    DONE_ON,
    QUESTION_ON,
    FAILED_ON,
    RAINBOW,
];

pub fn agent_done_on(s: &PrefStore) -> bool {
    bool_pref_get(s, DONE_ON, true)
}

pub fn set_agent_done_on(s: &PrefStore, on: bool) {
    bool_pref_set(s, DONE_ON, on);
}

pub fn agent_question_on(s: &PrefStore) -> bool {
    bool_pref_get(s, QUESTION_ON, true)
}

pub fn set_agent_question_on(s: &PrefStore, on: bool) {
    bool_pref_set(s, QUESTION_ON, on);
}

pub fn agent_failed_on(s: &PrefStore) -> bool {
    bool_pref_get(s, FAILED_ON, true)
}

pub fn set_agent_failed_on(s: &PrefStore, on: bool) {
    bool_pref_set(s, FAILED_ON, on);
}

pub fn agent_rainbow(s: &PrefStore) -> bool {
    bool_pref_get(s, RAINBOW, true)
}

pub fn set_agent_rainbow(s: &PrefStore, on: bool) {
    bool_pref_set(s, RAINBOW, on);
}

/// "Exact status from Claude Code": a NEW shell is handed the plugin while this is on.
pub fn agent_hooks_on(s: &PrefStore) -> bool {
    bool_pref_get(s, HOOKS, true)
}

pub fn set_agent_hooks_on(s: &PrefStore, on: bool) {
    bool_pref_set(s, HOOKS, on);
}

pub fn taskbar_badge_on(s: &PrefStore) -> bool {
    bool_pref_get(s, TASKBAR_BADGE, true)
}

pub fn set_taskbar_badge_on(s: &PrefStore, on: bool) {
    bool_pref_set(s, TASKBAR_BADGE, on);
}

/// Keep agent sessions open: with an agent in the front tab Ctrl+D never reaches it.
pub fn agent_guard(s: &PrefStore) -> bool {
    bool_pref_get(s, AGENT_GUARD, true)
}

pub fn set_agent_guard(s: &PrefStore, on: bool) {
    bool_pref_set(s, AGENT_GUARD, on);
}

/// Any of `AGENT_LOOK_KEYS` changed.
pub fn on_agent_look_change(s: &PrefStore, cb: impl Fn() + Send + Sync + 'static) -> Subscription {
    on_any_change(s, &AGENT_LOOK_KEYS, cb)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const ALL_KEYS: [&str; 7] = [
        DONE_ON,
        QUESTION_ON,
        FAILED_ON,
        RAINBOW,
        HOOKS,
        TASKBAR_BADGE,
        AGENT_GUARD,
    ];

    #[test]
    fn the_switches_are_on_unless_stored_zero() {
        let s = PrefStore::memory();
        assert!(agent_done_on(&s));
        assert!(agent_question_on(&s));
        assert!(agent_failed_on(&s));
        assert!(agent_rainbow(&s));
        assert!(agent_hooks_on(&s));
        assert!(taskbar_badge_on(&s));
        assert!(agent_guard(&s));

        set_agent_done_on(&s, false);
        assert_eq!(s.get(DONE_ON).as_deref(), Some("0"));
        set_agent_done_on(&s, true);
        assert_eq!(s.get(DONE_ON).as_deref(), Some("1"));

        set_agent_question_on(&s, false);
        set_agent_failed_on(&s, false);
        set_agent_rainbow(&s, false);
        set_agent_hooks_on(&s, false);
        set_taskbar_badge_on(&s, false);
        set_agent_guard(&s, false);
        assert!(!agent_question_on(&s));
        assert!(!agent_failed_on(&s));
        assert!(!agent_rainbow(&s));
        assert!(!agent_hooks_on(&s));
        assert!(!taskbar_badge_on(&s));
        assert!(!agent_guard(&s));

        s.set(FAILED_ON, "x");
        assert!(agent_failed_on(&s), "a stored value other than 0 reads on");
    }

    #[test]
    fn the_keys_are_flares_own() {
        let s = PrefStore::memory();
        let setters: [fn(&PrefStore, bool); 7] = [
            set_agent_done_on,
            set_agent_question_on,
            set_agent_failed_on,
            set_agent_rainbow,
            set_agent_hooks_on,
            set_taskbar_badge_on,
            set_agent_guard,
        ];
        for (setter, key) in setters.iter().zip(ALL_KEYS) {
            setter(&s, false);
            let raw = s.get(key).expect("setter writes its key");
            assert_eq!(raw, "0");
            assert!(key.starts_with("flare."), "{key} is not a flare.* key");
        }
        for key in AGENT_LOOK_KEYS {
            assert!(key.starts_with("flare."), "{key} is not a flare.* key");
        }
    }

    #[test]
    fn a_change_of_any_look_key_calls_the_listener_once() {
        let s = PrefStore::memory();
        let calls = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&calls);
        let sub = on_agent_look_change(&s, move || {
            c.fetch_add(1, Ordering::SeqCst);
        });

        s.set("flare.term.agentIndicator", "minimal");
        s.set("flare.term.agentColor", "#e07a2f");
        set_agent_done_on(&s, false);
        assert_eq!(calls.load(Ordering::SeqCst), 3);

        set_agent_hooks_on(&s, false);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "agentHooks is not a look key"
        );

        sub.cancel();
        set_agent_rainbow(&s, false);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "a cancelled listener stays quiet"
        );
    }

    #[test]
    fn the_indicator_and_colour_keys_have_no_second_writer() {
        let src = include_str!("agent_prefs.rs");
        let before_tests = src.split("#[cfg(test)]").next().unwrap_or(src);
        // The one allowed mention of the indicator key is its entry in AGENT_LOOK_KEYS.
        let without_list = before_tests
            .split("pub const AGENT_LOOK_KEYS")
            .next()
            .unwrap_or(before_tests)
            .to_string()
            + before_tests.split("];").nth(1).unwrap_or("");
        assert!(!without_list.contains("agentIndicator\""));
        assert!(!without_list.contains("fn set_agent_colour"));
        assert!(!without_list.contains("fn agent_indicator"));
    }
}

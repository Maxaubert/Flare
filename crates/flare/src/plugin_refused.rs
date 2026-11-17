//! A plugin call Flare refused leaves a `plugin-refused` crumb in `diag.jsonl` (rule 1273, 2026-11-16): the plugin's
//! id, the method, the error code and the way it came (`page` for a plugin's page, `pipe` for its helper). Never the
//! error's words, the params or anything a page holds: those can carry what the plugin was given. One crumb per plugin,
//! method and code a minute, so a helper that retries four times a second does not fill the log.
//!
//! A plugin's title that never changed in the menu (Bitwarden 0.3.2) left nothing in the log to tell whether Flare had
//! refused the call; now a refusal is always there to read.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The same refusal is noted again after this long.
pub const EVERY: Duration = Duration::from_secs(60);
/// Refusals remembered at most; past it the memory starts again (it is only a minute of quiet).
const KEPT_MAX: usize = 256;

/// When each (plugin, method, code) was last noted.
#[derive(Debug, Default)]
pub struct Throttle {
    last: HashMap<(String, String, String), Instant>,
}

impl Throttle {
    /// Whether this refusal is noted now (and remembered as noted).
    pub fn due(&mut self, plugin: &str, method: &str, code: &str, now: Instant) -> bool {
        let key = (plugin.to_string(), method.to_string(), code.to_string());
        if self
            .last
            .get(&key)
            .is_some_and(|t| now.saturating_duration_since(*t) < EVERY)
        {
            return false;
        }
        if self.last.len() >= KEPT_MAX {
            self.last.clear();
        }
        self.last.insert(key, now);
        true
    }
}

fn throttle() -> &'static Mutex<Throttle> {
    static T: OnceLock<Mutex<Throttle>> = OnceLock::new();
    T.get_or_init(Mutex::default)
}

/// Notes that Flare refused `method` from `plugin` with `code`; `via` is `page` or `pipe`.
pub fn note(plugin: &str, method: &str, code: &str, via: &str) {
    let due = throttle().lock().unwrap_or_else(|e| e.into_inner()).due(
        plugin,
        method,
        code,
        Instant::now(),
    );
    if !due {
        return;
    }
    use crate::diag_ctl::{crumb, txt};
    crumb(
        "plugin-refused",
        &[
            ("plugin", txt(plugin)),
            ("method", txt(method)),
            ("code", txt(code)),
            ("via", txt(via)),
        ],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_refusal_is_noted_once_a_minute_and_another_at_once() {
        let mut t = Throttle::default();
        let at = Instant::now();
        assert!(t.due("vault", "actions.set", "bad_params", at));
        assert!(!t.due(
            "vault",
            "actions.set",
            "bad_params",
            at + Duration::from_secs(59)
        ));
        // Another code, method or plugin is its own.
        assert!(t.due("vault", "actions.set", "forbidden", at));
        assert!(t.due("vault", "storage.set", "bad_params", at));
        assert!(t.due("other", "actions.set", "bad_params", at));
        assert!(t.due("vault", "actions.set", "bad_params", at + EVERY));
    }

    #[test]
    fn the_memory_starts_again_when_full() {
        let mut t = Throttle::default();
        let at = Instant::now();
        for i in 0..KEPT_MAX {
            assert!(t.due(&format!("p{i}"), "m", "c", at));
        }
        assert!(t.due("last", "m", "c", at));
        assert_eq!(t.last.len(), 1);
    }
}

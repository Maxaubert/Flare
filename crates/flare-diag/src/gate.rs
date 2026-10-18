//! M9 Task 17: the error gate (which errors reach the log, and how often).
//!
//! Port of Prism Terminal's `core/shared/diagGate.ts`: one error, not a flood. The same error is
//! written once per window, its copies are counted and the next line says how many; at most
//! `budget` lines per kind per window, whatever they say; `sweep` writes what is still held.

use std::collections::HashMap;

/// The window the callers use (Prism Terminal's default).
pub const DEFAULT_EVERY_MS: i64 = 10_000;
/// The lines per kind per window the callers use (Prism Terminal's default).
pub const DEFAULT_BUDGET: u32 = 10;

/// Past this many distinct errors, a new one over budget is not remembered.
const MAX_KEYS: usize = 200;
/// A time long before any clock reading: a key never sent is due at once.
const NEVER: i64 = i64::MIN / 2;

struct Entry<T> {
    kind: String,
    line: T,
    last_sent: i64,
    held: u32,
}

#[derive(Clone, Copy)]
struct Window {
    start: i64,
    sent: u32,
}

/// The error gate. Entries keep their insertion order, as the JS `Map` did, so `sweep` answers in the
/// same order as Prism Terminal's.
pub struct DiagGate<T: Clone> {
    every_ms: i64,
    budget: u32,
    entries: Vec<(String, Entry<T>)>,
    windows: HashMap<String, Window>,
}

impl<T: Clone> DiagGate<T> {
    /// A gate that writes each error at most once per `every_ms` and at most `budget` lines per kind
    /// per window.
    pub fn new(every_ms: i64, budget: u32) -> Self {
        Self {
            every_ms,
            budget,
            entries: Vec::new(),
            windows: HashMap::new(),
        }
    }

    /// Takes one line for `kind` (an error's key is its kind and message). Some((line, repeats)) when the
    /// line is written now, with `repeats` when copies were held since the last one; None when it is held.
    pub fn offer(&mut self, kind: &str, key: &str, line: T, now: i64) -> Option<(T, Option<u32>)> {
        let idx = self.entries.iter().position(|(k, _)| k == key);
        if let Some(i) = idx {
            let e = &mut self.entries[i].1;
            if now.saturating_sub(e.last_sent) < self.every_ms {
                e.held += 1;
                return None;
            }
        }
        if !self.allow(kind, now) {
            match idx {
                Some(i) => self.entries[i].1.held += 1,
                None if self.entries.len() < MAX_KEYS => self.entries.push((
                    key.to_owned(),
                    Entry {
                        kind: kind.to_owned(),
                        line,
                        last_sent: NEVER,
                        held: 1,
                    },
                )),
                None => {}
            }
            return None;
        }
        let held = idx.map_or(0, |i| self.entries[i].1.held);
        let entry = Entry {
            kind: kind.to_owned(),
            line: line.clone(),
            last_sent: now,
            held: 0,
        };
        match idx {
            Some(i) => self.entries[i].1 = entry,
            None => self.entries.push((key.to_owned(), entry)),
        }
        Some((line, (held > 0).then_some(held)))
    }

    /// The held copies whose time has come: one line each, with how many copies it stands for.
    pub fn sweep(&mut self, now: i64) -> Vec<(T, u32)> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < self.entries.len() {
            let (last, held, kind) = {
                let e = &self.entries[i].1;
                (e.last_sent, e.held, e.kind.clone())
            };
            let quiet = now.saturating_sub(last);
            if quiet < self.every_ms {
                i += 1;
                continue;
            }
            if held == 0 {
                // Quiet for a while: forget it, so the map stays small.
                if quiet >= self.every_ms * 6 {
                    self.entries.remove(i);
                } else {
                    i += 1;
                }
                continue;
            }
            if !self.allow(&kind, now) {
                i += 1;
                continue;
            }
            let e = &mut self.entries[i].1;
            out.push((e.line.clone(), e.held));
            e.held = 0;
            e.last_sent = now;
            i += 1;
        }
        out
    }

    /// Whether one more line of this kind fits in its window (and counts it when it does).
    fn allow(&mut self, kind: &str, now: i64) -> bool {
        let every = self.every_ms;
        let fresh = match self.windows.get(kind) {
            Some(w) => now.saturating_sub(w.start) >= every,
            None => true,
        };
        if fresh {
            self.windows.insert(
                kind.to_owned(),
                Window {
                    start: now,
                    sent: 0,
                },
            );
        }
        let budget = self.budget;
        let Some(w) = self.windows.get_mut(kind) else {
            return false;
        };
        if w.sent >= budget {
            return false;
        }
        w.sent += 1;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    #[test]
    fn writes_an_error_once_holds_its_copies_and_says_how_many_on_the_next_line() {
        let mut gate = DiagGate::<Value>::new(10_000, 10);
        let line = json!({"msg": "boom"});
        assert_eq!(
            gate.offer("page-error", "boom", line.clone(), 0),
            Some((line.clone(), None))
        );
        // Sixty a second for five seconds: none written.
        for t in (16..5000).step_by(16) {
            assert!(gate.offer("page-error", "boom", line.clone(), t).is_none());
        }
        assert!(gate.sweep(5000).is_empty());
        let (again, repeats) = gate
            .offer("page-error", "boom", line.clone(), 10_000)
            .expect("written");
        assert_eq!(again, line);
        assert!(repeats.is_some_and(|n| n > 300), "{repeats:?}");
    }

    #[test]
    fn sweeps_a_burst_that_stopped_so_its_copies_are_still_counted() {
        let mut gate = DiagGate::<Value>::new(10_000, 10);
        let line = json!({"msg": "boom"});
        gate.offer("page-error", "boom", line.clone(), 0);
        gate.offer("page-error", "boom", line.clone(), 100);
        gate.offer("page-error", "boom", line.clone(), 200);
        assert!(gate.sweep(9_999).is_empty());
        assert_eq!(gate.sweep(10_000), vec![(line.clone(), 2)]);
        assert!(gate.sweep(20_000).is_empty());
    }

    #[test]
    fn caps_the_lines_per_kind_whatever_they_say() {
        // A message with a counter in it is a new key every time.
        let mut gate = DiagGate::<Value>::new(10_000, 10);
        let written = (0..100_i64)
            .filter(|&i| {
                gate.offer(
                    "page-error",
                    &format!("n{i}"),
                    json!({"msg": format!("n{i}")}),
                    i,
                )
                .is_some()
            })
            .count();
        assert_eq!(written, 10);
        // Another kind has its own budget.
        assert!(
            gate.offer("page-rejection", "r", json!({"msg": "r"}), 100)
                .is_some()
        );
        // The held ones come out as the budget allows, ten per window.
        assert_eq!(gate.sweep(10_000).len(), 10);
    }

    #[test]
    fn a_quiet_key_is_forgotten_after_six_windows() {
        let mut gate = DiagGate::<Value>::new(10_000, 10);
        gate.offer("page-error", "boom", json!({"msg": "boom"}), 0);
        assert_eq!(gate.entries.len(), 1);
        assert!(gate.sweep(59_999).is_empty());
        assert_eq!(gate.entries.len(), 1);
        assert!(gate.sweep(60_000).is_empty());
        assert!(gate.entries.is_empty());
    }
}

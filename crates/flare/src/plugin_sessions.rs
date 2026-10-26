//! What the plugins that hold `sessions` hear of Claude sessions (plugins phase 5, design 6 and 13): the followers of
//! the tabs' transcripts and each plugin's share of what they hand over. UI thread state; the followers' threads
//! read the files.
//!
//! - A tab is followed only while a page of a plugin that holds `sessions` listens to `session` and may see the tab
//!   (its own tab, any tab with `tabs`, a tab where the person clicked one of its actions), and the tab's Claude
//!   session is known. Nobody listening, the plugin turned off, the session left or the tab closed: the follower is
//!   dropped and its thread ends ([`PluginSessions::sync`]).
//! - Each plugin hears at most [`RATE`] entries a second; what is over is dropped and counted, and the next event it
//!   hears carries the count (`dropped`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use flare_convo::follow::{Follower, Said};
use flare_convo::track::Source;

use crate::appctl::{Post, UiMsg};
use crate::mods_ctl::{ModsMsg, ModsMsgBox};

/// Entries one plugin hears in a second, and as many at once (design 6).
pub const RATE: f64 = 50.0;
/// `session.read` calls one plugin makes in a second on one tab (design 13).
pub const READ_RATE: f64 = 1.0;
/// `tabs.input` calls on one tab in a second, whoever makes them (design 13).
pub const INPUT_RATE: f64 = 2.0;
/// The most bytes of text one `tabs.input` takes (design 7).
pub const INPUT_MAX: usize = 4 * 1024;
/// How long after a plugin's paste its Enter is written (the checked send's wait: the program has the words by then).
pub const ENTER_AFTER_MS: u64 = flare_dictation::route::SEND_ENTER_MS;

/// A tab to follow: its folder (where Claude Code files the session) and its Claude session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Want {
    pub tab: String,
    pub folder: String,
    pub session: String,
}

struct Followed {
    session: String,
    /// Held for its life: dropping it stops the thread.
    #[cfg_attr(not(test), allow(dead_code))]
    follower: Follower,
}

/// A plugin's share: its bucket, and what it was not handed since it last heard an event.
struct Share {
    bucket: flare_ctl::session::Bucket,
    dropped: u64,
}

#[derive(Default)]
pub struct PluginSessions {
    followed: HashMap<String, Followed>,
    shares: HashMap<String, Share>,
}

impl PluginSessions {
    /// Brings the followers in line with the tabs that are wanted: one per wanted tab and session, none for the
    /// rest. `dir` is Claude's folder (None: none is known, nothing runs). True when one started or stopped.
    pub fn sync(&mut self, wants: &[Want], dir: Option<PathBuf>, post: &Post) -> bool {
        let before = self.followed.len();
        self.followed.retain(|tab, f| {
            wants
                .iter()
                .any(|w| &w.tab == tab && w.session == f.session)
        });
        let mut changed = self.followed.len() != before;
        let Some(dir) = dir else {
            changed |= !self.followed.is_empty();
            self.followed.clear();
            return changed;
        };
        for w in wants {
            if self.followed.contains_key(&w.tab) {
                continue;
            }
            let Some(transcript) = flare_convo::transcript_path_in(&dir, &w.folder, &w.session)
            else {
                continue;
            };
            let post = Arc::clone(post);
            let (tab, session) = (w.tab.clone(), w.session.clone());
            let follower = Follower::start(
                Source {
                    transcript,
                    claude_dir: dir.clone(),
                    session: w.session.clone(),
                },
                Arc::new(move |said: Said| {
                    post(UiMsg::Mods(ModsMsgBox(ModsMsg::Said {
                        tab: tab.clone(),
                        session: session.clone(),
                        said,
                    })));
                }),
            );
            self.followed.insert(
                w.tab.clone(),
                Followed {
                    session: w.session.clone(),
                    follower,
                },
            );
            changed = true;
        }
        changed
    }

    /// Whether `tab` is followed for `session` now (a batch for a session since left, or a tab no longer followed,
    /// is dropped).
    pub fn follows(&self, tab: &str, session: &str) -> bool {
        self.followed.get(tab).is_some_and(|f| f.session == session)
    }

    /// How many tabs are followed (the tests, and the "nothing runs while nobody listens" check).
    pub fn count(&self) -> usize {
        self.followed.len()
    }

    /// The tabs followed, sorted.
    pub fn tabs(&self) -> Vec<String> {
        let mut v: Vec<String> = self.followed.keys().cloned().collect();
        v.sort();
        v
    }

    /// A follower's exit flag (a test waits for it to see the thread really stop).
    #[cfg(test)]
    pub fn exit_flag(&self, tab: &str) -> Option<Arc<std::sync::atomic::AtomicBool>> {
        self.followed.get(tab).map(|f| f.follower.exit_flag())
    }

    /// One entry for `plugin` at `now`: `Some(dropped)` when it may hear it (with what it missed since its last
    /// event, now reported), None when it is over its share (the entry is counted as dropped).
    pub fn spend(&mut self, plugin: &str, now: Instant) -> Option<u64> {
        let share = self
            .shares
            .entry(plugin.to_string())
            .or_insert_with(|| Share {
                bucket: flare_ctl::session::Bucket::new(RATE, RATE, now),
                dropped: 0,
            });
        if share.bucket.take(now) {
            Some(std::mem::take(&mut share.dropped))
        } else {
            share.dropped += 1;
            None
        }
    }

    /// `n` entries `plugin` will not hear (the follower skipped them): the next event it hears says so.
    pub fn add_dropped(&mut self, plugin: &str, n: u64, now: Instant) {
        if n == 0 {
            return;
        }
        self.shares
            .entry(plugin.to_string())
            .or_insert_with(|| Share {
                bucket: flare_ctl::session::Bucket::new(RATE, RATE, now),
                dropped: 0,
            })
            .dropped += n;
    }

    /// What `plugin` was not handed and has not heard of yet (tests).
    #[cfg(test)]
    pub fn dropped_of(&self, plugin: &str) -> u64 {
        self.shares.get(plugin).map_or(0, |s| s.dropped)
    }

    /// The plugin stopped: its share starts afresh when it is on again.
    pub fn forget_plugin(&mut self, plugin: &str) {
        self.shares.remove(plugin);
    }
}

/// An entry as a plugin is handed it: `{role, text, tool?, at?}` (the `session` event adds the tab, the session and
/// what was dropped).
pub fn entry_json(e: &flare_convo::said::Entry) -> serde_json::Value {
    let mut v = serde_json::json!({"role": e.role.name(), "text": handed_text(&e.text)});
    if let Some(t) = &e.tool {
        v["tool"] = serde_json::json!(handed_text(t));
    }
    if let Some(at) = &e.at {
        v["at"] = serde_json::json!(at);
    }
    v
}

/// The text of an entry as a plugin is handed it: the transcript's clean text, also without the characters that
/// print nothing (`flare_ctl::text::invisible`), which could carry words a person never sees.
pub fn handed_text(text: &str) -> String {
    text.chars()
        .filter(|c| !flare_ctl::text::invisible(*c))
        .collect()
}

/// Why `text` may not go to a shell as a plugin's typing, if it may not: empty, longer than [`INPUT_MAX`] bytes, or
/// holding a control character other than the tab and the line breaks a paste carries (rule 1: nothing a plugin types
/// can end the bracketed paste early or press a key of its own).
pub fn input_refusal(text: &str) -> Option<String> {
    if text.is_empty() {
        return Some("text is required".into());
    }
    if text.len() > INPUT_MAX {
        return Some(format!("text is at most {} KB", INPUT_MAX / 1024));
    }
    let bad = text.chars().find(|&c| {
        let n = c as u32;
        !matches!(c, '\t' | '\n' | '\r') && (n < 0x20 || (0x7F..=0x9F).contains(&n))
    });
    bad.map(|c| {
        format!(
            "text holds the control character U+{:04X}: only a tab and line breaks may be typed",
            c as u32
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_plugin_hears_fifty_entries_a_second_and_the_next_event_says_how_many_it_missed() {
        let mut s = PluginSessions::default();
        let t0 = Instant::now();
        let heard: Vec<u64> = (0..60).filter_map(|_| s.spend("p", t0)).collect();
        assert_eq!(heard.len(), 50);
        assert!(heard.iter().all(|d| *d == 0));
        assert_eq!(s.dropped_of("p"), 10);
        // Another plugin has its own share.
        assert_eq!(s.spend("q", t0), Some(0));
        // A tenth of a second later five more fit, and the first carries the ten it missed.
        let t1 = t0 + Duration::from_millis(100);
        assert_eq!(s.spend("p", t1), Some(10));
        assert_eq!(s.spend("p", t1), Some(0));
        // What the follower skipped is reported the same way.
        s.add_dropped("p", 7, t1);
        assert_eq!(s.spend("p", t1), Some(7));
        s.forget_plugin("p");
        assert_eq!(s.dropped_of("p"), 0);
    }

    #[test]
    fn typing_is_refused_when_empty_too_long_or_holding_a_control_character() {
        assert!(input_refusal("/compact").is_none());
        assert!(input_refusal("two\nlines\r\nand\ta tab").is_none());
        assert!(input_refusal("\u{5b57}\u{1f389}").is_none());
        assert!(input_refusal(&"x".repeat(INPUT_MAX)).is_none());
        assert!(
            input_refusal(&"x".repeat(INPUT_MAX + 1))
                .unwrap()
                .contains("4 KB")
        );
        assert!(input_refusal("").is_some());
        for bad in [
            "end\u{1b}[201~rm -rf ~",
            "\u{3}",
            "a\u{0}b",
            "del\u{7f}",
            "c1\u{9b}201~",
            "bell\u{7}",
            "\u{8}back",
        ] {
            let why = input_refusal(bad).unwrap_or_else(|| panic!("{bad:?} was allowed"));
            assert!(why.contains("control character"), "{why}");
        }
    }

    #[test]
    fn invisible_characters_are_not_handed_on() {
        assert_eq!(handed_text("a\u{200b}b\u{202e}c\u{e0041}d\ne"), "abcd\ne");
    }
}

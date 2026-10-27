//! When a question from an agent is worth a Windows notification (Surfaces stage 7). Pure: the window says
//! which tabs hold an open question, which tab is in front and whether the window has the keyboard; this says
//! which notifications to show now, and when to ask again.
//!
//! The rules:
//! - One notification per question. A question is an EPISODE: it starts when a tab gets the question mark and
//!   ends when the mark goes. However often the window asks again during the episode, it notifies at most once.
//! - None while the person is looking: the tab is in front AND the window has the keyboard. A question that was
//!   ever in view that way is seen, and never notified later (the person left it with the question on screen).
//! - A short grace before showing, so the question's text (read off the screen a moment after the mark) can be
//!   in the notification; the text arriving ends the grace at once.
//! - The setting off notifies nothing, and the episodes that began while it was off stay quiet if it is turned
//!   on in the middle of them.

use std::collections::HashMap;

/// How long a question waits for its text before the notification is shown without it.
pub const GRACE_MS: u64 = 600;
/// The longest title and body shown, in characters.
pub const TITLE_MAX: usize = 60;
pub const BODY_MAX: usize = 220;

/// A tab whose agent has an open question now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asking {
    pub tab: String,
    /// The tab's name as the strip shows it.
    pub name: String,
    /// "Claude", "Codex", "Antigravity".
    pub agent: String,
    /// The question's text, when the screen showed it.
    pub text: Option<String>,
}

/// A notification to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Toast {
    pub tab: String,
    pub title: String,
    pub body: String,
}

#[derive(Debug)]
struct Episode {
    began: u64,
    /// The person has looked at the tab with the window active while the question was up.
    seen: bool,
    /// Shown, or decided against: either way, not again in this episode.
    done: bool,
}

/// The notifier's memory: one entry per tab with an open question.
#[derive(Debug, Default)]
pub struct Notifier {
    episodes: HashMap<String, Episode>,
}

/// What the window says on each look.
pub struct Look<'a> {
    pub now: u64,
    /// "Notify me when an agent asks".
    pub enabled: bool,
    /// The tab in front (a shell tab), if any.
    pub front: Option<&'a str>,
    /// The window has the keyboard (not minimised, not behind another app).
    pub focused: bool,
}

impl Notifier {
    pub fn new() -> Self {
        Self::default()
    }

    /// The tabs with an open question now. Returns the notifications to show.
    pub fn update(&mut self, look: &Look<'_>, asking: &[Asking]) -> Vec<Toast> {
        // An episode ends when its tab no longer asks.
        self.episodes
            .retain(|tab, _| asking.iter().any(|a| &a.tab == tab));
        let mut out = Vec::new();
        for a in asking {
            let looking = look.focused && look.front == Some(a.tab.as_str());
            let ep = self.episodes.entry(a.tab.clone()).or_insert(Episode {
                began: look.now,
                seen: false,
                // A question raised while the setting is off is never announced later.
                done: !look.enabled,
            });
            if ep.done {
                continue;
            }
            if looking {
                ep.seen = true;
            }
            if ep.seen {
                ep.done = true;
                continue;
            }
            if !look.enabled {
                ep.done = true;
                continue;
            }
            let has_text = a.text.as_deref().is_some_and(|t| !t.trim().is_empty());
            if has_text || look.now >= ep.began + GRACE_MS {
                ep.done = true;
                out.push(toast_of(a));
            }
        }
        out
    }

    /// When to look again (a question is waiting out its grace), or None.
    pub fn next_due(&self) -> Option<u64> {
        self.episodes
            .values()
            .filter(|e| !e.done)
            .map(|e| e.began + GRACE_MS)
            .min()
    }

    /// A tab went away: its episode with it.
    pub fn forget(&mut self, tab: &str) {
        self.episodes.remove(tab);
    }

    /// No tab holds a question: nothing to look at.
    pub fn is_idle(&self) -> bool {
        self.episodes.is_empty()
    }

    /// Open episodes that have not been decided (the e2e probe and the tests read it).
    pub fn waiting(&self) -> usize {
        self.episodes.values().filter(|e| !e.done).count()
    }
}

fn toast_of(a: &Asking) -> Toast {
    let title = one_line(&a.name, TITLE_MAX);
    let title = if title.is_empty() {
        a.agent.clone()
    } else {
        title
    };
    let text = a
        .text
        .as_deref()
        .map(|t| one_line(t, BODY_MAX))
        .filter(|t| !t.is_empty());
    let body = text.unwrap_or_else(|| format!("{} has a question for you.", a.agent));
    Toast {
        tab: a.tab.clone(),
        title,
        body,
    }
}

/// Text from a screen or a hook, as one safe line: controls, line breaks and the characters that reorder or hide
/// text become spaces, runs of spaces become one, and it is cut at `max` characters with an ellipsis.
pub fn one_line(text: &str, max: usize) -> String {
    let mut out = String::new();
    let mut gap = true;
    let mut n = 0;
    for c in text.chars() {
        let c = if c.is_control() || hidden(c) { ' ' } else { c };
        if c.is_whitespace() {
            if !gap {
                out.push(' ');
                gap = true;
                n += 1;
            }
        } else {
            out.push(c);
            gap = false;
            n += 1;
        }
        if n > max {
            break;
        }
    }
    let trimmed = out.trim_end();
    if trimmed.chars().count() > max {
        let cut: String = trimmed.chars().take(max.saturating_sub(1)).collect();
        return format!("{}\u{2026}", cut.trim_end());
    }
    trimmed.to_string()
}

/// A character that changes how text reads without showing: a zero-width one, a bidirectional control, a tag character,
/// a variation selector and the byte order mark.
fn hidden(c: char) -> bool {
    matches!(c as u32,
        0x00AD | 0x034F | 0x061C | 0x115F | 0x1160 | 0x17B4 | 0x17B5 | 0x180B..=0x180E
        | 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x206F | 0x3164
        | 0xFE00..=0xFE0F | 0xFEFF | 0xFFA0 | 0xFFF9..=0xFFFB | 0xE0000..=0xE0FFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_from_the_screen_is_one_clean_line() {
        assert_eq!(one_line("a\r\nb\tc   d", 50), "a b c d");
        assert_eq!(one_line("  padded  ", 50), "padded");
        assert_eq!(one_line("x\u{202e}evil\u{200b}y", 50), "x evil y");
        assert_eq!(one_line("a\u{e0041}b", 50), "a b");
        assert_eq!(one_line("a\u{1b}[31mb", 50), "a [31mb");
        assert_eq!(one_line("", 5), "");
        assert_eq!(one_line("   ", 5), "");
    }

    #[test]
    fn a_long_line_is_cut_with_an_ellipsis() {
        let long = "word ".repeat(200);
        let cut = one_line(&long, 40);
        assert!(cut.chars().count() <= 40, "{}", cut.chars().count());
        assert!(cut.ends_with('\u{2026}'));
        let exact = "a".repeat(40);
        assert_eq!(one_line(&exact, 40), exact);
        let over = "a".repeat(41);
        assert_eq!(one_line(&over, 40).chars().count(), 40);
    }

    fn asking(tab: &str, text: Option<&str>) -> Asking {
        Asking {
            tab: tab.into(),
            name: format!("proj {tab}"),
            agent: "Claude".into(),
            text: text.map(str::to_string),
        }
    }

    fn look(now: u64, front: Option<&str>, focused: bool) -> Look<'_> {
        Look {
            now,
            enabled: true,
            front,
            focused,
        }
    }

    #[test]
    fn a_hidden_tab_asking_with_its_text_notifies_at_once() {
        let mut n = Notifier::new();
        let out = n.update(
            &look(0, Some("a"), true),
            &[asking("b", Some("Which colour?"))],
        );
        assert_eq!(
            out,
            vec![Toast {
                tab: "b".into(),
                title: "proj b".into(),
                body: "Which colour?".into()
            }]
        );
    }

    #[test]
    fn without_the_text_it_waits_out_the_grace_then_shows_without_it() {
        let mut n = Notifier::new();
        let a = [asking("b", None)];
        assert!(n.update(&look(100, Some("a"), true), &a).is_empty());
        assert_eq!(n.next_due(), Some(100 + GRACE_MS));
        assert!(
            n.update(&look(100 + GRACE_MS - 1, Some("a"), true), &a)
                .is_empty()
        );
        let out = n.update(&look(100 + GRACE_MS, Some("a"), true), &a);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].body, "Claude has a question for you.");
        assert_eq!(n.next_due(), None);
    }

    #[test]
    fn the_text_arriving_in_the_grace_ends_it() {
        let mut n = Notifier::new();
        assert!(
            n.update(&look(0, Some("a"), true), &[asking("b", None)])
                .is_empty()
        );
        let out = n.update(
            &look(200, Some("a"), true),
            &[asking("b", Some("Overwrite it?"))],
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].body, "Overwrite it?");
    }

    #[test]
    fn nothing_while_looking_at_that_tab_in_an_active_window() {
        let mut n = Notifier::new();
        let a = [asking("a", Some("Which?"))];
        assert!(n.update(&look(0, Some("a"), true), &a).is_empty());
        assert!(n.update(&look(5_000, Some("a"), true), &a).is_empty());
        // Walking away with the question still up does not announce what the person already saw.
        assert!(n.update(&look(6_000, Some("b"), true), &a).is_empty());
        assert!(n.update(&look(9_000, Some("b"), false), &a).is_empty());
        assert_eq!(n.waiting(), 0);
    }

    #[test]
    fn an_unfocused_window_notifies_even_for_the_tab_in_front() {
        let mut n = Notifier::new();
        let out = n.update(&look(0, Some("a"), false), &[asking("a", Some("Which?"))]);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn a_minimised_window_with_no_front_tab_notifies() {
        let mut n = Notifier::new();
        let out = n.update(&look(0, None, false), &[asking("a", Some("Which?"))]);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn looking_during_the_grace_cancels_the_notification() {
        let mut n = Notifier::new();
        let a = [asking("b", None)];
        assert!(n.update(&look(0, Some("a"), true), &a).is_empty());
        assert!(n.update(&look(300, Some("b"), true), &a).is_empty());
        assert!(n.update(&look(5_000, Some("a"), true), &a).is_empty());
        assert_eq!(n.waiting(), 0);
    }

    #[test]
    fn one_question_is_one_notification_however_often_the_window_asks() {
        let mut n = Notifier::new();
        let a = [asking("b", Some("Which?"))];
        assert_eq!(n.update(&look(0, Some("a"), true), &a).len(), 1);
        for t in 1..50 {
            assert!(n.update(&look(t * 100, Some("a"), true), &a).is_empty());
        }
        // The text changing during the episode (the next of several questions) is not a new episode.
        let a2 = [asking("b", Some("And then?"))];
        assert!(n.update(&look(6_000, Some("a"), true), &a2).is_empty());
    }

    #[test]
    fn a_new_question_after_the_mark_went_is_a_new_notification() {
        let mut n = Notifier::new();
        let a = [asking("b", Some("Which?"))];
        assert_eq!(n.update(&look(0, Some("a"), true), &a).len(), 1);
        assert!(n.update(&look(100, Some("a"), true), &[]).is_empty());
        assert_eq!(n.update(&look(200, Some("a"), true), &a).len(), 1);
    }

    #[test]
    fn two_tabs_asking_are_two_notifications() {
        let mut n = Notifier::new();
        let out = n.update(
            &look(0, Some("a"), true),
            &[asking("b", Some("One?")), asking("c", Some("Two?"))],
        );
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn the_setting_off_is_silent_and_stays_silent_for_that_question() {
        let mut n = Notifier::new();
        let a = [asking("b", Some("Which?"))];
        let off = Look {
            enabled: false,
            ..look(0, Some("a"), true)
        };
        assert!(n.update(&off, &a).is_empty());
        // Turned on in the middle of the question: no surprise.
        assert!(n.update(&look(1_000, Some("a"), true), &a).is_empty());
        // The next question is announced.
        assert!(n.update(&look(2_000, Some("a"), true), &[]).is_empty());
        assert_eq!(n.update(&look(3_000, Some("a"), true), &a).len(), 1);
    }

    #[test]
    fn the_setting_turned_off_while_a_question_waits_cancels_it() {
        let mut n = Notifier::new();
        let a = [asking("b", None)];
        assert!(n.update(&look(0, Some("a"), true), &a).is_empty());
        let off = Look {
            enabled: false,
            ..look(100, Some("a"), true)
        };
        assert!(n.update(&off, &a).is_empty());
        assert!(n.update(&look(5_000, Some("a"), true), &a).is_empty());
        assert_eq!(n.next_due(), None);
    }

    #[test]
    fn a_closed_tab_forgets_its_episode() {
        let mut n = Notifier::new();
        assert!(
            n.update(&look(0, Some("a"), true), &[asking("b", None)])
                .is_empty()
        );
        n.forget("b");
        assert_eq!(n.next_due(), None);
        assert_eq!(n.waiting(), 0);
    }

    #[test]
    fn the_title_and_body_are_clean_single_lines() {
        let mut n = Notifier::new();
        let mut a = asking("b", Some("Line one\nline two\u{202e}x"));
        a.name = "bad\u{202e}name\r\n".into();
        let out = n.update(&look(0, Some("a"), true), &[a]);
        assert_eq!(out[0].title, "bad name");
        assert_eq!(out[0].body, "Line one line two x");
        let long = asking("c", Some(&"x".repeat(1000)));
        let out = n.update(&look(1, Some("a"), true), &[long]);
        assert!(out[0].body.chars().count() <= BODY_MAX);
    }

    #[test]
    fn a_blank_name_falls_back_to_the_agent() {
        let mut n = Notifier::new();
        let mut a = asking("b", Some("Which?"));
        a.name = "  ".into();
        let out = n.update(&look(0, Some("a"), true), &[a]);
        assert_eq!(out[0].title, "Claude");
    }
}

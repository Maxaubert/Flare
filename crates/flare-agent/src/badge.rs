//! The taskbar badge count and its disc pixels (Task 14). Ported from Prism Terminal's
//! `taskbarBadge.ts`: the count of tabs that need a look, the text on the disc and its size at
//! every display scaling. The pixels are drawn by `flare-win`; this file only decides them.

use std::collections::HashSet;

/// The marked tabs and the switches that count them.
pub struct AttentionIn<'a> {
    pub done: &'a HashSet<String>,
    pub question: &'a HashSet<String>,
    pub working: &'a HashSet<String>,
    pub failed: Option<&'a HashSet<String>>,
    pub done_on: bool,
    pub question_on: bool,
    pub failed_on: bool,
}

/// What the badge says: how many tabs need a look, and whether one of them is a question.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Attention {
    pub count: usize,
    pub question: bool,
}

/// Unique ids of marked tabs that are not working. A question sets `question`.
pub fn attention_count(p: &AttentionIn) -> Attention {
    let mut ids: HashSet<&str> = HashSet::new();
    let mut question = false;
    if p.done_on {
        ids.extend(p.done.iter().map(String::as_str));
    }
    if p.question_on {
        for id in p.question.iter().filter(|id| !p.working.contains(*id)) {
            question = true;
            ids.insert(id.as_str());
        }
    }
    if p.failed_on
        && let Some(failed) = p.failed
    {
        ids.extend(failed.iter().map(String::as_str));
    }
    let count = ids.iter().filter(|id| !p.working.contains(**id)).count();
    Attention { count, question }
}

/// The text on the disc: the count, and "9+" past nine.
pub fn badge_text(count: usize) -> String {
    if count > 9 {
        "9+".to_string()
    } else {
        count.to_string()
    }
}

/// The accessible description: "1 tab needs a look" or "N tabs need a look".
pub fn badge_description(count: usize) -> String {
    if count == 1 {
        "1 tab needs a look".to_string()
    } else {
        format!("{count} tabs need a look")
    }
}

pub const BADGE_FILL: &str = "#25242c";
pub const BADGE_INK: &str = "#ffffff";

/// The disc's diameter in device pixels: 16 logical px times the scale. The scale is capped at
/// 4; a scale that is not finite or not above 0 counts as 1.
pub fn badge_pixels(scale: f64) -> u32 {
    let s = if scale.is_finite() && scale > 0.0 {
        scale.min(4.0)
    } else {
        1.0
    };
    (16.0 * s).round() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn counts_every_tab_showing_a_mark_once_a_question_and_a_finish_alike() {
        let done = set(&["a", "b"]);
        let question = set(&["b", "c"]);
        let working = set(&[]);
        let p = AttentionIn {
            done: &done,
            question: &question,
            working: &working,
            failed: None,
            done_on: true,
            question_on: true,
            failed_on: true,
        };
        assert_eq!(
            attention_count(&p),
            Attention {
                count: 3,
                question: true
            }
        );
    }

    #[test]
    fn counts_only_the_marks_whose_switch_is_on() {
        let done = set(&["a"]);
        let question = set(&["c"]);
        let working = set(&[]);
        let base = |done_on, question_on| AttentionIn {
            done: &done,
            question: &question,
            working: &working,
            failed: None,
            done_on,
            question_on,
            failed_on: true,
        };
        assert_eq!(
            attention_count(&base(false, true)),
            Attention {
                count: 1,
                question: true
            }
        );
        assert_eq!(
            attention_count(&base(true, false)),
            Attention {
                count: 1,
                question: false
            }
        );
    }

    #[test]
    fn does_not_count_a_tab_that_is_working_again() {
        let done = set(&["a"]);
        let question = set(&[]);
        let working = set(&["a"]);
        let p = AttentionIn {
            done: &done,
            question: &question,
            working: &working,
            failed: None,
            done_on: true,
            question_on: true,
            failed_on: true,
        };
        assert_eq!(
            attention_count(&p),
            Attention {
                count: 0,
                question: false
            }
        );
    }

    #[test]
    fn counts_a_failed_tab_too_once_and_only_while_its_switch_is_on() {
        let done = set(&["a"]);
        let question = set(&[]);
        let working = set(&[]);
        let failed = set(&["a", "f"]);
        let on = AttentionIn {
            done: &done,
            question: &question,
            working: &working,
            failed: Some(&failed),
            done_on: true,
            question_on: true,
            failed_on: true,
        };
        assert_eq!(
            attention_count(&on),
            Attention {
                count: 2,
                question: false
            }
        );
        let off = AttentionIn {
            failed_on: false,
            ..on
        };
        assert_eq!(
            attention_count(&off),
            Attention {
                count: 1,
                question: false
            }
        );
        let failed_working = set(&["f"]);
        let working_failed = AttentionIn {
            working: &failed_working,
            ..on
        };
        assert_eq!(
            attention_count(&working_failed),
            Attention {
                count: 1,
                question: false
            }
        );
    }

    #[test]
    fn the_text_is_the_count_and_9_plus_past_nine() {
        assert_eq!(badge_text(1), "1");
        assert_eq!(badge_text(9), "9");
        assert_eq!(badge_text(10), "9+");
    }

    #[test]
    fn the_description_is_singular_and_plural() {
        assert_eq!(badge_description(1), "1 tab needs a look");
        assert_eq!(badge_description(2), "2 tabs need a look");
        assert_eq!(badge_description(12), "12 tabs need a look");
    }

    #[test]
    fn draws_at_the_displays_physical_size() {
        assert_eq!(badge_pixels(1.0), 16);
        assert_eq!(badge_pixels(1.5), 24);
        assert_eq!(badge_pixels(2.25), 36);
        assert_eq!(badge_pixels(3.0), 48);
    }

    #[test]
    fn a_nonsense_scale_is_1_and_a_huge_one_is_capped() {
        assert_eq!(badge_pixels(0.0), 16);
        assert_eq!(badge_pixels(f64::NAN), 16);
        assert_eq!(badge_pixels(-1.0), 16);
        assert_eq!(badge_pixels(9.0), 64);
    }
}

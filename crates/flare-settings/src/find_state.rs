//! The Find, rail and flash key machine (M7 Task 18).
//!
//! Source: Prism Terminal v0.55.6's `core/renderer/settings/layout/SettingsFrame.tsx` (the field,
//! list and rail keys, `choose`, `mark`, `clear`, the status text and the heading), `flash.ts`
//! (the wait for a late row, the flash curve, the centring) and the e2e `settingsKeys` and
//! `settingsSearch`. Pure state: the Slint view reports the keys and draws what this says.

use crate::find::{IndexEntry, PageNames, search_settings};

/// The keys the machine understands. Anything else the view does not pass here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Down,
    Up,
    Left,
    Right,
    Home,
    End,
    Enter,
    Space,
    Escape,
    Tab,
    ShiftTab,
}

/// What the find field wants done with a key.
#[derive(Debug, PartialEq, Eq)]
pub enum FieldOut {
    /// The machine took the key; the host must not act on it.
    Consumed,
    /// The key is the host's (an empty field lets Escape through, and so does every other key).
    PassThrough,
    /// Put the keyboard on result `i` (its cursor is set).
    Mark(usize),
    /// Choose result `i`; the caller then calls [`FindState::choose`].
    Choose(usize),
}

/// What the result list wants done with a key.
#[derive(Debug, PartialEq, Eq)]
pub enum ListOut {
    Consumed,
    /// Put the keyboard on result `i` (its cursor is set).
    Mark(usize),
    /// Up from the first result: the keyboard returns to the field (the cursor is cleared).
    ToField,
    /// Choose result `i`; the caller then calls [`FindState::choose`].
    Choose(usize),
    /// Escape: the query is cleared and the keyboard returns to the field.
    Clear,
}

/// The row a chosen result leads to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chosen {
    pub id: String,
    pub page: String,
    pub view: Option<String>,
    pub section_id: Option<String>,
}

/// The Find field's state: the query, the hits it gives, and the keyboard cursor.
pub struct FindState {
    query: String,
    index: Vec<IndexEntry>,
    names: PageNames,
    hits: Vec<IndexEntry>,
    cursor: Option<usize>,
}

impl FindState {
    /// A new field over `index`, with the page names the results show.
    pub fn new(index: Vec<IndexEntry>, names: PageNames) -> Self {
        Self {
            query: String::new(),
            index,
            names,
            hits: Vec::new(),
            cursor: None,
        }
    }

    /// Replaces the query, recomputes the hits and clears the cursor.
    pub fn set_query(&mut self, q: &str) {
        self.query = q.to_string();
        self.cursor = None;
        self.hits = search_settings(&self.index, &self.query, &self.names)
            .into_iter()
            .cloned()
            .collect();
    }

    /// Replaces the index (a row came or went, such as the Mark colours with the indicator) and,
    /// while searching, recomputes the hits over it. The query stays.
    pub fn set_index(&mut self, index: Vec<IndexEntry>) {
        if index == self.index {
            return;
        }
        self.index = index;
        if self.searching() {
            let q = self.query.clone();
            self.set_query(&q);
        }
    }

    /// True while the query holds something other than spaces.
    pub fn searching(&self) -> bool {
        !self.query.trim().is_empty()
    }

    pub fn hits(&self) -> &[IndexEntry] {
        &self.hits
    }

    pub fn cursor(&self) -> Option<usize> {
        self.cursor
    }

    /// A key pressed in the find field.
    pub fn field_key(&mut self, k: Key) -> FieldOut {
        match k {
            Key::Escape if self.owns_escape() => {
                self.set_query("");
                FieldOut::Consumed
            }
            Key::Down if !self.hits.is_empty() => {
                self.cursor = Some(0);
                FieldOut::Mark(0)
            }
            Key::Enter if !self.hits.is_empty() => FieldOut::Choose(0),
            _ => FieldOut::PassThrough,
        }
    }

    /// A key pressed in the result list.
    pub fn list_key(&mut self, k: Key) -> ListOut {
        let last = self.hits.len().checked_sub(1);
        match k {
            Key::Down => match last {
                Some(last) => {
                    let next = self.cursor.map_or(0, |c| (c + 1).min(last));
                    self.cursor = Some(next);
                    ListOut::Mark(next)
                }
                None => ListOut::Consumed,
            },
            Key::Up => match self.cursor {
                Some(c) if c > 0 => {
                    self.cursor = Some(c - 1);
                    ListOut::Mark(c - 1)
                }
                _ => {
                    self.cursor = None;
                    ListOut::ToField
                }
            },
            Key::Enter | Key::Space => match self.cursor {
                Some(c) if c < self.hits.len() => ListOut::Choose(c),
                _ => ListOut::Consumed,
            },
            Key::Escape => {
                self.set_query("");
                ListOut::Clear
            }
            _ => ListOut::Consumed,
        }
    }

    /// Chooses result `i`: returns what it leads to and clears the query and the cursor.
    /// `None` (and no change) when there is no result `i`.
    pub fn choose(&mut self, i: usize) -> Option<Chosen> {
        let hit = self.hits.get(i)?.clone();
        self.set_query("");
        Some(Chosen {
            id: hit.id,
            page: hit.page,
            view: hit.view,
            section_id: hit.section_id,
        })
    }

    /// The status line: `None` unless the field is searching.
    pub fn status(&self) -> Option<String> {
        if !self.searching() {
            return None;
        }
        Some(match self.hits.len() {
            0 => "No results".to_string(),
            1 => "1 result".to_string(),
            n => format!("{n} results"),
        })
    }

    /// The heading above the pane: "Results" while searching, else the page label.
    pub fn heading(&self, page_label: &str) -> String {
        if self.searching() {
            "Results".to_string()
        } else {
            page_label.to_string()
        }
    }

    /// The line under an empty result list, when searching found nothing.
    pub fn nothing_text(&self) -> Option<String> {
        if self.searching() && self.hits.is_empty() {
            Some(format!(
                "Nothing matches {}. Try a shorter word.",
                self.query.trim()
            ))
        } else {
            None
        }
    }

    /// True while the query holds text (the field then owns Escape).
    pub fn owns_escape(&self) -> bool {
        !self.query.is_empty()
    }

    /// True while searching: no rail item is chosen.
    pub fn no_rail_item_is_chosen(&self) -> bool {
        self.searching()
    }
}

/// The rail's key move: Down and Up step (clamped), Home and End jump. `None` for other keys
/// or an empty rail.
pub fn rail_key(current: usize, count: usize, k: Key) -> Option<usize> {
    let last = count.checked_sub(1)?;
    match k {
        Key::Down => Some((current + 1).min(last)),
        Key::Up => Some(current.saturating_sub(1)),
        Key::Home => Some(0),
        Key::End => Some(last),
        _ => None,
    }
}

/// Tab from the find field lands on the first rail page; other keys are not the rail's.
pub fn rail_entry(from_field: bool, k: Key) -> Option<usize> {
    (from_field && k == Key::Tab).then_some(0)
}

/// How long a late row is looked for, in ms.
pub const WAIT_MS: u64 = 3000;
/// How often it is looked for again, in ms.
pub const POLL_MS: u64 = 60;
/// The flash's length, in ms.
pub const FLASH_MS: u64 = 1400;

/// What a poll of the flash watch decided.
#[derive(Debug, PartialEq, Eq)]
pub enum FlashStep {
    /// The row exists: flash it now.
    Found,
    /// Not yet: poll again at `at_ms` (an absolute time in ms).
    Retry { at_ms: u64 },
    /// The wait ran out and the section exists: flash the section instead.
    Section,
    /// The wait ran out and nothing exists to flash.
    GiveUp,
}

/// The wait for a row that may be drawn late (PT's `flashPref`).
pub struct FlashWatch {
    started_ms: u64,
    id: String,
    section_id: Option<String>,
}

impl FlashWatch {
    pub fn new(now_ms: u64, id: &str, section_id: Option<&str>) -> Self {
        Self {
            started_ms: now_ms,
            id: id.to_string(),
            section_id: section_id.map(str::to_string),
        }
    }

    /// The row this watch is for.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The section this watch falls back to.
    pub fn section_id(&self) -> Option<&str> {
        self.section_id.as_deref()
    }

    /// One poll: `row_exists` and `section_exists` are what the view sees at `now_ms`.
    pub fn poll(&mut self, now_ms: u64, row_exists: bool, section_exists: bool) -> FlashStep {
        if row_exists {
            return FlashStep::Found;
        }
        if now_ms.saturating_sub(self.started_ms) < WAIT_MS {
            return FlashStep::Retry {
                at_ms: now_ms + POLL_MS,
            };
        }
        if section_exists {
            FlashStep::Section
        } else {
            FlashStep::GiveUp
        }
    }
}

/// The flash's opacity factor at `t_ms` after it starts: 1.0 held to 30 percent of
/// [`FLASH_MS`], then an ease-out (CSS `ease-out`, cubic-bezier(0, 0, 0.58, 1)) to 0 at
/// [`FLASH_MS`]. Under reduced motion (`still`) it is always 0.0.
pub fn flash_alpha(t_ms: u64, still: bool) -> f64 {
    if still || t_ms >= FLASH_MS {
        return 0.0;
    }
    let hold = FLASH_MS * 3 / 10;
    if t_ms <= hold {
        return 1.0;
    }
    let x = (t_ms - hold) as f64 / (FLASH_MS - hold) as f64;
    1.0 - ease_out(x)
}

/// CSS `ease-out` progress at input fraction `x`, found by bisection on the curve's x.
fn ease_out(x: f64) -> f64 {
    let (x1, y1, x2, y2) = (0.0_f64, 0.0_f64, 0.58_f64, 1.0_f64);
    let bez = |s: f64, a: f64, b: f64| {
        let u = 1.0 - s;
        3.0 * u * u * s * a + 3.0 * u * s * s * b + s * s * s
    };
    let (mut lo, mut hi) = (0.0_f64, 1.0_f64);
    for _ in 0..60 {
        let mid = (lo + hi) / 2.0;
        if bez(mid, x1, x2) < x {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    bez((lo + hi) / 2.0, y1, y2)
}

/// The scroll offset that puts a row's middle at the pane's middle, clamped to the content
/// (PT's `scrollIntoView({ block: 'center' })`). Content shorter than the pane gives 0.
pub fn centre_scroll(row_y: f64, row_h: f64, pane_h: f64, content_h: f64) -> f64 {
    let target = row_y + row_h / 2.0 - pane_h / 2.0;
    target.clamp(0.0, (content_h - pane_h).max(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, label: &str, page: &str, section_id: Option<&str>) -> IndexEntry {
        IndexEntry {
            id: id.to_string(),
            page: page.to_string(),
            view: None,
            section: String::new(),
            section_id: section_id.map(str::to_string),
            label: label.to_string(),
            sub: String::new(),
            icon: String::new(),
            keywords: None,
        }
    }

    fn names() -> PageNames {
        [
            ("appearance", "Appearance"),
            ("terminal", "Terminal"),
            ("agents", "Agents"),
            ("dictation", "Dictation"),
            ("diagnostics", "Diagnostics"),
            ("about", "About"),
        ]
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
    }

    fn index() -> Vec<IndexEntry> {
        vec![
            entry(
                "flare.term.shell",
                "Default shell",
                "terminal",
                Some("shell"),
            ),
            entry("flare.term.font", "Text size", "terminal", Some("text")),
            entry("flare.look.theme", "Theme", "appearance", None),
        ]
    }

    fn machine() -> FindState {
        FindState::new(index(), names())
    }

    #[test]
    fn a_new_index_redoes_the_hits_of_the_query_in_the_field() {
        let mut m = machine();
        m.set_query("theme");
        assert_eq!(m.hits().len(), 1);
        let smaller: Vec<IndexEntry> = index()
            .into_iter()
            .filter(|e| e.id != "flare.look.theme")
            .collect();
        m.set_index(smaller);
        assert!(m.hits().is_empty(), "a row that left the index is no hit");
        m.set_index(index());
        assert_eq!(m.hits().len(), 1, "it is found again once back");
        m.set_query("");
        m.set_index(Vec::new());
        assert!(m.hits().is_empty() && !m.searching());
    }

    #[test]
    fn the_status_line_exists_only_while_typing() {
        let mut m = machine();
        assert_eq!(m.status(), None);
        m.set_query("   ");
        assert_eq!(m.status(), None);
        m.set_query("zebra");
        assert_eq!(m.status().as_deref(), Some("No results"));
        m.set_query("the");
        assert_eq!(m.status().as_deref(), Some("1 result"));
        m.set_query("te");
        assert_eq!(m.status().as_deref(), Some("2 results"));
        let mut three = FindState::new(
            vec![
                entry("a.one", "Font one", "terminal", None),
                entry("a.two", "Font two", "terminal", None),
                entry("a.three", "Font three", "terminal", None),
            ],
            names(),
        );
        three.set_query("font");
        assert_eq!(three.status().as_deref(), Some("3 results"));
    }

    #[test]
    fn escape_clears_text_and_an_empty_field_lets_it_through() {
        let mut m = machine();
        assert_eq!(m.field_key(Key::Escape), FieldOut::PassThrough);
        m.set_query("text");
        assert!(m.owns_escape());
        assert_eq!(m.field_key(Key::Escape), FieldOut::Consumed);
        assert!(!m.searching());
        assert!(!m.owns_escape());
        assert_eq!(m.field_key(Key::Escape), FieldOut::PassThrough);
    }

    #[test]
    fn down_marks_the_first_result_and_enter_chooses_it() {
        let mut m = machine();
        m.set_query("text");
        assert_eq!(m.field_key(Key::Down), FieldOut::Mark(0));
        assert_eq!(m.cursor(), Some(0));
        let mut n = machine();
        n.set_query("text");
        assert_eq!(n.field_key(Key::Enter), FieldOut::Choose(0));
        // Nothing to choose, nothing to mark: the keys are the host's.
        let mut none = machine();
        none.set_query("zebra");
        assert_eq!(none.field_key(Key::Down), FieldOut::PassThrough);
        assert_eq!(none.field_key(Key::Enter), FieldOut::PassThrough);
        assert_eq!(none.field_key(Key::Tab), FieldOut::PassThrough);
    }

    #[test]
    fn list_down_up_enter_space_escape() {
        let mut m = machine();
        m.set_query("t");
        assert!(m.hits().len() >= 2);
        assert_eq!(m.list_key(Key::Down), ListOut::Mark(0));
        assert_eq!(m.list_key(Key::Down), ListOut::Mark(1));
        assert_eq!(m.list_key(Key::Up), ListOut::Mark(0));
        // Up from the first result returns to the field.
        assert_eq!(m.list_key(Key::Up), ListOut::ToField);
        assert_eq!(m.cursor(), None);
        m.list_key(Key::Down);
        m.list_key(Key::Down);
        assert_eq!(m.cursor(), Some(1));
        // Down stops at the last result.
        let last = m.hits().len() - 1;
        for _ in 0..5 {
            m.list_key(Key::Down);
        }
        assert_eq!(m.cursor(), Some(last));
        m.list_key(Key::Up);
        m.list_key(Key::Up);
        assert_eq!(m.list_key(Key::Enter), ListOut::Choose(m.cursor().unwrap()));
        let i = m.cursor().unwrap();
        assert_eq!(m.list_key(Key::Space), ListOut::Choose(i));
        assert_eq!(m.list_key(Key::Escape), ListOut::Clear);
        assert!(!m.searching());
        assert_eq!(m.cursor(), None);
        // Enter with no cursor does nothing.
        m.set_query("t");
        assert_eq!(m.list_key(Key::Enter), ListOut::Consumed);
    }

    #[test]
    fn choosing_clears_the_query_and_returns_the_row() {
        let mut m = machine();
        m.set_query("shell");
        let chosen = m.choose(0).expect("a hit");
        assert_eq!(
            chosen,
            Chosen {
                id: "flare.term.shell".to_string(),
                page: "terminal".to_string(),
                view: None,
                section_id: Some("shell".to_string()),
            }
        );
        assert!(!m.searching());
        assert_eq!(m.cursor(), None);
        assert!(m.hits().is_empty());
        // Out of range: nothing chosen.
        m.set_query("shell");
        assert_eq!(m.choose(9), None);
    }

    #[test]
    fn no_rail_item_is_chosen_while_searching() {
        let mut m = machine();
        assert!(!m.no_rail_item_is_chosen());
        m.set_query("theme");
        assert!(m.no_rail_item_is_chosen());
        assert_eq!(m.heading("Terminal"), "Results");
        m.set_query("");
        assert_eq!(m.heading("Terminal"), "Terminal");
    }

    #[test]
    fn the_nothing_text_names_the_query_trimmed() {
        let mut m = machine();
        assert_eq!(m.nothing_text(), None);
        m.set_query("  zebra ");
        assert_eq!(
            m.nothing_text().as_deref(),
            Some("Nothing matches zebra. Try a shorter word.")
        );
        m.set_query("theme");
        assert_eq!(m.nothing_text(), None);
    }

    #[test]
    fn rail_down_up_home_end_and_tab_entry() {
        assert_eq!(rail_key(0, 6, Key::Down), Some(1));
        assert_eq!(rail_key(5, 6, Key::Down), Some(5));
        assert_eq!(rail_key(3, 6, Key::Up), Some(2));
        assert_eq!(rail_key(0, 6, Key::Up), Some(0));
        assert_eq!(rail_key(3, 6, Key::Home), Some(0));
        // End lands on About (index 5 of 6).
        assert_eq!(rail_key(0, 6, Key::End), Some(5));
        assert_eq!(rail_key(0, 6, Key::Enter), None);
        assert_eq!(rail_key(0, 0, Key::Down), None);
        assert_eq!(rail_entry(true, Key::Tab), Some(0));
        assert_eq!(rail_entry(false, Key::Tab), None);
        assert_eq!(rail_entry(true, Key::Down), None);
    }

    #[test]
    fn a_late_row_is_polled_every_60_ms_for_3_seconds_then_the_section_flashes() {
        let mut w = FlashWatch::new(1000, "flare.term.shell", Some("shell"));
        assert_eq!(w.id(), "flare.term.shell");
        assert_eq!(w.section_id(), Some("shell"));
        assert_eq!(w.poll(1000, false, true), FlashStep::Retry { at_ms: 1060 });
        assert_eq!(w.poll(1060, false, true), FlashStep::Retry { at_ms: 1120 });
        assert_eq!(w.poll(3940, false, true), FlashStep::Retry { at_ms: 4000 });
        // At 3000 ms after the start the wait is over: the section stands in.
        assert_eq!(w.poll(4000, false, true), FlashStep::Section);
        let mut gone = FlashWatch::new(0, "x", None);
        assert_eq!(gone.poll(3000, false, false), FlashStep::GiveUp);
        let mut late = FlashWatch::new(0, "x", None);
        assert_eq!(late.poll(2990, true, false), FlashStep::Found);
        let mut found_at_once = FlashWatch::new(0, "x", Some("s"));
        assert_eq!(found_at_once.poll(0, true, true), FlashStep::Found);
    }

    #[test]
    fn the_flash_holds_then_fades_and_reduced_motion_shows_none() {
        assert_eq!(flash_alpha(0, false), 1.0);
        assert_eq!(flash_alpha(420, false), 1.0);
        assert_eq!(flash_alpha(FLASH_MS, false), 0.0);
        assert_eq!(flash_alpha(FLASH_MS + 50, false), 0.0);
        let mid = flash_alpha(900, false);
        assert!(mid > 0.0 && mid < 1.0, "mid fade {mid}");
        // Ease-out: halfway through the fade's time the colour has dropped more than half.
        assert!(flash_alpha(420 + 490, false) < 0.5);
        let mut last = 1.0;
        for t in (420..=1400).step_by(10) {
            let a = flash_alpha(t, false);
            assert!(a <= last + 1e-9, "not monotonic at {t}");
            last = a;
        }
        assert_eq!(flash_alpha(0, true), 0.0);
        assert_eq!(flash_alpha(700, true), 0.0);
    }

    #[test]
    fn a_chosen_row_is_scrolled_to_the_middle_of_the_pane() {
        // Row at 500 of 40 high in a 200 pane over 2000 of content: 500 + 20 - 100 = 420.
        assert_eq!(centre_scroll(500.0, 40.0, 200.0, 2000.0), 420.0);
        // Near the top: clamps at 0.
        assert_eq!(centre_scroll(10.0, 40.0, 200.0, 2000.0), 0.0);
        // Near the bottom: clamps at content minus pane.
        assert_eq!(centre_scroll(1990.0, 40.0, 200.0, 2000.0), 1800.0);
        // Content shorter than the pane: no scroll.
        assert_eq!(centre_scroll(50.0, 40.0, 200.0, 120.0), 0.0);
    }
}

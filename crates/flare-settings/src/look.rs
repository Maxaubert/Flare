//! LookStore: the theme and look preferences (Task 16).
//! Source: PT `core/renderer/lib/termLook.ts` (the store half), `settings/theme/{palette,useTermSetup}.ts`,
//! `ThemeWall.tsx` (`land`) and `AppearancePage.tsx` (`onThemePicked`, `extraDirty`).
//!
//! Over M3's `PrefStore`. Keys are `flare.term.*` (decision D9). The editor's draft is memory only.
//! M7 builds the pages on this; M4 builds no UI for it.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use flare_theme::colour::canonical_stored;
use flare_theme::edges::WindowEdges;
use flare_theme::fonts::{font_id_or_default, font_pct_from};
use flare_theme::look::{AppliedLook, LookInput, apply_look};
use flare_theme::preset::HOST_DEFAULT_THEME;
use flare_theme::resolve::CustomPalette;
use flare_theme::retired::live_theme_id;
use flare_theme::see_through::{SEE_THROUGH_TOP, see_through_level};

use crate::custom_theme::{
    CUSTOM_KEY, CustomTheme, palette_of, parse_custom, serialize_custom, with_ground_alpha,
};
use crate::kv::{EnumPref, PrefStore, Subscription, bool_pref_get, bool_pref_set, on_any_change};
use crate::window_colours::{
    ACCENT, BACKGROUND, EDGES_KEY, NO_COLOURS, SECONDARY, custom_window_colours,
    read_window_colours, set_custom_window_colours, set_window_colours, window_edges,
};

const THEME_KEY: &str = "flare.term.theme";
const FONT_KEY: &str = "flare.term.font";
const FONT_PCT_KEY: &str = "flare.term.fontPct";
const SEE_THROUGH_KEY: &str = "flare.term.seeThrough";
const INDICATOR_KEY: &str = "flare.term.agentIndicator";
const WORKING_KEY: &str = "flare.term.agentColor";
const FINISHED_KEY: &str = "flare.term.agentDoneColor";
const QUESTION_KEY: &str = "flare.term.agentQuestionColor";

/// The caret's shape (rule 1233): one of the [`CARET_SHAPES`] words, `block` until somebody chooses.
const CARET_SHAPE: EnumPref = EnumPref {
    key: "flare.term.caretShape",
    allowed: &["block", "hollow", "bar", "underline", "pill"],
    fallback: "block",
};
/// Whether the caret ticks (rule 1233). On until switched off; only a stored "0" is off.
const CARET_BLINK_KEY: &str = "flare.term.caretBlink";

/// The caret shapes the Settings list offers, in order: the stored word and the name shown.
pub const CARET_SHAPES: [(&str, &str); 5] = [
    ("block", "Block"),
    ("hollow", "Hollow block"),
    ("bar", "Bar"),
    ("underline", "Underline"),
    ("pill", "Pill"),
];

/// Every key on_look_change hears (the draft is heard besides): the theme, the custom theme, see
/// through, the font and its size, the caret, the indicator and the agent colours, the window colours,
/// the edges.
pub const LOOK_KEYS: [&str; 15] = [
    THEME_KEY,
    CUSTOM_KEY,
    SEE_THROUGH_KEY,
    FONT_KEY,
    FONT_PCT_KEY,
    CARET_SHAPE.key,
    CARET_BLINK_KEY,
    INDICATOR_KEY,
    WORKING_KEY,
    FINISHED_KEY,
    QUESTION_KEY,
    ACCENT.key,
    BACKGROUND.key,
    SECONDARY.key,
    EDGES_KEY,
];

/// How an agent shows on its tab. Flare draws Ring and Minimal; the others read as the default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentIndicator {
    Off,
    Minimal,
    Ring,
    Full,
}

impl AgentIndicator {
    fn word(self) -> &'static str {
        match self {
            AgentIndicator::Off => "off",
            AgentIndicator::Minimal => "minimal",
            AgentIndicator::Ring => "ring",
            AgentIndicator::Full => "full",
        }
    }
}

/// The three agent marks that wear a colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentColourState {
    Working,
    Finished,
    Question,
}

impl AgentColourState {
    fn key(self) -> &'static str {
        match self {
            AgentColourState::Working => WORKING_KEY,
            AgentColourState::Finished => FINISHED_KEY,
            AgentColourState::Question => QUESTION_KEY,
        }
    }
}

const COLOUR_STATES: [AgentColourState; 3] = [
    AgentColourState::Working,
    AgentColourState::Finished,
    AgentColourState::Question,
];

/// The theme-bound settings a save carries besides the palette. `""` follows the theme.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Extras {
    pub indicator_color: String,
    pub done_color: String,
    pub question_color: String,
    pub see_through: u8,
}

/// Whether the settings deviate from the selected theme's stock, and the extras in force.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetupState {
    pub dirty: bool,
    pub extras: Extras,
}

/// Owned copies of everything `LookInput` borrows.
#[derive(Clone, Debug, PartialEq)]
pub struct LookSnapshot {
    pub theme_id: String,
    pub custom: Option<CustomPalette>,
    pub draft: Option<CustomPalette>,
    pub background: Option<String>,
    pub accent: Option<String>,
    pub secondary: Option<String>,
    pub edges: WindowEdges,
    pub see_through: u8,
}

impl LookSnapshot {
    /// The borrowed form `apply_look` takes.
    pub fn as_input(&self, glass_possible: bool) -> LookInput<'_> {
        LookInput {
            theme_id: &self.theme_id,
            custom: self.custom.as_ref(),
            draft: self.draft.as_ref(),
            background: self.background.as_deref(),
            accent: self.accent.as_deref(),
            secondary: self.secondary.as_deref(),
            edges: self.edges,
            see_through: self.see_through,
            glass_possible,
        }
    }
}

type Callback = Arc<dyn Fn() + Send + Sync>;

#[derive(Default)]
struct DraftShared {
    draft: Mutex<Option<CustomPalette>>,
    listeners: Mutex<DraftListeners>,
}

#[derive(Default)]
struct DraftListeners {
    next_id: u64,
    list: Vec<(u64, Callback)>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The theme and look preferences. Cheap to clone; clones share the store and the draft.
#[derive(Clone)]
pub struct LookStore {
    store: PrefStore,
    shared: Arc<DraftShared>,
}

impl LookStore {
    pub fn new(store: PrefStore) -> Self {
        Self {
            shared: store.shared::<DraftShared>(),
            store,
        }
    }

    /// The theme in force: never set, empty or `style` is `pt-default`; a retired id is its replacement.
    pub fn theme_id(&self) -> String {
        match self.store.get(THEME_KEY) {
            Some(v) if !v.is_empty() && v != "style" => live_theme_id(&v).to_string(),
            _ => HOST_DEFAULT_THEME.to_string(),
        }
    }

    pub fn set_theme_id(&self, id: &str) {
        self.store.set(THEME_KEY, id);
    }

    pub fn font_id(&self) -> &'static str {
        font_id_or_default(self.store.get(FONT_KEY).as_deref())
    }

    pub fn set_font_id(&self, id: &str) {
        self.store.set(FONT_KEY, id);
    }

    pub fn font_pct(&self) -> u32 {
        font_pct_from(self.store.get(FONT_PCT_KEY).as_deref())
    }

    pub fn set_font_pct(&self, pct: u32) {
        self.store.set(FONT_PCT_KEY, &pct.to_string());
    }

    /// The caret shape's word: `block`, `hollow`, `bar`, `underline` or `pill`; anything else reads as `block`.
    pub fn caret_shape(&self) -> &'static str {
        CARET_SHAPE.get(&self.store)
    }

    /// Stores a shape word as given, or `block` when it is not one of the five.
    pub fn set_caret_shape(&self, word: &str) {
        CARET_SHAPE.set(&self.store, word);
    }

    /// Whether the caret ticks: on until switched off.
    pub fn caret_blink(&self) -> bool {
        bool_pref_get(&self.store, CARET_BLINK_KEY, true)
    }

    pub fn set_caret_blink(&self, on: bool) {
        bool_pref_set(&self.store, CARET_BLINK_KEY, on);
    }

    pub fn see_through(&self) -> u8 {
        see_through_level(self.store.get(SEE_THROUGH_KEY).as_deref())
    }

    /// Stored clamped to 0..=70.
    pub fn set_see_through(&self, level: u8) {
        self.store
            .set(SEE_THROUGH_KEY, &level.min(SEE_THROUGH_TOP).to_string());
    }

    /// Ring and Minimal read as themselves; anything else reads as Ring.
    pub fn agent_indicator(&self) -> AgentIndicator {
        match self.store.get(INDICATOR_KEY).as_deref() {
            Some("minimal") => AgentIndicator::Minimal,
            _ => AgentIndicator::Ring,
        }
    }

    pub fn set_agent_indicator(&self, v: AgentIndicator) {
        self.store.set(INDICATOR_KEY, v.word());
    }

    /// The colour chosen for a mark in its canonical form, or `""` to follow the theme.
    pub fn agent_colour_choice(&self, s: AgentColourState) -> String {
        canonical_stored(self.store.get(s.key()).as_deref()).unwrap_or_default()
    }

    /// A six or eight digit hex picks a colour; anything else (`""` included) gives the mark back to the theme.
    pub fn set_agent_colour(&self, s: AgentColourState, hex: &str) {
        self.write_choice(s, Some(hex));
    }

    fn write_choice(&self, s: AgentColourState, hex: Option<&str>) {
        match canonical_stored(hex) {
            Some(v) => self.store.set(s.key(), &v),
            None => self.store.remove(s.key()),
        }
    }

    /// The one Custom theme, or None before any save (or after a corrupt one).
    pub fn custom(&self) -> Option<CustomTheme> {
        self.store
            .get(CUSTOM_KEY)
            .and_then(|raw| parse_custom(&raw))
    }

    pub fn save_custom(&self, c: &CustomTheme) {
        self.store.set(CUSTOM_KEY, &serialize_custom(c));
    }

    /// The colour editor's draft. Memory only.
    pub fn draft(&self) -> Option<CustomPalette> {
        lock(&self.shared.draft).clone()
    }

    /// Sets or drops the draft and tells every listener once.
    pub fn set_draft(&self, d: Option<CustomPalette>) {
        *lock(&self.shared.draft) = d;
        let hits: Vec<Callback> = lock(&self.shared.listeners)
            .list
            .iter()
            .map(|(_, cb)| Arc::clone(cb))
            .collect();
        for cb in hits {
            cb();
        }
    }

    /// What every theme-bound setting is out of the box.
    pub fn extras_defaults() -> Extras {
        Extras {
            indicator_color: String::new(),
            done_color: String::new(),
            question_color: String::new(),
            see_through: 0,
        }
    }

    /// Picking a theme puts the look's extras back to their defaults. The font and its size belong to no
    /// theme and are untouched.
    pub fn reset_term_extras(&self) {
        self.store.remove(INDICATOR_KEY);
        for s in COLOUR_STATES {
            self.store.remove(s.key());
        }
        // Written, not removed: a stored level is also the sign the settings were ever set.
        self.store.set(SEE_THROUGH_KEY, "0");
    }

    /// Re-applies the non-colour half of a saved Custom. The indicator's volume is NOT applied: it is how
    /// loudly a tab speaks, the owner's own (a Custom imported from Prism Terminal keeps its `indicator`
    /// field, and picking it must not change theirs: review #77). Colours only when valid, and the
    /// see-through level is the save's (none is 0).
    pub fn apply_custom_extras(&self, c: Option<&CustomTheme>) {
        let Some(c) = c else { return };
        self.write_choice(AgentColourState::Working, c.indicator_color.as_deref());
        self.write_choice(AgentColourState::Finished, c.done_color.as_deref());
        self.write_choice(AgentColourState::Question, c.question_color.as_deref());
        let level = c.see_through.unwrap_or(0).min(SEE_THROUGH_TOP);
        self.store.set(SEE_THROUGH_KEY, &level.to_string());
    }

    fn current_extras(&self) -> Extras {
        Extras {
            indicator_color: self.agent_colour_choice(AgentColourState::Working),
            done_color: self.agent_colour_choice(AgentColourState::Finished),
            question_color: self.agent_colour_choice(AgentColourState::Question),
            see_through: self.see_through(),
        }
    }

    /// Dirty = the extras deviate from the selected theme's stock: any preset arrives with the defaults,
    /// the Custom with what it saved. A picked Background never counts.
    pub fn setup_state(&self) -> SetupState {
        let extras = self.current_extras();
        let src = if self.theme_id() == "custom" {
            self.custom()
        } else {
            None
        };
        let d = Self::extras_defaults();
        let baseline = match &src {
            Some(c) => Extras {
                indicator_color: c.indicator_color.clone().unwrap_or(d.indicator_color),
                done_color: c.done_color.clone().unwrap_or(d.done_color),
                question_color: c.question_color.clone().unwrap_or(d.question_color),
                see_through: c.see_through.unwrap_or(d.see_through).min(SEE_THROUGH_TOP),
            },
            None => d,
        };
        SetupState {
            dirty: extras != baseline,
            extras,
        }
    }

    /// Saves the whole setup in force as Custom, and selects it.
    pub fn save_term_setup(&self) {
        let id = self.theme_id();
        let palette = palette_of(&id, self.custom().as_ref(), &id);
        self.save_as_custom(&with_ground_alpha(&palette));
    }

    /// Saves a palette (the ground opaque) with the setup in force as Custom, and selects it: Save
    /// changes, and the colour editor's save.
    pub fn save_as_custom(&self, palette: &CustomPalette) {
        let extras = self.setup_state().extras;
        let keep = |s: String| (!s.is_empty()).then_some(s);
        self.save_custom(&CustomTheme {
            palette: with_ground_alpha(palette),
            indicator: None,
            indicator_color: keep(extras.indicator_color),
            done_color: keep(extras.done_color),
            question_color: keep(extras.question_color),
            see_through: Some(extras.see_through),
        });
        self.set_theme_id("custom");
    }

    /// Picking a preset returns the look to its defaults. The indicator's volume is carried across: it is
    /// how loudly a tab speaks, not what the terminal looks like.
    pub fn pick_preset(&self, id: &str) {
        let volume = self.agent_indicator();
        self.set_theme_id(id);
        self.reset_term_extras();
        if volume != AgentIndicator::Ring {
            self.set_agent_indicator(volume);
        }
    }

    /// A theme pick landing (PT `land` and `onThemePicked`). A saved Custom brings its extras and the
    /// window colours its save kept; a preset forgets the window colours. `edited` is the colour editor's
    /// save (already stored by `save_as_custom`): the kept colours are cleared first and the window wears
    /// the edited background.
    pub fn land_theme(&self, id: &str, edited: bool) {
        if edited {
            set_custom_window_colours(&self.store, None);
            set_window_colours(&self.store, &NO_COLOURS);
            return;
        }
        match (id == "custom", self.custom()) {
            (true, Some(saved)) => {
                self.set_theme_id("custom");
                self.apply_custom_extras(Some(&saved));
                set_window_colours(&self.store, &custom_window_colours(&self.store));
            }
            _ => {
                self.pick_preset(id);
                set_window_colours(&self.store, &NO_COLOURS);
            }
        }
    }

    /// A pick with unsaved changes asks first.
    pub fn pick_needs_ask(&self) -> bool {
        self.setup_state().dirty
    }

    /// Hears any look key, the window colour keys, the edges key, or the draft.
    pub fn on_look_change(&self, cb: impl Fn() + Send + Sync + 'static) -> Subscription {
        let cb: Callback = Arc::new(cb);
        let for_keys = Arc::clone(&cb);
        let keys = on_any_change(&self.store, &LOOK_KEYS, move || for_keys());
        let id = {
            let mut l = lock(&self.shared.listeners);
            l.next_id += 1;
            let id = l.next_id;
            l.list.push((id, cb));
            id
        };
        let shared = Arc::clone(&self.shared);
        Subscription::new(move || {
            drop(keys);
            lock(&shared.listeners).list.retain(|(i, _)| *i != id);
        })
    }

    /// Owned copies of everything `apply_look` reads.
    pub fn input_snapshot(&self) -> LookSnapshot {
        let colours = read_window_colours(&self.store);
        LookSnapshot {
            theme_id: self.theme_id(),
            custom: self.custom().map(|c| c.palette),
            draft: self.draft(),
            background: colours.background,
            accent: colours.accent,
            secondary: colours.secondary,
            edges: window_edges(&self.store),
            see_through: self.see_through(),
        }
    }

    /// What the window wears, from the settings in force.
    pub fn look(&self, glass_possible: bool) -> AppliedLook {
        apply_look(&self.input_snapshot().as_input(glass_possible))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::window_colours::{WindowColours, read_window_colours, set_window_edges};
    use flare_theme::look::apply_look;
    use flare_theme::resolve::{resolve_saved_theme, resolve_theme};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn store() -> (PrefStore, LookStore) {
        let p = PrefStore::memory();
        (p.clone(), LookStore::new(p))
    }

    fn counter() -> (Arc<AtomicUsize>, impl Fn() + Send + Sync + 'static) {
        let n = Arc::new(AtomicUsize::new(0));
        let n2 = Arc::clone(&n);
        (n, move || {
            n2.fetch_add(1, Ordering::SeqCst);
        })
    }

    const CUSTOM_JSON: &str = r##"{"bg":"#111111","fg":"#eeeeee","cursor":"#ffffff","ansi":{}}"##;

    #[test]
    fn a_fresh_store_wears_pt_default_at_100_percent() {
        let (_, l) = store();
        assert_eq!(l.theme_id(), "pt-default");
        assert_eq!(l.font_pct(), 100);
        assert_eq!(l.font_id(), "cascadia");
        assert_eq!(l.see_through(), 0);
    }

    #[test]
    fn a_theme_and_a_size_round_trip() {
        let (_, l) = store();
        l.set_theme_id("dracula");
        l.set_font_pct(120);
        assert_eq!(l.theme_id(), "dracula");
        assert_eq!(l.font_pct(), 120);
        assert_eq!(flare_theme::fonts::base_font_px(l.font_pct()), 16);
    }

    #[test]
    fn a_stored_style_reads_as_the_host_default() {
        let (p, l) = store();
        p.set(THEME_KEY, "style");
        assert_eq!(l.theme_id(), "pt-default");
        p.set(THEME_KEY, "");
        assert_eq!(l.theme_id(), "pt-default");
    }

    #[test]
    fn a_retired_theme_reads_as_its_replacement() {
        let (p, l) = store();
        let (old, new) = flare_theme::presets_data::RETIRED[0];
        p.set(THEME_KEY, old);
        assert_eq!(l.theme_id(), new);
    }

    #[test]
    fn an_off_menu_percentage_falls_back_to_100() {
        let (p, l) = store();
        p.set(FONT_PCT_KEY, "733");
        assert_eq!(l.font_pct(), 100);
        p.set(FONT_PCT_KEY, "20");
        assert_eq!(l.font_pct(), 100);
        p.set(FONT_PCT_KEY, "125");
        assert_eq!(l.font_pct(), 130);
    }

    #[test]
    fn a_theme_leaves_the_font_and_its_size_alone() {
        let (_, l) = store();
        l.set_font_id("consolas");
        l.set_font_pct(140);
        l.reset_term_extras();
        let c = parse_custom(
            r##"{"bg":"#111111","fg":"#eeeeee","cursor":"#fff","ansi":{},"font":"cascadia","fontPct":90}"##,
        );
        l.apply_custom_extras(c.as_ref());
        assert_eq!(l.font_id(), "consolas");
        assert_eq!(l.font_pct(), 140);
    }

    #[test]
    fn the_one_custom_theme_overwrites_its_slot() {
        let (_, l) = store();
        assert_eq!(l.custom(), None);
        let mut a = parse_custom(CUSTOM_JSON).unwrap();
        l.save_custom(&a);
        assert_eq!(l.custom(), Some(a.clone()));
        a.palette.bg = "#222222".into();
        l.save_custom(&a);
        assert_eq!(l.custom().unwrap().palette.bg, "#222222");
    }

    #[test]
    fn a_corrupt_save_reads_as_no_custom_theme() {
        let (p, l) = store();
        p.set(CUSTOM_KEY, "{nope");
        assert_eq!(l.custom(), None);
    }

    #[test]
    fn the_indicator_defaults_to_ring_and_round_trips_minimal() {
        let (_, l) = store();
        assert_eq!(l.agent_indicator(), AgentIndicator::Ring);
        l.set_agent_indicator(AgentIndicator::Minimal);
        assert_eq!(l.agent_indicator(), AgentIndicator::Minimal);
    }

    #[test]
    fn garbage_reads_as_the_default_indicator() {
        let (p, l) = store();
        for junk in ["soup", "full", "off", "loud"] {
            p.set(INDICATOR_KEY, junk);
            assert_eq!(l.agent_indicator(), AgentIndicator::Ring, "{junk}");
        }
    }

    #[test]
    fn reads_a_stored_value_flare_draws_as_itself() {
        let (p, l) = store();
        p.set(INDICATOR_KEY, "ring");
        assert_eq!(l.agent_indicator(), AgentIndicator::Ring);
        p.set(INDICATOR_KEY, "minimal");
        assert_eq!(l.agent_indicator(), AgentIndicator::Minimal);
    }

    #[test]
    fn never_answers_full_or_off_even_when_stored() {
        let (_, l) = store();
        l.set_agent_indicator(AgentIndicator::Full);
        assert_eq!(l.agent_indicator(), AgentIndicator::Ring);
        l.set_agent_indicator(AgentIndicator::Off);
        assert_eq!(l.agent_indicator(), AgentIndicator::Ring);
    }

    #[test]
    fn does_not_write_a_saved_custom_full_over_the_choice() {
        let (p, l) = store();
        l.set_agent_indicator(AgentIndicator::Minimal);
        let c = parse_custom(
            r##"{"bg":"#111111","fg":"#eeeeee","cursor":"#fff","ansi":{},"indicator":"full"}"##,
        );
        l.apply_custom_extras(c.as_ref());
        assert_eq!(p.get(INDICATOR_KEY).as_deref(), Some("minimal"));
    }

    #[test]
    fn picking_a_custom_keeps_the_owners_indicator_whatever_the_custom_carries() {
        let (p, l) = store();
        l.set_agent_indicator(AgentIndicator::Minimal);
        for carried in ["ring", "minimal"] {
            let c = parse_custom(&format!(
                r##"{{"bg":"#111111","fg":"#eeeeee","cursor":"#fff","ansi":{{}},"indicator":"{carried}"}}"##
            ));
            l.apply_custom_extras(c.as_ref());
            assert_eq!(
                p.get(INDICATOR_KEY).as_deref(),
                Some("minimal"),
                "{carried}"
            );
        }
    }

    #[test]
    fn the_working_colour_follows_the_theme_until_picked_and_can_be_given_back() {
        let (p, l) = store();
        assert_eq!(l.agent_colour_choice(AgentColourState::Working), "");
        l.set_agent_colour(AgentColourState::Working, "#22c55e");
        assert_eq!(l.agent_colour_choice(AgentColourState::Working), "#22c55e");
        l.set_agent_colour(AgentColourState::Working, "");
        assert_eq!(p.get(WORKING_KEY), None);
        assert_eq!(l.agent_colour_choice(AgentColourState::Working), "");
    }

    #[test]
    fn garbage_reads_as_following_the_theme() {
        let (p, l) = store();
        p.set(WORKING_KEY, "soup");
        assert_eq!(l.agent_colour_choice(AgentColourState::Working), "");
    }

    #[test]
    fn the_three_agent_colours_accept_hex8_and_refuse_7_or_9_digits() {
        let (_, l) = store();
        for s in COLOUR_STATES {
            l.set_agent_colour(s, "#22C55E80");
            assert_eq!(l.agent_colour_choice(s), "#22c55e80");
            l.set_agent_colour(s, "#22c55e8");
            assert_eq!(l.agent_colour_choice(s), "");
            l.set_agent_colour(s, "#22c55e80");
            l.set_agent_colour(s, "#22c55e800");
            assert_eq!(l.agent_colour_choice(s), "");
        }
    }

    #[test]
    fn a_picked_opaque_colour_is_stored_exactly_as_it_always_was() {
        let (p, l) = store();
        l.set_agent_colour(AgentColourState::Finished, "#22C55E");
        assert_eq!(p.get(FINISHED_KEY).as_deref(), Some("#22c55e"));
        l.set_agent_colour(AgentColourState::Question, "#22c55eff");
        assert_eq!(p.get(QUESTION_KEY).as_deref(), Some("#22c55e"));
    }

    #[test]
    fn apply_custom_extras_writes_only_valid_colours() {
        let (p, l) = store();
        l.set_agent_colour(AgentColourState::Working, "#abcdef");
        l.set_agent_colour(AgentColourState::Question, "#abcdef");
        let c = parse_custom(
            r##"{"bg":"#111111","fg":"#eeeeee","cursor":"#fff","ansi":{},"indicatorColor":"soup","doneColor":"#11223380","questionColor":"#zzzzzz"}"##,
        );
        l.apply_custom_extras(c.as_ref());
        assert_eq!(p.get(WORKING_KEY), None);
        assert_eq!(p.get(FINISHED_KEY).as_deref(), Some("#11223380"));
        assert_eq!(p.get(QUESTION_KEY), None);
    }

    #[test]
    fn no_extra_carries_an_opacity() {
        let (p, l) = store();
        let c = parse_custom(
            r##"{"bg":"#111111","fg":"#eeeeee","cursor":"#fff","ansi":{},"opacity":60,"acrylic":true}"##,
        );
        l.apply_custom_extras(c.as_ref());
        assert_eq!(p.get("flare.term.opacity"), None);
        assert_eq!(p.get("flare.term.acrylic"), None);
    }

    #[test]
    fn the_ground_alpha_is_the_sliders_never_the_colours() {
        let (_, l) = store();
        let mut c = parse_custom(CUSTOM_JSON).unwrap();
        c.palette.bg = "#12121205".into();
        l.save_custom(&c);
        l.set_theme_id("custom");
        l.set_see_through(70);
        assert!((l.look(true).ground_alpha - 0.3).abs() < 1e-9);
        l.set_see_through(0);
        assert!((l.look(true).ground_alpha - 1.0).abs() < 1e-9);
    }

    #[test]
    fn see_through_is_zero_by_default_and_the_window_solid() {
        let (_, l) = store();
        let a = l.look(true);
        assert_eq!(l.see_through(), 0);
        assert!(!a.glass);
        assert!((a.ground_alpha - 1.0).abs() < 1e-9);
    }

    #[test]
    fn a_level_paints_the_ground_at_one_minus_level_and_turns_glass_on() {
        let (_, l) = store();
        l.set_see_through(40);
        let a = l.look(true);
        assert!(a.glass);
        assert!((a.ground_alpha - 0.6).abs() < 1e-9);
        l.set_see_through(0);
        assert!(!l.look(true).glass);
    }

    #[test]
    fn see_through_reads_clamped_and_nonsense_as_zero() {
        let (p, l) = store();
        l.set_see_through(95);
        assert_eq!(l.see_through(), 70);
        assert!((l.look(true).ground_alpha - 0.3).abs() < 1e-9);
        p.set(SEE_THROUGH_KEY, "glass");
        assert_eq!(l.see_through(), 0);
        p.set(SEE_THROUGH_KEY, "");
        assert_eq!(l.see_through(), 0);
    }

    #[test]
    fn a_picked_or_custom_alpha_is_no_see_through() {
        let (_, l) = store();
        BACKGROUND.set(&l.store, Some("#33333399"));
        assert!((l.look(true).ground_alpha - 1.0).abs() < 1e-9);
        BACKGROUND.set(&l.store, None);
        let mut c = parse_custom(CUSTOM_JSON).unwrap();
        c.palette.bg = "#11111180".into();
        l.save_custom(&c);
        l.set_theme_id("custom");
        assert!((l.look(true).ground_alpha - 1.0).abs() < 1e-9);
    }

    #[test]
    fn custom_saves_the_ground_opaque() {
        let mut p = parse_custom(CUSTOM_JSON).unwrap().palette;
        p.bg = "#20202080".into();
        assert_eq!(with_ground_alpha(&p).bg, "#202020");
        let (_, l) = store();
        l.save_as_custom(&p);
        assert_eq!(l.custom().unwrap().palette.bg, "#202020");
    }

    #[test]
    fn a_theme_pick_puts_its_own_level_back_and_a_custom_brings_its_saved_one() {
        let (_, l) = store();
        l.set_see_through(30);
        l.reset_term_extras();
        assert_eq!(l.see_through(), 0);
        let mut c = parse_custom(CUSTOM_JSON).unwrap();
        c.see_through = Some(25);
        l.apply_custom_extras(Some(&c));
        assert_eq!(l.see_through(), 25);
        c.see_through = None;
        l.apply_custom_extras(Some(&c));
        assert_eq!(l.see_through(), 0);
    }

    #[test]
    fn pt_default_and_unset_settings_read_as_flare_says() {
        let (_, l) = store();
        assert_eq!(l.theme_id(), "pt-default");
        assert_eq!(l.agent_indicator(), AgentIndicator::Ring);
        assert_eq!(l.agent_colour_choice(AgentColourState::Working), "");
        l.set_theme_id("nord");
        l.set_agent_indicator(AgentIndicator::Minimal);
        l.set_agent_colour(AgentColourState::Working, "#123456");
        assert_eq!(l.theme_id(), "nord");
        assert_eq!(l.agent_indicator(), AgentIndicator::Minimal);
        assert_eq!(l.agent_colour_choice(AgentColourState::Working), "#123456");
    }

    #[test]
    fn saving_pt_default_edited_keeps_its_orange_not_the_palette_blue() {
        let (_, l) = store();
        l.set_theme_id("pt-default");
        let mut p = palette_of("pt-default", None, "pt-default");
        assert_eq!(p.accent.as_deref(), Some("#fe8f34"));
        p.cursor = "#ff00aa".into();
        l.save_as_custom(&p);
        assert_eq!(
            l.custom().unwrap().palette.accent.as_deref(),
            Some("#fe8f34")
        );
        assert_eq!(l.theme_id(), "custom");
        let c = l.custom();
        assert_eq!(
            flare_theme::resolve::preset_accent(
                "custom",
                "custom",
                c.as_ref().map(|c| &c.palette),
                None
            )
            .as_deref(),
            Some("#fe8f34")
        );
    }

    #[test]
    fn a_custom_edited_again_keeps_it() {
        let (_, l) = store();
        let mut p = palette_of("pt-default", None, "pt-default");
        p.cursor = "#ff00aa".into();
        l.save_as_custom(&p);
        let again = palette_of("custom", l.custom().as_ref(), "custom");
        assert_eq!(again.accent.as_deref(), Some("#fe8f34"));
    }

    #[test]
    fn an_older_custom_with_no_accent_asks_for_none() {
        let (_, l) = store();
        l.save_custom(&parse_custom(CUSTOM_JSON).unwrap());
        l.set_theme_id("custom");
        let c = l.custom();
        assert_eq!(
            flare_theme::resolve::preset_accent(
                "custom",
                "custom",
                c.as_ref().map(|c| &c.palette),
                None
            ),
            None
        );
    }

    #[test]
    fn the_draft_tells_every_listener_once() {
        let (_, l) = store();
        l.set_theme_id("pt-default");
        let (n, cb) = counter();
        let sub = l.on_look_change(cb);
        let mut d = palette_of("pt-default", None, "pt-default");
        d.cursor = "#ff00aa".into();
        d.accent = Some("#00ff88".into());
        l.set_draft(Some(d.clone()));
        assert_eq!(n.load(Ordering::SeqCst), 1);
        assert_eq!(l.draft(), Some(d));
        let worn = l.look(true);
        assert_ne!(
            worn.term.cursor,
            resolve_saved_theme("pt-default", None, None).cursor
        );
        drop(sub);
        l.set_draft(None);
        assert_eq!(n.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn cancel_drops_the_draft_and_the_saved_theme_is_back() {
        let (_, l) = store();
        l.set_theme_id("pt-default");
        let before = l.look(true);
        let mut d = palette_of("pt-default", None, "pt-default");
        d.cursor = "#ff00aa".into();
        l.set_draft(Some(d));
        l.set_draft(None);
        assert_eq!(l.draft(), None);
        assert_eq!(l.look(true).term, before.term);
        assert_eq!(
            l.look(true).term,
            flare_theme::resolve::on_ground(&resolve_saved_theme("pt-default", None, None), None)
        );
    }

    #[test]
    fn another_theme_never_sees_the_draft() {
        let (_, l) = store();
        l.set_theme_id("pt-default");
        let mut d = palette_of("pt-default", None, "pt-default");
        d.cursor = "#ff00aa".into();
        l.set_draft(Some(d.clone()));
        // A card for another theme resolves as saved; only the theme in force wears the draft.
        assert_eq!(
            resolve_theme("paper", "pt-default", None, Some(&d), None),
            resolve_saved_theme("paper", None, None)
        );
        assert_eq!(
            resolve_saved_theme("pt-default", None, None),
            resolve_saved_theme("pt-default", None, None)
        );
    }

    #[test]
    fn a_preset_at_zero_is_clean_a_moved_slider_is_a_change_to_save() {
        let (_, l) = store();
        l.set_theme_id("dracula");
        assert!(!l.setup_state().dirty);
        l.set_see_through(35);
        assert!(l.setup_state().dirty);
        l.set_see_through(0);
        assert!(!l.setup_state().dirty);
    }

    #[test]
    fn a_picked_background_never_lights_save() {
        let (_, l) = store();
        l.set_theme_id("dracula");
        BACKGROUND.set(&l.store, Some("#ffffff"));
        assert!(!l.setup_state().dirty);
        BACKGROUND.set(&l.store, Some("#ffffff99"));
        assert!(!l.setup_state().dirty);
    }

    #[test]
    fn a_save_keeps_the_level_with_custom_on_an_opaque_ground_and_is_clean_afterwards() {
        let (_, l) = store();
        l.set_theme_id("dracula");
        l.set_see_through(35);
        l.save_term_setup();
        let c = l.custom().unwrap();
        assert_eq!(c.see_through, Some(35));
        assert_eq!(c.palette.bg.len(), 7);
        assert!(!l.setup_state().dirty);
        l.set_see_through(20);
        assert!(l.setup_state().dirty);
    }

    #[test]
    fn the_editor_opens_on_an_opaque_ground() {
        let (_, l) = store();
        let mut c = parse_custom(CUSTOM_JSON).unwrap();
        c.palette.bg = "#11111180".into();
        l.save_custom(&c);
        assert_eq!(
            with_ground_alpha(&palette_of("custom", l.custom().as_ref(), "custom")).bg,
            "#111111"
        );
    }

    #[test]
    fn landing_a_preset_forgets_the_window_colours() {
        let (p, l) = store();
        BACKGROUND.set(&p, Some("#101010"));
        ACCENT.set(&p, Some("#ff8800"));
        SECONDARY.set(&p, Some("#2a2a2a"));
        l.land_theme("nord", false);
        assert_eq!(read_window_colours(&p), NO_COLOURS);
        assert_eq!(l.theme_id(), "nord");
    }

    #[test]
    fn landing_custom_restores_what_it_kept() {
        let (p, l) = store();
        l.save_custom(&parse_custom(CUSTOM_JSON).unwrap());
        let kept = WindowColours {
            background: Some("#101010".into()),
            accent: Some("#ff8800".into()),
            secondary: Some("#2a2a2a".into()),
        };
        set_custom_window_colours(&p, Some(&kept));
        l.land_theme("nord", false);
        assert_eq!(read_window_colours(&p), NO_COLOURS);
        l.land_theme("custom", false);
        assert_eq!(read_window_colours(&p), kept);
        assert_eq!(l.theme_id(), "custom");
    }

    #[test]
    fn the_editors_save_clears_the_kept_colours_first() {
        let (p, l) = store();
        let kept = WindowColours {
            background: Some("#101010".into()),
            accent: None,
            secondary: None,
        };
        set_custom_window_colours(&p, Some(&kept));
        set_window_colours(&p, &kept);
        let pal = palette_of("pt-default", None, "pt-default");
        l.save_as_custom(&pal);
        l.land_theme("custom", true);
        assert_eq!(custom_window_colours(&p), NO_COLOURS);
        assert_eq!(read_window_colours(&p), NO_COLOURS);
        assert_eq!(l.theme_id(), "custom");
    }

    #[test]
    fn a_clean_pick_lands_at_once_and_a_dirty_one_asks() {
        let (_, l) = store();
        l.set_theme_id("dracula");
        assert!(!l.pick_needs_ask());
        l.set_see_through(10);
        assert!(l.pick_needs_ask());
    }

    #[test]
    fn a_preset_pick_carries_the_indicators_volume_across() {
        let (_, l) = store();
        l.set_agent_indicator(AgentIndicator::Minimal);
        l.set_agent_colour(AgentColourState::Working, "#123456");
        l.pick_preset("nord");
        assert_eq!(l.agent_indicator(), AgentIndicator::Minimal);
        assert_eq!(l.agent_colour_choice(AgentColourState::Working), "");
    }

    #[test]
    fn look_reads_everything_through_apply_look() {
        let (p, l) = store();
        ACCENT.set(&p, Some("#e07a2f"));
        BACKGROUND.set(&p, Some("#1a1b26"));
        SECONDARY.set(&p, Some("#2a2a2a"));
        set_window_edges(&p, WindowEdges::Solid);
        l.set_theme_id("dracula");
        l.set_see_through(30);
        let want = apply_look(&LookInput {
            theme_id: "dracula",
            custom: None,
            draft: None,
            background: Some("#1a1b26"),
            accent: Some("#e07a2f"),
            secondary: Some("#2a2a2a"),
            edges: WindowEdges::Solid,
            see_through: 30,
            glass_possible: true,
        });
        assert_eq!(l.look(true), want);
        assert!(!l.look(false).glass);
    }

    #[test]
    fn on_look_change_hears_every_look_key_and_the_draft() {
        let (p, l) = store();
        let (n, cb) = counter();
        let sub = l.on_look_change(cb);
        let mut expect = 0;
        let mut step = |f: &dyn Fn()| {
            f();
            expect += 1;
            assert_eq!(n.load(Ordering::SeqCst), expect);
        };
        step(&|| l.set_theme_id("nord"));
        step(&|| l.save_custom(&parse_custom(CUSTOM_JSON).unwrap()));
        step(&|| l.set_see_through(10));
        step(&|| l.set_font_id("consolas"));
        step(&|| l.set_font_pct(110));
        step(&|| ACCENT.set(&p, Some("#112233")));
        step(&|| BACKGROUND.set(&p, Some("#112233")));
        step(&|| SECONDARY.set(&p, Some("#112233")));
        step(&|| set_window_edges(&p, WindowEdges::Solid));
        step(&|| l.set_draft(None));
        drop(sub);
        l.set_theme_id("dracula");
        assert_eq!(n.load(Ordering::SeqCst), expect);
    }

    #[test]
    fn two_stores_over_one_pref_store_share_the_draft_and_its_listeners() {
        let p = PrefStore::memory();
        let (a, b) = (LookStore::new(p.clone()), LookStore::new(p.clone()));
        let (n, cb) = counter();
        let _sub = b.on_look_change(cb);
        a.set_draft(parse_custom(CUSTOM_JSON).map(|c| c.palette));
        assert!(b.draft().is_some(), "the other store sees the draft");
        assert_eq!(n.load(Ordering::SeqCst), 1, "and hears it set");
        assert!(LookStore::new(PrefStore::memory()).draft().is_none());
    }

    #[test]
    fn the_caret_is_a_block_that_blinks_until_chosen_and_a_change_is_heard() {
        let p = PrefStore::memory();
        let l = LookStore::new(p.clone());
        assert_eq!(l.caret_shape(), "block");
        assert!(l.caret_blink());
        let (n, cb) = counter();
        let _sub = l.on_look_change(cb);
        for (word, _) in CARET_SHAPES {
            l.set_caret_shape(word);
            assert_eq!(l.caret_shape(), word);
            assert_eq!(p.get("flare.term.caretShape").as_deref(), Some(word));
        }
        l.set_caret_shape("beam");
        assert_eq!(
            p.get("flare.term.caretShape").as_deref(),
            Some("block"),
            "junk is never stored"
        );
        p.set("flare.term.caretShape", "nope");
        assert_eq!(l.caret_shape(), "block");
        l.set_caret_blink(false);
        assert!(!l.caret_blink());
        assert_eq!(p.get("flare.term.caretBlink").as_deref(), Some("0"));
        l.set_caret_blink(true);
        assert!(l.caret_blink());
        assert!(
            n.load(Ordering::SeqCst) >= 7,
            "the look hears every caret write"
        );
    }
}

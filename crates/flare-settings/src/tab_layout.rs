//! TabLayout pref (Task 3): where the tab strip sits, horizontal along the top or the vertical sidebar.
//! Key `flare.window.tabLayout`; words `horizontal` and `vertical`; `horizontal` until somebody chooses.

use crate::kv::{EnumPref, PrefStore, Subscription};

const PREF: EnumPref = EnumPref {
    key: "flare.window.tabLayout",
    allowed: &["horizontal", "vertical"],
    fallback: "horizontal",
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabLayout {
    Horizontal,
    Vertical,
}

impl TabLayout {
    fn word(self) -> &'static str {
        match self {
            TabLayout::Horizontal => "horizontal",
            TabLayout::Vertical => "vertical",
        }
    }
}

pub fn tab_layout(s: &PrefStore) -> TabLayout {
    match PREF.get(s) {
        "vertical" => TabLayout::Vertical,
        _ => TabLayout::Horizontal,
    }
}

pub fn set_tab_layout(s: &PrefStore, v: TabLayout) {
    PREF.set(s, v.word());
}

/// Stores a word as given, or the fallback when it is not one of the two.
pub fn set_tab_layout_word(s: &PrefStore, word: &str) {
    PREF.set(s, word);
}

pub fn on_tab_layout_change(s: &PrefStore, cb: impl Fn() + Send + Sync + 'static) -> Subscription {
    s.on_change(PREF.key, cb)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn tab_layout_is_horizontal_until_chosen() {
        assert_eq!(tab_layout(&PrefStore::memory()), TabLayout::Horizontal);
    }

    #[test]
    fn tab_layout_is_remembered_under_its_own_key() {
        let s = PrefStore::memory();
        set_tab_layout(&s, TabLayout::Vertical);
        assert_eq!(
            s.get("flare.window.tabLayout"),
            Some("vertical".to_string())
        );
        assert_eq!(tab_layout(&s), TabLayout::Vertical);
        set_tab_layout(&s, TabLayout::Horizontal);
        assert_eq!(tab_layout(&s), TabLayout::Horizontal);
    }

    #[test]
    fn tab_layout_reads_unknown_as_horizontal_and_never_stores_one() {
        let s = PrefStore::memory();
        s.set("flare.window.tabLayout", "diagonal");
        assert_eq!(tab_layout(&s), TabLayout::Horizontal);
        set_tab_layout_word(&s, "left");
        assert_eq!(
            s.get("flare.window.tabLayout"),
            Some("horizontal".to_string())
        );
    }

    #[test]
    fn tab_layout_listeners() {
        let s = PrefStore::memory();
        let hits = Arc::new(AtomicUsize::new(0));
        let h = Arc::clone(&hits);
        let sub = on_tab_layout_change(&s, move || {
            h.fetch_add(1, Ordering::SeqCst);
        });
        set_tab_layout(&s, TabLayout::Vertical);
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        sub.cancel();
        set_tab_layout(&s, TabLayout::Horizontal);
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }
}

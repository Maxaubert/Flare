//! TabSwitch pref (Task 3): Ctrl+Tab walks the tabs in order, or in most-recent order.
//! Key `flare.window.tabSwitch`; words `order` and `recent`; `order` until somebody chooses.

use crate::kv::{EnumPref, PrefStore, Subscription};

const PREF: EnumPref = EnumPref {
    key: "flare.window.tabSwitch",
    allowed: &["order", "recent"],
    fallback: "order",
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabSwitch {
    Order,
    Recent,
}

impl TabSwitch {
    fn word(self) -> &'static str {
        match self {
            TabSwitch::Order => "order",
            TabSwitch::Recent => "recent",
        }
    }
}

pub fn tab_switch(s: &PrefStore) -> TabSwitch {
    match PREF.get(s) {
        "recent" => TabSwitch::Recent,
        _ => TabSwitch::Order,
    }
}

pub fn set_tab_switch(s: &PrefStore, v: TabSwitch) {
    PREF.set(s, v.word());
}

/// Stores a word as given, or the fallback when it is not one of the two.
pub fn set_tab_switch_word(s: &PrefStore, word: &str) {
    PREF.set(s, word);
}

pub fn on_tab_switch_change(s: &PrefStore, cb: impl Fn() + Send + Sync + 'static) -> Subscription {
    s.on_change(PREF.key, cb)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn tab_switch_is_in_order_until_chosen() {
        assert_eq!(tab_switch(&PrefStore::memory()), TabSwitch::Order);
    }

    #[test]
    fn tab_switch_is_remembered_under_its_own_key() {
        let s = PrefStore::memory();
        set_tab_switch(&s, TabSwitch::Recent);
        assert_eq!(s.get("flare.window.tabSwitch"), Some("recent".to_string()));
        assert_eq!(tab_switch(&s), TabSwitch::Recent);
        set_tab_switch(&s, TabSwitch::Order);
        assert_eq!(s.get("flare.window.tabSwitch"), Some("order".to_string()));
        assert_eq!(tab_switch(&s), TabSwitch::Order);
    }

    #[test]
    fn tab_switch_reads_unknown_as_order_and_never_stores_one() {
        let s = PrefStore::memory();
        s.set("flare.window.tabSwitch", "mru");
        assert_eq!(tab_switch(&s), TabSwitch::Order);
        set_tab_switch_word(&s, "random");
        assert_eq!(s.get("flare.window.tabSwitch"), Some("order".to_string()));
    }

    #[test]
    fn tab_switch_listeners() {
        let s = PrefStore::memory();
        let hits = Arc::new(AtomicUsize::new(0));
        let h = Arc::clone(&hits);
        let sub = on_tab_switch_change(&s, move || {
            h.fetch_add(1, Ordering::SeqCst);
        });
        set_tab_switch(&s, TabSwitch::Recent);
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        sub.cancel();
        set_tab_switch(&s, TabSwitch::Order);
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }
}

//! The bookkeeping of the web panel's pop-ups (rules 249, 250), apart from COM so it is tested on its own: the
//! requests the browser holds while their view is made, and per tab a stack of the pop-ups that are open, the one on
//! top shown over the page.
//!
//! A request (`R`) holds the browser's deferral: the host's request type completes it when it is dropped, so every
//! way a request leaves the book (its view made, a failure, the panel or tab closed, Flare quitting) completes it.
//! The book itself only hands requests and views (`V`) out; it never calls the browser, so nothing it holds is
//! borrowed while the host does.

use std::collections::{BTreeMap, HashMap};

use crate::backend::Slot;

/// One open pop-up: the number its request was given, the site it shows (for the strip) and its view.
pub(crate) struct Open<V> {
    pub id: u64,
    pub site: String,
    pub view: V,
}

/// The pop-ups of every panel: requests waiting for their view, and open ones stacked per panel.
pub(crate) struct PopupBook<R, V> {
    seq: u64,
    waiting: BTreeMap<u64, (Slot, R)>,
    stacks: HashMap<Slot, Vec<Open<V>>>,
}

impl<R, V> Default for PopupBook<R, V> {
    fn default() -> Self {
        Self {
            seq: 0,
            waiting: BTreeMap::new(),
            stacks: HashMap::new(),
        }
    }
}

impl<R, V> PopupBook<R, V> {
    /// The pop-ups the panel of `slot` holds and the window holds, waiting ones included (what the limits count).
    pub fn counts(&self, slot: &Slot) -> (usize, usize) {
        let waiting_here = self.waiting.values().filter(|(s, _)| s == slot).count();
        let open_here = self.stacks.get(slot).map_or(0, Vec::len);
        let open_all: usize = self.stacks.values().map(Vec::len).sum();
        (waiting_here + open_here, self.waiting.len() + open_all)
    }

    /// Holds a request of the panel of `slot` while its view is made; the number names it from here on.
    pub fn ask(&mut self, slot: &Slot, request: R) -> u64 {
        self.seq += 1;
        self.waiting.insert(self.seq, (slot.clone(), request));
        self.seq
    }

    /// The panel a waiting request belongs to.
    pub fn waiting_slot(&self, id: u64) -> Option<&Slot> {
        self.waiting.get(&id).map(|(s, _)| s)
    }

    /// Takes a waiting request out (its view is made, or it failed). None once it was let go.
    pub fn take(&mut self, id: u64) -> Option<(Slot, R)> {
        self.waiting.remove(&id)
    }

    /// Puts an open pop-up on top of the stack of `slot`.
    pub fn push(&mut self, slot: &Slot, id: u64, site: String, view: V) {
        self.stacks
            .entry(slot.clone())
            .or_default()
            .push(Open { id, site, view });
    }

    /// The pop-up on top of the stack of `slot`: the one shown.
    pub fn top(&self, slot: &Slot) -> Option<&V> {
        self.stacks
            .get(slot)
            .and_then(|s| s.last())
            .map(|o| &o.view)
    }

    /// The panel an open pop-up belongs to.
    pub fn slot_of(&self, id: u64) -> Option<&Slot> {
        self.stacks
            .iter()
            .find(|(_, s)| s.iter().any(|o| o.id == id))
            .map(|(slot, _)| slot)
    }

    /// Takes an open pop-up out, wherever it is in its stack: its panel, its view and whether it was the one on top
    /// (the next one down, or the page, shows now).
    pub fn close(&mut self, id: u64) -> Option<(Slot, V, bool)> {
        let slot = self.slot_of(id)?.clone();
        let stack = self.stacks.get_mut(&slot)?;
        let at = stack.iter().position(|o| o.id == id)?;
        let top = at + 1 == stack.len();
        let open = stack.remove(at);
        if stack.is_empty() {
            self.stacks.remove(&slot);
        }
        Some((slot, open.view, top))
    }

    /// The number of the pop-up on top of the stack of `slot`.
    pub fn top_id(&self, slot: &Slot) -> Option<u64> {
        self.stacks.get(slot).and_then(|s| s.last()).map(|o| o.id)
    }

    /// The pop-up went to another site: true when that is a change (the strip says the new one).
    pub fn set_site(&mut self, id: u64, site: String) -> bool {
        for stack in self.stacks.values_mut() {
            if let Some(o) = stack.iter_mut().find(|o| o.id == id) {
                if o.site == site {
                    return false;
                }
                o.site = site;
                return true;
            }
        }
        false
    }

    /// The sites of the pop-ups of `slot`, the one on top last.
    pub fn sites(&self, slot: &Slot) -> Vec<String> {
        self.stacks
            .get(slot)
            .map(|s| s.iter().map(|o| o.site.clone()).collect())
            .unwrap_or_default()
    }

    /// Every open pop-up's view (the window moved: each is told).
    pub fn views(&self) -> impl Iterator<Item = &V> {
        self.stacks.values().flatten().map(|o| &o.view)
    }

    /// The open pop-ups of `slot`, the one on top last.
    pub fn open_of(&self, slot: &Slot) -> &[Open<V>] {
        self.stacks.get(slot).map_or(&[], Vec::as_slice)
    }

    /// Takes out everything of one panel (it closes) or of every panel (`None`: Flare quits, the browser is gone):
    /// the waiting requests and the open views. The caller lets the requests go (each completes its deferral) and
    /// closes the views.
    pub fn drain(&mut self, slot: Option<&Slot>) -> (Vec<R>, Vec<V>) {
        let ids: Vec<u64> = self
            .waiting
            .iter()
            .filter(|(_, (s, _))| slot.is_none_or(|want| s == want))
            .map(|(id, _)| *id)
            .collect();
        let requests = ids
            .into_iter()
            .filter_map(|id| self.waiting.remove(&id).map(|(_, r)| r))
            .collect();
        let slots: Vec<Slot> = self
            .stacks
            .keys()
            .filter(|s| slot.is_none_or(|want| *s == want))
            .cloned()
            .collect();
        let views = slots
            .into_iter()
            .filter_map(|s| self.stacks.remove(&s))
            .flatten()
            // The top first: the last shown closes first.
            .rev()
            .map(|o| o.view)
            .collect();
        (requests, views)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    /// A request that counts its completion when it is let go, as the host's completes the browser's deferral.
    struct Deferral(Rc<Cell<u32>>);

    impl Drop for Deferral {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    fn book() -> (PopupBook<Deferral, &'static str>, Rc<Cell<u32>>) {
        (PopupBook::default(), Rc::default())
    }

    #[test]
    fn a_waiting_request_counts_toward_the_limits_until_it_leaves() {
        let (mut b, done) = book();
        let (a, c) = (Slot::panel("a"), Slot::panel("c"));
        let one = b.ask(&a, Deferral(done.clone()));
        b.ask(&c, Deferral(done.clone()));
        assert_eq!(b.counts(&a), (1, 2));
        b.push(&a, 9, "bank.example".into(), "view");
        assert_eq!(b.counts(&a), (2, 3), "waiting and open both count");
        assert_eq!(b.waiting_slot(one), Some(&a));
        let (slot, r) = b.take(one).expect("still waiting");
        assert_eq!(slot, a);
        drop(r);
        assert_eq!(
            done.get(),
            1,
            "a request taken out is completed when let go"
        );
        assert!(b.take(one).is_none(), "a request leaves once");
        assert_eq!(b.counts(&a), (1, 2));
    }

    #[test]
    fn the_top_of_a_stack_shows_and_closing_it_shows_the_next_one_down() {
        let (mut b, _done) = book();
        let a = Slot::panel("a");
        b.push(&a, 1, "shop.example".into(), "first");
        b.push(&a, 2, "bank.example".into(), "second");
        assert_eq!(b.top(&a), Some(&"second"));
        assert_eq!(b.sites(&a), ["shop.example", "bank.example"]);
        assert_eq!(b.close(2), Some((a.clone(), "second", true)));
        assert_eq!(b.top(&a), Some(&"first"));
        // A pop-up under the top closes without changing what shows.
        b.push(&a, 3, "acs.example".into(), "third");
        assert_eq!(b.close(1), Some((a.clone(), "first", false)));
        assert_eq!(b.top_id(&a), Some(3));
        assert_eq!(b.close(3), Some((a.clone(), "third", true)));
        assert!(b.top(&a).is_none(), "the page shows again");
        assert!(b.close(3).is_none(), "a pop-up closes once");
        assert!(b.sites(&a).is_empty());
    }

    #[test]
    fn a_new_site_is_a_change_once() {
        let (mut b, _done) = book();
        let a = Slot::panel("a");
        b.push(&a, 1, "about".into(), "v");
        assert!(b.set_site(1, "bank.example".into()));
        assert!(!b.set_site(1, "bank.example".into()));
        assert!(!b.set_site(7, "x.example".into()), "no such pop-up");
        assert_eq!(b.sites(&a), ["bank.example"]);
    }

    #[test]
    fn closing_a_panel_completes_its_waiting_requests_and_hands_out_its_views_only() {
        let (mut b, done) = book();
        let (a, c) = (Slot::panel("a"), Slot::panel("c"));
        b.ask(&a, Deferral(done.clone()));
        b.ask(&a, Deferral(done.clone()));
        b.ask(&c, Deferral(done.clone()));
        b.push(&a, 10, "x".into(), "a1");
        b.push(&a, 11, "y".into(), "a2");
        b.push(&c, 12, "z".into(), "c1");
        let (requests, views) = b.drain(Some(&a));
        assert_eq!(requests.len(), 2);
        assert_eq!(views, ["a2", "a1"], "the top closes first");
        drop(requests);
        assert_eq!(done.get(), 2);
        assert_eq!(b.counts(&c), (2, 2), "the other tab keeps its own");
        assert!(b.top(&a).is_none());
    }

    #[test]
    fn quitting_completes_every_waiting_request_and_hands_out_every_view() {
        let (mut b, done) = book();
        for t in ["a", "c", "d"] {
            b.ask(&Slot::panel(t), Deferral(done.clone()));
        }
        b.push(&Slot::panel("a"), 5, "x".into(), "a1");
        b.push(&Slot::panel("d"), 6, "x".into(), "d1");
        let (requests, views) = b.drain(None);
        drop(requests);
        assert_eq!(done.get(), 3);
        assert_eq!(views.len(), 2);
        assert_eq!(b.counts(&Slot::panel("a")), (0, 0));
        assert_eq!(b.views().count(), 0);
    }

    #[test]
    fn a_book_let_go_completes_what_still_waits() {
        let (mut b, done) = book();
        b.ask(&Slot::panel("a"), Deferral(done.clone()));
        drop(b);
        assert_eq!(done.get(), 1);
    }
}

//! The "Preview" chip of a tab (Surfaces stage 7): when the terminal read a local dev server's address in a
//! tab's output (`flare_term::dev_server`), the chip is offered once something LISTENS on that port, and it
//! goes when nothing does any more (the process that printed the address is gone) or when the terminal
//! was cleared. Pure: the window says what the terminal read and what the listening check found, and asks
//! what to check and when.
//!
//! - One chip per tab, for the newest address.
//! - A new address replaces the chip at once; the new one waits for its port.
//! - The port is looked up in the system's listening table on a worker (never by connecting to it: a
//!   dev server's log would show a client that says nothing). A port not yet listening is looked for again,
//!   a few times over a few seconds, for a server that prints its address a moment before it listens.
//! - A shown chip is checked every [`RECHECK_MS`]; it goes after [`MISSES_TO_GO`] failed checks in a row, so a
//!   server restarting under a file watcher does not flicker the chip away. Only the chip of the tab in front is
//!   re-checked, and not while the window is minimised: a chip nobody can see costs nothing, and is looked at
//!   once as it comes to the front.
//! - A look answers with three things, not two: [`PortLook::Nobody`] (nothing listens), [`PortLook::InTree`] (every listener is
//!   in the tab's process tree) or [`PortLook::Foreign`] (a process outside the tree holds the port). A foreign holder
//!   drops the chip AT ONCE, with no miss grace, while waiting and while shown. A chip that comes to the front is
//!   hidden until its first look has answered, and a click is only honoured after a look of its own says
//!   [`PortLook::InTree`] for the same chip.
//! - The same address printed AGAIN (a server stopped and started on its port, without the screen being cleared)
//!   starts a new wait for a tab that holds no live chip for it.

use std::collections::HashMap;

/// When a port that was not listening is looked for again, from the address being read.
pub const WAIT_STEPS_MS: [u64; 6] = [0, 400, 1_200, 3_000, 7_000, 15_000];
/// How often a shown chip's port is looked at again.
pub const RECHECK_MS: u64 = 2_500;
/// Failed looks in a row after which a shown chip goes.
pub const MISSES_TO_GO: u32 = 2;

/// What a look at a port found, held against the tab's own process tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortLook {
    /// Nobody listens (or the tables could not be read): one miss.
    Nobody,
    /// Every listener is in the tab's process tree.
    InTree,
    /// A process outside the tab's tree holds the port: the chip goes at once.
    Foreign,
}

impl From<bool> for PortLook {
    fn from(up: bool) -> Self {
        if up {
            PortLook::InTree
        } else {
            PortLook::Nobody
        }
    }
}

/// The address a chip offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChipServer {
    pub url: String,
    pub port: u16,
}

/// A look the window must make on a worker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    pub tab: String,
    pub port: u16,
    /// Handed back with the answer; an answer for an older chip is dropped.
    pub token: u64,
}

#[derive(Debug)]
enum Phase {
    /// Not seen listening yet: `step` is the next entry of `WAIT_STEPS_MS` to use.
    Waiting { step: usize },
    /// Listening when last looked at; `misses` failed looks in a row.
    Shown { misses: u32 },
    /// Never seen listening in all the waiting: no chip, until a new address.
    GaveUp,
}

#[derive(Debug)]
struct Chip {
    server: ChipServer,
    token: u64,
    began: u64,
    next_at: Option<u64>,
    in_flight: bool,
    phase: Phase,
    /// False from the moment the tab comes to the front until a look made after that has answered; the chip is not
    /// offered meanwhile.
    verified: bool,
    /// A look was already out when the tab came to the front: its answer is older than the focus and is not enough.
    stale_flight: bool,
}

/// Every tab's chip.
#[derive(Debug, Default)]
pub struct DevChips {
    chips: HashMap<String, Chip>,
    /// The terminal's change counter last answered, by tab (kept after a clear, so a clear is answered once).
    seqs: HashMap<String, u64>,
    /// The terminal's reprint counter last answered, by tab.
    reprints: HashMap<String, u64>,
    tokens: u64,
    /// Who may re-check a shown chip: None until the window says (every tab then), else the tab in front, and
    /// nobody while the window is minimised.
    focus: Option<Option<String>>,
}

impl DevChips {
    pub fn new() -> Self {
        Self::default()
    }

    /// The terminal's reading for `tab`: its change counter and the address it holds now (None after a clear).
    /// Does nothing when the counter is the one already answered.
    pub fn on_reading(&mut self, tab: &str, seq: u64, server: Option<ChipServer>, now: u64) {
        if self.seqs.get(tab) == Some(&seq) {
            return;
        }
        self.seqs.insert(tab.to_string(), seq);
        match server {
            None => {
                self.chips.remove(tab);
            }
            Some(server) => self.begin(tab, server, now),
        }
    }

    fn begin(&mut self, tab: &str, server: ChipServer, now: u64) {
        self.tokens += 1;
        self.chips.insert(
            tab.to_string(),
            Chip {
                server,
                token: self.tokens,
                began: now,
                next_at: Some(now),
                in_flight: false,
                phase: Phase::Waiting { step: 0 },
                verified: true,
                stale_flight: false,
            },
        );
    }

    /// The terminal's reprint counter for `tab` and the address it holds: when the counter moved and the tab has no
    /// chip that is shown or being waited for, the address is looked for again (the server was started again).
    pub fn on_reprint(&mut self, tab: &str, reprint: u64, server: Option<ChipServer>, now: u64) {
        if self.reprints.insert(tab.to_string(), reprint) == Some(reprint) {
            return;
        }
        let Some(server) = server else { return };
        let live = self
            .chips
            .get(tab)
            .is_some_and(|c| !matches!(c.phase, Phase::GaveUp));
        if !live {
            self.begin(tab, server, now);
        }
    }

    /// The tab in front, or None when nothing is in view (the window is minimised). A chip that comes to the front is
    /// looked at once at `now`; the others keep their phase and are confirmed when they are seen. True when a look
    /// became due.
    pub fn set_focus(&mut self, front: Option<&str>, now: u64) -> bool {
        let new = Some(front.map(str::to_string));
        if self.focus == new {
            return false;
        }
        self.focus = new;
        let Some(front) = front else { return false };
        match self.chips.get_mut(front) {
            Some(c) if matches!(c.phase, Phase::Shown { .. }) => {
                c.verified = false;
                if c.in_flight {
                    c.stale_flight = true;
                    false
                } else {
                    c.next_at = Some(now);
                    true
                }
            }
            _ => false,
        }
    }

    /// Whether the tab's shown chip may be looked at now.
    fn watched(&self, tab: &str) -> bool {
        match &self.focus {
            None => true,
            Some(front) => front.as_deref() == Some(tab),
        }
    }

    /// The change counter last answered for the tab, if any.
    pub fn seen_seq(&self, tab: &str) -> Option<u64> {
        self.seqs.get(tab).copied()
    }

    /// The looks that are due now. Each is reported once until its answer comes.
    pub fn due(&mut self, now: u64) -> Vec<Check> {
        let mut out = Vec::new();
        let focus = self.focus.clone();
        for (tab, c) in &mut self.chips {
            if c.in_flight || matches!(c.phase, Phase::GaveUp) {
                continue;
            }
            let seen = match &focus {
                None => true,
                Some(front) => front.as_deref() == Some(tab.as_str()),
            };
            if matches!(c.phase, Phase::Shown { .. }) && !seen {
                continue;
            }
            if c.next_at.is_some_and(|t| t <= now) {
                c.in_flight = true;
                c.next_at = None;
                out.push(Check {
                    tab: tab.clone(),
                    port: c.server.port,
                    token: c.token,
                });
            }
        }
        out.sort_by(|a, b| a.tab.cmp(&b.tab));
        out
    }

    /// The answer of a look at the port.
    pub fn on_check(&mut self, tab: &str, token: u64, look: PortLook, now: u64) {
        let Some(c) = self.chips.get_mut(tab) else {
            return;
        };
        if c.token != token {
            return;
        }
        c.in_flight = false;
        if look == PortLook::Foreign {
            // Not our server: gone at once, whatever the phase. A new reading or reprint starts over.
            self.chips.remove(tab);
            return;
        }
        let stale = std::mem::take(&mut c.stale_flight);
        let listening = look == PortLook::InTree;
        match (&mut c.phase, listening) {
            (Phase::Shown { .. }, true) if stale => {
                // The answer predates the tab coming to the front: look once more, still unconfirmed.
                c.next_at = Some(now);
            }
            (Phase::Waiting { .. }, true) | (Phase::Shown { .. }, true) => {
                c.phase = Phase::Shown { misses: 0 };
                c.verified = true;
                c.next_at = Some(now + RECHECK_MS);
            }
            (Phase::Waiting { step }, false) => {
                *step += 1;
                match WAIT_STEPS_MS.get(*step) {
                    Some(at) => c.next_at = Some(c.began + at),
                    None => {
                        c.phase = Phase::GaveUp;
                        c.next_at = None;
                    }
                }
            }
            (Phase::Shown { misses }, false) => {
                *misses += 1;
                if *misses >= MISSES_TO_GO {
                    self.chips.remove(tab);
                } else {
                    c.next_at = Some(now + RECHECK_MS / 2);
                }
            }
            (Phase::GaveUp, _) => {}
        }
    }

    /// The answer of the look a CLICK made. It is not one of the periodic looks: the one that may be out keeps its
    /// own flight, and this answer never counts as a miss, a hit or the end of a stale flight (one server state would
    /// otherwise count twice). It only takes the chip when another process holds the port.
    pub fn on_click_check(&mut self, tab: &str, token: u64, look: PortLook) {
        if look != PortLook::Foreign {
            return;
        }
        if self.chips.get(tab).is_some_and(|c| c.token == token) {
            self.chips.remove(tab);
        }
    }

    /// The chip to show on the tab, if any.
    pub fn shown(&self, tab: &str) -> Option<&ChipServer> {
        self.chips
            .get(tab)
            .filter(|c| matches!(c.phase, Phase::Shown { .. }) && c.verified)
            .map(|c| &c.server)
    }

    /// The address the tab's chip is about, shown or still being looked for (a remote tab's tunnel stays while it is
    /// there, rule 1213).
    pub fn server(&self, tab: &str) -> Option<&ChipServer> {
        self.chips.get(tab).map(|c| &c.server)
    }

    /// The token of the tab's chip (what a look made for it hands back), if it holds one.
    pub fn token(&self, tab: &str) -> Option<u64> {
        self.chips.get(tab).map(|c| c.token)
    }

    /// The tab's shell is gone, or the tab closed.
    pub fn forget(&mut self, tab: &str) {
        self.chips.remove(tab);
        self.seqs.remove(tab);
        self.reprints.remove(tab);
    }

    /// When to look again, or None when nothing is pending (every chip is waiting for an answer, gave up, or there is none).
    pub fn next_due(&self) -> Option<u64> {
        self.chips
            .iter()
            .filter(|(t, c)| {
                !c.in_flight && (!matches!(c.phase, Phase::Shown { .. }) || self.watched(t))
            })
            .filter_map(|(_, c)| c.next_at)
            .min()
    }

    /// Tabs that hold a chip or are looking for a port (the e2e probe and the tests read it).
    pub fn tracked(&self) -> usize {
        self.chips.len()
    }

    /// The addresses being tracked, by tab, in tab order (the e2e probe reads it).
    pub fn tracked_urls(&self) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = self
            .chips
            .iter()
            .map(|(t, c)| (t.clone(), c.server.url.clone()))
            .collect();
        v.sort();
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(port: u16) -> ChipServer {
        ChipServer {
            url: format!("http://localhost:{port}/"),
            port,
        }
    }

    /// Drives one tab's chip the way the window does: due looks answered from a set of listening ports.
    struct Rig {
        chips: DevChips,
        now: u64,
        listening: Vec<u16>,
        foreign: Vec<u16>,
        looks: Vec<(u64, Check)>,
    }

    impl Rig {
        fn new() -> Self {
            Self {
                chips: DevChips::new(),
                now: 0,
                listening: Vec::new(),
                foreign: Vec::new(),
                looks: Vec::new(),
            }
        }

        fn run(&mut self, ms: u64) {
            let end = self.now + ms;
            loop {
                for c in self.chips.due(self.now) {
                    let look = if self.foreign.contains(&c.port) {
                        PortLook::Foreign
                    } else {
                        PortLook::from(self.listening.contains(&c.port))
                    };
                    self.looks.push((self.now, c.clone()));
                    self.chips.on_check(&c.tab, c.token, look, self.now);
                }
                match self.chips.next_due() {
                    Some(t) if t <= end => self.now = t.max(self.now),
                    _ => break,
                }
            }
            self.now = end;
        }
    }

    #[test]
    fn a_clicks_look_never_counts_as_one_of_the_periodic_looks() {
        let mut r = Rig::new();
        r.listening.push(3000);
        r.chips.on_reading("a", 1, Some(server(3000)), 0);
        r.run(0);
        assert!(r.chips.shown("a").is_some());
        // A periodic look is out when the person clicks.
        let due = r.chips.due(RECHECK_MS);
        assert_eq!(due.len(), 1);
        let token = due[0].token;
        r.chips.on_click_check("a", token, PortLook::Nobody);
        assert!(
            r.chips.due(RECHECK_MS).is_empty(),
            "the click's answer did not free the flight: no second look starts over the first"
        );
        r.chips.on_click_check("a", token, PortLook::InTree);
        assert!(r.chips.due(RECHECK_MS).is_empty());
        // Another process holding the port still takes the chip at once.
        r.chips.on_click_check("a", token, PortLook::Foreign);
        assert!(r.chips.shown("a").is_none());
        assert_eq!(r.chips.tracked(), 0);
    }

    #[test]
    fn a_listening_port_shows_the_chip_at_once() {
        let mut r = Rig::new();
        r.listening.push(3000);
        r.chips.on_reading("a", 1, Some(server(3000)), 0);
        assert!(r.chips.shown("a").is_none(), "not before the look");
        r.run(0);
        assert_eq!(r.chips.shown("a"), Some(&server(3000)));
    }

    #[test]
    fn a_server_that_prints_before_it_listens_gets_a_chip_when_it_does() {
        let mut r = Rig::new();
        r.chips.on_reading("a", 1, Some(server(3000)), 0);
        r.run(1_000);
        assert!(r.chips.shown("a").is_none());
        r.listening.push(3000);
        r.run(1_000);
        assert_eq!(r.chips.shown("a"), Some(&server(3000)));
        let times: Vec<u64> = r.looks.iter().map(|l| l.0).collect();
        assert_eq!(&times[..3], &[0, 400, 1_200], "the waiting steps");
    }

    #[test]
    fn a_port_that_never_listens_gives_up_and_is_not_looked_at_again() {
        let mut r = Rig::new();
        r.chips.on_reading("a", 1, Some(server(9)), 0);
        r.run(60_000);
        assert!(r.chips.shown("a").is_none());
        assert_eq!(r.looks.len(), WAIT_STEPS_MS.len());
        assert_eq!(r.chips.next_due(), None, "nothing left to wake for");
    }

    #[test]
    fn the_chip_goes_when_the_port_stops_listening() {
        let mut r = Rig::new();
        r.listening.push(3000);
        r.chips.on_reading("a", 1, Some(server(3000)), 0);
        r.run(10_000);
        assert!(r.chips.shown("a").is_some());
        r.listening.clear();
        r.run(RECHECK_MS);
        assert!(r.chips.shown("a").is_some(), "one miss keeps it");
        r.run(RECHECK_MS);
        assert!(r.chips.shown("a").is_none(), "two misses end it");
        assert_eq!(r.chips.tracked(), 0);
    }

    #[test]
    fn a_blink_of_the_server_does_not_take_the_chip() {
        let mut r = Rig::new();
        r.listening.push(3000);
        r.chips.on_reading("a", 1, Some(server(3000)), 0);
        r.run(5_000);
        r.listening.clear();
        r.run(RECHECK_MS);
        r.listening.push(3000);
        r.run(RECHECK_MS * 4);
        assert!(r.chips.shown("a").is_some());
    }

    #[test]
    fn a_new_address_replaces_the_chip_and_waits_for_its_own_port() {
        let mut r = Rig::new();
        r.listening.extend([3000, 4000]);
        r.chips.on_reading("a", 1, Some(server(3000)), 0);
        r.run(0);
        assert_eq!(r.chips.shown("a").map(|s| s.port), Some(3000));
        r.now = 100;
        r.chips.on_reading("a", 2, Some(server(4000)), 100);
        assert!(
            r.chips.shown("a").is_none(),
            "the old chip goes with the new reading"
        );
        r.run(0);
        assert_eq!(r.chips.shown("a").map(|s| s.port), Some(4000));
    }

    #[test]
    fn at_most_one_chip_per_tab_and_tabs_are_apart() {
        let mut r = Rig::new();
        r.listening.extend([3000, 4000]);
        r.chips.on_reading("a", 1, Some(server(3000)), 0);
        r.chips.on_reading("b", 1, Some(server(4000)), 0);
        r.run(0);
        assert_eq!(r.chips.shown("a").map(|s| s.port), Some(3000));
        assert_eq!(r.chips.shown("b").map(|s| s.port), Some(4000));
        r.chips.forget("a");
        assert!(r.chips.shown("a").is_none());
        assert!(r.chips.shown("b").is_some());
    }

    #[test]
    fn a_clear_takes_the_chip_at_once_without_waiting_for_the_port() {
        let mut r = Rig::new();
        r.listening.push(3000);
        r.chips.on_reading("a", 1, Some(server(3000)), 0);
        r.run(0);
        assert!(r.chips.shown("a").is_some());
        r.chips.on_reading("a", 2, None, 50);
        assert!(r.chips.shown("a").is_none());
        // The server still listens, but the screen was cleared: no chip returns by itself.
        r.run(30_000);
        assert!(r.chips.shown("a").is_none());
        assert_eq!(r.chips.tracked(), 0);
    }

    #[test]
    fn a_server_started_again_on_its_port_gets_its_chip_back() {
        let mut r = Rig::new();
        r.listening.push(3000);
        r.chips.on_reading("a", 1, Some(server(3000)), 0);
        r.chips.on_reprint("a", 0, Some(server(3000)), 0);
        r.run(10_000);
        assert!(r.chips.shown("a").is_some());
        // Ctrl+C: the chip goes after two misses.
        r.listening.clear();
        r.run(RECHECK_MS * 3);
        assert!(r.chips.shown("a").is_none());
        // Started again: the same address is printed (same seq, reprint moved).
        r.listening.push(3000);
        r.chips.on_reading("a", 1, Some(server(3000)), r.now);
        r.chips.on_reprint("a", 1, Some(server(3000)), r.now);
        r.run(0);
        assert!(r.chips.shown("a").is_some(), "back");
    }

    #[test]
    fn a_reprint_after_the_server_crashed_before_listening_looks_again() {
        let mut r = Rig::new();
        r.chips.on_reading("a", 1, Some(server(3000)), 0);
        r.chips.on_reprint("a", 0, Some(server(3000)), 0);
        r.run(60_000);
        assert_eq!(r.looks.len(), WAIT_STEPS_MS.len(), "gave up");
        r.listening.push(3000);
        r.chips.on_reprint("a", 1, Some(server(3000)), r.now);
        r.run(0);
        assert!(r.chips.shown("a").is_some());
    }

    #[test]
    fn a_reprint_while_a_chip_is_live_changes_nothing() {
        let mut r = Rig::new();
        r.listening.push(3000);
        r.chips.on_reading("a", 1, Some(server(3000)), 0);
        r.run(0);
        let looks = r.looks.len();
        r.chips.on_reprint("a", 5, Some(server(3000)), 10);
        r.chips.on_reprint("a", 6, Some(server(3000)), 20);
        r.run(0);
        assert_eq!(r.looks.len(), looks);
        assert!(r.chips.shown("a").is_some());
    }

    #[test]
    fn only_the_front_tab_is_rechecked_and_it_is_looked_at_once_when_it_comes_forward() {
        let mut r = Rig::new();
        r.listening.extend([3000, 4000]);
        r.chips.on_reading("a", 1, Some(server(3000)), 0);
        r.chips.on_reading("b", 1, Some(server(4000)), 0);
        r.run(0);
        r.chips.set_focus(Some("a"), 0);
        let before = r.looks.len();
        r.run(RECHECK_MS * 4);
        let b_looks = r.looks[before..].iter().filter(|l| l.1.tab == "b").count();
        let a_looks = r.looks[before..].iter().filter(|l| l.1.tab == "a").count();
        assert_eq!(b_looks, 0, "a background chip is not looked at");
        assert!(a_looks >= 3);
        // Minimised: nobody.
        r.chips.set_focus(None, r.now);
        let n = r.looks.len();
        r.run(RECHECK_MS * 4);
        assert_eq!(r.looks.len(), n);
        assert_eq!(r.chips.next_due(), None, "nothing to wake for");
        // b comes forward: one look at once.
        assert!(r.chips.set_focus(Some("b"), r.now));
        r.run(0);
        assert_eq!(r.looks.len(), n + 1);
        assert_eq!(r.looks.last().map(|l| l.1.tab.as_str()), Some("b"));
    }

    #[test]
    fn a_foreign_holder_takes_a_shown_chip_at_once_with_no_grace() {
        let mut r = Rig::new();
        r.listening.push(3000);
        r.chips.on_reading("a", 1, Some(server(3000)), 0);
        r.run(0);
        assert!(r.chips.shown("a").is_some());
        // The server exits and a stranger binds the port.
        r.listening.clear();
        r.foreign.push(3000);
        r.run(RECHECK_MS);
        assert!(r.chips.shown("a").is_none(), "one look is enough");
        assert_eq!(r.chips.tracked(), 0);
    }

    #[test]
    fn a_foreign_holder_while_waiting_ends_the_wait() {
        let mut r = Rig::new();
        r.foreign.push(3000);
        r.chips.on_reading("a", 1, Some(server(3000)), 0);
        r.run(0);
        assert_eq!(r.chips.tracked(), 0);
        assert_eq!(r.looks.len(), 1);
        assert_eq!(r.chips.next_due(), None);
    }

    #[test]
    fn a_chip_coming_to_the_front_is_hidden_until_its_first_look_answers() {
        let mut c = DevChips::new();
        c.on_reading("a", 1, Some(server(3000)), 0);
        let l = c.due(0).remove(0);
        c.on_check("a", l.token, PortLook::InTree, 0);
        assert!(c.shown("a").is_some());
        c.set_focus(Some("b"), 1);
        assert!(c.set_focus(Some("a"), 10), "a look became due");
        assert!(c.shown("a").is_none(), "hidden until the look answers");
        let l = c.due(10).remove(0);
        c.on_check("a", l.token, PortLook::InTree, 11);
        assert!(c.shown("a").is_some());
    }

    #[test]
    fn a_look_already_out_when_the_tab_comes_forward_does_not_confirm_it() {
        let mut c = DevChips::new();
        c.on_reading("a", 1, Some(server(3000)), 0);
        let l = c.due(0).remove(0);
        c.on_check("a", l.token, PortLook::InTree, 0);
        c.set_focus(Some("a"), 1);
        let old = c.due(2_500).remove(0);
        // The tab goes back and forth while that look is out.
        c.set_focus(Some("b"), 2_501);
        c.set_focus(Some("a"), 2_502);
        c.on_check("a", old.token, PortLook::InTree, 2_503);
        assert!(c.shown("a").is_none(), "the old look predates the focus");
        let fresh = c.due(2_503).remove(0);
        c.on_check("a", fresh.token, PortLook::InTree, 2_504);
        assert!(c.shown("a").is_some());
        assert_eq!(c.token("a"), Some(fresh.token));
    }

    #[test]
    fn the_same_reading_twice_is_one_chip_and_one_look() {
        let mut r = Rig::new();
        r.listening.push(3000);
        r.chips.on_reading("a", 1, Some(server(3000)), 0);
        r.chips.on_reading("a", 1, Some(server(3000)), 10);
        r.run(0);
        assert_eq!(r.looks.len(), 1);
    }

    #[test]
    fn a_late_answer_for_a_replaced_chip_is_dropped() {
        let mut c = DevChips::new();
        c.on_reading("a", 1, Some(server(3000)), 0);
        let old = c.due(0).remove(0);
        c.on_reading("a", 2, Some(server(4000)), 5);
        c.on_check("a", old.token, PortLook::InTree, 6);
        assert!(
            c.shown("a").is_none(),
            "the old port answered for the new address"
        );
        let new = c.due(6).remove(0);
        assert_eq!(new.port, 4000);
        c.on_check("a", new.token, PortLook::InTree, 7);
        assert!(c.shown("a").is_some());
    }

    #[test]
    fn a_look_in_flight_is_not_asked_again() {
        let mut c = DevChips::new();
        c.on_reading("a", 1, Some(server(3000)), 0);
        assert_eq!(c.due(0).len(), 1);
        assert_eq!(c.due(100).len(), 0);
        assert_eq!(c.next_due(), None);
    }

    #[test]
    fn the_counter_is_remembered_so_the_window_asks_the_terminal_only_for_news() {
        let mut c = DevChips::new();
        assert_eq!(c.seen_seq("a"), None);
        c.on_reading("a", 4, Some(server(3000)), 0);
        assert_eq!(c.seen_seq("a"), Some(4));
    }

    #[test]
    fn answers_for_a_forgotten_tab_do_nothing() {
        let mut c = DevChips::new();
        c.on_reading("a", 1, Some(server(3000)), 0);
        let look = c.due(0).remove(0);
        c.forget("a");
        c.on_check("a", look.token, PortLook::InTree, 1);
        assert_eq!(c.tracked(), 0);
        assert_eq!(c.next_due(), None);
    }
}

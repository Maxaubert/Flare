//! The 60 fps cap and the frame ring's slot ownership (rule 1311). Used by the sim host on each frame callback
//! (PR 2) and by Flare's frame bookkeeping (PR 4b); pure, on a clock the caller passes in.
//!
//! The cap never makes a lone frame wait: a frame goes at once if at least [`SPACING_US`] passed since the last one
//! sent. A frame that comes sooner is held and goes when the spacing has passed, and a newer frame replaces it
//! meanwhile (newest wins). So a scrolling app's 77 callbacks a second are thinned, and a single tap's frame is
//! never late.
//!
//! The ring is [`RING_SLOTS`] RGBA surfaces. The sim host writes a frame into a free slot and sends its record; Flare
//! shows it and says it is done only once the GPU work that sampled it has finished (`on_submitted_work_done`), so
//! a slot is never rewritten while Flare samples it. With no free slot the frame is dropped and counted.

/// The shortest time between two frames sent, in microseconds: 60 a second with room for the callback's jitter.
pub const SPACING_US: u64 = 15_000;
/// The ring's size. Flare frees a slot one frame late; the PR 2 bench counts "no free slot" drops and PR 4b raises
/// this to 4 if they show up (open check 12).
pub const RING_SLOTS: usize = 3;

/// The 60 fps cap. `T` is the frame (in the sim host, the framebuffer's sequence number and time).
#[derive(Debug)]
pub struct FrameCap<T> {
    last_sent_us: Option<u64>,
    held: Option<T>,
    /// Frames replaced while held (never sent).
    capped: u64,
}

impl<T> Default for FrameCap<T> {
    fn default() -> Self {
        Self {
            last_sent_us: None,
            held: None,
            capped: 0,
        }
    }
}

impl<T> FrameCap<T> {
    /// A frame came at `now_us`: it is returned when it goes at once (a held frame it replaces is counted);
    /// otherwise it is held (replacing a held one, which is counted) until [`FrameCap::due_us`].
    pub fn offer(&mut self, frame: T, now_us: u64) -> Option<T> {
        if self.spaced(now_us) {
            if self.held.take().is_some() {
                self.capped += 1;
            }
            self.last_sent_us = Some(now_us);
            return Some(frame);
        }
        if self.held.replace(frame).is_some() {
            self.capped += 1;
        }
        None
    }

    /// When the held frame is due (the caller's timer fires then), None with nothing held.
    pub fn due_us(&self) -> Option<u64> {
        self.held.as_ref()?;
        Some(self.last_sent_us.map_or(0, |l| l + SPACING_US))
    }

    /// The timer fired: the held frame, if it is due.
    pub fn poll(&mut self, now_us: u64) -> Option<T> {
        if self.held.is_some() && self.spaced(now_us) {
            self.last_sent_us = Some(now_us);
            return self.held.take();
        }
        None
    }

    /// Frames replaced while held, for the once-a-minute log line.
    pub fn capped(&self) -> u64 {
        self.capped
    }

    fn spaced(&self, now_us: u64) -> bool {
        self.last_sent_us
            .is_none_or(|l| now_us.saturating_sub(l) >= SPACING_US)
    }
}

/// Where a ring slot is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotState {
    /// The sim host may write into it.
    Free,
    /// Written and sent to Flare; Flare has not drawn it yet.
    Sent,
    /// Flare's GPU work samples it; it comes back only from that work's done callback.
    Shown,
}

/// The ownership of one ring's slots, as both sides agree on it.
#[derive(Debug)]
pub struct Ring {
    slots: [SlotState; RING_SLOTS],
    /// The slot to try first, so slots are used in turn.
    next: usize,
    /// Frames dropped for want of a free slot.
    dropped: u64,
}

impl Default for Ring {
    fn default() -> Self {
        Self {
            slots: [SlotState::Free; RING_SLOTS],
            next: 0,
            dropped: 0,
        }
    }
}

impl Ring {
    /// A free slot for the next frame, now Sent; None (the frame is dropped and counted) when Flare holds them all.
    pub fn take_free(&mut self) -> Option<u8> {
        for i in 0..RING_SLOTS {
            let s = (self.next + i) % RING_SLOTS;
            if self.slots[s] == SlotState::Free {
                self.slots[s] = SlotState::Sent;
                self.next = (s + 1) % RING_SLOTS;
                return Some(s as u8);
            }
        }
        self.dropped += 1;
        None
    }

    /// Flare draws this slot now. False for a slot that was not sent.
    pub fn shown(&mut self, slot: u8) -> bool {
        match self.slots.get_mut(usize::from(slot)) {
            Some(s @ SlotState::Sent) => {
                *s = SlotState::Shown;
                true
            }
            _ => false,
        }
    }

    /// The slot is free again: the GPU work that sampled it is done, or Flare skipped it for a newer frame without
    /// drawing it. False for a slot that was already free (a stray or repeated record changes nothing).
    pub fn done(&mut self, slot: u8) -> bool {
        match self.slots.get_mut(usize::from(slot)) {
            Some(s @ (SlotState::Sent | SlotState::Shown)) => {
                *s = SlotState::Free;
                true
            }
            _ => false,
        }
    }

    pub fn state(&self, slot: u8) -> Option<SlotState> {
        self.slots.get(usize::from(slot)).copied()
    }

    /// Every slot is free: an old ring may be let go of.
    pub fn all_done(&self) -> bool {
        self.slots.iter().all(|s| *s == SlotState::Free)
    }

    pub fn dropped(&self) -> u64 {
        self.dropped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs a cap over callbacks at `times_us`, firing its timer when due; the times frames were sent.
    fn run(times_us: &[u64]) -> Vec<(u64, u64)> {
        let mut cap = FrameCap::default();
        let mut sent = Vec::new();
        for (seq, &t) in times_us.iter().enumerate() {
            // The timer fires before this callback if the held frame came due.
            if let Some(due) = cap.due_us()
                && due <= t
                && let Some(f) = cap.poll(due)
            {
                sent.push((due, f));
            }
            if let Some(f) = cap.offer(seq as u64, t) {
                sent.push((t, f));
            }
        }
        if let Some(due) = cap.due_us()
            && let Some(f) = cap.poll(due)
        {
            sent.push((due, f));
        }
        sent
    }

    #[test]
    fn seventy_seven_callbacks_a_second_become_sixty() {
        // Settings while scrolling (research 2026-11-17): the 60 refreshes of a second, plus 17 extra callbacks a
        // couple of ms after one of them, spread through the second.
        let mut times: Vec<u64> = (0..60).map(|i| i * 1_000_000 / 60).collect();
        for k in 0..17 {
            times.push(k * 1_000_000 / 17 + 2_000);
        }
        times.sort_unstable();
        assert_eq!(times.len(), 77);
        let sent = run(&times);
        // Never two frames closer than the spacing, so never more than a 60 Hz display shows plus the jitter room.
        for w in sent.windows(2) {
            assert!(w[1].0 - w[0].0 >= SPACING_US, "{w:?}");
        }
        // Every refresh still shows, and the extras are folded in: 66 sends here. The 15 ms spacing allows at most
        // 66 a second (1000 / 15), so "sixty" is the display's rate plus the room left for the callback's jitter.
        assert!(
            (60..=(1_000_000 / SPACING_US) as usize).contains(&sent.len()),
            "77 callbacks thinned to about 60: {}",
            sent.len()
        );
        assert!(sent.len() < times.len());
        // The last frame of the burst is always shown.
        assert_eq!(sent.last().unwrap().1, 76);
        // A steady 60 Hz stream goes through untouched: every refresh is sent, none waits.
        let steady: Vec<u64> = (0..60).map(|i| i * 1_000_000 / 60).collect();
        let sent = run(&steady);
        assert_eq!(sent.len(), 60);
        assert!(sent.iter().zip(&steady).all(|(s, t)| s.0 == *t));
    }

    #[test]
    fn the_newest_held_frame_wins() {
        let mut cap = FrameCap::default();
        assert_eq!(cap.offer("a", 0), Some("a"));
        assert_eq!(cap.offer("b", 3_000), None);
        assert_eq!(cap.offer("c", 6_000), None);
        assert_eq!(cap.offer("d", 9_000), None);
        assert_eq!(cap.capped(), 2, "b and c were never sent");
        assert_eq!(cap.due_us(), Some(SPACING_US));
        assert_eq!(cap.poll(14_999), None, "not before the spacing");
        assert_eq!(cap.poll(15_000), Some("d"));
        assert_eq!(cap.due_us(), None);
        assert_eq!(cap.poll(40_000), None, "nothing held");
    }

    #[test]
    fn a_lone_frame_is_sent_at_once() {
        let mut cap = FrameCap::default();
        assert_eq!(cap.offer(1, 5_000_000), Some(1), "the first frame");
        assert_eq!(
            cap.offer(2, 5_500_000),
            Some(2),
            "a tap half a second later"
        );
        assert_eq!(cap.offer(3, 5_500_000 + SPACING_US), Some(3), "just spaced");
        assert_eq!(cap.due_us(), None, "nothing waits");
        // A held frame whose timer has not fired yet is replaced by a newer one that may go at once.
        assert_eq!(cap.offer(4, 5_520_000), None);
        assert_eq!(cap.offer(5, 5_600_000), Some(5), "newest wins, no wait");
        assert_eq!(cap.capped(), 1);
        assert_eq!(cap.poll(5_600_000), None);
    }

    #[test]
    fn a_slot_comes_back_only_after_its_work_is_done() {
        let mut ring = Ring::default();
        let s = ring.take_free().unwrap();
        assert_eq!(ring.state(s), Some(SlotState::Sent));
        assert!(ring.shown(s));
        assert_eq!(ring.state(s), Some(SlotState::Shown));
        // Two more frames take the other slots; the shown one is not handed out.
        let a = ring.take_free().unwrap();
        let b = ring.take_free().unwrap();
        assert!(a != s && b != s && a != b);
        assert_eq!(ring.take_free(), None);
        // Flare drew `a` instead and skipped `b`: `b` is free at once, `s` only once its work is done.
        assert!(ring.shown(a));
        assert!(ring.done(b));
        assert_eq!(ring.take_free(), Some(b));
        assert!(!ring.all_done());
        assert!(ring.done(s), "the work-done callback");
        assert!(!ring.done(s), "a repeated record changes nothing");
        assert_eq!(ring.take_free(), Some(s));
        assert!(ring.done(a) && ring.done(b) && ring.done(s));
        assert!(ring.all_done(), "an old ring may go now");
        assert!(!ring.shown(9) && !ring.done(9), "no such slot");
        assert!(!ring.shown(s), "not sent");
    }

    #[test]
    fn a_slot_flare_holds_is_never_handed_out() {
        let mut ring = Ring::default();
        let held = ring.take_free().unwrap();
        ring.shown(held);
        // However many frames come, the held slot is never written while Flare samples it.
        for _ in 0..100 {
            if let Some(s) = ring.take_free() {
                assert_ne!(s, held);
                ring.done(s);
            }
        }
        assert_eq!(ring.state(held), Some(SlotState::Shown));
    }

    #[test]
    fn with_no_free_slot_the_frame_is_dropped_and_counted() {
        let mut ring = Ring::default();
        for _ in 0..RING_SLOTS {
            assert!(ring.take_free().is_some());
        }
        assert_eq!(ring.dropped(), 0);
        assert_eq!(ring.take_free(), None);
        assert_eq!(ring.take_free(), None);
        assert_eq!(ring.dropped(), 2);
        ring.done(1);
        assert_eq!(ring.take_free(), Some(1));
        assert_eq!(ring.dropped(), 2);
    }
}

//! Where a scroll event is in its trackpad gesture, so the terminal can scroll like Terminal.app:
//! with the fingers, never with the inertia after they lift (rule 1129).
//!
//! macOS sends a trackpad scroll in phases: `NSEvent.phase` is Began, Changed and Ended while the
//! fingers are down; after the lift AppKit sends synthetic momentum events, whose `momentumPhase`
//! is Began, Changed or Ended and whose `phase` is None. A plain mouse wheel has both None. winit
//! folds the two into one `TouchPhase` (a momentum Changed reads as Moved, like a finger move), so
//! the window reads them from the event AppKit is dispatching ([`current`]).

/// One scroll event's place in its gesture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollPhase {
    /// No gesture: a plain mouse wheel (both phases None), or any event off macOS.
    Wheel,
    /// The fingers touched down (`phase` Began or MayBegin).
    Began,
    /// The fingers move (`phase` Changed or Stationary).
    Changed,
    /// The fingers lifted (`phase` Ended or Cancelled).
    Ended,
    /// After the lift: AppKit's inertia (`momentumPhase` is not None).
    Momentum,
}

/// `NSEventPhase` bits (AppKit's values, stable since 10.7).
const BEGAN: usize = 0x1;
const ENDED: usize = 0x8;
const CANCELLED: usize = 0x10;
const MAY_BEGIN: usize = 0x20;

/// Pure: the phase of an event from its raw `phase` and `momentumPhase` bits. Momentum wins: an
/// event with any momentum phase is inertia, whatever its `phase` says.
pub fn from_bits(phase: usize, momentum: usize) -> ScrollPhase {
    if momentum != 0 {
        return ScrollPhase::Momentum;
    }
    if phase == 0 {
        ScrollPhase::Wheel
    } else if phase & (BEGAN | MAY_BEGIN) != 0 {
        ScrollPhase::Began
    } else if phase & (ENDED | CANCELLED) != 0 {
        ScrollPhase::Ended
    } else {
        ScrollPhase::Changed
    }
}

/// The phase of the scroll event AppKit is dispatching now (`NSApp.currentEvent`), read inside the
/// window's wheel handler, which winit calls from the view's `scrollWheel:`. None off the main
/// thread or when the current event is not a scroll (winit queued the wheel behind another event).
#[cfg(target_os = "macos")]
pub fn current() -> Option<ScrollPhase> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSEventType};
    let mtm = MainThreadMarker::new()?;
    let ev = NSApplication::sharedApplication(mtm).currentEvent()?;
    if ev.r#type() != NSEventType::ScrollWheel {
        return None;
    }
    Some(from_bits(ev.phase().0, ev.momentumPhase().0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn momentum_wins_and_each_finger_phase_is_read() {
        // (phase, momentumPhase, read as)
        let table = [
            (0, 0, ScrollPhase::Wheel),
            (BEGAN, 0, ScrollPhase::Began),
            (MAY_BEGIN, 0, ScrollPhase::Began),
            (0x4, 0, ScrollPhase::Changed),
            (0x2, 0, ScrollPhase::Changed),
            (ENDED, 0, ScrollPhase::Ended),
            (CANCELLED, 0, ScrollPhase::Ended),
            (0, BEGAN, ScrollPhase::Momentum),
            (0, 0x4, ScrollPhase::Momentum),
            (0, ENDED, ScrollPhase::Momentum),
            (0x4, 0x4, ScrollPhase::Momentum),
        ];
        for (phase, momentum, want) in table {
            assert_eq!(from_bits(phase, momentum), want, "{phase:#x} {momentum:#x}");
        }
    }
}

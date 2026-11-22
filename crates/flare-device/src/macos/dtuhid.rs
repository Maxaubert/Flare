//! DTUHID input (rule 1313): touches, keys and hardware buttons as the dictionaries the simulator's `dtuhidd`
//! digitizer service takes, on CoreSimulator 1155.4 and newer. One connection per device, held for the session.
//!
//! - Envelope: `{messageType, isBarrier, featureIdentifier, payload}`.
//! - `IndigoDigitizerEvent`: `pointOne` (and `pointTwo` for a second finger) as 0..1 ratios of the portrait screen,
//!   `eventType` 0 start, 1 position, 2 end, `edge` 0, `target` 0.
//! - `IndigoKeyboardButtonEvent`: a USB HID keyboard `usageCode`, `state` 1 down, 2 up (0 is refused by `dtuhidd`).
//! - `IndigoButtonEvent`: a Consumer page (0x0C) `usagePage` and `usageCode`, `state` as keys.
//!
//! Every integer is a `uint64`. Each event is sent, then its send barrier is waited for; never a reply per event.
//!
//! Ported from idb (`FBSimulatorControl/HID/DTUHIDModels.swift`, `SimulatorDigitizerHIDTransport.swift` and
//! `SimulatorHIDButtonIdentity.swift`, facebook/idb, MIT):
//!
//! Copyright (c) Meta Platforms, Inc. and affiliates.
//!
//! Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated
//! documentation files (the "Software"), to deal in the Software without restriction, including without limitation
//! the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and
//! to permit persons to whom the Software is furnished to do so, subject to the following conditions:
//!
//! The above copyright notice and this permission notice shall be included in all copies or substantial portions of
//! the Software.
//!
//! THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO
//! THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
//! AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF
//! CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS
//! IN THE SOFTWARE.

use super::xpc::{Conn, XVal};
use crate::geom::{Orientation, Size, to_portrait_ratio};
use crate::source::{Button, Phase, Touch};

/// The digitizer service: touches, keys and buttons all go to it.
pub const DIGITIZER: &str = "com.apple.coredevice.feature.remote.hid.digitizer";

/// The HID Consumer page.
pub const CONSUMER_PAGE: u16 = 0x0C;

/// A contact's phase on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventType {
    Start = 0,
    Position = 1,
    End = 2,
}

/// The envelope every message shares.
pub fn envelope(kind: &str, payload: XVal, barrier: bool) -> XVal {
    XVal::dict([
        ("messageType", XVal::Str(kind.into())),
        ("isBarrier", XVal::Bool(barrier)),
        ("featureIdentifier", XVal::Str(DIGITIZER.into())),
        ("payload", payload),
    ])
}

fn point(r: (f64, f64)) -> XVal {
    XVal::dict([("x", XVal::F64(r.0)), ("y", XVal::F64(r.1))])
}

/// A digitizer event: one contact, or two (a pinch), in 0..1 portrait ratios.
pub fn digitizer(event: EventType, one: (f64, f64), two: Option<(f64, f64)>) -> XVal {
    let mut payload = vec![("pointOne".to_string(), point(one))];
    if let Some(t) = two {
        payload.push(("pointTwo".to_string(), point(t)));
    }
    payload.extend([
        ("eventType".to_string(), XVal::U64(event as u64)),
        ("edge".to_string(), XVal::U64(0)),
        ("target".to_string(), XVal::U64(0)),
    ]);
    envelope("IndigoDigitizerEvent", XVal::Dict(payload), false)
}

fn state(down: bool) -> XVal {
    XVal::U64(if down { 1 } else { 2 })
}

/// A keyboard key going down or up.
pub fn key(usage: u32, down: bool) -> XVal {
    envelope(
        "IndigoKeyboardButtonEvent",
        XVal::dict([
            ("usageCode", XVal::U64(u64::from(usage))),
            ("state", state(down)),
        ]),
        false,
    )
}

/// A hardware button going down or up.
pub fn button(page: u16, code: u16, down: bool) -> XVal {
    envelope(
        "IndigoButtonEvent",
        XVal::dict([
            ("usagePage", XVal::U64(u64::from(page))),
            ("usageCode", XVal::U64(u64::from(code))),
            ("state", state(down)),
        ]),
        false,
    )
}

/// The liveness barrier: a key-up of usage 0 marked as a barrier, whose reply says `dtuhidd` is there.
pub fn liveness() -> XVal {
    envelope(
        "IndigoKeyboardButtonEvent",
        XVal::dict([("usageCode", XVal::U64(0)), ("state", state(false))]),
        true,
    )
}

/// The Consumer page usage of a hardware button (idb's identities), or None for what is not a HID button (shake
/// and rotation have their own ways).
pub fn consumer_usage(b: Button) -> Option<(u16, u16)> {
    Some(match b {
        // Menu.
        Button::Home => (CONSUMER_PAGE, 0x40),
        // Power: the lock / side button.
        Button::Lock => (CONSUMER_PAGE, 0x30),
        Button::VolumeUp => (CONSUMER_PAGE, 0xE9),
        Button::VolumeDown => (CONSUMER_PAGE, 0xEA),
        // Shake and rotation have their own ways; Back, Recents and Power are Android's (rule 1371).
        Button::Shake
        | Button::RotateLeft
        | Button::RotateRight
        | Button::Back
        | Button::Recents
        | Button::Power => return None,
    })
}

/// Which contacts are down, so a move without a start becomes a start and an end without one is dropped. A
/// contact's start remembers the connection's interruption count: a gesture the connection lost on the way is
/// dropped, not continued (idb's `DigitizerContactTracker`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Contacts {
    one: Option<u32>,
    two: Option<u32>,
}

impl Contacts {
    /// The wire phase of `phase` for one finger (`two` false) or a pinch (`two` true), or None to send nothing.
    pub fn event(&mut self, two: bool, phase: Phase, interruptions: u32) -> Option<EventType> {
        let slot = if two { &mut self.two } else { &mut self.one };
        match (phase, *slot) {
            (Phase::Start | Phase::Move, None) => {
                *slot = Some(interruptions);
                Some(EventType::Start)
            }
            (Phase::Start | Phase::Move, Some(at)) if at == interruptions => {
                Some(EventType::Position)
            }
            (Phase::Start | Phase::Move, Some(_)) => {
                // The connection dropped mid-gesture: the rest of it is not sent.
                *slot = None;
                None
            }
            (Phase::End, Some(_)) => {
                *slot = None;
                Some(EventType::End)
            }
            (Phase::End, None) => None,
        }
    }

    /// Which contacts are down: one finger, a pinch.
    pub fn down(&self) -> (bool, bool) {
        (self.one.is_some(), self.two.is_some())
    }

    /// Forgets every contact (after an end was sent for each, or the connection was made again).
    pub fn clear(&mut self) {
        *self = Contacts::default();
    }
}

/// A touch in device points (current orientation) as its wire message, the contact tracked. `portrait` is the
/// screen's portrait size in points.
pub fn touch_message(
    contacts: &mut Contacts,
    t: &Touch,
    portrait: Size,
    o: Orientation,
    interruptions: u32,
) -> Option<XVal> {
    let ev = contacts.event(t.second.is_some(), t.phase, interruptions)?;
    let one = to_portrait_ratio(t.x, t.y, portrait, o);
    let two = t.second.map(|(x, y)| to_portrait_ratio(x, y, portrait, o));
    Some(digitizer(ev, one, two))
}

/// The end of every contact still down, at the portrait centre (where it ends does not matter, only that it does).
pub fn release_all(contacts: &mut Contacts) -> Vec<XVal> {
    let (one, two) = contacts.down();
    contacts.clear();
    let mut out = Vec::new();
    if one {
        out.push(digitizer(EventType::End, (0.5, 0.5), None));
    }
    if two {
        out.push(digitizer(EventType::End, (0.5, 0.5), Some((0.5, 0.5))));
    }
    out
}

/// One device's DTUHID connection.
pub struct Dtuhid {
    pub conn: Conn,
}

impl Dtuhid {
    /// Sends one message and waits for its send barrier. False when it did not leave (the connection is gone).
    pub fn send(&self, m: &XVal) -> bool {
        self.conn.send(m)
    }

    pub fn interruptions(&self) -> u32 {
        self.conn.interruptions()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::Touch;

    const PHONE: Size = Size { w: 402.0, h: 874.0 };

    fn payload(m: &XVal) -> &XVal {
        m.get("payload").unwrap()
    }

    fn ratio(m: &XVal, which: &str) -> (f64, f64) {
        let p = payload(m).get(which).unwrap();
        match (p.get("x"), p.get("y")) {
            (Some(XVal::F64(x)), Some(XVal::F64(y))) => (*x, *y),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_tap_is_start_then_end_with_portrait_ratios() {
        let mut c = Contacts::default();
        let down = touch_message(
            &mut c,
            &Touch::one(Phase::Start, 201.0, 437.0),
            PHONE,
            Orientation::Rot0,
            0,
        )
        .unwrap();
        assert_eq!(
            down.get("messageType"),
            Some(&XVal::Str("IndigoDigitizerEvent".into()))
        );
        assert_eq!(
            down.get("featureIdentifier"),
            Some(&XVal::Str(DIGITIZER.into()))
        );
        assert_eq!(down.get("isBarrier"), Some(&XVal::Bool(false)));
        assert_eq!(
            payload(&down).get("eventType"),
            Some(&XVal::U64(0)),
            "start"
        );
        assert_eq!(ratio(&down, "pointOne"), (0.5, 0.5));
        assert!(payload(&down).get("pointTwo").is_none(), "one finger");
        let up = touch_message(
            &mut c,
            &Touch::one(Phase::End, 201.0, 437.0),
            PHONE,
            Orientation::Rot0,
            0,
        )
        .unwrap();
        assert_eq!(payload(&up).get("eventType"), Some(&XVal::U64(2)), "end");
        assert_eq!(c.down(), (false, false));
        // A corner, and a point in landscape: the ratios are of the portrait screen (the digitizer does not turn).
        let m = touch_message(
            &mut c,
            &Touch::one(Phase::Start, 0.0, 874.0),
            PHONE,
            Orientation::Rot0,
            0,
        )
        .unwrap();
        assert_eq!(ratio(&m, "pointOne"), (0.0, 1.0));
        let m = touch_message(
            &mut c,
            &Touch::one(Phase::Move, 874.0, 0.0),
            PHONE,
            Orientation::Rot90,
            0,
        )
        .unwrap();
        assert_eq!(
            payload(&m).get("eventType"),
            Some(&XVal::U64(1)),
            "a move of a finger down"
        );
        assert_eq!(
            ratio(&m, "pointOne"),
            to_portrait_ratio(874.0, 0.0, PHONE, Orientation::Rot90)
        );
        // A move with no finger down starts one; an end with none is not sent.
        let mut c = Contacts::default();
        let m = touch_message(
            &mut c,
            &Touch::one(Phase::Move, 10.0, 10.0),
            PHONE,
            Orientation::Rot0,
            0,
        )
        .unwrap();
        assert_eq!(payload(&m).get("eventType"), Some(&XVal::U64(0)));
        assert!(
            touch_message(
                &mut c,
                &Touch::one(Phase::End, 10.0, 10.0),
                PHONE,
                Orientation::Rot0,
                0
            )
            .is_some()
        );
        assert!(
            touch_message(
                &mut c,
                &Touch::one(Phase::End, 10.0, 10.0),
                PHONE,
                Orientation::Rot0,
                0
            )
            .is_none()
        );
        // A gesture the connection lost on the way is not continued.
        assert!(
            touch_message(
                &mut c,
                &Touch::one(Phase::Start, 1.0, 1.0),
                PHONE,
                Orientation::Rot0,
                0
            )
            .is_some()
        );
        assert!(
            touch_message(
                &mut c,
                &Touch::one(Phase::Move, 2.0, 2.0),
                PHONE,
                Orientation::Rot0,
                1
            )
            .is_none()
        );
        assert_eq!(c.down(), (false, false));
        // A pinch carries both fingers and has its own contact.
        let pinch = Touch {
            phase: Phase::Start,
            x: 171.0,
            y: 407.0,
            second: Some((231.0, 467.0)),
        };
        let m = touch_message(&mut c, &pinch, PHONE, Orientation::Rot0, 0).unwrap();
        assert_eq!(ratio(&m, "pointTwo"), (231.0 / 402.0, 467.0 / 874.0));
        assert_eq!(c.down(), (false, true));
        // Releasing ends what is down, once.
        assert_eq!(release_all(&mut c).len(), 1);
        assert!(release_all(&mut c).is_empty());
    }

    #[test]
    fn integers_are_sent_as_uint64_and_state_zero_is_never_sent() {
        let mut c = Contacts::default();
        let mut msgs = vec![
            touch_message(
                &mut c,
                &Touch::one(Phase::Start, 5.0, 5.0),
                PHONE,
                Orientation::Rot0,
                0,
            )
            .unwrap(),
            key(0x04, true),
            key(0x04, false),
            button(CONSUMER_PAGE, 0x40, true),
            button(CONSUMER_PAGE, 0x40, false),
            liveness(),
        ];
        msgs.extend(release_all(&mut c));
        for m in &msgs {
            let mut all = Vec::new();
            m.walk("", &mut all);
            for (k, v) in all {
                if matches!(
                    k,
                    "eventType" | "edge" | "target" | "usageCode" | "usagePage" | "state"
                ) {
                    assert!(matches!(v, XVal::U64(_)), "{k} is {v:?}");
                }
                assert!(!matches!(v, XVal::I64(_)), "no signed integers: {k}");
                if k == "state" {
                    assert_ne!(v, &XVal::U64(0), "state 0 is refused by dtuhidd");
                }
            }
        }
        assert_eq!(
            payload(&key(0x28, true)).get("state"),
            Some(&XVal::U64(1)),
            "down is 1"
        );
        assert_eq!(
            payload(&key(0x28, false)).get("state"),
            Some(&XVal::U64(2)),
            "up is 2"
        );
        assert_eq!(liveness().get("isBarrier"), Some(&XVal::Bool(true)));
    }

    #[test]
    fn buttons_use_consumer_page_usages() {
        assert_eq!(consumer_usage(Button::Home), Some((0x0C, 0x40)));
        assert_eq!(consumer_usage(Button::Lock), Some((0x0C, 0x30)));
        assert_eq!(consumer_usage(Button::VolumeUp), Some((0x0C, 0xE9)));
        assert_eq!(consumer_usage(Button::VolumeDown), Some((0x0C, 0xEA)));
        for b in [
            Button::Shake,
            Button::RotateLeft,
            Button::RotateRight,
            Button::Back,
            Button::Recents,
            Button::Power,
        ] {
            assert_eq!(consumer_usage(b), None, "{b:?} is not a HID button");
        }
        let m = button(CONSUMER_PAGE, 0xE9, true);
        assert_eq!(
            m.get("messageType"),
            Some(&XVal::Str("IndigoButtonEvent".into()))
        );
        assert_eq!(payload(&m).get("usagePage"), Some(&XVal::U64(0x0C)));
        assert_eq!(payload(&m).get("usageCode"), Some(&XVal::U64(0xE9)));
    }
}

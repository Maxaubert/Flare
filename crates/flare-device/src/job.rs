//! One agent input call on the device panel as a small state machine (shaped like
//! `flare_surfaces::agent_job`): tap, swipe, type, press. It never touches the device. It says where the finger dot
//! goes ([`Job::take_point`]), holds its input for the dot's glide ([`Job::glide_for`]), says which input to send
//! ([`Out::Send`]), then waits for the sim host's next "changed" count ([`Job::on_changed`]) or [`SETTLE_MS`],
//! whichever comes first, and answers ([`Out::Done`]). The changed count goes on while the stream is paused, so a
//! job finishes with the window minimised too. The window owns the source, the clock and the timers (rule 34).
//!
//! The person's press, key or scroll ends a running job with [`Fail::Interrupted`] (rule 1320); a cancel (rule 229)
//! ends it at once. Either way [`Job::release`] gives the input that lets go of what the job held down, so no
//! finger or key is left down in the simulator.

use std::collections::VecDeque;

use flare_surfaces::agent_cursor::GLIDE_MAX_MS;
use serde_json::{Value, json};

use crate::geom::{Size, swipe_path};
use crate::keymap::{self, Typing, usage};
use crate::source::{Button, Key, Phase, Touch};
use crate::tree::RefMap;

/// How long a job waits for the screen to change after its last input.
pub const SETTLE_MS: u64 = 300;
/// A tap's finger stays down this long.
pub const TAP_MS: u64 = 50;
/// A swipe takes this long unless the agent says.
pub const SWIPE_MS: u64 = 300;
/// The time between two typed characters.
pub const KEY_MS: u64 = 20;
/// How far a swipe by direction goes: this share of the screen.
const DIRECTION_SHARE: f64 = 0.5;
/// The badge while text is typed.
pub const TYPING: &str = "Typing...";

/// Why a call failed, in the channel's own words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fail {
    /// The person took over (or the call was cancelled): look again.
    Interrupted,
    /// No Xcode, no device, still mode, a remote session.
    Unavailable,
    /// A stale ref.
    NotFound,
    /// The device is booting.
    Busy,
    /// The touch connection dropped: try again.
    Retry,
    /// Not allowed: shutting down a device Flare did not boot, installing from outside the allowed folders.
    Forbidden,
}

impl Fail {
    /// The error code on the control channel.
    pub fn code(self) -> &'static str {
        match self {
            Fail::Interrupted => "interrupted",
            Fail::Unavailable => "unavailable",
            Fail::NotFound => "not_found",
            Fail::Busy => "busy",
            Fail::Retry => "retry",
            Fail::Forbidden => "forbidden",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobError {
    pub fail: Fail,
    pub message: String,
}

/// Where an action lands: a ref of the last look, or a device point.
#[derive(Clone, Debug, PartialEq)]
pub enum At {
    Ref(String),
    Point(f64, f64),
}

/// Which way a finger moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

/// Where a swipe ends.
#[derive(Clone, Debug, PartialEq)]
pub enum SwipeTo {
    Point(f64, f64),
    /// Half the screen that way.
    Direction(Direction),
}

/// What a press presses.
#[derive(Clone, Debug, PartialEq)]
pub enum Press {
    Button(Button),
    /// A key by its `device_press` name (return, delete, the arrows ...).
    Key(String),
}

/// An agent input call.
#[derive(Clone, Debug, PartialEq)]
pub enum DeviceOp {
    Tap {
        at: At,
        hold_ms: Option<u64>,
    },
    /// From a point or ref (the screen's centre with none) to a point or a direction.
    Swipe {
        from: Option<At>,
        to: SwipeTo,
        duration_ms: Option<u64>,
    },
    /// Taps `at` first if given, types, presses Return if `submit`.
    Type {
        text: String,
        at: Option<At>,
        submit: bool,
    },
    Press(Press),
}

impl DeviceOp {
    pub fn name(&self) -> &'static str {
        match self {
            DeviceOp::Tap { .. } => "tap",
            DeviceOp::Swipe { .. } => "swipe",
            DeviceOp::Type { .. } => "type",
            DeviceOp::Press(_) => "press",
        }
    }
}

/// One input to send to the source.
#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    Touch(Touch),
    Key(Key),
    Button(Button),
    /// Text the keymap cannot type: through the simulator's pasteboard and Cmd-V.
    Paste(String),
}

/// What a job asks of the window next.
#[derive(Debug, PartialEq)]
pub enum Out {
    /// Nothing until the next tick or changed count.
    Idle,
    Send(Vec<Input>),
    Done(Result<Value, JobError>),
}

#[derive(Debug)]
enum Stage {
    /// Waiting for the dot's glide; `until_ms` None until the window says how long it takes.
    Glide { until_ms: Option<u64> },
    /// Sending the queue, each input at its time from `start_ms`.
    Send { start_ms: u64 },
    /// Waiting for the next changed count above `base`, at most until `until_ms`.
    Settle { base: u64, until_ms: u64 },
}

pub struct Job {
    op: &'static str,
    stage: Stage,
    /// Each input with its time from the start of sending.
    queue: VecDeque<(u64, Input)>,
    point: Option<(f64, f64)>,
    badge: Option<&'static str>,
    /// What is held down now (sent and not yet let go of), for [`Job::release`].
    finger: Option<(f64, f64)>,
    keys: Vec<u32>,
}

fn err(fail: Fail, message: impl Into<String>) -> JobError {
    JobError {
        fail,
        message: message.into(),
    }
}

fn touch(phase: Phase, (x, y): (f64, f64)) -> Input {
    Input::Touch(Touch::one(phase, x, y))
}

fn key(usage: u32, down: bool) -> Input {
    Input::Key(Key { usage, down })
}

/// The inputs that type `text` from time `t`, and the time after them.
fn type_inputs(text: &str, mut t: u64, q: &mut VecDeque<(u64, Input)>) -> u64 {
    match keymap::typing(text) {
        Typing::Keys(keys) => {
            for k in keys {
                for e in keymap::events(k) {
                    q.push_back((t, Input::Key(e)));
                }
                t += KEY_MS;
            }
        }
        Typing::Paste(s) => {
            q.push_back((t, Input::Paste(s)));
            t += KEY_MS;
        }
    }
    t
}

impl Job {
    pub fn op(&self) -> &'static str {
        self.op
    }

    /// What the keyboard badge says while this job runs ("Typing...", a key's name), if anything.
    pub fn badge(&self) -> Option<&'static str> {
        self.badge
    }

    /// The person took over: the call ends.
    pub fn interrupted() -> JobError {
        err(
            Fail::Interrupted,
            "the user touched the device and took over; look again before doing anything more",
        )
    }

    /// The caller went away (rule 229): the call ends at once, as a takeover would.
    pub fn cancelled() -> JobError {
        err(Fail::Interrupted, "the call was cancelled")
    }

    /// Starts a job. `refs` are the last look's; a ref not among them fails here with `not_found`. `screen` is the
    /// device's screen in points (current orientation).
    pub fn start(op: DeviceOp, refs: &RefMap, screen: Size) -> Result<Job, JobError> {
        let name = op.name();
        let resolve = |at: &At| -> Result<(f64, f64), JobError> {
            let p = match at {
                At::Point(x, y) => (*x, *y),
                At::Ref(r) => refs.get(r).map(|f| f.center()).ok_or_else(|| {
                    err(
                        Fail::NotFound,
                        format!(
                            "there is no element {r}; refs last until the next look, so look again"
                        ),
                    )
                })?,
            };
            let inside = (0.0..=screen.w).contains(&p.0) && (0.0..=screen.h).contains(&p.1);
            if inside {
                Ok(p)
            } else {
                Err(err(
                    Fail::NotFound,
                    format!(
                        "({}, {}) is off the screen, which is {} x {} points",
                        p.0, p.1, screen.w, screen.h
                    ),
                ))
            }
        };
        let mut q = VecDeque::new();
        let mut point = None;
        let mut badge = None;
        match op {
            DeviceOp::Tap { at, hold_ms } => {
                let p = resolve(&at)?;
                point = Some(p);
                q.push_back((0, touch(Phase::Start, p)));
                q.push_back((hold_ms.unwrap_or(TAP_MS).max(1), touch(Phase::End, p)));
            }
            DeviceOp::Swipe {
                from,
                to,
                duration_ms,
            } => {
                let centre = (screen.w / 2.0, screen.h / 2.0);
                let from = match &from {
                    Some(at) => resolve(at)?,
                    None => centre,
                };
                let to = match to {
                    SwipeTo::Point(x, y) => resolve(&At::Point(x, y))?,
                    SwipeTo::Direction(d) => {
                        let (dx, dy) = (screen.w * DIRECTION_SHARE, screen.h * DIRECTION_SHARE);
                        let (x, y) = match d {
                            Direction::Up => (from.0, from.1 - dy),
                            Direction::Down => (from.0, from.1 + dy),
                            Direction::Left => (from.0 - dx, from.1),
                            Direction::Right => (from.0 + dx, from.1),
                        };
                        (x.clamp(1.0, screen.w - 1.0), y.clamp(1.0, screen.h - 1.0))
                    }
                };
                point = Some(from);
                let path = swipe_path(from, to, duration_ms.unwrap_or(SWIPE_MS));
                let last = path.len() - 1;
                for (i, (x, y, t)) in path.into_iter().enumerate() {
                    let phase = match i {
                        0 => Phase::Start,
                        i if i == last => Phase::End,
                        _ => Phase::Move,
                    };
                    q.push_back((t, touch(phase, (x, y))));
                }
            }
            DeviceOp::Type { text, at, submit } => {
                badge = Some(TYPING);
                let mut t = 0;
                if let Some(at) = &at {
                    let p = resolve(at)?;
                    point = Some(p);
                    q.push_back((0, touch(Phase::Start, p)));
                    q.push_back((TAP_MS, touch(Phase::End, p)));
                    t = TAP_MS * 2;
                }
                t = type_inputs(&text, t, &mut q);
                if submit {
                    q.push_back((t, key(usage::RETURN, true)));
                    q.push_back((t, key(usage::RETURN, false)));
                }
            }
            DeviceOp::Press(Press::Button(b)) => q.push_back((0, Input::Button(b))),
            DeviceOp::Press(Press::Key(name)) => {
                let (Some(u), Some(label)) = (keymap::named(&name), keymap::label(&name)) else {
                    return Err(err(
                        Fail::NotFound,
                        format!(
                            "there is no key {name:?}; keys are return, delete, escape, tab, space and the arrows"
                        ),
                    ));
                };
                badge = Some(label);
                q.push_back((0, key(u, true)));
                q.push_back((0, key(u, false)));
            }
        }
        // With no point there is no dot to wait for: the input goes at the first tick.
        let until_ms = if point.is_some() { None } else { Some(0) };
        Ok(Job {
            op: name,
            stage: Stage::Glide { until_ms },
            queue: q,
            point,
            badge,
            finger: None,
            keys: Vec::new(),
        })
    }

    /// Where the finger dot goes before the input, once (the window moves the dot there and calls
    /// [`Job::glide_for`]). None for an action with no point (a button, typing with no ref): its input goes at the
    /// next tick.
    pub fn take_point(&mut self) -> Option<(f64, f64)> {
        self.point.take()
    }

    /// The dot's glide, now that the window has taken the point, lasts `ms` from `now_ms` (0 when it is not drawn:
    /// the tab is behind, the window minimised), at most [`GLIDE_MAX_MS`]. The input goes at the first tick after.
    pub fn glide_for(&mut self, ms: u64, now_ms: u64) {
        if let Stage::Glide { until_ms } = &mut self.stage {
            *until_ms = Some(now_ms + ms.min(GLIDE_MAX_MS));
        }
    }

    /// Time has passed; `changed` is the sim host's changed count now.
    pub fn on_tick(&mut self, now_ms: u64, changed: u64) -> Out {
        if let Stage::Glide {
            until_ms: Some(until),
        } = self.stage
            && now_ms >= until
        {
            self.stage = Stage::Send { start_ms: now_ms };
        }
        match self.stage {
            Stage::Glide { .. } => Out::Idle,
            Stage::Send { start_ms } => {
                let mut batch = Vec::new();
                while self
                    .queue
                    .front()
                    .is_some_and(|(t, _)| start_ms + t <= now_ms)
                {
                    if let Some((_, input)) = self.queue.pop_front() {
                        self.track(&input);
                        batch.push(input);
                    }
                }
                if self.queue.is_empty() {
                    self.stage = Stage::Settle {
                        base: changed,
                        until_ms: now_ms + SETTLE_MS,
                    };
                }
                if batch.is_empty() {
                    Out::Idle
                } else {
                    Out::Send(batch)
                }
            }
            Stage::Settle { until_ms, .. } if now_ms >= until_ms => {
                Out::Done(Ok(json!({"changed": false})))
            }
            Stage::Settle { .. } => Out::Idle,
        }
    }

    /// The sim host's changed count rose to `count`: the job answers once the screen changed after its input.
    pub fn on_changed(&mut self, count: u64) -> Out {
        match self.stage {
            Stage::Settle { base, .. } if count > base => Out::Done(Ok(json!({"changed": true}))),
            _ => Out::Idle,
        }
    }

    /// The input that lets go of what the job holds down (a finger mid-swipe or mid-press, a key or shift), to send
    /// when it ends early. The window sends it and waits for nothing.
    pub fn release(&mut self) -> Vec<Input> {
        self.queue.clear();
        let mut out = Vec::new();
        if let Some(p) = self.finger.take() {
            out.push(touch(Phase::End, p));
        }
        for u in self.keys.drain(..).rev() {
            out.push(key(u, false));
        }
        out
    }

    fn track(&mut self, input: &Input) {
        match input {
            Input::Touch(t) => {
                self.finger = (t.phase != Phase::End).then_some((t.x, t.y));
            }
            Input::Key(k) if k.down => self.keys.push(k.usage),
            Input::Key(k) => self.keys.retain(|u| *u != k.usage),
            Input::Button(_) | Input::Paste(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::{Rect, UiList, UiRef};

    const PHONE: Size = Size { w: 402.0, h: 874.0 };

    fn refs() -> RefMap {
        UiList {
            refs: vec![UiRef {
                r: "d1".into(),
                role: "AXButton".into(),
                label: "OK".into(),
                value: String::new(),
                frame: Rect {
                    x: 100.0,
                    y: 200.0,
                    w: 40.0,
                    h: 20.0,
                },
                enabled: true,
            }],
            left_out: 0,
        }
        .ref_map()
    }

    fn tap(at: At) -> Job {
        Job::start(DeviceOp::Tap { at, hold_ms: None }, &refs(), PHONE).unwrap()
    }

    fn sent(out: Out) -> Vec<Input> {
        match out {
            Out::Send(v) => v,
            other => panic!("expected input, got {other:?}"),
        }
    }

    #[test]
    fn a_tap_waits_for_the_glide_then_for_the_next_change() {
        let mut j = tap(At::Ref("d1".into()));
        assert_eq!(j.op(), "tap");
        assert_eq!(j.take_point(), Some((120.0, 210.0)), "the ref's centre");
        assert_eq!(j.take_point(), None, "once");
        assert_eq!(
            j.on_tick(0, 5),
            Out::Idle,
            "nothing before the window says how long the glide is"
        );
        j.glide_for(400, 0);
        assert_eq!(j.on_tick(399, 5), Out::Idle, "the dot is still gliding");
        let first = sent(j.on_tick(400, 5));
        assert_eq!(first, vec![touch(Phase::Start, (120.0, 210.0))]);
        assert_eq!(j.on_tick(400 + TAP_MS - 1, 5), Out::Idle);
        assert_eq!(
            sent(j.on_tick(400 + TAP_MS, 5)),
            vec![touch(Phase::End, (120.0, 210.0))]
        );
        // Now it waits for the screen to change.
        assert_eq!(j.on_changed(5), Out::Idle, "not a change");
        assert_eq!(j.on_tick(500, 5), Out::Idle);
        assert_eq!(j.on_changed(6), Out::Done(Ok(json!({"changed": true}))));
        // A glide is never held longer than the cursor's longest.
        let mut j = tap(At::Point(10.0, 10.0));
        j.take_point();
        j.glide_for(5_000, 0);
        assert!(matches!(j.on_tick(GLIDE_MAX_MS, 0), Out::Send(_)));
        // A button has no point: it goes at the first tick.
        let mut j =
            Job::start(DeviceOp::Press(Press::Button(Button::Home)), &refs(), PHONE).unwrap();
        assert_eq!(j.take_point(), None);
        assert_eq!(sent(j.on_tick(0, 0)), vec![Input::Button(Button::Home)]);
    }

    #[test]
    fn with_no_change_the_job_answers_after_300_ms() {
        let mut j = tap(At::Point(10.0, 10.0));
        j.take_point();
        j.glide_for(0, 1_000);
        sent(j.on_tick(1_000, 9));
        sent(j.on_tick(1_000 + TAP_MS, 9));
        assert_eq!(j.on_tick(1_000 + TAP_MS + SETTLE_MS - 1, 9), Out::Idle);
        assert_eq!(
            j.on_tick(1_000 + TAP_MS + SETTLE_MS, 9),
            Out::Done(Ok(json!({"changed": false})))
        );
        // Typing with no ref: keys, shift where needed, then Return; the badge says so.
        let op = DeviceOp::Type {
            text: "Hi".into(),
            at: None,
            submit: true,
        };
        let mut j = Job::start(op, &refs(), PHONE).unwrap();
        assert_eq!(j.badge(), Some(TYPING));
        assert_eq!(j.take_point(), None);
        let mut all = sent(j.on_tick(0, 0));
        all.extend(sent(j.on_tick(KEY_MS, 0)));
        all.extend(sent(j.on_tick(KEY_MS * 2, 0)));
        let k = |u, d| key(u, d);
        assert_eq!(
            all,
            vec![
                k(0xE1, true),
                k(0x0B, true),
                k(0x0B, false),
                k(0xE1, false),
                k(0x0C, true),
                k(0x0C, false),
                k(0x28, true),
                k(0x28, false),
            ]
        );
        assert_eq!(
            j.on_tick(KEY_MS * 2 + SETTLE_MS, 0),
            Out::Done(Ok(json!({"changed": false})))
        );
        // Text the keymap cannot type is pasted whole.
        let op = DeviceOp::Type {
            text: "caf\u{e9}".into(),
            at: None,
            submit: false,
        };
        let mut j = Job::start(op, &refs(), PHONE).unwrap();
        j.take_point();
        assert_eq!(
            sent(j.on_tick(0, 0)),
            vec![Input::Paste("caf\u{e9}".into())]
        );
    }

    #[test]
    fn a_takeover_ends_the_job_interrupted() {
        // A long press: the finger is down when the person takes over.
        let op = DeviceOp::Tap {
            at: At::Point(50.0, 60.0),
            hold_ms: Some(1_000),
        };
        let mut j = Job::start(op, &refs(), PHONE).unwrap();
        j.take_point();
        j.glide_for(0, 0);
        sent(j.on_tick(0, 0));
        let e = Job::interrupted();
        assert_eq!(e.fail, Fail::Interrupted);
        assert_eq!(e.fail.code(), "interrupted");
        assert!(e.message.contains("look again"));
        // What the job held down is let go of, and nothing more of it is sent.
        assert_eq!(j.release(), vec![touch(Phase::End, (50.0, 60.0))]);
        assert_eq!(j.release(), vec![], "once");
        assert_eq!(j.on_tick(2_000, 0), Out::Idle);
        // Mid-swipe the finger is let go of where it is.
        let op = DeviceOp::Swipe {
            from: None,
            to: SwipeTo::Direction(Direction::Up),
            duration_ms: Some(400),
        };
        let mut j = Job::start(op, &refs(), PHONE).unwrap();
        assert_eq!(j.take_point(), Some((201.0, 437.0)), "the screen's centre");
        j.glide_for(0, 0);
        sent(j.on_tick(0, 0));
        let mid = sent(j.on_tick(200, 0));
        let Some(Input::Touch(last)) = mid.last() else {
            panic!("{mid:?}")
        };
        assert_eq!(last.phase, Phase::Move);
        assert_eq!(j.release(), vec![touch(Phase::End, (last.x, last.y))]);
        // Shift held mid-character is let go of too.
        let op = DeviceOp::Type {
            text: "A".into(),
            at: None,
            submit: false,
        };
        let mut j = Job::start(op, &refs(), PHONE).unwrap();
        j.take_point();
        j.track(&key(0xE1, true));
        assert_eq!(j.release(), vec![key(0xE1, false)]);
    }

    #[test]
    fn a_cancel_ends_the_job_at_once() {
        let mut j = tap(At::Point(10.0, 10.0));
        j.take_point();
        // Still gliding: nothing was sent, nothing to let go of.
        assert_eq!(j.release(), vec![]);
        let e = Job::cancelled();
        assert_eq!(e.fail, Fail::Interrupted);
        assert_eq!(e.message, "the call was cancelled");
    }

    #[test]
    fn a_stale_ref_is_not_found() {
        let e = Job::start(
            DeviceOp::Tap {
                at: At::Ref("d9".into()),
                hold_ms: None,
            },
            &refs(),
            PHONE,
        )
        .err()
        .unwrap();
        assert_eq!(e.fail, Fail::NotFound);
        assert_eq!(e.fail.code(), "not_found");
        assert!(e.message.contains("d9") && e.message.contains("look again"));
        // With no look at all, every ref is stale.
        let none = RefMap::default();
        let op = DeviceOp::Type {
            text: "x".into(),
            at: Some(At::Ref("d1".into())),
            submit: false,
        };
        assert_eq!(
            Job::start(op, &none, PHONE).err().unwrap().fail,
            Fail::NotFound
        );
        // A point off the screen, and an unknown key, are refused too.
        let off = Job::start(
            DeviceOp::Tap {
                at: At::Point(500.0, 10.0),
                hold_ms: None,
            },
            &refs(),
            PHONE,
        );
        assert_eq!(off.err().unwrap().fail, Fail::NotFound);
        let k = Job::start(DeviceOp::Press(Press::Key("f13".into())), &refs(), PHONE);
        assert_eq!(k.err().unwrap().fail, Fail::NotFound);
        let k = Job::start(DeviceOp::Press(Press::Key("return".into())), &refs(), PHONE).unwrap();
        assert_eq!(k.badge(), Some("Return"));
    }
}

//! A device source for tests and `--e2e` (on Windows too): scripted devices, a frame counter that "renders" a solid
//! colour per frame as a CPU picture (no ring, so the CPU path and the e2e scenarios run anywhere), every input
//! recorded, and switches to fail the next request, go broken, or boot slowly. Time is the caller's
//! ([`FakeSource::advance`]); nothing here sleeps or spawns.

use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc::{Receiver, Sender, channel};

use crate::geom::{Orientation, Size};
use crate::job::Fail;
use crate::source::{
    Answer, Button, DeviceInfo, DeviceSource, DeviceState, Key, ReqId, Screen, SourceError,
    SourceEvent, Touch,
};
use crate::target::Target;
use crate::tree::{Rect, UiNode};

/// What the fake's "PNG" is: the PNG signature and a marker (no encoder in a pure crate; nothing decodes it).
pub const FAKE_PNG: &[u8] = b"\x89PNG\r\n\x1a\nflare-fake-device";

/// One input the fake was given, in order.
#[derive(Clone, Debug, PartialEq)]
pub enum Recorded {
    Touch(Target, Touch),
    Key(Target, Key),
    Button(Target, Button),
    Text(Target, String),
    Launch(Target, String),
    Install(Target, String),
    Pause(Target, bool),
}

struct Attached {
    seq: u64,
    changed: u64,
    paused: bool,
}

/// The fake source.
pub struct FakeSource {
    tx: Sender<SourceEvent>,
    rx: Receiver<SourceEvent>,
    devices: Vec<DeviceInfo>,
    screen: Screen,
    tree: UiNode,
    attached: HashMap<Target, Attached>,
    inputs: Vec<Recorded>,
    next_id: ReqId,
    now_ms: u64,
    boot_ms: u64,
    /// Boots under way: when each is done, and its request.
    booting: Vec<(u64, Target, ReqId)>,
    fail_next: Option<SourceError>,
    broken: Option<String>,
    /// Like the sim host: no `State` event of its own on a boot or a shutdown, only the answer.
    no_state_events: bool,
}

/// The colour of frame `seq`: a different solid colour each frame, opaque.
pub fn colour(seq: u64) -> [u8; 4] {
    let c = |k: u64| ((seq.wrapping_mul(k) + k) % 256) as u8;
    [c(37), c(91), c(53), 255]
}

/// A fake iPhone (iOS 27.2) in `state`.
pub fn iphone(udid: &str, name: &str, state: DeviceState) -> Option<DeviceInfo> {
    Some(DeviceInfo {
        target: Target::sim(udid)?,
        name: name.to_string(),
        kind: "iPhone".to_string(),
        os: "iOS 27.2".to_string(),
        state,
        booted_by_flare: false,
    })
}

impl Default for FakeSource {
    fn default() -> Self {
        Self::new(
            iphone(
                "00000000-0000-4000-8000-000000000001",
                "iPhone 18 Pro",
                DeviceState::Shutdown,
            )
            .into_iter()
            .collect(),
        )
    }
}

impl FakeSource {
    /// A fake with these devices, an iPhone 18 Pro's screen (402 x 874 points, pictures at 1x) and a small tree.
    pub fn new(devices: Vec<DeviceInfo>) -> Self {
        let (tx, rx) = channel();
        let pt = Size::new(402.0, 874.0);
        let frame = |x, y, w, h| Rect { x, y, w, h };
        let leaf = |role: &str, label: &str, f| UiNode {
            role: role.into(),
            label: label.into(),
            frame: f,
            enabled: true,
            ..UiNode::default()
        };
        let tree = UiNode {
            role: "AXApplication".into(),
            frame: frame(0.0, 0.0, pt.w, pt.h),
            enabled: true,
            children: vec![
                leaf(
                    "AXStaticText",
                    "Fake device",
                    frame(16.0, 60.0, 200.0, 40.0),
                ),
                leaf("AXButton", "Tap me", frame(16.0, 120.0, 120.0, 44.0)),
            ],
            ..UiNode::default()
        };
        Self {
            tx,
            rx,
            devices,
            screen: Screen {
                size_px: pt,
                size_pt: pt,
                scale: 1.0,
                orientation: Orientation::Rot0,
            },
            tree,
            attached: HashMap::new(),
            inputs: Vec::new(),
            next_id: 0,
            now_ms: 0,
            boot_ms: 0,
            booting: Vec::new(),
            fail_next: None,
            broken: None,
            no_state_events: false,
        }
    }

    /// Every input so far, in order.
    pub fn inputs(&self) -> &[Recorded] {
        &self.inputs
    }

    /// Boots take this long (until [`FakeSource::advance`] passes it).
    pub fn set_boot_ms(&mut self, ms: u64) {
        self.boot_ms = ms;
    }

    /// The next request answers this error.
    pub fn fail_next(&mut self, fail: Fail, message: &str) {
        self.fail_next = Some(SourceError {
            fail,
            message: message.to_string(),
        });
    }

    /// The source breaks: a `Broken` event now, and every later request answers `unavailable` with `why`.
    pub fn go_broken(&mut self, why: &str) {
        self.broken = Some(why.to_string());
        self.send(SourceEvent::Broken(why.to_string()));
    }

    /// The UI tree `ui_tree` answers.
    pub fn set_tree(&mut self, tree: UiNode) {
        self.tree = tree;
    }

    /// The device turns (the picture's size stays, as the framebuffer does).
    pub fn rotate(&mut self, target: &Target, orientation: Orientation) {
        self.screen.orientation = orientation;
        self.send(SourceEvent::Orientation {
            target: target.clone(),
            orientation,
        });
        self.render(target);
    }

    /// The clock is at `now_ms`: boots that are due answer.
    pub fn advance(&mut self, now_ms: u64) {
        self.now_ms = now_ms;
        let (due, wait): (Vec<_>, Vec<_>) = std::mem::take(&mut self.booting)
            .into_iter()
            .partition(|(at, ..)| *at <= now_ms);
        self.booting = wait;
        for (_, target, id) in due {
            self.set_state(&target, DeviceState::Booted, true);
            self.answer(id, Ok(Answer::Done));
        }
    }

    /// The screen changed: a new frame (a CPU picture) unless paused, and the changed count either way.
    pub fn render(&mut self, target: &Target) {
        let size = self.screen.size_px;
        let Some(a) = self.attached.get_mut(target) else {
            return;
        };
        a.changed += 1;
        let (changed, paused) = (a.changed, a.paused);
        if !paused {
            a.seq += 1;
            let seq = a.seq;
            let px = colour(seq);
            let n = (size.w as usize) * (size.h as usize);
            let rgba = px.iter().copied().cycle().take(n * 4).collect();
            self.send(SourceEvent::Picture {
                target: target.clone(),
                seq,
                size_px: size,
                rgba,
            });
        }
        self.send(SourceEvent::Changed {
            target: target.clone(),
            count: changed,
        });
    }

    fn send(&self, e: SourceEvent) {
        // The receiver is our own field: a send cannot fail while self lives.
        let _ = self.tx.send(e);
    }

    fn answer(&self, id: ReqId, result: Result<Answer, SourceError>) {
        self.send(SourceEvent::Answer { id, result });
    }

    fn set_state(&mut self, target: &Target, state: DeviceState, by_flare: bool) {
        if let Some(d) = self.devices.iter_mut().find(|d| &d.target == target) {
            d.state = state;
            d.booted_by_flare = by_flare && state != DeviceState::Shutdown;
        }
        if !self.no_state_events {
            self.send(SourceEvent::State {
                target: target.clone(),
                state,
            });
        }
    }

    /// Sends no `State` event on a boot or a shutdown, as the sim host does (a test of the answer alone).
    pub fn without_state_events(mut self) -> Self {
        self.no_state_events = true;
        self
    }

    fn state_of(&self, target: &Target) -> Option<DeviceState> {
        self.devices
            .iter()
            .find(|d| &d.target == target)
            .map(|d| d.state)
    }

    /// A new request's id, or its error at once when the fake was told to fail or is broken.
    fn begin(&mut self) -> (ReqId, Option<SourceError>) {
        self.next_id += 1;
        let gate = self.broken.as_ref().map(|why| SourceError {
            fail: Fail::Unavailable,
            message: why.clone(),
        });
        (self.next_id, gate.or_else(|| self.fail_next.take()))
    }

    /// Runs a request: its error if it has one, else `f`'s answer.
    fn request(
        &mut self,
        f: impl FnOnce(&mut Self, ReqId) -> Option<Result<Answer, SourceError>>,
    ) -> ReqId {
        let (id, gate) = self.begin();
        match gate {
            Some(e) => self.answer(id, Err(e)),
            None => {
                if let Some(r) = f(self, id) {
                    self.answer(id, r);
                }
            }
        }
        id
    }

    fn not_found(target: &Target) -> SourceError {
        SourceError {
            fail: Fail::NotFound,
            message: format!("no device {target}"),
        }
    }

    /// Input reaches an attached device only (and none while broken): recorded, and the screen changes.
    fn input(&mut self, target: &Target, r: Recorded) {
        if self.broken.is_some() || !self.attached.contains_key(target) {
            return;
        }
        self.inputs.push(r);
        self.render(target);
    }
}

impl DeviceSource for FakeSource {
    fn list(&mut self) -> ReqId {
        self.request(|s, _| Some(Ok(Answer::Devices(s.devices.clone()))))
    }

    fn boot(&mut self, target: &Target) -> ReqId {
        self.request(|s, id| match s.state_of(target) {
            None => Some(Err(Self::not_found(target))),
            Some(DeviceState::Booted) => Some(Ok(Answer::Done)),
            Some(_) => {
                s.set_state(target, DeviceState::Booting, true);
                s.booting.push((s.now_ms + s.boot_ms, target.clone(), id));
                let now = s.now_ms;
                s.advance(now);
                None
            }
        })
    }

    fn shutdown(&mut self, target: &Target) -> ReqId {
        self.request(|s, _| {
            if s.state_of(target).is_none() {
                return Some(Err(Self::not_found(target)));
            }
            s.attached.remove(target);
            s.booting.retain(|(_, t, _)| t != target);
            s.set_state(target, DeviceState::Shutdown, false);
            Some(Ok(Answer::Done))
        })
    }

    fn attach(&mut self, target: &Target) -> ReqId {
        self.request(|s, _| match s.state_of(target) {
            None => Some(Err(Self::not_found(target))),
            Some(DeviceState::Booted) => {
                s.attached.entry(target.clone()).or_insert(Attached {
                    seq: 0,
                    changed: 0,
                    paused: false,
                });
                s.send(SourceEvent::Frames {
                    target: target.clone(),
                    ring: 0,
                    screen: s.screen,
                });
                s.render(target);
                Some(Ok(Answer::Done))
            }
            Some(DeviceState::Booting) => Some(Err(SourceError {
                fail: Fail::Busy,
                message: "the device is still booting".into(),
            })),
            Some(_) => Some(Err(SourceError {
                fail: Fail::Unavailable,
                message: "the device is shut down".into(),
            })),
        })
    }

    fn detach(&mut self, target: &Target) {
        self.attached.remove(target);
    }

    fn pause(&mut self, target: &Target, paused: bool) {
        if let Some(a) = self.attached.get_mut(target) {
            let was = std::mem::replace(&mut a.paused, paused);
            self.inputs.push(Recorded::Pause(target.clone(), paused));
            // Resuming shows the current screen at once.
            if was && !paused {
                self.render(target);
            }
        }
    }

    fn touch(&mut self, target: &Target, touch: Touch) {
        self.input(target, Recorded::Touch(target.clone(), touch));
    }

    fn key(&mut self, target: &Target, key: Key) {
        self.input(target, Recorded::Key(target.clone(), key));
    }

    fn button(&mut self, target: &Target, button: Button) {
        self.input(target, Recorded::Button(target.clone(), button));
    }

    fn text(&mut self, target: &Target, text: &str) -> ReqId {
        self.request(|s, _| {
            s.input(target, Recorded::Text(target.clone(), text.to_string()));
            Some(Ok(Answer::Done))
        })
    }

    fn snapshot(&mut self, _target: &Target) -> ReqId {
        self.request(|_, _| Some(Ok(Answer::Png(FAKE_PNG.to_vec()))))
    }

    fn ui_tree(&mut self, _target: &Target) -> ReqId {
        self.request(|s, _| Some(Ok(Answer::Tree(s.tree.clone()))))
    }

    fn launch(&mut self, target: &Target, bundle_id: &str) -> ReqId {
        self.request(|s, _| {
            s.input(
                target,
                Recorded::Launch(target.clone(), bundle_id.to_string()),
            );
            Some(Ok(Answer::Done))
        })
    }

    fn install(&mut self, target: &Target, app: &Path) -> ReqId {
        self.request(|s, _| {
            s.inputs
                .push(Recorded::Install(target.clone(), app.display().to_string()));
            Some(Ok(Answer::Done))
        })
    }

    fn events(&self) -> &Receiver<SourceEvent> {
        &self.rx
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::Phase;

    fn drain(f: &FakeSource) -> Vec<SourceEvent> {
        f.events().try_iter().collect()
    }

    fn target() -> Target {
        Target::sim("00000000-0000-4000-8000-000000000001").unwrap()
    }

    #[test]
    fn a_fake_device_boots_attaches_renders_and_records_its_input() {
        let mut f = FakeSource::default();
        let t = target();
        let id = f.list();
        let ev = drain(&f);
        assert!(
            matches!(&ev[0], SourceEvent::Answer { id: i, result: Ok(Answer::Devices(d)) } if *i == id && d.len() == 1)
        );
        // Not booted: attach is refused.
        f.attach(&t);
        assert!(
            matches!(&drain(&f)[0], SourceEvent::Answer { result: Err(e), .. } if e.fail == Fail::Unavailable)
        );
        f.boot(&t);
        let ev = drain(&f);
        assert!(ev.contains(&SourceEvent::State {
            target: t.clone(),
            state: DeviceState::Booted
        }));
        f.attach(&t);
        let ev = drain(&f);
        assert!(matches!(&ev[0], SourceEvent::Frames { ring: 0, .. }));
        let SourceEvent::Picture {
            seq: 1,
            rgba,
            size_px,
            ..
        } = &ev[1]
        else {
            panic!("{:?}", ev[1])
        };
        assert_eq!(rgba.len(), (size_px.w * size_px.h * 4.0) as usize);
        assert_eq!(&rgba[..4], &colour(1));
        assert_ne!(colour(1), colour(2), "a new colour each frame");
        // Input is recorded and changes the screen.
        let touch = Touch::one(Phase::Start, 10.0, 20.0);
        f.touch(&t, touch);
        f.button(&t, Button::Home);
        assert_eq!(
            f.inputs(),
            &[
                Recorded::Touch(t.clone(), touch),
                Recorded::Button(t.clone(), Button::Home)
            ]
        );
        assert!(drain(&f).contains(&SourceEvent::Changed {
            target: t.clone(),
            count: 3
        }));
        // Paused: no picture, the count goes on; resuming shows the screen at once.
        f.pause(&t, true);
        f.key(
            &t,
            Key {
                usage: 4,
                down: true,
            },
        );
        let ev = drain(&f);
        assert!(!ev.iter().any(|e| matches!(e, SourceEvent::Picture { .. })));
        assert!(ev.contains(&SourceEvent::Changed {
            target: t.clone(),
            count: 4
        }));
        f.pause(&t, false);
        assert!(
            drain(&f)
                .iter()
                .any(|e| matches!(e, SourceEvent::Picture { seq: 4, .. }))
        );
        // Flare booted it, so the list says so; a shutdown clears it.
        f.list();
        let ev = drain(&f);
        let SourceEvent::Answer {
            result: Ok(Answer::Devices(d)),
            ..
        } = &ev[0]
        else {
            panic!()
        };
        assert!(d[0].booted_by_flare && d[0].state == DeviceState::Booted);
        f.shutdown(&t);
        f.touch(&t, touch);
        assert_eq!(f.inputs().len(), 5, "input after shutdown goes nowhere");
    }

    #[test]
    fn a_fake_device_can_fail_go_broken_and_boot_slowly() {
        let mut f = FakeSource::default();
        let t = target();
        f.set_boot_ms(5_000);
        let id = f.boot(&t);
        assert!(drain(&f).contains(&SourceEvent::State {
            target: t.clone(),
            state: DeviceState::Booting
        }));
        f.attach(&t);
        assert!(
            matches!(&drain(&f)[0], SourceEvent::Answer { result: Err(e), .. } if e.fail == Fail::Busy)
        );
        f.advance(4_999);
        assert!(drain(&f).is_empty());
        f.advance(5_000);
        assert!(drain(&f).contains(&SourceEvent::Answer {
            id,
            result: Ok(Answer::Done)
        }));
        f.fail_next(Fail::Retry, "touch connection dropped");
        f.snapshot(&t);
        f.snapshot(&t);
        let ev = drain(&f);
        assert!(
            matches!(&ev[0], SourceEvent::Answer { result: Err(e), .. } if e.fail == Fail::Retry)
        );
        assert!(
            matches!(
                &ev[1],
                SourceEvent::Answer {
                    result: Ok(Answer::Png(_)),
                    ..
                }
            ),
            "only the next one"
        );
        f.go_broken("SimDevice missing");
        f.ui_tree(&t);
        let ev = drain(&f);
        assert_eq!(ev[0], SourceEvent::Broken("SimDevice missing".into()));
        assert!(matches!(&ev[1], SourceEvent::Answer { result: Err(e), .. }
            if e.fail == Fail::Unavailable && e.message == "SimDevice missing"));
        let other = Target::sim("00000000-0000-4000-8000-0000000000ff").unwrap();
        let mut f = FakeSource::default();
        f.boot(&other);
        assert!(
            matches!(&drain(&f)[0], SourceEvent::Answer { result: Err(e), .. } if e.fail == Fail::NotFound)
        );
    }
}

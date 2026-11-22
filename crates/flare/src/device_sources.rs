//! Both device sources behind one (Android plan PR A4, rule 1360): the iOS simulator's sim host (macOS) and the
//! Android emulator's emu host (Windows and macOS). `DeviceCtl` keeps talking to one `DeviceSource`; this routes each
//! request by its target's kind (`Target::source`), joins the two device lists into one, and keeps the two hosts'
//! request ids apart (each starts at 1). A kind with no source answers `unavailable` at once. Events come back tagged
//! with their kind ([`Multi::next`]), so one source going broken leaves the other live.

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::mpsc::{Receiver, channel};

use flare_device::job::Fail;
use flare_device::source::{
    Answer, Button, DeviceInfo, DeviceSource, Key, ReqId, SourceError, SourceEvent, SourceKind,
    Touch,
};
use flare_device::target::Target;
use flare_device::wire::{Hello, SetupStep};

/// The line a request about a kind with no source answers.
fn none_line(kind: SourceKind) -> String {
    match kind {
        SourceKind::Sim => "iPhone simulators are not available in this Flare".to_string(),
        SourceKind::Emu => "Android devices are not available in this Flare".to_string(),
    }
}

/// Where an inner answer goes.
#[derive(Clone, Copy, Debug)]
enum Route {
    One(ReqId),
    List(ReqId),
}

/// A device list waiting for each source's half.
#[derive(Default)]
struct Join {
    waiting: usize,
    devices: Vec<DeviceInfo>,
    err: Option<SourceError>,
}

/// The two sources as one.
pub struct Multi {
    sim: Option<Box<dyn DeviceSource>>,
    emu: Option<Box<dyn DeviceSource>>,
    next: ReqId,
    ids: HashMap<(SourceKind, ReqId), Route>,
    lists: HashMap<ReqId, Join>,
    /// Answers made here (no source for that kind, an empty list).
    local: VecDeque<SourceEvent>,
    /// The kind read first next time, so a busy source never starves the other.
    first: SourceKind,
    /// Never sent to: `events()` must hand out a receiver; `DeviceCtl` reads [`Multi::next`] instead.
    _tx: std::sync::mpsc::Sender<SourceEvent>,
    rx: Receiver<SourceEvent>,
}

impl Default for Multi {
    fn default() -> Self {
        let (tx, rx) = channel();
        Self {
            sim: None,
            emu: None,
            next: 1,
            ids: HashMap::new(),
            lists: HashMap::new(),
            local: VecDeque::new(),
            first: SourceKind::Sim,
            _tx: tx,
            rx,
        }
    }
}

impl Multi {
    /// The source of `kind` (None: it stops, its requests in flight are let go).
    pub fn set(&mut self, kind: SourceKind, source: Option<Box<dyn DeviceSource>>) {
        if source.is_none() {
            self.ids.retain(|(k, _), _| *k != kind);
        }
        match kind {
            SourceKind::Sim => self.sim = source,
            SourceKind::Emu => self.emu = source,
        }
    }

    pub fn has(&self, kind: SourceKind) -> bool {
        self.get_ref(kind).is_some()
    }

    pub fn is_empty(&self) -> bool {
        self.sim.is_none() && self.emu.is_none()
    }

    fn get(&mut self, kind: SourceKind) -> Option<&mut Box<dyn DeviceSource>> {
        match kind {
            SourceKind::Sim => self.sim.as_mut(),
            SourceKind::Emu => self.emu.as_mut(),
        }
    }

    fn get_ref(&self, kind: SourceKind) -> Option<&dyn DeviceSource> {
        match kind {
            SourceKind::Sim => self.sim.as_deref(),
            SourceKind::Emu => self.emu.as_deref(),
        }
    }

    /// The source's facts (`hello`) of one kind.
    pub fn hello_of(&self, kind: SourceKind) -> Option<Hello> {
        self.get_ref(kind)?.hello()
    }

    fn id(&mut self) -> ReqId {
        let id = self.next;
        self.next += 1;
        id
    }

    /// Sends a request with an answer to the source of `kind`, or answers `unavailable` here.
    fn ask(
        &mut self,
        kind: SourceKind,
        send: impl FnOnce(&mut dyn DeviceSource) -> ReqId,
    ) -> ReqId {
        let id = self.id();
        match self.get(kind) {
            Some(s) => {
                let inner = send(s.as_mut());
                self.ids.insert((kind, inner), Route::One(id));
            }
            None => self.local.push_back(SourceEvent::Answer {
                id,
                result: Err(SourceError {
                    fail: Fail::Unavailable,
                    message: none_line(kind),
                }),
            }),
        }
        id
    }

    /// Input and the like, with no answer: dropped for a kind with no source.
    fn tell(&mut self, kind: SourceKind, send: impl FnOnce(&mut dyn DeviceSource)) {
        if let Some(s) = self.get(kind) {
            send(s.as_mut());
        }
    }

    /// The next event of either source, with its kind; answers carry this source's ids. None when both are empty.
    pub fn next(&mut self) -> Option<(Option<SourceKind>, SourceEvent)> {
        if let Some(e) = self.local.pop_front() {
            return Some((None, e));
        }
        let order = match self.first {
            SourceKind::Sim => [SourceKind::Sim, SourceKind::Emu],
            SourceKind::Emu => [SourceKind::Emu, SourceKind::Sim],
        };
        self.first = order[1];
        for kind in order {
            loop {
                let Some(ev) = self.get_ref(kind).and_then(|s| s.events().try_recv().ok()) else {
                    break;
                };
                if let Some(e) = self.outer(kind, ev) {
                    return Some((Some(kind), e));
                }
            }
        }
        None
    }

    /// An inner event as this source's: answers mapped to their outer id (a joined list once both halves came).
    fn outer(&mut self, kind: SourceKind, ev: SourceEvent) -> Option<SourceEvent> {
        let SourceEvent::Answer { id, result } = ev else {
            return Some(ev);
        };
        match self.ids.remove(&(kind, id))? {
            Route::One(o) => Some(SourceEvent::Answer { id: o, result }),
            Route::List(o) => {
                let j = self.lists.get_mut(&o)?;
                j.waiting = j.waiting.saturating_sub(1);
                match result {
                    Ok(Answer::Devices(d)) => j.devices.extend(d),
                    Ok(_) => {}
                    // One source's failure never hides the other's devices.
                    Err(e) => {
                        log::info!("device list ({kind:?}): {}", e.message);
                        j.err.get_or_insert(e);
                    }
                }
                if j.waiting > 0 {
                    return None;
                }
                let j = self.lists.remove(&o)?;
                let result = match (j.devices.is_empty(), j.err) {
                    (true, Some(e)) => Err(e),
                    _ => Ok(Answer::Devices(j.devices)),
                };
                Some(SourceEvent::Answer { id: o, result })
            }
        }
    }
}

impl DeviceSource for Multi {
    fn list(&mut self) -> ReqId {
        let id = self.id();
        let mut join = Join::default();
        for kind in [SourceKind::Sim, SourceKind::Emu] {
            if let Some(s) = self.get(kind) {
                let inner = s.list();
                self.ids.insert((kind, inner), Route::List(id));
                join.waiting += 1;
            }
        }
        if join.waiting == 0 {
            self.local.push_back(SourceEvent::Answer {
                id,
                result: Ok(Answer::Devices(Vec::new())),
            });
        } else {
            self.lists.insert(id, join);
        }
        id
    }

    fn boot(&mut self, target: &Target) -> ReqId {
        self.ask(target.source(), |s| s.boot(target))
    }

    fn shutdown(&mut self, target: &Target) -> ReqId {
        self.ask(target.source(), |s| s.shutdown(target))
    }

    fn attach(&mut self, target: &Target) -> ReqId {
        self.ask(target.source(), |s| s.attach(target))
    }

    fn detach(&mut self, target: &Target) {
        self.tell(target.source(), |s| s.detach(target));
    }

    fn pause(&mut self, target: &Target, paused: bool) {
        self.tell(target.source(), |s| s.pause(target, paused));
    }

    fn touch(&mut self, target: &Target, touch: Touch) {
        self.tell(target.source(), |s| s.touch(target, touch));
    }

    fn key(&mut self, target: &Target, key: Key) {
        self.tell(target.source(), |s| s.key(target, key));
    }

    fn button(&mut self, target: &Target, button: Button) {
        self.tell(target.source(), |s| s.button(target, button));
    }

    fn text(&mut self, target: &Target, text: &str) -> ReqId {
        self.ask(target.source(), |s| s.text(target, text))
    }

    fn snapshot(&mut self, target: &Target) -> ReqId {
        self.ask(target.source(), |s| s.snapshot(target))
    }

    fn ui_tree(&mut self, target: &Target) -> ReqId {
        self.ask(target.source(), |s| s.ui_tree(target))
    }

    fn launch(&mut self, target: &Target, bundle_id: &str) -> ReqId {
        self.ask(target.source(), |s| s.launch(target, bundle_id))
    }

    fn install(&mut self, target: &Target, app: &Path) -> ReqId {
        self.ask(target.source(), |s| s.install(target, app))
    }

    /// Never read: `DeviceCtl` reads [`Multi::next`].
    fn events(&self) -> &Receiver<SourceEvent> {
        &self.rx
    }

    /// The emu host's facts when it runs, else the sim host's.
    fn hello(&self) -> Option<Hello> {
        self.hello_of(SourceKind::Emu)
            .or_else(|| self.hello_of(SourceKind::Sim))
    }

    fn setup(&mut self, step: SetupStep) -> Option<ReqId> {
        let s = self.emu.as_mut()?;
        let inner = s.setup(step)?;
        let id = self.id();
        self.ids.insert((SourceKind::Emu, inner), Route::One(id));
        Some(id)
    }
}

#[cfg(test)]
mod tests {
    use flare_device::fake::FakeSource;
    use flare_device::source::DeviceState;

    use super::*;

    fn drain(m: &mut Multi) -> Vec<(Option<SourceKind>, SourceEvent)> {
        std::iter::from_fn(|| m.next()).collect()
    }

    fn answer(
        evs: &[(Option<SourceKind>, SourceEvent)],
        id: ReqId,
    ) -> &Result<Answer, SourceError> {
        evs.iter()
            .find_map(|(_, e)| match e {
                SourceEvent::Answer { id: i, result } if *i == id => Some(result),
                _ => None,
            })
            .expect("answered")
    }

    /// Rule 1360: one list holds both kinds, each request reaches its own kind's source, and the two sources' ids
    /// never meet.
    #[test]
    fn both_sources_answer_as_one() {
        let mut m = Multi::default();
        m.set(SourceKind::Sim, Some(Box::new(FakeSource::default())));
        m.set(SourceKind::Emu, Some(Box::new(FakeSource::android())));
        let id = m.list();
        let evs = drain(&mut m);
        let Ok(Answer::Devices(list)) = answer(&evs, id) else {
            panic!("{evs:?}")
        };
        assert!(list.iter().any(|d| d.target.source() == SourceKind::Sim));
        let emu = list
            .iter()
            .find(|d| d.target.source() == SourceKind::Emu)
            .expect("an Android device")
            .clone();
        // Each fake starts its ids at 1: the outer ids still differ, and each answer finds its own.
        let a = m.boot(&emu.target);
        let b = m.snapshot(&list[0].target);
        assert_ne!(a, b);
        let evs = drain(&mut m);
        assert!(answer(&evs, a).is_ok());
        assert!(answer(&evs, b).is_ok() || answer(&evs, b).is_err());
        // Events say which source they came from.
        assert!(evs.iter().any(|(k, e)| *k == Some(SourceKind::Emu)
            && matches!(
                e,
                SourceEvent::State {
                    state: DeviceState::Booted,
                    ..
                } | SourceEvent::Answer { .. }
            )));
    }

    /// A kind with no source answers `unavailable` at once, and the list holds only the other kind.
    #[test]
    fn a_missing_kind_answers_unavailable_and_the_list_goes_on() {
        let mut m = Multi::default();
        m.set(SourceKind::Emu, Some(Box::new(FakeSource::android())));
        let sim = Target::sim("00000000-0000-4000-8000-0000000000aa").unwrap();
        let id = m.boot(&sim);
        let l = m.list();
        let evs = drain(&mut m);
        let Err(e) = answer(&evs, id) else {
            panic!("{evs:?}")
        };
        assert_eq!(e.fail, Fail::Unavailable);
        assert!(e.message.contains("iPhone"), "{}", e.message);
        let Ok(Answer::Devices(list)) = answer(&evs, l) else {
            panic!("{evs:?}")
        };
        assert!(!list.is_empty());
        assert!(list.iter().all(|d| d.target.source() == SourceKind::Emu));
        // No source at all: an empty list, at once.
        let mut none = Multi::default();
        let l = none.list();
        let evs = drain(&mut none);
        assert!(matches!(answer(&evs, l), Ok(Answer::Devices(d)) if d.is_empty()));
        assert!(none.setup(SetupStep::Plan).is_none());
    }
}

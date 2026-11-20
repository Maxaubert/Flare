//! DeviceCtl: the device panel (issue #248, kind 5), the person's half (device panel PR 4a).
//!
//! The model of what each tab's device panel shows: its setup state (no Xcode, not set up, no runtime, the device
//! list, booting, live, still, shut down outside Flare; rule 1318), the device source behind `dyn DeviceSource`, which
//! tab drives a device (a second tab on the same device is view only), and the person's pointer, wheel and keys over
//! the picture as touches in device points. The source answers through its event channel (rule 34); every request
//! here returns at once.
//!
//! Frames take the CPU path here: a source's picture (RGBA) is turned to the interface's orientation and handed to
//! Slint as a `SharedPixelBuffer`, and the header wears the "slow" mark. PR 4b adds the zero-copy ring for the real
//! simulator. Only the front tab's open panel is live; every other attached device is paused (the tab behind, the
//! panel closed, the window minimised or occluded) and resumes with its current frame.
//!
//! The device kind is offered only where a source exists: the fake source under `--e2e` and in tests (PR 4a), the
//! simulator on a Mac with Xcode from PR 4b. A build that offers none keeps a stored device panel in the file and
//! shows nothing (rule 1316). Restore never boots a device: it shows Boot.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use flare_device::fake::FakeSource;
use flare_device::geom::{Letterbox, Orientation, Size};
use flare_device::job::JobError;
use flare_device::keymap::{self, usage};
use flare_device::source::{
    Answer, Button, DeviceInfo, DeviceSource, DeviceState, InputState, ReqId, Screen, SourceEvent,
    Touch,
};
use flare_device::source::{Key, Phase};
use flare_device::target::{self, Target};
use slint::{Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel};

use crate::appctl::Post;
use crate::panel_ctl::PanelCtl;
use crate::{AppWindow, DeviceRow};

/// The target of a device panel whose device is not chosen yet: it shows the device list. Never stored.
pub const CHOOSE: &str = "device";

/// The corner radius of an iPhone's screen in points (the picture is drawn bare with it, no bezel).
pub const RADIUS_PT: f64 = 55.0;

/// Moves of a held touch are sent at most this often (about once per displayed frame).
pub const MOVE_MS: u64 = 16;

/// A wheel turn becomes a drag of this many moves (start, moves, end), as idb does.
pub const WHEEL_STEPS: u32 = 4;

/// The timer that counts a boot's seconds while the front panel shows it booting.
pub const BOOT_TICK: &str = "device.boot";

/// Where "Get Xcode" goes: Xcode's page in the Mac App Store.
pub const XCODE_STORE: &str = "https://apps.apple.com/app/xcode/id497799835";

/// The slow mark's reason on the CPU path.
pub const CPU_SLOW: &str =
    "The picture is copied by the CPU: this renderer cannot show the device's own frames.";

/// What the window asks of the device controller.
#[derive(Debug)]
pub enum DeviceMsg {
    /// The source has events waiting (a reader thread's wake, PR 4b).
    #[allow(dead_code)] // the simulator source's reader posts it (PR 4b)
    Wake,
    /// A second passed while the front panel shows a boot.
    Tick,
}

/// Whether Xcode can run a simulator here (spec "Setup", states 2 and 3). The fake source says Ready.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // the Xcode pick answers them (PR 4b); tests set them now
pub enum Xcode {
    Ready,
    Missing,
    NotSetUp,
}

/// What a tab's device panel shows (rule 1318: each one line and at most one button).
#[derive(Clone, Debug, PartialEq)]
pub enum Setup {
    NoXcode,
    NotSetUp,
    /// Xcode has no iPhone simulator (no runtime, or no device).
    NoRuntime,
    /// The device list is being read.
    Reading,
    Choose,
    Booting {
        name: String,
        secs: u64,
    },
    /// Booted: the picture (or, before its first frame, a line).
    Live {
        name: String,
    },
    /// Shut down: outside Flare (`again`), or not booted at all since the panel was restored.
    ShutDown {
        name: String,
        again: bool,
    },
    /// A request failed: its one line.
    Failed {
        why: String,
    },
}

/// The button of a setup state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetupButton {
    GetXcode,
    OpenXcode,
    Boot,
    BootAgain,
    TryAgain,
}

impl SetupButton {
    pub fn label(self) -> &'static str {
        match self {
            SetupButton::GetXcode => "Get Xcode",
            SetupButton::OpenXcode => "Open Xcode",
            SetupButton::Boot => "Boot",
            SetupButton::BootAgain => "Boot again",
            SetupButton::TryAgain => "Try again",
        }
    }
}

/// How a setup state is drawn: the Slint state (0 the picture, 1 a line, 2 the list, 3 booting), its line and its
/// button.
#[derive(Clone, Debug, PartialEq)]
pub struct SetupView {
    pub state: i32,
    pub line: String,
    pub button: Option<SetupButton>,
}

impl Setup {
    /// The state's line and button. `pictured`: a live device has its first frame.
    pub fn view(&self, pictured: bool) -> SetupView {
        let v = |state, line: String, button| SetupView {
            state,
            line,
            button,
        };
        match self {
            Setup::NoXcode => v(
                1,
                "The device panel needs Xcode.".into(),
                Some(SetupButton::GetXcode),
            ),
            Setup::NotSetUp => v(
                1,
                "Open Xcode once to finish its setup.".into(),
                Some(SetupButton::OpenXcode),
            ),
            Setup::NoRuntime => v(
                1,
                "Xcode has no iPhone simulator yet. Add one in Xcode > Settings > Components."
                    .into(),
                Some(SetupButton::OpenXcode),
            ),
            Setup::Reading => v(1, "Looking for devices...".into(), None),
            Setup::Choose => v(2, "Choose a device.".into(), None),
            Setup::Booting { name, secs } => v(3, format!("Starting {name}... {secs} s"), None),
            Setup::Live { name } if !pictured => v(1, format!("Opening {name}..."), None),
            Setup::Live { .. } => v(0, String::new(), None),
            Setup::ShutDown { name, again: true } => v(
                1,
                format!("{name} was shut down."),
                Some(SetupButton::BootAgain),
            ),
            Setup::ShutDown { name, again: false } => {
                v(1, format!("{name} is shut down."), Some(SetupButton::Boot))
            }
            Setup::Failed { why } => v(1, why.clone(), Some(SetupButton::TryAgain)),
        }
    }

    /// The name `device.state` reports.
    pub fn name(&self) -> &'static str {
        match self {
            Setup::NoXcode => "no_xcode",
            Setup::NotSetUp => "not_set_up",
            Setup::NoRuntime => "no_runtime",
            Setup::Reading => "reading",
            Setup::Choose => "choose",
            Setup::Booting { .. } => "booting",
            Setup::Live { .. } => "live",
            Setup::ShutDown { .. } => "shut_down",
            Setup::Failed { .. } => "failed",
        }
    }
}

/// What the window does for the controller (it holds the panels and the openers).
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// The tab's device panel shows this device now (a pick in the list).
    SetTarget {
        tab: String,
        target: String,
    },
    /// The tab's device panel goes: its device no longer exists.
    Drop(String),
    OpenUrl(String),
    OpenXcode,
}

/// A request in flight.
#[derive(Clone, Debug)]
enum Pending {
    List,
    Boot(Target),
    Attach(Target),
    Shutdown(Target),
}

/// The newest CPU picture of a device not yet handed to Slint.
struct RawPicture {
    seq: u64,
    size_px: Size,
    rgba: Vec<u8>,
}

/// One device this controller attached (or is attaching).
#[derive(Default)]
struct Dev {
    screen: Option<Screen>,
    /// The picture on show (already turned) and its frame number.
    picture: Option<(u64, Image)>,
    raw: Option<RawPicture>,
    attaching: bool,
    attached: bool,
    paused: bool,
    input: Option<InputState>,
    /// The tab whose panel drives it; another tab showing it is view only.
    driver: Option<String>,
    changed: u64,
}

/// One tab's device panel.
#[derive(Default)]
struct TabDev {
    target: String,
    /// The device was live in this tab since the panel showed it: a shutdown then says "was shut down".
    seen_live: bool,
    /// The person asked for the list (More > Choose device).
    choosing: bool,
    failed: Option<String>,
}

/// A touch the person is holding down.
#[derive(Clone, Copy, Debug)]
struct Press {
    last: (f64, f64),
    pinch: bool,
    sent_ms: u64,
    pending: Option<(f64, f64)>,
}

/// The device source: the fake (`--e2e`, tests) or another (the simulator, PR 4b).
enum Src {
    Fake(Box<FakeSource>),
    Other(Box<dyn DeviceSource>),
}

impl Src {
    fn get(&mut self) -> &mut dyn DeviceSource {
        match self {
            Src::Fake(f) => f.as_mut(),
            Src::Other(s) => s.as_mut(),
        }
    }

    fn events(&self) -> &std::sync::mpsc::Receiver<SourceEvent> {
        match self {
            Src::Fake(f) => f.events(),
            Src::Other(s) => s.events(),
        }
    }
}

/// What the window showed last (to set only what changed).
#[derive(Default)]
struct Pushed {
    owned: bool,
    seq: Option<(String, u64)>,
    /// The device list's rows on show (name, system, booted): a new model only when they change, so a refresh
    /// between a press and its release on a row never rebuilds the rows under the pointer (and loses the click).
    rows: Vec<(String, String, bool)>,
}

pub struct DeviceCtl {
    post: Post,
    clock: Rc<dyn Fn() -> u64>,
    src: Option<Src>,
    xcode: Xcode,
    devices: Option<Vec<DeviceInfo>>,
    listing: bool,
    /// The list could not be read: why (the panel says it and offers Try again; nothing is dropped meanwhile).
    list_error: Option<String>,
    pending: HashMap<ReqId, Pending>,
    devs: HashMap<Target, Dev>,
    tabs: HashMap<String, TabDev>,
    /// When each device's boot began (ms on the clock), for the line's seconds.
    booting: HashMap<Target, u64>,
    /// The tab whose panel the window shows (front, open), if it is a device panel.
    shown: Option<String>,
    /// Tab labels, for "Driven from <tab>".
    names: HashMap<String, String>,
    splitting: bool,
    minimised: bool,
    occluded: bool,
    /// The source cannot give live frames: the still line's reason (rule 1312).
    broken: Option<String>,
    press: Option<Press>,
    pushed: Pushed,
    ticking: bool,
    /// An agent call running in a tab (PR 5 starts them): the person's input ends it with `interrupted`.
    jobs: HashMap<String, &'static str>,
    /// Calls the person's input ended, for PR 5 to answer.
    ended: Vec<(String, JobError)>,
    /// Counters for the `device.state` probe.
    frames_shown: u64,
    person_inputs: u64,
    takeovers: u64,
}

/// The sort of the device list: iPhones first, then the newest system, then the name, newest first.
fn sort_devices(list: &mut [DeviceInfo]) {
    list.sort_by(|a, b| {
        let iphone = |d: &DeviceInfo| d.kind != "iPhone";
        iphone(a)
            .cmp(&iphone(b))
            .then_with(|| version_of(&b.os).cmp(&version_of(&a.os)))
            .then_with(|| b.name.cmp(&a.name))
    });
}

/// "iOS 27.2" as numbers for the sort.
fn version_of(os: &str) -> Vec<u32> {
    os.rsplit(' ')
        .next()
        .unwrap_or("")
        .split('.')
        .map(|p| p.parse().unwrap_or(0))
        .collect()
}

/// A portrait RGBA picture turned to the interface's orientation (the framebuffer never turns, spike S5): its size
/// and pixels. `Rot90` turns it a quarter clockwise.
pub fn turn_rgba(w: usize, h: usize, rgba: &[u8], o: Orientation) -> (usize, usize, Vec<u8>) {
    if rgba.len() < w * h * 4 {
        return (0, 0, Vec::new());
    }
    if o == Orientation::Rot0 {
        return (w, h, rgba[..w * h * 4].to_vec());
    }
    let (nw, nh) = match o {
        Orientation::Rot90 | Orientation::Rot270 => (h, w),
        _ => (w, h),
    };
    let mut out = vec![0u8; nw * nh * 4];
    for y in 0..h {
        for x in 0..w {
            let (nx, ny) = match o {
                Orientation::Rot90 => (h - 1 - y, x),
                Orientation::Rot180 => (w - 1 - x, h - 1 - y),
                Orientation::Rot270 => (y, w - 1 - x),
                Orientation::Rot0 => (x, y),
            };
            let s = (y * w + x) * 4;
            let d = (ny * nw + nx) * 4;
            out[d..d + 4].copy_from_slice(&rgba[s..s + 4]);
        }
    }
    (nw, nh, out)
}

/// The HID usage of a named key as Slint names it (the arrows are its private-use characters).
fn named_usage(text: &str) -> Option<u32> {
    Some(match text {
        "\n" | "\r" => usage::RETURN,
        "\u{8}" | "\u{7f}" => usage::DELETE,
        "\t" => usage::TAB,
        "\u{1b}" => usage::ESCAPE,
        "\u{F700}" => usage::UP,
        "\u{F701}" => usage::DOWN,
        "\u{F702}" => usage::LEFT,
        "\u{F703}" => usage::RIGHT,
        "\u{F728}" => usage::FORWARD_DELETE,
        _ => return None,
    })
}

impl DeviceCtl {
    /// A controller over `fake` (the fake source under `--e2e` and in tests), or offering nothing (None).
    pub fn new(post: Post, clock: Rc<dyn Fn() -> u64>, fake: Option<FakeSource>) -> Self {
        Self {
            post,
            clock,
            src: fake.map(|f| Src::Fake(Box::new(f))),
            xcode: Xcode::Ready,
            devices: None,
            listing: false,
            list_error: None,
            pending: HashMap::new(),
            devs: HashMap::new(),
            tabs: HashMap::new(),
            booting: HashMap::new(),
            shown: None,
            names: HashMap::new(),
            splitting: false,
            minimised: false,
            occluded: false,
            broken: None,
            press: None,
            pushed: Pushed::default(),
            ticking: false,
            jobs: HashMap::new(),
            ended: Vec::new(),
            frames_shown: 0,
            person_inputs: 0,
            takeovers: 0,
        }
    }

    /// Another source (the simulator, PR 4b).
    #[allow(dead_code)] // PR 4b hands the simulator's source over
    pub fn set_source(&mut self, source: Box<dyn DeviceSource>) {
        self.src = Some(Src::Other(source));
    }

    /// This build offers the device panel (a source exists).
    pub fn offered(&self) -> bool {
        self.src.is_some()
    }

    /// The fake source, when it is the one in use (tests and the e2e probe).
    pub fn fake(&self) -> Option<&FakeSource> {
        match &self.src {
            Some(Src::Fake(f)) => Some(f),
            _ => None,
        }
    }

    #[allow(dead_code)] // tests drive the fake
    pub fn fake_mut(&mut self) -> Option<&mut FakeSource> {
        match &mut self.src {
            Some(Src::Fake(f)) => Some(f),
            _ => None,
        }
    }

    /// What Xcode can do here (the worker's answer in PR 4b; tests set it).
    #[allow(dead_code)] // PR 4b's Xcode pick
    pub fn set_xcode(&mut self, x: Xcode) {
        self.xcode = x;
    }

    pub fn set_minimised(&mut self, on: bool) {
        self.minimised = on;
    }

    pub fn set_occluded(&mut self, on: bool) {
        self.occluded = on;
    }

    /// The tab whose device panel is on show.
    pub fn shown(&self) -> Option<&str> {
        self.shown.as_deref()
    }

    fn now(&self) -> u64 {
        (self.clock)()
    }

    fn source(&mut self) -> Option<&mut dyn DeviceSource> {
        self.src.as_mut().map(Src::get)
    }

    // ---- the model ----------------------------------------------------------------------------------

    /// Follows the panels: which tabs show which device, which device drives from where, attach the front one and
    /// pause the rest. `front` is the front shell tab. Returns what the window must do (a panel whose device is gone
    /// is dropped once the list is read).
    pub fn update(
        &mut self,
        panels: &PanelCtl,
        front: Option<&str>,
        splitting: bool,
        names: HashMap<String, String>,
    ) -> Vec<Effect> {
        self.names = names;
        self.drain();
        let list = panels.device_panels();
        let live: HashSet<&str> = list.iter().map(|(t, ..)| t.as_str()).collect();
        self.tabs.retain(|t, _| live.contains(t.as_str()));
        self.jobs.retain(|t, _| live.contains(t.as_str()));
        for (tab, target, _) in &list {
            let t = self.tabs.entry(tab.clone()).or_default();
            if t.target != *target {
                *t = TabDev {
                    target: target.clone(),
                    ..TabDev::default()
                };
            }
        }
        self.shown = front
            .filter(|f| list.iter().any(|(t, _, open)| t == f && *open))
            .map(str::to_string);
        self.splitting = splitting && self.shown.is_some();
        if self.shown.is_none() {
            self.press = None;
        }
        // The list is read once a device panel exists (on a worker for the simulator).
        if !list.is_empty() && self.devices.is_none() && !self.listing && self.list_error.is_none()
        {
            self.list();
        }
        let mut effects = Vec::new();
        // A panel whose device no longer exists goes, once the list says so.
        if let Some(devices) = &self.devices {
            for (tab, target, _) in &list {
                if let Some(t) = target::parse(target)
                    && !devices.iter().any(|d| d.target == t)
                {
                    effects.push(Effect::Drop(tab.clone()));
                }
            }
        }
        // Who drives each device: the tab that showed it first, while its panel still shows it.
        let showing: HashMap<&str, Target> = list
            .iter()
            .filter_map(|(tab, target, _)| Some((tab.as_str(), target::parse(target)?)))
            .collect();
        // A driver whose panel closed or shows another device lets go.
        let open_on: HashMap<&str, Target> = list
            .iter()
            .filter(|(_, _, open)| *open)
            .filter_map(|(tab, target, _)| Some((tab.as_str(), target::parse(target)?)))
            .collect();
        for (t, d) in &mut self.devs {
            if d.driver
                .as_deref()
                .is_some_and(|tab| open_on.get(tab) != Some(t))
            {
                d.driver = None;
            }
        }
        let shown_target = self.shown.as_deref().and_then(|s| showing.get(s).cloned());
        // Devices no panel shows any more are let go.
        let wanted: HashSet<Target> = showing.values().cloned().collect();
        let gone: Vec<Target> = self
            .devs
            .keys()
            .filter(|t| !wanted.contains(*t))
            .cloned()
            .collect();
        for t in gone {
            if let Some(s) = self.source() {
                s.detach(&t);
            }
            self.devs.remove(&t);
        }
        // The front panel's device: attach once it is booted; it drives from this tab unless another does.
        if let Some(t) = &shown_target {
            let booted = self.state_of(t) == Some(DeviceState::Booted);
            let tab = self.shown.clone().unwrap_or_default();
            let dev = self.devs.entry(t.clone()).or_default();
            if dev.driver.is_none() {
                dev.driver = Some(tab.clone());
            }
            if booted && !dev.attached && !dev.attaching && self.src.is_some() {
                dev.attaching = true;
                if let Some(id) = self.source().map(|s| s.attach(t)) {
                    self.pending.insert(id, Pending::Attach(t.clone()));
                }
            }
        }
        // Only the front panel's device streams (rule 1311's pause): the others, and every device while the window
        // is minimised or occluded, are paused. A device that resumes shows its current frame at once.
        let hidden = self.minimised || self.occluded;
        let mut changes = Vec::new();
        for (t, d) in &mut self.devs {
            if !d.attached {
                continue;
            }
            let pause = hidden || shown_target.as_ref() != Some(t);
            if pause != d.paused {
                d.paused = pause;
                changes.push((t.clone(), pause));
            }
        }
        for (t, pause) in changes {
            if let Some(s) = self.source() {
                s.pause(&t, pause);
            }
        }
        self.drain();
        effects
    }

    fn list(&mut self) {
        let Some(id) = self.source().map(|s| s.list()) else {
            return;
        };
        self.listing = true;
        self.pending.insert(id, Pending::List);
        self.drain();
    }

    fn state_of(&self, t: &Target) -> Option<DeviceState> {
        self.devices
            .as_ref()?
            .iter()
            .find(|d| &d.target == t)
            .map(|d| d.state)
    }

    fn info_of(&self, t: &Target) -> Option<&DeviceInfo> {
        self.devices.as_ref()?.iter().find(|d| &d.target == t)
    }

    /// Reads every event the source has (the fake answers at once; the simulator's reader wakes the window).
    pub fn drain(&mut self) {
        loop {
            let Some(ev) = self.src.as_ref().and_then(|s| s.events().try_recv().ok()) else {
                return;
            };
            self.on_event(ev);
        }
    }

    fn on_event(&mut self, ev: SourceEvent) {
        match ev {
            SourceEvent::Answer { id, result } => {
                let Some(p) = self.pending.remove(&id) else {
                    return;
                };
                match (p, result) {
                    (Pending::List, Ok(Answer::Devices(mut d))) => {
                        sort_devices(&mut d);
                        self.devices = Some(d);
                        self.listing = false;
                    }
                    (Pending::List, Ok(_)) => self.listing = false,
                    (Pending::List, Err(e)) => {
                        self.listing = false;
                        log::warn!("device list: {}", e.message);
                        self.list_error = Some(e.message);
                    }
                    (Pending::Attach(t), r) => {
                        if let Some(d) = self.devs.get_mut(&t) {
                            d.attaching = false;
                            match r {
                                Ok(_) => {
                                    d.attached = true;
                                    d.paused = false;
                                }
                                Err(e) => {
                                    log::info!("attach {t}: {}", e.message);
                                    self.fail(&t, e.message);
                                }
                            }
                        }
                    }
                    (Pending::Boot(t), r) => {
                        self.booting.remove(&t);
                        if let Err(e) = r {
                            self.fail(&t, e.message);
                        }
                    }
                    (Pending::Shutdown(t), r) => {
                        if let Err(e) = r {
                            self.fail(&t, e.message);
                        }
                    }
                }
            }
            SourceEvent::Frames { target, screen, .. } => {
                self.devs.entry(target).or_default().screen = Some(screen);
            }
            SourceEvent::Orientation {
                target,
                orientation,
            } => {
                if let Some(s) = self.devs.get_mut(&target).and_then(|d| d.screen.as_mut()) {
                    s.orientation = orientation;
                }
            }
            SourceEvent::Picture {
                target,
                seq,
                size_px,
                rgba,
            } => {
                if let Some(d) = self.devs.get_mut(&target) {
                    d.raw = Some(RawPicture { seq, size_px, rgba });
                }
            }
            // The ring's frames are PR 4b's (the zero-copy path).
            SourceEvent::Frame { .. } => {}
            SourceEvent::Changed { target, count } => {
                if let Some(d) = self.devs.get_mut(&target) {
                    d.changed = count;
                }
            }
            SourceEvent::State { target, state } => {
                if let Some(list) = self.devices.as_mut()
                    && let Some(d) = list.iter_mut().find(|d| d.target == target)
                {
                    d.state = state;
                }
                match state {
                    DeviceState::Booted => {
                        self.booting.remove(&target);
                    }
                    DeviceState::Booting => {
                        let now = self.now();
                        self.booting.entry(target).or_insert(now);
                    }
                    DeviceState::Shutdown | DeviceState::ShuttingDown => {
                        self.booting.remove(&target);
                        if let Some(d) = self.devs.get_mut(&target) {
                            d.attached = false;
                            d.attaching = false;
                            d.picture = None;
                            d.raw = None;
                            d.input = None;
                        }
                    }
                }
            }
            SourceEvent::Input { target, state } => {
                if let Some(d) = self.devs.get_mut(&target) {
                    d.input = Some(state);
                }
            }
            SourceEvent::Broken(why) => self.broken = Some(why),
        }
    }

    /// A request about `t` failed: the tabs that show it say why.
    fn fail(&mut self, t: &Target, why: String) {
        for tab in self.tabs.values_mut() {
            if target::parse(&tab.target).as_ref() == Some(t) {
                tab.failed = Some(why.clone());
            }
        }
    }

    /// What the tab's device panel shows now.
    pub fn setup_of(&self, tab: &str) -> Option<Setup> {
        let t = self.tabs.get(tab)?;
        match self.xcode {
            Xcode::Missing => return Some(Setup::NoXcode),
            Xcode::NotSetUp => return Some(Setup::NotSetUp),
            Xcode::Ready => {}
        }
        if let Some(why) = t.failed.as_ref().or(self.list_error.as_ref()) {
            return Some(Setup::Failed { why: why.clone() });
        }
        let Some(devices) = &self.devices else {
            return Some(Setup::Reading);
        };
        let target = target::parse(&t.target);
        if t.choosing || target.is_none() {
            return Some(if devices.is_empty() {
                Setup::NoRuntime
            } else {
                Setup::Choose
            });
        }
        let target = target?;
        let Some(d) = devices.iter().find(|d| d.target == target) else {
            // Gone: the panel is dropped at the next update.
            return Some(Setup::Reading);
        };
        let name = d.name.clone();
        Some(match d.state {
            DeviceState::Booting => Setup::Booting {
                secs: self
                    .booting
                    .get(&target)
                    .map_or(0, |at| self.now().saturating_sub(*at) / 1000),
                name,
            },
            DeviceState::Booted => Setup::Live { name },
            DeviceState::Shutdown | DeviceState::ShuttingDown => Setup::ShutDown {
                name,
                again: t.seen_live,
            },
        })
    }

    /// The device the tab's panel shows, if it names one.
    fn target_of(&self, tab: &str) -> Option<Target> {
        target::parse(&self.tabs.get(tab)?.target)
    }

    /// The shown tab drives its device (not view only), and it is live with a picture.
    fn drives(&self, tab: &str) -> Option<Target> {
        let t = self.target_of(tab)?;
        let d = self.devs.get(&t)?;
        (d.driver.as_deref() == Some(tab) && d.attached && self.broken.is_none()).then_some(t)
    }

    /// The screen of the shown device in points, in its current orientation.
    fn screen_pt(&self, t: &Target) -> Option<Size> {
        let s = self.devs.get(t)?.screen?;
        Some(s.orientation.size(s.size_pt))
    }

    // ---- the window -----------------------------------------------------------------------------------

    /// Pushes the front tab's device panel into the window's `panel-device-*` properties (it owns `panel-kind` 5
    /// while one is on show), and hands its newest picture to Slint.
    pub fn push(&mut self, app: &AppWindow) {
        self.sync_tick();
        let Some(tab) = self.shown.clone() else {
            if self.pushed.owned {
                self.pushed = Pushed::default();
                if app.get_panel_kind() == 5 {
                    app.set_panel_kind(0);
                    app.set_panel_title("".into());
                    app.set_panel_sub("".into());
                }
                app.set_panel_device_frame(Image::default());
                app.set_panel_device_has_frame(false);
                app.set_panel_device_rows(ModelRc::default());
                app.set_panel_device_controls(false);
                app.set_panel_device_still("".into());
                app.set_panel_device_slow("".into());
                app.set_panel_device_foot("".into());
            }
            return;
        };
        let Some(setup) = self.setup_of(&tab) else {
            return;
        };
        let target = self.target_of(&tab);
        // The newest picture of the shown device, turned and handed over (the CPU path).
        let mut pictured = false;
        if let Some(t) = &target
            && let Some(d) = self.devs.get_mut(t)
        {
            if let Some(raw) = d.raw.take() {
                let o = d.screen.map(|s| s.orientation).unwrap_or_default();
                let (w, h, px) =
                    turn_rgba(raw.size_px.w as usize, raw.size_px.h as usize, &raw.rgba, o);
                if w > 0 {
                    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(w as u32, h as u32);
                    buf.make_mut_bytes().copy_from_slice(&px);
                    d.picture = Some((raw.seq, Image::from_rgba8(buf)));
                }
            }
            pictured = d.picture.is_some() && d.attached;
            if let Some((seq, img)) = &d.picture
                && self.pushed.seq.as_ref() != Some(&(tab.clone(), *seq))
            {
                app.set_panel_device_frame(img.clone());
                self.pushed.seq = Some((tab.clone(), *seq));
                self.frames_shown += 1;
            }
        }
        if pictured && let Some(t) = self.tabs.get_mut(&tab) {
            t.seen_live = true;
        }
        let view = setup.view(pictured);
        self.pushed.owned = true;
        app.set_panel_kind(5);
        let info = target.as_ref().and_then(|t| self.info_of(t));
        app.set_panel_title(info.map_or("Device", |d| d.name.as_str()).into());
        app.set_panel_sub(info.map_or("", |d| d.os.as_str()).into());
        app.set_panel_note("".into());
        app.set_panel_device_state(view.state);
        app.set_panel_device_line(view.line.into());
        app.set_panel_device_button(view.button.map_or("", SetupButton::label).into());
        app.set_panel_device_has_frame(pictured);
        if !pictured {
            app.set_panel_device_frame(Image::default());
            self.pushed.seq = None;
        }
        let pt = target.as_ref().and_then(|t| self.screen_pt(t));
        app.set_panel_device_pt_w(pt.map_or(0.0, |s| s.w as f32));
        app.set_panel_device_pt_h(pt.map_or(0.0, |s| s.h as f32));
        app.set_panel_device_radius_pt(RADIUS_PT as f32);
        let rows: Vec<(String, String, bool)> = if view.state == 2 {
            self.devices
                .iter()
                .flatten()
                .map(|d| (d.name.clone(), d.os.clone(), d.state == DeviceState::Booted))
                .collect()
        } else {
            Vec::new()
        };
        if rows != self.pushed.rows {
            let model: Vec<DeviceRow> = rows
                .iter()
                .map(|(name, os, booted)| DeviceRow {
                    name: name.as_str().into(),
                    os: os.as_str().into(),
                    booted: *booted,
                })
                .collect();
            app.set_panel_device_rows(ModelRc::from(Rc::new(VecModel::from(model))));
            self.pushed.rows = rows;
        }
        let live = view.state == 0;
        let still = match (&self.broken, live) {
            (Some(why), true) => {
                format!("Live view unavailable: {why}. Showing a picture every half second.")
            }
            _ => String::new(),
        };
        app.set_panel_device_still(still.into());
        app.set_panel_device_slow(
            if live && self.broken.is_none() {
                CPU_SLOW
            } else {
                ""
            }
            .into(),
        );
        app.set_panel_device_foot(self.foot(&tab).into());
        app.set_panel_device_controls(live && self.drives(&tab).is_some());
    }

    /// The line along the bottom of a live picture: view only, or the touch connection.
    fn foot(&self, tab: &str) -> String {
        let Some(t) = self.target_of(tab) else {
            return String::new();
        };
        let Some(d) = self.devs.get(&t) else {
            return String::new();
        };
        if let Some(driver) = d.driver.as_deref().filter(|d| *d != tab) {
            let name = self.names.get(driver).map_or("another tab", String::as_str);
            return format!("Driven from \"{name}\". View only here.");
        }
        match &d.input {
            Some(InputState::Connecting) => "Connecting touch...".into(),
            Some(InputState::Lost) => "Touch connection lost. Connecting again...".into(),
            Some(InputState::Broken { why }) => format!("Touch is unavailable: {why}"),
            _ => String::new(),
        }
    }

    /// The boot's seconds tick only while the front panel shows a boot.
    fn sync_tick(&mut self) {
        let want = self
            .shown
            .as_deref()
            .and_then(|t| self.setup_of(t))
            .is_some_and(|s| matches!(s, Setup::Booting { .. }));
        if want == self.ticking {
            return;
        }
        self.ticking = want;
        if want {
            let post = self.post.clone();
            crate::timer_registry::set(BOOT_TICK, 1_000, true, move || {
                post(crate::appctl::UiMsg::Device(DeviceMsg::Tick));
            });
        } else {
            crate::timer_registry::clear(BOOT_TICK);
        }
    }

    // ---- the person's input -------------------------------------------------------------------------

    /// The person pressed, typed or scrolled in the tab's device panel: a running agent call there ends with
    /// `interrupted` (takeover, rule 1320). Not while the splitter is held.
    pub fn person_input(&mut self, tab: &str) {
        if self.splitting {
            return;
        }
        self.person_inputs += 1;
        if self.jobs.remove(tab).is_some() {
            self.takeovers += 1;
            self.ended
                .push((tab.to_string(), flare_device::job::Job::interrupted()));
        }
    }

    /// An agent call runs in the tab (PR 5 starts them; tests use it to see the takeover).
    #[allow(dead_code)] // PR 5's agent jobs
    pub fn job_started(&mut self, tab: &str, op: &'static str) {
        self.jobs.insert(tab.to_string(), op);
    }

    /// The calls the person's input ended since the last look.
    #[allow(dead_code)] // PR 5 answers them
    pub fn take_ended(&mut self) -> Vec<(String, JobError)> {
        std::mem::take(&mut self.ended)
    }

    /// The box's letterbox of the shown device.
    fn letterbox(&self, app: &AppWindow, t: &Target) -> Option<Letterbox> {
        let size = Size::new(
            f64::from(app.get_panel_device_w()),
            f64::from(app.get_panel_device_h()),
        );
        Letterbox::fit(size, self.screen_pt(t)?)
    }

    /// The shown device the person may drive, and its letterbox.
    fn input_target(&self, app: &AppWindow) -> Option<(String, Target, Letterbox)> {
        let tab = self.shown.clone()?;
        let t = self.drives(&tab)?;
        let lb = self.letterbox(app, &t)?;
        Some((tab, t, lb))
    }

    fn send_touch(
        &mut self,
        t: &Target,
        phase: Phase,
        at: (f64, f64),
        pinch: bool,
        lb: &Letterbox,
    ) {
        let second = pinch.then_some((lb.device.w - at.0, lb.device.h - at.1));
        let touch = Touch {
            phase,
            x: at.0,
            y: at.1,
            second,
        };
        if let Some(s) = self.source() {
            s.touch(t, touch);
        }
    }

    /// The pointer over the device box (kind: 0 down, 1 up, 2 move, 3 leave; button 0 left; mods 4 alt). A press
    /// becomes a touch start in device points, moves while held a move (at most one per frame), the release its end.
    /// A press outside the screen (in the bars) is dropped, and so is a right click.
    pub fn page_pointer(
        &mut self,
        app: &AppWindow,
        kind: i32,
        x: f32,
        y: f32,
        button: i32,
        mods: i32,
    ) {
        let Some((tab, t, lb)) = self.input_target(app) else {
            return;
        };
        let at = lb.to_device(f64::from(x), f64::from(y));
        let now = self.now();
        match kind {
            0 => {
                if button != 0 {
                    return;
                }
                let Some(at) = at else { return };
                self.person_input(&tab);
                let pinch = mods & 4 != 0;
                self.press = Some(Press {
                    last: at,
                    pinch,
                    sent_ms: now,
                    pending: None,
                });
                self.send_touch(&t, Phase::Start, at, pinch, &lb);
            }
            2 => {
                let Some(mut p) = self.press else { return };
                let Some(at) = at else { return };
                p.last = at;
                if now.saturating_sub(p.sent_ms) >= MOVE_MS {
                    p.sent_ms = now;
                    p.pending = None;
                    self.send_touch(&t, Phase::Move, at, p.pinch, &lb);
                } else {
                    p.pending = Some(at);
                }
                self.press = Some(p);
            }
            1 | 3 => {
                if kind == 1 && button != 0 {
                    return;
                }
                let Some(p) = self.press.take() else { return };
                if let Some(m) = p.pending {
                    self.send_touch(&t, Phase::Move, m, p.pinch, &lb);
                }
                let end = if kind == 1 {
                    at.unwrap_or(p.last)
                } else {
                    p.last
                };
                self.send_touch(&t, Phase::End, end, p.pinch, &lb);
            }
            _ => {}
        }
        self.drain();
    }

    /// A wheel or trackpad turn over the device box: a short drag at the pointer (start, moves, end), the content
    /// following the fingers as on the device. Outside the screen it is dropped.
    pub fn page_wheel(&mut self, app: &AppWindow, x: f32, y: f32, dx: f32, dy: f32) {
        let Some((tab, t, lb)) = self.input_target(app) else {
            return;
        };
        let Some(from) = lb.to_device(f64::from(x), f64::from(y)) else {
            return;
        };
        if self.press.is_some() || (dx == 0.0 && dy == 0.0) {
            return;
        }
        self.person_input(&tab);
        let clamp = |v: f64, max: f64| v.clamp(0.0, max);
        let to = (
            clamp(from.0 + f64::from(dx) / lb.scale, lb.device.w),
            clamp(from.1 + f64::from(dy) / lb.scale, lb.device.h),
        );
        self.send_touch(&t, Phase::Start, from, false, &lb);
        for i in 1..=WHEEL_STEPS {
            let k = f64::from(i) / f64::from(WHEEL_STEPS);
            let at = (from.0 + (to.0 - from.0) * k, from.1 + (to.1 - from.1) * k);
            self.send_touch(&t, Phase::Move, at, false, &lb);
        }
        self.send_touch(&t, Phase::End, to, false, &lb);
        self.drain();
    }

    /// A key typed into the focused device panel: a HID key with the US keymap, or (text the keymap cannot type)
    /// through the simulator's own pasteboard, never the Mac's (rule 1314). Each press is a whole key.
    pub fn page_key(&mut self, down: bool, text: &str, _mods: i32, _repeat: bool) {
        let Some(tab) = self.shown.clone() else {
            return;
        };
        let Some(t) = self.drives(&tab) else { return };
        if !down || text.is_empty() {
            return;
        }
        let keys: Vec<Key> = if let Some(u) = named_usage(text) {
            vec![
                Key {
                    usage: u,
                    down: true,
                },
                Key {
                    usage: u,
                    down: false,
                },
            ]
        } else {
            // Modifier keys alone and the other private-use keys (F1 ...) type nothing.
            if text
                .chars()
                .any(|c| c.is_control() || ('\u{E000}'..='\u{F8FF}').contains(&c))
            {
                return;
            }
            match keymap::typing(text) {
                keymap::Typing::Keys(k) => k.into_iter().flat_map(keymap::events).collect(),
                keymap::Typing::Paste(s) => {
                    self.person_input(&tab);
                    if let Some(src) = self.source() {
                        src.text(&t, &s);
                    }
                    self.drain();
                    return;
                }
            }
        };
        self.person_input(&tab);
        for k in keys {
            if let Some(s) = self.source() {
                s.key(&t, k);
            }
        }
        self.drain();
    }

    /// A header button or a More menu row.
    pub fn act(&mut self, name: &str) -> Vec<Effect> {
        let Some(tab) = self.shown.clone() else {
            return Vec::new();
        };
        match name {
            "choose" => {
                if let Some(t) = self.tabs.get_mut(&tab) {
                    t.choosing = true;
                }
                // The list is read again: a device may have come or gone.
                self.list();
            }
            "shutdown" => {
                if let Some(t) = self.target_of(&tab)
                    && let Some(id) = self.source().map(|s| s.shutdown(&t))
                {
                    self.pending.insert(id, Pending::Shutdown(t));
                }
            }
            other => {
                let Some(b) = Button::parse(other) else {
                    return Vec::new();
                };
                let Some(t) = self.drives(&tab) else {
                    return Vec::new();
                };
                self.person_input(&tab);
                if let Some(s) = self.source() {
                    s.button(&t, b);
                }
            }
        }
        self.drain();
        Vec::new()
    }

    /// The setup state's one button.
    pub fn setup_clicked(&mut self) -> Vec<Effect> {
        let Some(tab) = self.shown.clone() else {
            return Vec::new();
        };
        let Some(button) = self.setup_of(&tab).and_then(|s| s.view(false).button) else {
            return Vec::new();
        };
        match button {
            SetupButton::GetXcode => return vec![Effect::OpenUrl(XCODE_STORE.into())],
            SetupButton::OpenXcode => return vec![Effect::OpenXcode],
            SetupButton::Boot | SetupButton::BootAgain => {
                if let Some(t) = self.target_of(&tab) {
                    self.boot(&t);
                }
            }
            SetupButton::TryAgain => {
                if let Some(t) = self.tabs.get_mut(&tab) {
                    t.failed = None;
                }
                self.list_error = None;
                if let Some(t) = self.target_of(&tab)
                    && let Some(d) = self.devs.get_mut(&t)
                {
                    d.attaching = false;
                }
                self.list();
            }
        }
        self.drain();
        Vec::new()
    }

    /// Boots a device the person asked for (never on restore, never on its own: rule 1317's half here).
    fn boot(&mut self, t: &Target) {
        let now = self.now();
        self.booting.insert(t.clone(), now);
        if let Some(id) = self.source().map(|s| s.boot(t)) {
            self.pending.insert(id, Pending::Boot(t.clone()));
        }
    }

    /// The person picked row `i` of the list: the panel shows that device, booted if it is not.
    pub fn pick(&mut self, i: i32) -> Vec<Effect> {
        let Some(tab) = self.shown.clone() else {
            return Vec::new();
        };
        let Some(d) = usize::try_from(i)
            .ok()
            .and_then(|i| self.devices.as_ref()?.get(i))
            .cloned()
        else {
            return Vec::new();
        };
        if let Some(t) = self.tabs.get_mut(&tab) {
            t.choosing = false;
        }
        if d.state == DeviceState::Shutdown {
            self.boot(&d.target);
        }
        self.drain();
        vec![Effect::SetTarget {
            tab,
            target: d.target.name(),
        }]
    }

    /// A message for the controller (the window's turn follows and pushes).
    pub fn on_msg(&mut self, _msg: DeviceMsg) {
        self.drain();
    }

    // ---- the probe ------------------------------------------------------------------------------------

    /// What `device.state` reports: the shown panel's state, its device, the counters and the fake's inputs.
    pub fn probe(&self) -> serde_json::Value {
        use serde_json::json;
        let tab = self.shown.as_deref();
        let setup = tab.and_then(|t| self.setup_of(t));
        let target = tab.and_then(|t| self.target_of(t));
        let dev = target.as_ref().and_then(|t| self.devs.get(t));
        let screen = target.as_ref().and_then(|t| self.screen_pt(t));
        let inputs: Vec<serde_json::Value> = self
            .fake()
            .map(|f| f.inputs())
            .unwrap_or_default()
            .iter()
            .map(|r| match r {
                flare_device::fake::Recorded::Touch(_, t) => {
                    json!({"touch": t.phase, "x": t.x, "y": t.y})
                }
                flare_device::fake::Recorded::Key(_, k) => json!({"key": k.usage, "down": k.down}),
                flare_device::fake::Recorded::Button(_, b) => json!({"button": b.name()}),
                flare_device::fake::Recorded::Text(_, s) => json!({"text": s}),
                flare_device::fake::Recorded::Pause(_, p) => json!({"pause": p}),
                other => json!({"other": format!("{other:?}")}),
            })
            .collect();
        json!({
            "offered": self.offered(),
            "tab": tab,
            "state": setup.as_ref().map(Setup::name),
            "target": target.map(|t| t.name()),
            "attached": dev.is_some_and(|d| d.attached),
            "paused": dev.is_some_and(|d| d.paused),
            "driver": dev.and_then(|d| d.driver.clone()),
            "screen": screen.map(|s| json!({"w": s.w, "h": s.h})),
            "frames_shown": self.frames_shown,
            "person_inputs": self.person_inputs,
            "takeovers": self.takeovers,
            "devices": self.devices.as_ref().map_or(0, Vec::len),
            "inputs": inputs,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_setup_state_has_one_line_and_at_most_one_button() {
        let name = || "iPhone 18 Pro".to_string();
        let all = [
            Setup::NoXcode,
            Setup::NotSetUp,
            Setup::NoRuntime,
            Setup::Reading,
            Setup::Choose,
            Setup::Booting {
                name: name(),
                secs: 3,
            },
            Setup::Live { name: name() },
            Setup::ShutDown {
                name: name(),
                again: true,
            },
            Setup::ShutDown {
                name: name(),
                again: false,
            },
            Setup::Failed {
                why: "the device is still booting".into(),
            },
        ];
        for s in all {
            let v = s.view(false);
            assert!(!v.line.is_empty(), "{s:?} says a line");
            assert!(!v.line.contains('\n'), "{s:?}: one line");
        }
        assert_eq!(
            Setup::NoXcode.view(false).button,
            Some(SetupButton::GetXcode)
        );
        assert_eq!(
            Setup::Booting {
                name: name(),
                secs: 12
            }
            .view(false)
            .line,
            "Starting iPhone 18 Pro... 12 s"
        );
        assert_eq!(Setup::Live { name: name() }.view(true).state, 0);
    }

    #[test]
    fn a_build_with_no_source_offers_no_device_panel() {
        // A release build of PR 4a (no `--e2e`): nothing new shows (rule 1318).
        let post: Post = std::sync::Arc::new(|_| {});
        let d = DeviceCtl::new(post.clone(), Rc::new(|| 0), None);
        assert!(!d.offered());
        assert_eq!(d.probe()["offered"], false);
        assert!(DeviceCtl::new(post, Rc::new(|| 0), Some(FakeSource::default())).offered());
    }

    #[test]
    fn a_picture_turns_with_the_interface() {
        // 2 x 1: red, green.
        let px = [255, 0, 0, 255, 0, 255, 0, 255];
        let (w, h, out) = turn_rgba(2, 1, &px, Orientation::Rot90);
        assert_eq!((w, h), (1, 2));
        // A quarter clockwise: the left pixel goes to the top.
        assert_eq!(&out[..4], &[255, 0, 0, 255]);
        let (w, h, out) = turn_rgba(2, 1, &px, Orientation::Rot270);
        assert_eq!((w, h), (1, 2));
        assert_eq!(&out[..4], &[0, 255, 0, 255]);
        let (_, _, out) = turn_rgba(2, 1, &px, Orientation::Rot180);
        assert_eq!(&out[..4], &[0, 255, 0, 255]);
        assert_eq!(turn_rgba(2, 2, &px, Orientation::Rot0).0, 0, "too short");
    }

    #[test]
    fn the_list_puts_iphones_and_the_newest_first() {
        let d = |name: &str, kind: &str, os: &str, n: u8| DeviceInfo {
            target: Target::sim(&format!("00000000-0000-4000-8000-0000000000{n:02x}")).unwrap(),
            name: name.into(),
            kind: kind.into(),
            os: os.into(),
            state: DeviceState::Shutdown,
            booted_by_flare: false,
        };
        let mut list = vec![
            d("iPad Pro", "iPad", "iPadOS 27.2", 1),
            d("iPhone 17", "iPhone", "iOS 26.4", 2),
            d("iPhone 18", "iPhone", "iOS 27.2", 3),
            d("iPhone 18 Pro", "iPhone", "iOS 27.2", 4),
        ];
        sort_devices(&mut list);
        let names: Vec<&str> = list.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(
            names,
            ["iPhone 18 Pro", "iPhone 18", "iPhone 17", "iPad Pro"]
        );
    }
}

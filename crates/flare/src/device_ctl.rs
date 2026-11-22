//! DeviceCtl: the device panel (issue #248, kind 5), the person's half (device panel PR 4a).
//!
//! The model of what each tab's device panel shows: its setup state (no Xcode, not set up, no runtime, the device
//! list, booting, live, still, shut down outside Flare; rule 1318), the device source behind `dyn DeviceSource`, which
//! tab drives a device (a second tab on the same device is view only), and the person's pointer, wheel and keys over
//! the picture as touches in device points. The source answers through its event channel (rule 34); every request
//! here returns at once.
//!
//! Frames: the real simulator's go through the zero-copy ring (`DeviceFrames`, PR 4b, rule 1311), shown by the render
//! notifier; the controller only says which device the window shows and turns the picture by the interface's
//! orientation (`panel-device-turn`). A source's CPU picture (the fake source, and the still mode's screenshots) is
//! turned here and handed to Slint as a `SharedPixelBuffer`, and the header wears the "slow" mark, as it does when the
//! ring's import is refused. Only the front tab's open panel is live; every other attached device is paused (the tab
//! behind, the panel closed, the window minimised or occluded) and resumes with its current frame.
//!
//! The device kind is offered only where a source exists: the fake source under `--e2e` and in tests (PR 4a), the
//! simulator on macOS on Apple silicon (PR 4b), started by the first device panel once the Xcode pick (a worker) found
//! an Xcode, and stopped two minutes after the last device panel goes. A build that offers none keeps a stored device
//! panel in the file and shows nothing (rule 1316). Restore never boots a device: it shows Boot. At quit, Flare shuts
//! down the simulators it booted (rule 1319), remembered in `<profile>/device/booted.json`, with one detached
//! `simctl shutdown` it never waits for.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[path = "device_agent.rs"]
mod agent;

pub use agent::{AgentCall, AgentReq, REMOTE_WORDS, Reply};
use agent::{AgentEvent, AgentWait};

use flare_device::fake::FakeSource;
use flare_device::geom::{Letterbox, Orientation, Size};
use flare_device::keymap::{self, usage};
use flare_device::source::{
    Answer, Button, DeviceInfo, DeviceSource, DeviceState, InputState, ReqId, Screen, SourceEvent,
    Touch,
};
use flare_device::source::{Key, Phase};
use flare_device::target::{self, Target};
use slint::{Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel};

use crate::appctl::Post;
use crate::device_frames::{DeviceFrames, FramePath};
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

/// The timer that stops the sim host once no device panel is left for [`IDLE_MS`].
pub const IDLE_TICK: &str = "device.idle";

/// The sim host stops this long after the last device panel goes (spec "Parts").
pub const IDLE_MS: u64 = 2 * 60 * 1000;

/// The timer that reads the list again while Xcode still needs its setup (open check 5).
pub const RELIST_TICK: &str = "device.relist";

/// While Xcode still needs its first launch or licence, the list is read again at most this often (never in a loop:
/// each failed list's answer refreshes the window, which would ask for the list at once).
pub const NOT_SET_UP_RETRY_MS: u64 = 5_000;

/// The words `simctl` uses when Xcode still needs its first launch or its licence (open check 5): the list fails with
/// one of them, and the panel then says "Open Xcode once to finish its setup."
const NOT_SET_UP_WORDS: &[&str] = &[
    "runfirstlaunch",
    "first launch",
    "license",
    "licence",
    "xcodebuild -license",
];

#[cfg_attr(not(target_os = "macos"), allow(dead_code))] // only the macOS install uses it
/// What the window asks of the device controller.
#[derive(Debug)]
pub enum DeviceMsg {
    /// The source has events waiting (its threads' wake, at most one queued).
    Wake,
    /// A second passed while the front panel shows a boot.
    Tick,
    /// The Xcode pick answered (a worker, at startup).
    Xcode(XcodeAnswer),
    /// The agent's jobs and indicators move on (every 16 ms while one runs or moves, PR 5).
    AgentTick,
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))] // only the macOS install uses it
/// The Xcode pick's answer: the Xcode found, and the simulators Flare booted in an earlier run (`booted.json`).
#[derive(Debug, Default)]
pub struct XcodeAnswer {
    pub xcode: Option<flare_device::xcode::Xcode>,
    pub booted: Booted,
}

/// What a source message asks of the window: nothing, a redraw (only frames came), or a refresh (the panel changed).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Woken {
    Nothing,
    Redraw,
    Refresh,
    /// Only the agent's indicators moved: the window sets them and nothing else (PR 5).
    Indicators,
}

/// The simulators Flare booted itself (rule 1319): only these are shut down at quit, also after a crash, since the
/// list lives in `<profile>/device/booted.json`. An entry goes when its device is seen shut down or is gone.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Booted {
    /// udid (lower case) and when Flare booted it (Unix ms).
    pub devices: Vec<(String, u64)>,
}

impl Booted {
    pub fn has(&self, udid: &str) -> bool {
        self.devices.iter().any(|(u, _)| u == udid)
    }

    /// Flare booted `udid` at `at_ms`. True when the list changed.
    pub fn record(&mut self, udid: &str, at_ms: u64) -> bool {
        if self.has(udid) {
            return false;
        }
        self.devices.push((udid.to_string(), at_ms));
        true
    }

    /// `udid` is no longer Flare's to shut down. True when the list changed.
    pub fn forget(&mut self, udid: &str) -> bool {
        let n = self.devices.len();
        self.devices.retain(|(u, _)| u != udid);
        n != self.devices.len()
    }

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))] // only the macOS install uses it
    /// The file, read leniently: a missing or broken file is an empty list (rule 41).
    pub fn load(path: &Path) -> Booted {
        let Some(v) = std::fs::read_to_string(path) // sync-ok: the Xcode pick's worker
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        else {
            return Booted::default();
        };
        Booted::from_json(&v)
    }

    /// `{"devices": [{"udid": "...", "booted_ms": 0}]}`; an entry that is not a device's is skipped.
    pub fn from_json(v: &serde_json::Value) -> Booted {
        let mut b = Booted::default();
        for d in v["devices"].as_array().into_iter().flatten() {
            if let Some(u) = d["udid"].as_str()
                && let Some(t) = Target::sim(u)
            {
                b.record(t.id(), d["booted_ms"].as_u64().unwrap_or(0));
            }
        }
        b
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "devices": self
                .devices
                .iter()
                .map(|(u, t)| serde_json::json!({"udid": u, "booted_ms": t}))
                .collect::<Vec<_>>(),
        })
    }

    /// Writes the file (its folder made). BLOCKING: a worker, or the quit (a file of a few lines).
    pub fn save(&self, path: &Path) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir); // sync-ok: a worker or the quit, a tiny file
        }
        let text = serde_json::to_string_pretty(&self.to_json()).unwrap_or_default();
        if let Err(e) = std::fs::write(path, text) {
            // sync-ok: a worker or the quit, a tiny file
            log::warn!("device: booted.json not written: {e}");
        }
    }
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))] // only the macOS install uses it
/// `<profile>/device/booted.json`.
pub fn booted_file(profile: &Path) -> PathBuf {
    profile.join("device").join("booted.json")
}

/// Where the device panel is offered with the real simulator: macOS on Apple silicon (spec "Setup", state 1), and
/// its menu entry once the Xcode pick found an Xcode.
pub fn menu_entry_offered(macos: bool, apple_silicon: bool, xcode: Option<Xcode>) -> bool {
    macos && apple_silicon && matches!(xcode, Some(Xcode::Ready | Xcode::NotSetUp))
}

/// This Mac can run the real simulator source (macOS on Apple silicon).
pub const fn real_platform() -> bool {
    cfg!(all(target_os = "macos", target_arch = "aarch64"))
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))] // only the macOS install uses it
/// The detached shutdown at quit (rule 1319): one `/bin/sh` that runs `simctl shutdown` for each device in turn,
/// with the chosen Xcode. Not guarded, so it outlives Flare; quit never waits for it.
pub fn shutdown_command(developer_dir: &Path, udids: &[String]) -> std::process::Command {
    let mut c = std::process::Command::new("/bin/sh"); // sync-ok: builds the command; spawned detached
    c.arg("-c")
        .arg(r#"for u in "$@"; do /usr/bin/xcrun simctl shutdown "$u" >/dev/null 2>&1; done"#)
        .arg("flare-device-shutdown")
        .args(udids)
        .env("DEVELOPER_DIR", developer_dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    c
}

/// Starts `cmd` and lets it go: never waited for (a thread reaps it, so no zombie is left while Flare runs on).
pub fn spawn_detached(mut cmd: std::process::Command) -> bool {
    match cmd.spawn() {
        // sync-ok: a spawn returns at once; the child is reaped on its own thread
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait(); // sync-ok: its own reaper thread, never the UI thread
            });
            true
        }
        Err(e) => {
            log::warn!("device: the shutdown at quit did not start: {e}");
            false
        }
    }
}

/// The real source, made by the first device panel: the source and its frame half (macOS).
pub struct Started {
    pub source: Box<dyn DeviceSource>,
    #[cfg(target_os = "macos")]
    pub ring: Option<std::sync::Arc<dyn crate::device_frames::RingSource>>,
}

/// What a real source needs to start: the Xcode the pick chose (the sim host), or an Android SDK (the emu host,
/// Android plan PR A2; not wired yet).
#[derive(Clone, Debug)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))] // only the macOS install reads the Xcode
pub enum SourceSetup {
    Xcode(flare_device::xcode::Xcode),
    #[allow(dead_code)] // the emu host is wired in Android plan PR A4
    AndroidSdk(AndroidSdk),
}

/// An Android SDK the emu host can use: Flare's own (`android-sdk` in its data folder) or the person's, with an
/// emulator at or above the floor (rule 1361).
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(dead_code)] // the emu host is wired in Android plan PR A4
pub struct AndroidSdk {
    /// The SDK's root folder (`ANDROID_SDK_ROOT` for the emulators Flare starts).
    pub root: PathBuf,
    /// The emulator's version, `37.3.10`.
    pub emulator_version: String,
}

/// Makes the real source for its setup: the sim host for an Xcode, later the emu host for an Android SDK. None when
/// this build cannot start that source (it is logged, and the panel stays as it is).
pub type Starter = Box<dyn FnMut(&SourceSetup) -> Option<Started>>;

/// Shuts devices down at quit, detached (tests record it).
pub type ShutdownSpawn = Rc<dyn Fn(&Path, &[String])>;

/// The real simulator's half of the controller (PR 4b).
struct Real {
    starter: Starter,
    /// The Xcode pick's answer: None while it runs.
    xcode: Option<Option<flare_device::xcode::Xcode>>,
    /// The source runs (started by a device panel).
    running: bool,
    /// No device panel since then (ms on the clock): the sim host stops after `IDLE_MS`.
    idle_since: Option<u64>,
    booted_path: Option<PathBuf>,
    spawn: ShutdownSpawn,
}

/// Whether Xcode can run a simulator here (spec "Setup", states 2 and 3). The fake source says Ready.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    // Android's setup (spec 2026-11-22 "Setup", states 1 to 4 and 6). Data and lines only: the panel shows them from
    // Android plan PR A4.
    /// No Android emulator here yet: Flare can download it from Google (state 1).
    #[allow(dead_code)] // shown from Android plan PR A4
    SdkMissing,
    /// Google's licence, before anything is downloaded (state 2). `text` is the licence from Google's package list,
    /// shown in full under the line; `download_mb` the download's size.
    #[allow(dead_code)] // shown from Android plan PR A4
    Licence {
        text: String,
        download_mb: u64,
    },
    /// The download runs (state 3).
    #[allow(dead_code)] // shown from Android plan PR A4
    Downloading {
        done_mb: u64,
        total_mb: u64,
    },
    /// Windows Hypervisor Platform is off (state 4).
    #[allow(dead_code)] // shown from Android plan PR A4
    NoHypervisor,
    /// It was turned on: Windows must restart (state 4, after [Turn on]).
    #[allow(dead_code)] // shown from Android plan PR A4
    RestartNeeded,
    /// An emulator started outside Flare without a gRPC token: Flare cannot reach it (state 6).
    #[allow(dead_code)] // shown from Android plan PR A4
    NoAccess,
}

/// The button of a setup state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetupButton {
    GetXcode,
    OpenXcode,
    Boot,
    BootAgain,
    TryAgain,
    /// Android: shows Google's licence (nothing is downloaded yet).
    SetUp,
    /// Android: the person accepts Google's licence; the download starts.
    AcceptAndSetUp,
    /// Android: back out of the licence, or stop the download (nothing half-unpacked is left).
    Cancel,
    /// Android on Windows: turn on Windows Hypervisor Platform (Windows asks through its own UAC prompt).
    TurnOn,
}

impl SetupButton {
    pub fn label(self) -> &'static str {
        match self {
            SetupButton::GetXcode => "Get Xcode",
            SetupButton::OpenXcode => "Open Xcode",
            SetupButton::Boot => "Boot",
            SetupButton::BootAgain => "Boot again",
            SetupButton::TryAgain => "Try again",
            SetupButton::SetUp => "Set up",
            SetupButton::AcceptAndSetUp => "Accept and set up",
            SetupButton::Cancel => "Cancel",
            SetupButton::TurnOn => "Turn on",
        }
    }
}

/// How a setup state is drawn: the Slint state (0 the picture, 1 a line, 2 the list, 3 booting), its line and its
/// button. `cancel` is the second button only the licence and the download have ([Cancel], spec "Setup").
#[derive(Clone, Debug, PartialEq)]
pub struct SetupView {
    pub state: i32,
    pub line: String,
    pub button: Option<SetupButton>,
    pub cancel: bool,
}

impl Setup {
    /// The state's line and button. `pictured`: a live device has its first frame.
    pub fn view(&self, pictured: bool) -> SetupView {
        let v = |state, line: String, button| SetupView {
            state,
            line,
            button,
            cancel: false,
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
            Setup::SdkMissing => v(
                1,
                "Android devices need Google's Android emulator (about 2 GB, from Google).".into(),
                Some(SetupButton::SetUp),
            ),
            Setup::Licence { download_mb, .. } => SetupView {
                cancel: true,
                ..v(
                    1,
                    format!(
                        "Google's licence for the Android emulator. Setting up downloads {} from Google.",
                        size_words(*download_mb)
                    ),
                    Some(SetupButton::AcceptAndSetUp),
                )
            },
            Setup::Downloading { done_mb, total_mb } => v(
                1,
                format!("Downloading the Android emulator... {done_mb} of {total_mb} MB"),
                Some(SetupButton::Cancel),
            ),
            Setup::NoHypervisor => v(
                1,
                "Android needs Windows Hypervisor Platform. Turning it on needs a restart.".into(),
                Some(SetupButton::TurnOn),
            ),
            Setup::RestartNeeded => v(1, "Restart Windows to finish.".into(), None),
            Setup::NoAccess => v(
                1,
                "This emulator was started without access for other apps. Start it from Flare."
                    .into(),
                None,
            ),
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
            Setup::SdkMissing => "sdk_missing",
            Setup::Licence { .. } => "licence",
            Setup::Downloading { .. } => "downloading",
            Setup::NoHypervisor => "no_hypervisor",
            Setup::RestartNeeded => "restart_needed",
            Setup::NoAccess => "no_access",
        }
    }
}

/// A download's size for the person: "about 2.5 GB", or "about 400 MB" under a gigabyte.
fn size_words(mb: u64) -> String {
    if mb >= 1000 {
        format!("about {:.1} GB", mb as f64 / 1000.0)
    } else {
        format!("about {mb} MB")
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
    /// The tab's device panel closes (the agent shut its device down, PR 5).
    Close(String),
}

/// A request in flight.
#[derive(Clone, Debug)]
enum Pending {
    List,
    Boot(Target),
    Attach(Target),
    Shutdown(Target),
    /// An agent call waits on it (PR 5).
    Agent(AgentWait),
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
    /// A ring frame came since attach (the picture shows; the notifier draws it).
    ring_frame: bool,
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

    fn get_ref(&self) -> &dyn DeviceSource {
        match self {
            Src::Fake(f) => f.as_ref(),
            Src::Other(s) => s.as_ref(),
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
    /// Xcode still needs its setup: the list is read again no sooner than this (ms on the clock).
    relist_at: Option<u64>,
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
    /// The agent's calls and its indicators (PR 5, `device_agent.rs`).
    agent: agent::Agent,
    /// Counters for the `device.state` probe.
    frames_shown: u64,
    person_inputs: u64,
    takeovers: u64,
    /// The zero-copy ring's frames, shared with the render notifier (rule 1311).
    frames: Rc<RefCell<DeviceFrames>>,
    /// The real simulator (macOS on Apple silicon), once installed.
    real: Option<Real>,
    /// The simulators Flare booted (rule 1319).
    booted: Booted,
    /// A source event changed what the panel shows since the last wake (else only frames came).
    state_changed: bool,
    /// One wake queued at most however fast the source's threads post (the `WakeGate` pattern).
    wake_gate: Arc<AtomicBool>,
    /// A ring frame came since the last wake (a redraw draws it).
    new_frame: bool,
    /// The sim host's facts were logged (`device-hello`), and the frame path last logged (`device-path`).
    hello_logged: bool,
    path_logged: FramePath,
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

/// The turn Slint gives the portrait ring picture for the interface's orientation, in degrees clockwise (`Rot90` is a
/// quarter clockwise, as `turn_rgba` turns a CPU picture).
pub fn turn_degrees(o: Orientation) -> i32 {
    match o {
        Orientation::Rot0 => 0,
        Orientation::Rot90 => 90,
        Orientation::Rot180 => 180,
        Orientation::Rot270 => 270,
    }
}

/// `simctl`'s error asks for Xcode's first launch or its licence (open check 5).
pub fn not_set_up(message: &str) -> bool {
    let m = message.to_lowercase();
    NOT_SET_UP_WORDS.iter().any(|w| m.contains(w))
}

fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
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
            relist_at: None,
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
            agent: agent::Agent::default(),
            frames_shown: 0,
            person_inputs: 0,
            takeovers: 0,
            frames: Rc::new(RefCell::new(DeviceFrames::default())),
            real: None,
            booted: Booted::default(),
            state_changed: false,
            wake_gate: Arc::new(AtomicBool::new(false)),
            new_frame: false,
            hello_logged: false,
            path_logged: FramePath::None,
        }
    }

    /// Another source (tests: a source of their own).
    #[allow(dead_code)]
    pub fn set_source(&mut self, source: Box<dyn DeviceSource>) {
        self.src = Some(Src::Other(source));
    }

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))] // only the macOS install uses it
    /// The real simulator (macOS on Apple silicon): `starter` makes the sim host when a device panel first needs it,
    /// once the Xcode pick (`DeviceMsg::Xcode`) found an Xcode. `booted` is `<profile>/device/booted.json`; `spawn`
    /// shuts devices down at quit, detached.
    pub fn install_real(
        &mut self,
        starter: Starter,
        booted_path: Option<PathBuf>,
        spawn: ShutdownSpawn,
    ) {
        self.src = None;
        self.real = Some(Real {
            starter,
            xcode: None,
            running: false,
            idle_since: None,
            booted_path,
            spawn,
        });
    }

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))] // only the macOS install uses it
    /// The wake the source's threads call after each event: one `UiMsg::Device(Wake)` queued at most.
    pub fn waker(&self) -> Arc<dyn Fn() + Send + Sync> {
        let gate = Arc::clone(&self.wake_gate);
        let post = self.post.clone();
        Arc::new(move || {
            if !gate.swap(true, Ordering::SeqCst) {
                post(crate::appctl::UiMsg::Device(DeviceMsg::Wake));
            }
        })
    }

    /// The ring's frames, for the render notifier's weak slot.
    pub fn frames(&self) -> Rc<RefCell<DeviceFrames>> {
        Rc::clone(&self.frames)
    }

    /// This build offers the device panel (a source exists, or the real one can be started).
    pub fn offered(&self) -> bool {
        self.src.is_some() || self.real.is_some()
    }

    /// The tab menu's Device panel entry: the fake source (`--e2e`), or the real one where Xcode was found.
    pub fn menu_offered(&self) -> bool {
        if matches!(self.src, Some(Src::Fake(_))) {
            return true;
        }
        let xcode = self.real.as_ref().and_then(|r| r.xcode.as_ref()).map(|x| {
            if x.is_some() {
                self.xcode
            } else {
                Xcode::Missing
            }
        });
        self.real.is_some() && menu_entry_offered(cfg!(target_os = "macos"), real_platform(), xcode)
    }

    /// The Xcode pick answered: the setup state follows, and the simulators booted in an earlier run are known.
    pub fn on_xcode(&mut self, answer: XcodeAnswer) {
        let Some(real) = self.real.as_mut() else {
            return;
        };
        match &answer.xcode {
            Some(x) => {
                log::info!(
                    "device: Xcode {} at {} (DEVELOPER_DIR {})",
                    x.version,
                    x.app.display(),
                    x.developer_dir().display()
                );
                self.xcode = Xcode::Ready;
            }
            None => {
                log::info!("device: no Xcode found: the device panel says it needs Xcode");
                self.xcode = Xcode::Missing;
            }
        }
        real.xcode = Some(answer.xcode);
        for (u, at) in answer.booted.devices {
            self.booted.record(&u, at);
        }
    }

    /// The Xcode app the pick chose ("Open Xcode").
    pub fn xcode_app(&self) -> Option<PathBuf> {
        Some(self.real.as_ref()?.xcode.clone()??.app)
    }

    /// Starts the real source if it is installed, found an Xcode and is not running.
    fn ensure_source(&mut self) {
        if self.src.is_some() {
            return;
        }
        let Some(real) = self.real.as_mut() else {
            return;
        };
        let Some(Some(xcode)) = real.xcode.clone() else {
            return;
        };
        let Some(started) = (real.starter)(&SourceSetup::Xcode(xcode)) else {
            log::warn!("device: the sim host cannot start in this build");
            return;
        };
        real.running = true;
        real.idle_since = None;
        #[cfg(target_os = "macos")]
        self.frames.borrow_mut().set_source(started.ring);
        self.src = Some(Src::Other(started.source));
        log::info!("device: the sim host is started");
    }

    /// No device panel for `IDLE_MS`: the sim host stops (it is told `quit`; the guard ends it either way).
    fn idle(&mut self, has_panels: bool) {
        let now = self.now();
        let Some(real) = self.real.as_mut() else {
            return;
        };
        if has_panels || !real.running {
            real.idle_since = None;
            return;
        }
        let since = *real.idle_since.get_or_insert_with(|| {
            let post = self.post.clone();
            crate::timer_registry::set(IDLE_TICK, IDLE_MS, false, move || {
                post(crate::appctl::UiMsg::Device(DeviceMsg::Tick));
            });
            now
        });
        if now.saturating_sub(since) < IDLE_MS {
            return;
        }
        real.running = false;
        real.idle_since = None;
        self.stop_source();
        log::info!("device: no device panel for two minutes: the sim host stops");
    }

    /// Drops the running source (its `Drop` tells the sim host to quit) and what it held.
    fn stop_source(&mut self) {
        #[cfg(target_os = "macos")]
        self.frames.borrow_mut().set_source(None);
        self.src = None;
        self.hello_logged = false;
        self.pending.clear();
        self.devs.clear();
        self.devices = None;
        self.listing = false;
        self.relist_at = None;
        self.broken = None;
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
        if on {
            // Rule 1321: the indicators go at once with the window.
            self.agent_hide_indicators();
        }
    }

    pub fn set_occluded(&mut self, on: bool) {
        self.occluded = on;
        if on {
            self.agent_hide_indicators();
        }
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
        let list = panels.device_panels();
        // The real source starts with the first device panel and stops two minutes after the last one.
        if !list.is_empty() {
            self.ensure_source();
        }
        self.idle(!list.is_empty());
        self.drain();
        let live: HashSet<&str> = list.iter().map(|(t, ..)| t.as_str()).collect();
        let closed: Vec<String> = self
            .tabs
            .keys()
            .filter(|t| !live.contains(t.as_str()))
            .cloned()
            .collect();
        for tab in closed {
            self.tabs.remove(&tab);
            self.agent_forget_tab(&tab);
        }
        for (tab, target, _) in &list {
            let t = self.tabs.entry(tab.clone()).or_default();
            if t.target != *target {
                let was = std::mem::replace(
                    t,
                    TabDev {
                        target: target.clone(),
                        ..TabDev::default()
                    },
                );
                // The panel shows another device now: the agent's refs and call on the old one end.
                if !was.target.is_empty() && was.target != CHOOSE {
                    self.agent_forget_tab(tab);
                }
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
        let now = self.now();
        if !list.is_empty()
            && self.devices.is_none()
            && !self.listing
            && self.list_error.is_none()
            && self.relist_at.is_none_or(|t| now >= t)
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
        effects.extend(self.take_effects());
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

    /// A boot or a shutdown answered: the device reads its new state at once, and the list is read again.
    fn settled(&mut self, t: &Target, state: DeviceState) {
        if let Some(d) = self
            .devices
            .as_mut()
            .and_then(|l| l.iter_mut().find(|d| &d.target == t))
        {
            d.state = state;
        }
        self.list();
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

    /// Reads every event the source has (the fake answers at once; the simulator's threads wake the window).
    pub fn drain(&mut self) {
        while let Some(ev) = self.src.as_ref().and_then(|s| s.events().try_recv().ok()) {
            self.on_event(ev);
        }
        if !self.hello_logged
            && let Some(h) = self.src.as_ref().and_then(|s| s.get_ref().hello())
        {
            self.hello_logged = true;
            use flare_diag::agent_diag::DiagValue as V;
            crate::diag_ctl::agent_sink().record(
                "device-hello",
                &[
                    ("version", V::Str(h.version.clone())),
                    ("xcode", V::Str(h.xcode_version.clone())),
                    ("coresim", V::Str(h.coresim_version.clone())),
                    ("simkit", V::Str(h.simkit_version.clone())),
                    (
                        "input",
                        V::Str(
                            h.input_path
                                .map_or_else(String::new, |p| format!("{p:?}").to_lowercase()),
                        ),
                    ),
                    ("ax", V::Bool(h.ax)),
                ],
                0,
            );
        }
    }

    /// The frame path changed (GPU, CPU): one `device-path` line, with why the import was refused.
    fn log_path(&mut self) {
        let (path, why) = {
            let f = self.frames.borrow();
            (f.path(), f.refused().unwrap_or_default())
        };
        if path == self.path_logged || path == FramePath::None {
            return;
        }
        self.path_logged = path;
        log::info!(
            "device panel: frames by {}{}",
            path.name(),
            if why.is_empty() {
                String::new()
            } else {
                format!(" ({why})")
            }
        );
        use flare_diag::agent_diag::DiagValue as V;
        crate::diag_ctl::agent_sink().record(
            "device-path",
            &[("route", V::Str(path.name().into())), ("why", V::Str(why))],
            0,
        );
    }

    /// Saves `booted.json` on a worker (rule 34).
    fn save_booted(&self) {
        let Some(path) = self.real.as_ref().and_then(|r| r.booted_path.clone()) else {
            return;
        };
        let b = self.booted.clone();
        std::thread::spawn(move || b.save(&path));
    }

    fn on_event(&mut self, ev: SourceEvent) {
        // Only a frame (or the changed count) leaves the panel as it is: the notifier draws it.
        let quiet = match &ev {
            SourceEvent::Frame { target, .. } => self
                .devs
                .get(target)
                .is_some_and(|d| d.ring_frame || !d.attached),
            SourceEvent::Changed { .. } => true,
            _ => false,
        };
        if !quiet {
            self.state_changed = true;
        }
        match ev {
            SourceEvent::Answer { id, result } => {
                let Some(p) = self.pending.remove(&id) else {
                    return;
                };
                match (p, result) {
                    (Pending::List, Ok(Answer::Devices(mut d))) => {
                        sort_devices(&mut d);
                        // Rule 1319: a device Flare booted stays Flare's until it is seen shut down or gone.
                        let mut forgot = false;
                        for (u, _) in self.booted.devices.clone() {
                            let seen = d.iter().find(|x| x.target.id() == u);
                            if seen.is_none_or(|x| x.state == DeviceState::Shutdown) {
                                forgot |= self.booted.forget(&u);
                            }
                        }
                        for x in &mut d {
                            x.booted_by_flare = self.booted.has(x.target.id());
                        }
                        if forgot {
                            self.save_booted();
                        }
                        self.devices = Some(d);
                        self.listing = false;
                        if self.xcode == Xcode::NotSetUp {
                            self.xcode = Xcode::Ready;
                        }
                        self.relist_at = None;
                        self.agent_event(&AgentEvent::Listed(None));
                    }
                    (Pending::List, Ok(_)) => self.listing = false,
                    (Pending::List, Err(e)) => {
                        self.listing = false;
                        log::warn!("device list: {}", e.message);
                        self.agent_event(&AgentEvent::Listed(Some(flare_ctl::CtlError::new(
                            e.fail.code(),
                            format!("the device list could not be read: {}", e.message),
                        ))));
                        if not_set_up(&e.message) {
                            // Open check 5: simctl asks for Xcode's first launch or its licence. The list is read
                            // again in a while (the person may finish the setup), never at once in a loop.
                            self.xcode = Xcode::NotSetUp;
                            self.relist_at = Some(self.now() + NOT_SET_UP_RETRY_MS);
                            let post = self.post.clone();
                            crate::timer_registry::set(
                                RELIST_TICK,
                                NOT_SET_UP_RETRY_MS,
                                false,
                                move || post(crate::appctl::UiMsg::Device(DeviceMsg::Tick)),
                            );
                        } else {
                            self.list_error = Some(e.message);
                        }
                    }
                    (Pending::Attach(t), r) => {
                        let mut err = None;
                        if let Some(d) = self.devs.get_mut(&t) {
                            d.attaching = false;
                            match r {
                                Ok(_) => {
                                    d.attached = true;
                                    d.paused = false;
                                }
                                Err(e) => {
                                    log::info!("attach {t}: {}", e.message);
                                    err = Some(flare_ctl::CtlError::new(
                                        e.fail.code(),
                                        e.message.clone(),
                                    ));
                                    self.fail(&t, e.message);
                                }
                            }
                        }
                        self.agent_event(&AgentEvent::Attached(t, err));
                    }
                    (Pending::Agent(w), r) => self.on_agent_answer(w, r),
                    (Pending::Boot(t), r) => {
                        self.booting.remove(&t);
                        match r {
                            Err(e) => {
                                self.agent_event(&AgentEvent::Booted(
                                    t.clone(),
                                    Err(flare_ctl::CtlError::new(e.fail.code(), e.message.clone())),
                                ));
                                self.fail(&t, e.message);
                            }
                            Ok(_) => {
                                // Flare booted it: shut down at quit (rule 1319), also after a crash; only such a
                                // device may the agent shut down (rule 1322). The file is the real source's.
                                if self.booted.record(t.id(), unix_ms()) && self.real.is_some() {
                                    self.save_booted();
                                }
                                self.agent_event(&AgentEvent::Booted(t.clone(), Ok(())));
                                if let Some(x) = self
                                    .devices
                                    .as_mut()
                                    .and_then(|l| l.iter_mut().find(|d| d.target == t))
                                {
                                    x.booted_by_flare = true;
                                }
                                // The sim host answers a boot and sends no state of its own (the fake does): the
                                // device reads booted now, so the panel attaches, and the list is read again.
                                self.settled(&t, DeviceState::Booted);
                            }
                        }
                    }
                    (Pending::Shutdown(t), r) => match r {
                        Err(e) => self.fail(&t, e.message),
                        Ok(_) => self.settled(&t, DeviceState::Shutdown),
                    },
                }
            }
            SourceEvent::Frames { target, screen, .. } => {
                self.agent_event(&AgentEvent::NewSession(target.clone()));
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
            // A frame in an Android device's region (rule 1363): the frame thread copies it into the ring from
            // Android plan PR A4. No source sends it yet.
            SourceEvent::Region { .. } => {}
            // A ring frame: the notifier takes the newest in its next render (`DeviceFrames`).
            SourceEvent::Frame { target, .. } => {
                if let Some(d) = self.devs.get_mut(&target)
                    && d.attached
                {
                    d.ring_frame = true;
                    self.new_frame = true;
                }
            }
            SourceEvent::Changed { target, count } => {
                if let Some(d) = self.devs.get_mut(&target) {
                    d.changed = count;
                }
                self.agent_event(&AgentEvent::Changed(target, count));
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
                        // Booted by someone else meanwhile: a boot call waiting for it answers too.
                        self.agent_event(&AgentEvent::Booted(target.clone(), Ok(())));
                    }
                    DeviceState::Booting => {
                        let now = self.now();
                        self.booting.entry(target).or_insert(now);
                    }
                    DeviceState::Shutdown | DeviceState::ShuttingDown => {
                        self.booting.remove(&target);
                        if state == DeviceState::Shutdown && self.booted.forget(target.id()) {
                            self.save_booted();
                        }
                        if let Some(d) = self.devs.get_mut(&target) {
                            d.attached = false;
                            d.attaching = false;
                            d.ring_frame = false;
                            d.picture = None;
                            d.raw = None;
                            d.input = None;
                        }
                        if state == DeviceState::Shutdown {
                            self.agent_event(&AgentEvent::ShutDown(target));
                        }
                    }
                }
            }
            SourceEvent::Input { target, state } => {
                let lost = state == InputState::Lost;
                if let Some(d) = self.devs.get_mut(&target) {
                    d.input = Some(state);
                }
                if lost {
                    self.agent_event(&AgentEvent::InputLost(target));
                }
            }
            SourceEvent::Broken(why) => {
                use flare_diag::agent_diag::DiagValue as V;
                crate::diag_ctl::agent_sink().record(
                    "device-still",
                    &[("why", V::Str(why.clone()))],
                    0,
                );
                self.agent_event(&AgentEvent::Broken(why.clone()));
                self.broken = Some(why);
            }
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
        // The Xcode pick has not answered yet.
        if self.real.as_ref().is_some_and(|r| r.xcode.is_none()) {
            return Some(Setup::Reading);
        }
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
                app.set_panel_device_turn(0);
                app.set_panel_device_rows(ModelRc::default());
                app.set_panel_device_controls(false);
                app.set_panel_device_still("".into());
                app.set_panel_device_slow("".into());
                app.set_panel_device_foot("".into());
            }
            self.frames.borrow_mut().show(None);
            self.push_indicators(app);
            return;
        };
        let Some(setup) = self.setup_of(&tab) else {
            return;
        };
        let target = self.target_of(&tab);
        // The newest picture of the shown device, turned and handed over (the CPU path).
        let mut pictured = false;
        let mut ring = None;
        let mut turn = 0;
        if let Some(t) = &target
            && let Some(d) = self.devs.get_mut(t)
        {
            // The ring's frames: the notifier draws them; Slint turns the portrait picture (spike S5).
            if d.ring_frame && d.attached && !d.paused && self.broken.is_none() {
                ring = Some(t.clone());
                turn = d.screen.map_or(0, |s| turn_degrees(s.orientation));
            }
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
            pictured = (d.picture.is_some() || d.ring_frame) && d.attached;
            if ring.is_none()
                && let Some((seq, img)) = &d.picture
                && self.pushed.seq.as_ref() != Some(&(tab.clone(), *seq))
            {
                app.set_panel_device_frame(img.clone());
                self.pushed.seq = Some((tab.clone(), *seq));
                self.frames_shown += 1;
            }
        }
        if ring.is_none() && self.frames.borrow().path() != FramePath::None {
            // Back from the ring to a CPU picture (still mode): the picture shows again.
            self.pushed.seq = None;
        }
        self.log_path();
        let ring_path = ring.as_ref().map(|_| self.frames.borrow().path());
        self.frames.borrow_mut().show(ring);
        app.set_panel_device_turn(turn);
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
        // The slow mark: a CPU picture (the fake source), or the ring's import refused. Never on the zero-copy path.
        let slow = live
            && self.broken.is_none()
            && match ring_path {
                Some(p) => p == FramePath::Cpu,
                None => true,
            };
        app.set_panel_device_slow(if slow { CPU_SLOW } else { "" }.into());
        app.set_panel_device_foot(self.foot(&tab).into());
        app.set_panel_device_controls(live && self.drives(&tab).is_some());
        let shown = self.frames_shown + self.frames.borrow().frames();
        self.sample_fps(shown);
        self.push_indicators(app);
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
        if self.agent_takeover(tab) {
            self.takeovers += 1;
        }
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
                // Rule 1371: a button this device does not have does nothing.
                if !t.source().has(b) {
                    return Vec::new();
                }
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
            // Android's setup buttons act from Android plan PR A2b and A4; no state shows them yet.
            SetupButton::SetUp
            | SetupButton::AcceptAndSetUp
            | SetupButton::Cancel
            | SetupButton::TurnOn => {
                log::info!("device panel: {} is not wired yet", button.label());
                return Vec::new();
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

    /// A message for the controller. A frame alone asks only for a redraw (the notifier draws it); anything else
    /// asks for the window's turn (which pushes).
    pub fn on_msg(&mut self, msg: DeviceMsg) -> Woken {
        match msg {
            DeviceMsg::Wake => {
                self.wake_gate.store(false, Ordering::SeqCst);
            }
            DeviceMsg::Tick => {
                self.state_changed = true;
            }
            DeviceMsg::Xcode(a) => {
                self.on_xcode(a);
                self.state_changed = true;
            }
            DeviceMsg::AgentTick => {
                self.agent_tick();
            }
        }
        self.drain();
        if !self.agent.effects.is_empty() {
            self.state_changed = true;
        }
        let framed = std::mem::take(&mut self.new_frame);
        if std::mem::take(&mut self.state_changed) {
            Woken::Refresh
        } else if framed {
            Woken::Redraw
        } else if self.agent.ticking || self.agent.pushed.is_some() {
            Woken::Indicators
        } else {
            Woken::Nothing
        }
    }

    /// Quit (rule 1319): the sim host is told to quit, and with the setting on the simulators Flare booted are shut
    /// down by one detached `simctl shutdown` that quit never waits for. A device Flare did not boot is never shut
    /// down. Returns the udids handed to it.
    pub fn quit(&mut self, shutdown_on: bool) -> Vec<String> {
        let Some(real) = self.real.as_ref() else {
            return Vec::new();
        };
        let dev_dir = real.xcode.clone().flatten().map(|x| x.developer_dir());
        let spawn = Rc::clone(&real.spawn);
        let path = real.booted_path.clone();
        self.stop_source();
        if !shutdown_on {
            return Vec::new();
        }
        let Some(dev_dir) = dev_dir else {
            return Vec::new();
        };
        let udids: Vec<String> = self.booted.devices.iter().map(|(u, _)| u.clone()).collect();
        if udids.is_empty() {
            return udids;
        }
        log::info!(
            "device: shutting down {} simulator(s) Flare booted",
            udids.len()
        );
        spawn(&dev_dir, &udids);
        self.booted = Booted::default();
        if let Some(p) = path {
            self.booted.save(&p);
        }
        udids
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
        let fr = self.frames.borrow();
        json!({
            "offered": self.offered(),
            "menu": self.menu_offered(),
            "path": fr.path().name(),
            "ring_showing": target.as_ref().is_some_and(|t| fr.showing(t)),
            "refused": fr.refused(),
            "hello": self
                .src
                .as_ref()
                .and_then(|s| s.get_ref().hello())
                .and_then(|h| serde_json::to_value(h).ok()),
            "ring_frames": fr.frames(),
            "slots_held": fr.held(),
            "booted_by_flare": self.booted.devices.iter().map(|(u, _)| u.clone()).collect::<Vec<_>>(),
            "tab": tab,
            "state": setup.as_ref().map(Setup::name),
            "target": target.map(|t| t.name()),
            "attached": dev.is_some_and(|d| d.attached),
            "paused": dev.is_some_and(|d| d.paused),
            "driver": dev.and_then(|d| d.driver.clone()),
            "screen": screen.map(|s| json!({"w": s.w, "h": s.h})),
            "frames_shown": self.frames_shown + fr.frames(),
            "person_inputs": self.person_inputs,
            "takeovers": self.takeovers,
            "devices": self.devices.as_ref().map_or(0, Vec::len),
            "inputs": inputs,
            "agent": self.agent_probe(),
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
            Setup::SdkMissing,
            Setup::Licence {
                text: "Terms and Conditions\n\nThis is the Android SDK License Agreement".into(),
                download_mb: 2_538,
            },
            Setup::Downloading {
                done_mb: 120,
                total_mb: 2_538,
            },
            Setup::NoHypervisor,
            Setup::RestartNeeded,
            Setup::NoAccess,
        ];
        let mut names = HashSet::new();
        for s in all {
            let v = s.view(false);
            assert!(!v.line.is_empty(), "{s:?} says a line");
            assert!(!v.line.contains('\n'), "{s:?}: one line");
            assert!(
                names.insert(s.name()) || matches!(s, Setup::ShutDown { again: false, .. }),
                "{s:?}: a name of its own"
            );
            assert_eq!(
                v.cancel,
                matches!(s, Setup::Licence { .. }),
                "{s:?}: only the licence has a second button"
            );
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

    /// Android's setup states (spec 2026-11-22 "Setup", states 1 to 4 and 6): their lines and buttons, as data.
    #[test]
    fn android_setup_states_say_the_specs_lines() {
        let v = Setup::SdkMissing.view(false);
        assert_eq!(
            v.line,
            "Android devices need Google's Android emulator (about 2 GB, from Google)."
        );
        assert_eq!(v.button.map(SetupButton::label), Some("Set up"));
        let v = Setup::Licence {
            text: "licence".into(),
            download_mb: 2_538,
        }
        .view(false);
        assert_eq!(
            v.line,
            "Google's licence for the Android emulator. Setting up downloads about 2.5 GB from Google."
        );
        assert_eq!(v.button.map(SetupButton::label), Some("Accept and set up"));
        assert!(v.cancel);
        assert_eq!(SetupButton::Cancel.label(), "Cancel");
        let v = Setup::Downloading {
            done_mb: 120,
            total_mb: 2_538,
        }
        .view(false);
        assert_eq!(v.line, "Downloading the Android emulator... 120 of 2538 MB");
        assert_eq!(v.button, Some(SetupButton::Cancel));
        let v = Setup::NoHypervisor.view(false);
        assert_eq!(
            v.line,
            "Android needs Windows Hypervisor Platform. Turning it on needs a restart."
        );
        assert_eq!(v.button.map(SetupButton::label), Some("Turn on"));
        assert_eq!(
            Setup::RestartNeeded.view(false),
            SetupView {
                state: 1,
                line: "Restart Windows to finish.".into(),
                button: None,
                cancel: false
            }
        );
        let v = Setup::NoAccess.view(false);
        assert_eq!(
            v.line,
            "This emulator was started without access for other apps. Start it from Flare."
        );
        assert_eq!(v.button, None);
        assert_eq!(size_words(421), "about 421 MB");
        assert_eq!(size_words(1_000), "about 1.0 GB");
    }

    #[test]
    fn a_build_with_no_source_offers_no_device_panel() {
        // A build with no source (Windows, an Intel Mac, no `--e2e`): nothing new shows (rule 1318).
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

    fn post() -> Post {
        std::sync::Arc::new(|_| {})
    }

    fn found_xcode() -> flare_device::xcode::Xcode {
        flare_device::xcode::Xcode {
            app: PathBuf::from("/Applications/Xcode.app"),
            version: "27.2".into(),
        }
    }

    /// The real source's place taken by a fake (no sim host): what `install_real` starts.
    fn starter_of(fake: FakeSource) -> Starter {
        let mut fake = Some(fake);
        Box::new(move |setup| {
            assert!(matches!(setup, SourceSetup::Xcode(_)), "{setup:?}");
            Some(Started {
                source: Box::new(fake.take().expect("started once")),
                #[cfg(target_os = "macos")]
                ring: None,
            })
        })
    }

    type Spawned = Rc<RefCell<Vec<(PathBuf, Vec<String>)>>>;

    fn recorder() -> (Spawned, ShutdownSpawn) {
        let spawned: Spawned = Rc::default();
        let s = Rc::clone(&spawned);
        let spawn: ShutdownSpawn = Rc::new(move |dir: &Path, udids: &[String]| {
            s.borrow_mut().push((dir.to_path_buf(), udids.to_vec()));
        });
        (spawned, spawn)
    }

    /// Max, 2026-11-22: "i click a device then click boot but nothing happens". The sim host answers a boot and
    /// sends no state event of its own; the device must still read booted, so the panel attaches.
    #[test]
    fn a_boot_answered_without_a_state_event_reads_booted() {
        let b = "00000000-0000-4000-8000-00000000000b";
        let fake = FakeSource::new(vec![
            flare_device::fake::iphone(b, "iPhone B", DeviceState::Shutdown).unwrap(),
        ])
        .without_state_events();
        let (_spawned, spawn) = recorder();
        let mut d = DeviceCtl::new(post(), Rc::new(|| 0), None);
        d.install_real(starter_of(fake), None, spawn);
        d.on_xcode(XcodeAnswer {
            xcode: Some(found_xcode()),
            booted: Booted::default(),
        });
        d.ensure_source();
        d.list();
        let tb = Target::sim(b).unwrap();
        assert_eq!(d.state_of(&tb), Some(DeviceState::Shutdown));
        d.boot(&tb);
        d.drain();
        assert_eq!(d.state_of(&tb), Some(DeviceState::Booted));
        // The More menu's Shut down, as `act("shutdown")` sends it.
        if let Some(id) = d.source().map(|s| s.shutdown(&tb)) {
            d.pending.insert(id, Pending::Shutdown(tb.clone()));
        }
        d.drain();
        assert_eq!(d.state_of(&tb), Some(DeviceState::Shutdown));
    }

    #[test]
    fn quit_shuts_down_only_devices_flare_booted_and_does_not_wait() {
        let (a, b, c) = (
            "00000000-0000-4000-8000-00000000000a",
            "00000000-0000-4000-8000-00000000000b",
            "00000000-0000-4000-8000-00000000000c",
        );
        let iphone = flare_device::fake::iphone;
        // A runs, booted by someone else; B is shut down; C runs, booted by Flare in a run that crashed.
        let fake = FakeSource::new(vec![
            iphone(a, "iPhone A", DeviceState::Booted).unwrap(),
            iphone(b, "iPhone B", DeviceState::Shutdown).unwrap(),
            iphone(c, "iPhone C", DeviceState::Booted).unwrap(),
        ]);
        let (spawned, spawn) = recorder();
        let mut d = DeviceCtl::new(post(), Rc::new(|| 0), None);
        d.install_real(starter_of(fake), None, spawn);
        assert!(
            d.offered(),
            "the kind is offered before the pick answers (restore keeps the panel)"
        );
        d.on_xcode(XcodeAnswer {
            xcode: Some(found_xcode()),
            booted: Booted {
                devices: vec![(c.into(), 1)],
            },
        });
        d.ensure_source();
        d.list();
        let tb = Target::sim(b).unwrap();
        d.boot(&tb);
        d.drain();
        assert_eq!(d.state_of(&tb), Some(DeviceState::Booted));
        let flags: Vec<(String, bool)> = d
            .devices
            .iter()
            .flatten()
            .map(|x| (x.name.clone(), x.booted_by_flare))
            .collect();
        assert!(flags.contains(&("iPhone A".into(), false)), "{flags:?}");
        assert!(flags.contains(&("iPhone B".into(), true)), "{flags:?}");
        assert!(flags.contains(&("iPhone C".into(), true)), "{flags:?}");

        let t0 = std::time::Instant::now();
        let mut udids = d.quit(true);
        udids.sort();
        assert_eq!(udids, vec![b.to_string(), c.to_string()], "never A");
        let got = spawned.borrow().clone();
        assert_eq!(got.len(), 1, "one detached shutdown for all of them");
        assert_eq!(
            got[0].0,
            PathBuf::from("/Applications/Xcode.app/Contents/Developer"),
            "with the chosen Xcode"
        );
        assert!(d.src.is_none(), "the sim host is told to go");
        assert!(d.booted.devices.is_empty());
        assert!(t0.elapsed() < std::time::Duration::from_secs(1));

        // The setting off: nothing is shut down.
        let fake = FakeSource::new(vec![iphone(b, "iPhone B", DeviceState::Shutdown).unwrap()]);
        let (spawned, spawn) = recorder();
        let mut d = DeviceCtl::new(post(), Rc::new(|| 0), None);
        d.install_real(starter_of(fake), None, spawn);
        d.on_xcode(XcodeAnswer {
            xcode: Some(found_xcode()),
            booted: Booted::default(),
        });
        d.ensure_source();
        d.list();
        d.boot(&tb);
        d.drain();
        assert!(d.quit(false).is_empty());
        assert!(spawned.borrow().is_empty());

        // The real spawn never waits for its child: a command that takes 5 s returns at once.
        #[cfg(unix)]
        let mut slow = std::process::Command::new("/bin/sleep");
        #[cfg(unix)]
        slow.arg("5");
        // Windows has no /bin/sleep: ping waits about a second per echo.
        #[cfg(windows)]
        let mut slow = std::process::Command::new("ping");
        #[cfg(windows)]
        slow.args(["-n", "6", "127.0.0.1"]);
        slow.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let t0 = std::time::Instant::now();
        assert!(spawn_detached(slow));
        assert!(
            t0.elapsed() < std::time::Duration::from_secs(1),
            "quit does not wait"
        );
        // And the command it runs: simctl shutdown per device, with the chosen Xcode, nothing on the console.
        let cmd = shutdown_command(
            Path::new("/X.app/Contents/Developer"),
            &[b.into(), c.into()],
        );
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert!(args[1].contains("simctl shutdown"), "{args:?}");
        assert_eq!(&args[3..], &[b.to_string(), c.to_string()]);
        assert!(
            cmd.get_envs()
                .any(|(k, v)| k == "DEVELOPER_DIR"
                    && v == Some("/X.app/Contents/Developer".as_ref()))
        );
    }

    #[test]
    fn booted_json_round_trips_and_skips_what_is_no_device() {
        let mut b = Booted::default();
        assert!(b.record("00000000-0000-4000-8000-00000000000b", 7));
        assert!(!b.record("00000000-0000-4000-8000-00000000000b", 8), "once");
        let v = b.to_json();
        assert_eq!(Booted::from_json(&v), b);
        let odd = serde_json::json!({"devices": [{"udid": "../etc"}, {"udid": 3}, {"udid": "00000000-0000-4000-8000-00000000000c"}]});
        assert_eq!(Booted::from_json(&odd).devices.len(), 1);
        assert_eq!(
            Booted::from_json(&serde_json::json!("nonsense")),
            Booted::default()
        );
        assert!(b.forget("00000000-0000-4000-8000-00000000000b"));
        assert!(b.devices.is_empty());
    }

    /// Open check 5: a list that fails because Xcode needs its setup is not asked again at once. Each answer refreshes
    /// the window, so an unthrottled relist would run `simctl list` in a loop while the panel is open.
    #[test]
    fn a_not_set_up_list_is_read_again_only_after_a_while() {
        let now = Rc::new(std::cell::Cell::new(1_000u64));
        let clock = {
            let now = Rc::clone(&now);
            Rc::new(move || now.get())
        };
        let mut fake = FakeSource::new(vec![
            flare_device::fake::iphone(
                "00000000-0000-4000-8000-00000000000a",
                "iPhone A",
                DeviceState::Shutdown,
            )
            .unwrap(),
        ]);
        fake.fail_next(
            flare_device::job::Fail::Unavailable,
            "xcrun: error: run xcodebuild -runFirstLaunch to install the required components",
        );
        let mut d = DeviceCtl::new(post(), clock, Some(fake));
        let mut panels = crate::panel_ctl::PanelCtl::new(post());
        panels.set_device_offered(true);
        assert!(panels.show("t1", flare_surfaces::PanelKind::Device, CHOOSE));
        let _ = d.update(&panels, Some("t1"), false, HashMap::new());
        d.drain();
        assert_eq!(d.xcode, Xcode::NotSetUp);
        assert_eq!(d.setup_of("t1").map(|s| s.name()), Some("not_set_up"));
        // The answer's refresh runs the update again: no new list yet.
        let _ = d.update(&panels, Some("t1"), false, HashMap::new());
        d.drain();
        assert!(d.devices.is_none() && !d.listing, "not read again at once");
        assert_eq!(d.xcode, Xcode::NotSetUp);
        // After the retry spacing the list is read again; it works now, so the setup state goes.
        now.set(1_000 + NOT_SET_UP_RETRY_MS);
        let _ = d.update(&panels, Some("t1"), false, HashMap::new());
        d.drain();
        assert_eq!(d.xcode, Xcode::Ready);
        assert!(d.devices.as_ref().is_some_and(|l| l.len() == 1));
        assert!(d.relist_at.is_none());
    }

    #[test]
    fn the_menu_entry_is_offered_only_on_apple_silicon_with_xcode() {
        assert!(menu_entry_offered(true, true, Some(Xcode::Ready)));
        assert!(menu_entry_offered(true, true, Some(Xcode::NotSetUp)));
        assert!(
            !menu_entry_offered(true, true, Some(Xcode::Missing)),
            "no Xcode"
        );
        assert!(
            !menu_entry_offered(true, true, None),
            "the pick has not answered"
        );
        assert!(
            !menu_entry_offered(true, false, Some(Xcode::Ready)),
            "an Intel Mac"
        );
        assert!(
            !menu_entry_offered(false, true, Some(Xcode::Ready)),
            "not a Mac"
        );
        // The controller: no source offers nothing; the real one once the pick found Xcode, on Apple silicon only.
        let mut d = DeviceCtl::new(post(), Rc::new(|| 0), None);
        assert!(!d.menu_offered());
        d.install_real(starter_of(FakeSource::default()), None, recorder().1);
        assert!(!d.menu_offered(), "before the pick answers");
        d.on_xcode(XcodeAnswer {
            xcode: Some(found_xcode()),
            booted: Booted::default(),
        });
        assert_eq!(d.menu_offered(), real_platform());
        assert_eq!(
            d.xcode_app(),
            Some(PathBuf::from("/Applications/Xcode.app"))
        );
        let mut none = DeviceCtl::new(post(), Rc::new(|| 0), None);
        none.install_real(starter_of(FakeSource::default()), None, recorder().1);
        none.on_xcode(XcodeAnswer::default());
        assert!(!none.menu_offered(), "no Xcode, no entry");
        assert_eq!(none.xcode, Xcode::Missing);
        // The fake (`--e2e`) offers it everywhere.
        assert!(DeviceCtl::new(post(), Rc::new(|| 0), Some(FakeSource::default())).menu_offered());
        // simctl asking for the first launch is "not set up" (open check 5).
        assert!(not_set_up(
            "xcrun: error: run xcodebuild -runFirstLaunch to install the required components"
        ));
        assert!(not_set_up(
            "You have not agreed to the Xcode license agreements."
        ));
        assert!(!not_set_up("Invalid device: 1234"));
    }

    #[test]
    fn the_ring_picture_turns_clockwise_with_the_interface() {
        assert_eq!(turn_degrees(Orientation::Rot0), 0);
        assert_eq!(turn_degrees(Orientation::Rot90), 90);
        assert_eq!(turn_degrees(Orientation::Rot180), 180);
        assert_eq!(turn_degrees(Orientation::Rot270), 270);
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

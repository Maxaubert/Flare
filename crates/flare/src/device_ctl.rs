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
use flare_device::source::{FrameMode, Key, Phase, SourceKind};
use flare_device::target::{self, Target};
use flare_device::wire::SetupStep;
use slint::{Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel};

use crate::appctl::Post;
use crate::device_frames::{DeviceFrames, FramePath};
use crate::device_sources::Multi;
use crate::panel_ctl::PanelCtl;
use crate::{AppWindow, DeviceRow};

/// The target of a device panel whose device is not chosen yet: it shows the device list. Never stored.
pub const CHOOSE: &str = "device";

/// The corner radius of an iPhone's screen in points (the picture is drawn bare with it, no bezel).
pub const RADIUS_PT: f64 = 55.0;

/// The corner radius of an Android device's picture in dp (the emulator's frames mask the display's own corners).
pub const RADIUS_DP: f64 = 24.0;

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

/// The still line over a live picture (rule 1312): the source is broken (the simulator's still mode), or an Android
/// emulator gives only still pictures (PR A7, the spec's last fallback), with the reason.
fn still_line(broken: Option<&str>, live: bool, mode: FrameMode, why: &str) -> String {
    match (broken, live, mode) {
        (_, false, _) => String::new(),
        (Some(why), true, _) => {
            format!("Live view unavailable: {why}. Showing a picture every half second.")
        }
        (None, true, FrameMode::Still) => format!(
            "Live view unavailable: {}. Showing a still picture three times a second.",
            if why.is_empty() {
                "the emulator sends no frames"
            } else {
                why
            }
        ),
        _ => String::new(),
    }
}

/// The slow mark's text, empty when the picture is not slow: a CPU picture (the fake source), the ring's import
/// refused, or an Android emulator whose frames come in its gRPC messages (PR A7). Never on the zero-copy path.
fn slow_line(live: bool, ring_path: Option<FramePath>, mode: FrameMode, why: &str) -> String {
    if !live {
        return String::new();
    }
    if mode == FrameMode::Slow {
        return format!(
            "The emulator sends its picture the slow way (at most 30 a second): {}.",
            if why.is_empty() {
                "its shared memory gave no frames"
            } else {
                why
            }
        );
    }
    match ring_path {
        Some(FramePath::Cpu) | None => CPU_SLOW.to_string(),
        Some(_) => String::new(),
    }
}

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
    /// The Android SDK search (where Android devices can run).
    pub android: Option<AndroidSdk>,
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
    /// The device's target name (`sim/<udid>` lower case, or `emu/<avd>`, rule 1366) and when Flare booted it (Unix
    /// ms).
    pub devices: Vec<(String, u64)>,
}

impl Booted {
    pub fn has(&self, target: &str) -> bool {
        self.devices.iter().any(|(u, _)| u == target)
    }

    /// Flare booted `target` (its name) at `at_ms`. True when the list changed.
    pub fn record(&mut self, target: &str, at_ms: u64) -> bool {
        if self.has(target) {
            return false;
        }
        self.devices.push((target.to_string(), at_ms));
        true
    }

    /// `target` is no longer Flare's to shut down. True when the list changed.
    pub fn forget(&mut self, target: &str) -> bool {
        let n = self.devices.len();
        self.devices.retain(|(u, _)| u != target);
        n != self.devices.len()
    }

    /// The devices of one kind, as targets.
    pub fn of(&self, kind: flare_device::source::SourceKind) -> Vec<Target> {
        self.devices
            .iter()
            .filter_map(|(n, _)| target::parse(n))
            .filter(|t| t.source() == kind)
            .collect()
    }

    /// The emulators' arguments for `flare-emu-host --shutdown` (rule 1366): `<avd>@<booted ms>`, or the bare AVD name
    /// when the time is not known. The emu host shuts an emulator down only when it started before Flare's boot ended,
    /// so an entry left from an emulator that idled out never ends one the person started later (Android Studio's).
    pub fn emu_shutdown_args(&self) -> Vec<String> {
        self.devices
            .iter()
            .filter_map(|(n, at)| match target::parse(n)? {
                Target::Emu { avd } if *at > 0 => Some(format!("{avd}@{at}")),
                Target::Emu { avd } => Some(avd),
                _ => None,
            })
            .collect()
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

    /// `{"devices": [{"udid": "...", "booted_ms": 0}, {"target": "emu/<avd>", "booted_ms": 0}]}`: a simulator by its
    /// udid (as since PR 4b), any device by its target; an entry that is not a device's is skipped.
    pub fn from_json(v: &serde_json::Value) -> Booted {
        let mut b = Booted::default();
        for d in v["devices"].as_array().into_iter().flatten() {
            let t = d["udid"]
                .as_str()
                .and_then(Target::sim)
                .or_else(|| d["target"].as_str().and_then(target::parse));
            if let Some(t) = t {
                b.record(&t.name(), d["booted_ms"].as_u64().unwrap_or(0));
            }
        }
        b
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "devices": self
                .devices
                .iter()
                .map(|(n, at)| match target::parse(n) {
                    Some(Target::Sim { udid }) => serde_json::json!({"udid": udid, "booted_ms": at}),
                    _ => serde_json::json!({"target": n, "booted_ms": at}),
                })
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

/// This computer can run Android devices: Windows on x64, macOS on Apple silicon (the emulator's own platforms).
pub const fn android_platform() -> bool {
    cfg!(any(
        all(windows, target_arch = "x86_64"),
        all(target_os = "macos", target_arch = "aarch64")
    ))
}

/// The detached shutdown of Android emulators at quit (rule 1366): `flare-emu-host --shutdown <avd>...` asks each one
/// Flare booted for a clean shutdown and exits. Not guarded, so it outlives Flare; quit never waits for it.
pub fn emu_shutdown_command(exe: &Path, avds: &[String]) -> std::process::Command {
    let mut c = std::process::Command::new(exe); // sync-ok: builds the command; spawned detached
    c.arg("--shutdown")
        .args(avds)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    c
}

/// Starts `cmd` and lets it go: never waited for (a thread reaps it, so no zombie is left while Flare runs on). No
/// console window on Windows (rule 1370).
pub fn spawn_detached(mut cmd: std::process::Command) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(flare_proc::job::NO_WINDOW);
    }
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

/// What a real source needs to start: the Xcode the pick chose (the sim host), or nothing more (the emu host, which
/// finds the Android SDK itself and can set one up, Android plan PR A4).
#[derive(Clone, Debug)]
pub enum SourceSetup {
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    // only the macOS starter reads the Xcode
    Xcode(flare_device::xcode::Xcode),
    Android,
}

/// The Android SDK search's answer (a worker at startup; the emu host's setup answers it again): an emulator at or above
/// the floor (rule 1361) with how many AVDs it can boot, one under the floor, or none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AndroidSdk {
    Ready { avds: usize },
    TooOld,
    Missing,
}

impl AndroidSdk {
    /// BLOCKING (reads the SDK and AVD folders): a worker's.
    pub fn find(places: &flare_device::emu::Places) -> AndroidSdk {
        use flare_device::emu::{avds, sdk};
        match sdk::find(places) {
            sdk::Found::Ready(_) => AndroidSdk::Ready {
                avds: avds::list(&avds::homes(places)).len(),
            },
            sdk::Found::TooOld(_) => AndroidSdk::TooOld,
            sdk::Found::Missing => AndroidSdk::Missing,
        }
    }

    /// From the emu host's facts (`sdk`: ready, too_old or missing), keeping the AVD count when it does not say.
    pub fn from_fact(fact: &str, avds: usize) -> Option<AndroidSdk> {
        Some(match fact {
            "ready" => AndroidSdk::Ready { avds },
            "too_old" => AndroidSdk::TooOld,
            "missing" => AndroidSdk::Missing,
            _ => return None,
        })
    }
}

/// Makes the real source for its setup: the sim host for an Xcode, the emu host for Android. None when this build
/// cannot start that source (it is logged, and the panel stays as it is).
pub type Starter = Box<dyn FnMut(&SourceSetup) -> Option<Started>>;

/// Shuts simulators down at quit, detached (tests record it).
pub type ShutdownSpawn = Rc<dyn Fn(&Path, &[String])>;

/// Shuts Android emulators down at quit, detached (`flare-emu-host --shutdown <avd>...`, rule 1366; tests record it).
pub type EmuShutdown = Rc<dyn Fn(&[String])>;

/// The real sources' half of the controller (PR 4b; Android plan PR A4).
struct Real {
    starter: Starter,
    /// iOS simulators can run here (macOS on Apple silicon).
    ios: bool,
    /// The Xcode pick's answer: None while it runs.
    xcode: Option<Option<flare_device::xcode::Xcode>>,
    /// Android devices can run here.
    android: bool,
    /// The Android SDK search's answer: None while it runs.
    sdk: Option<AndroidSdk>,
    /// No device panel since then (ms on the clock): the hosts stop after `IDLE_MS`.
    idle_since: Option<u64>,
    booted_path: Option<PathBuf>,
    spawn: ShutdownSpawn,
    emu_spawn: Option<EmuShutdown>,
}

/// Where Android setup stands (spec "Setup", states 1 to 4).
#[derive(Clone, Debug, Default, PartialEq)]
enum Flow {
    #[default]
    Idle,
    /// The plan is read (Google's package lists).
    Planning,
    /// Google's licences, shown before anything is downloaded; their hashes go with [Accept and set up].
    Licence {
        text: String,
        hashes: Vec<String>,
        download_mb: u64,
    },
    Downloading {
        done_mb: u64,
        total_mb: u64,
    },
    /// [Cancel] was pressed during the download: its answer ends it.
    Cancelling,
    Failed(String),
    /// [Turn on] ran: Windows asks, then needs a restart.
    TurningOn,
    RestartNeeded,
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
    // Android's setup (spec 2026-11-22 "Setup", states 1 to 4 and 6), shown from Android plan PR A4.
    /// No Android emulator here yet: Flare can download it from Google (state 1).
    SdkMissing,
    /// Google's licence, before anything is downloaded (state 2). `text` is the licence from Google's package list,
    /// shown in full under the line; `download_mb` the download's size.
    Licence {
        text: String,
        download_mb: u64,
    },
    /// The download runs (state 3).
    Downloading {
        done_mb: u64,
        total_mb: u64,
    },
    /// Windows Hypervisor Platform is off (state 4).
    NoHypervisor,
    /// It was turned on: Windows must restart (state 4, after [Turn on]).
    RestartNeeded,
    /// An emulator started outside Flare without a gRPC token: Flare cannot reach it (state 6). Shown when its attach
    /// or boot answers so (Android PR A7).
    NoAccess,
    /// A new Android device's first start: its apps are compiled once before Flare calls it ready (A2b).
    Preparing {
        name: String,
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
            Setup::Preparing { name } => v(3, format!("Preparing {name}..."), None),
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
            Setup::Preparing { .. } => "preparing",
        }
    }
}

/// A row of the device chooser after the devices (Android PR A7, the existing row style; no mockup, Max's call).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChooserRow {
    /// A Mac with no Android emulator set up: Android's setup, the same states as on Windows.
    SetUpAndroid,
    /// Android's emulator is set up in Flare's own folder: look for a newer one (the same setup states; the plan finds
    /// nothing to fetch when it is current).
    UpdateAndroid,
}

impl ChooserRow {
    pub fn label(self) -> &'static str {
        match self {
            ChooserRow::SetUpAndroid => "Android: set up (about 2.5 GB from Google)",
            ChooserRow::UpdateAndroid => "Update Android emulator",
        }
    }
}

/// The chooser's rows after the devices: Android's setup on a Mac (`ios`) whose Android emulator is not set up yet,
/// and its update once it is and runs from Flare's own SDK folder (`own_sdk`: setup only ever fills that folder, so on
/// the person's own SDK an update would plan a whole new install). On Windows the panel itself shows Android's setup
/// states before any list.
pub fn chooser_rows(
    ios: bool,
    android: bool,
    android_gate: Option<&Setup>,
    own_sdk: bool,
) -> Vec<ChooserRow> {
    if !android {
        return Vec::new();
    }
    match android_gate {
        Some(Setup::SdkMissing) if ios => vec![ChooserRow::SetUpAndroid],
        None if own_sdk => vec![ChooserRow::UpdateAndroid],
        _ => Vec::new(),
    }
}

/// The setup plan's answer (`flare-emu-host`, A2b) as the panel's next state: Google's licences to accept, or a line
/// why setup cannot run (not enough space), or nothing to fetch (Idle).
fn plan_flow(v: &serde_json::Value) -> Flow {
    if !v["needed"].as_bool().unwrap_or(true) {
        return Flow::Idle;
    }
    if v["enough_space"].as_bool() == Some(false) {
        return Flow::Failed(format!(
            "Android setup needs about {} of free disk space.",
            size_words(v["disk_mb"].as_u64().unwrap_or(0)).trim_start_matches("about ")
        ));
    }
    let licences = v["licences"].as_array().cloned().unwrap_or_default();
    let text = licences
        .iter()
        .filter_map(|l| l["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    let hashes = licences
        .iter()
        .filter_map(|l| l["hash"].as_str().map(str::to_string))
        .collect();
    Flow::Licence {
        text,
        hashes,
        download_mb: v["download_mb"].as_u64().unwrap_or(0),
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
    /// A step of Android setup.
    Setup(SetupReq),
}

/// Which step of Android setup a request is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SetupReq {
    Plan,
    Start,
    Cancel,
    TurnOn,
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
    /// How an Android device's frames come now (PR A7: the spec's fallbacks), and why when not live.
    mode: FrameMode,
    mode_why: String,
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

/// The device source: the fake (`--e2e`, tests), another (tests), or the real ones (the simulator and the Android
/// emulator behind one, `device_sources.rs`).
enum Src {
    Fake(Box<FakeSource>),
    Other(Box<dyn DeviceSource>),
    Real(Box<Multi>),
}

impl Src {
    fn get(&mut self) -> &mut dyn DeviceSource {
        match self {
            Src::Fake(f) => f.as_mut(),
            Src::Other(s) => s.as_mut(),
            Src::Real(m) => m.as_mut(),
        }
    }

    /// The next event, with the kind of the source that sent it (None: the one source there is).
    fn next(&mut self) -> Option<(Option<SourceKind>, SourceEvent)> {
        match self {
            Src::Fake(f) => f.events().try_recv().ok().map(|e| (None, e)),
            Src::Other(s) => s.events().try_recv().ok().map(|e| (None, e)),
            Src::Real(m) => m.next(),
        }
    }

    fn get_ref(&self) -> &dyn DeviceSource {
        match self {
            Src::Fake(f) => f.as_ref(),
            Src::Other(s) => s.as_ref(),
            Src::Real(m) => m.as_ref(),
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
    /// A source cannot give live frames: the still line's reason (rule 1312), per kind (None: the one source there is).
    broken: HashMap<Option<SourceKind>, String>,
    /// Android setup (spec "Setup", states 1 to 4).
    flow: Flow,
    /// Android devices in their first start's compile ("Preparing").
    preparing: HashSet<Target>,
    /// The emu host's facts from the newest setup's answer (newer than its `hello`).
    emu_facts: HashMap<String, String>,
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
            broken: HashMap::new(),
            flow: Flow::Idle,
            preparing: HashSet::new(),
            emu_facts: HashMap::new(),
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

    #[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))] // only the macOS and Windows installs use it
    /// The real sources: `starter` makes the sim host (macOS on Apple silicon, once the Xcode pick found an Xcode)
    /// and the emu host (where `android`) when a device panel first needs them. `booted` is
    /// `<profile>/device/booted.json`; `spawn` shuts simulators down at quit, detached, and `emu_spawn` Android
    /// emulators.
    pub fn install_real(
        &mut self,
        starter: Starter,
        booted_path: Option<PathBuf>,
        spawn: ShutdownSpawn,
    ) {
        self.src = None;
        self.real = Some(Real {
            starter,
            ios: real_platform(),
            xcode: None,
            android: false,
            sdk: None,
            idle_since: None,
            booted_path,
            spawn,
            emu_spawn: None,
        });
    }

    /// Android devices can run here (Windows; macOS on Apple silicon): the emu host is started with the first device
    /// panel, `emu_spawn` shuts the emulators Flare booted down at quit.
    pub fn install_android(&mut self, emu_spawn: EmuShutdown) {
        if let Some(r) = self.real.as_mut() {
            r.android = true;
            r.emu_spawn = Some(emu_spawn);
        }
    }

    /// Tests: iOS simulators can (or cannot) run here, whatever this computer is.
    #[cfg(test)]
    pub fn set_ios(&mut self, on: bool) {
        if let Some(r) = self.real.as_mut() {
            r.ios = on;
        }
    }

    #[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))] // only the real installs use it
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

    /// iPhone simulators are offered: macOS on Apple silicon with an Xcode found.
    fn ios_offered(&self) -> bool {
        let Some(r) = &self.real else { return false };
        let xcode = r.xcode.as_ref().map(|x| {
            if x.is_some() {
                self.xcode
            } else {
                Xcode::Missing
            }
        });
        r.ios && menu_entry_offered(cfg!(target_os = "macos"), real_platform(), xcode)
    }

    /// Android devices are offered wherever they can run (Windows x64, a Mac on Apple silicon): Flare sets the emulator
    /// up itself (spec "Setup"). On a Mac the setup is a row of the device chooser (Android PR A7).
    fn android_offered(&self) -> bool {
        self.real.as_ref().is_some_and(|r| r.android)
    }

    /// The chooser's rows after the devices (Android PR A7).
    fn chooser_rows(&self) -> Vec<ChooserRow> {
        let Some(r) = &self.real else {
            return Vec::new();
        };
        // An update fills Flare's own SDK folder only, so it is offered only while Android runs from it.
        let own = self.emu_fact("sdk_own").as_deref() == Some("true");
        chooser_rows(
            r.ios,
            self.android_offered(),
            self.android_gate().as_ref(),
            own,
        )
    }

    /// The tab menu's Device panel entry: the fake source (`--e2e`), or a real one this computer can run.
    pub fn menu_offered(&self) -> bool {
        if matches!(self.src, Some(Src::Fake(_))) {
            return true;
        }
        self.ios_offered() || self.android_offered()
    }

    /// The startup pick answered: the Xcode found (macOS), the Android SDK found, and the devices booted in an earlier
    /// run (`booted.json`).
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
            None if real.ios => {
                log::info!("device: no Xcode found: the device panel says it needs Xcode");
                self.xcode = Xcode::Missing;
            }
            None => self.xcode = Xcode::Missing,
        }
        real.xcode = Some(answer.xcode);
        if let Some(sdk) = answer.android {
            log::info!("device: Android SDK: {sdk:?}");
            real.sdk = Some(sdk);
        }
        for (u, at) in answer.booted.devices {
            self.booted.record(&u, at);
        }
    }

    /// The Xcode app the pick chose ("Open Xcode").
    pub fn xcode_app(&self) -> Option<PathBuf> {
        Some(self.real.as_ref()?.xcode.clone()??.app)
    }

    /// Starts each real source that is installed, can run and is not running.
    fn ensure_source(&mut self) {
        if matches!(self.src, Some(Src::Fake(_) | Src::Other(_))) {
            return;
        }
        // On a Mac the emu host starts only once Android has an SDK there, or its setup runs (the chooser's row, PR
        // A7): an iPhone-only Mac never starts it.
        let want_emu = self.android_offered()
            && self.real.as_ref().is_some_and(|r| {
                !r.ios
                    || matches!(r.sdk, Some(AndroidSdk::Ready { .. } | AndroidSdk::TooOld))
                    || self.flow != Flow::Idle
            });
        let Some(real) = self.real.as_mut() else {
            return;
        };
        let xcode = real.xcode.clone().flatten().filter(|_| real.ios);
        let Src::Real(multi) = self.src.get_or_insert_with(|| Src::Real(Box::default())) else {
            return;
        };
        if let Some(x) = xcode
            && !multi.has(SourceKind::Sim)
        {
            match (real.starter)(&SourceSetup::Xcode(x)) {
                Some(started) => {
                    #[cfg(target_os = "macos")]
                    self.frames.borrow_mut().set_source(started.ring);
                    multi.set(SourceKind::Sim, Some(started.source));
                    log::info!("device: the sim host is started");
                }
                None => log::warn!("device: the sim host cannot start in this build"),
            }
        }
        if want_emu && !multi.has(SourceKind::Emu) {
            match (real.starter)(&SourceSetup::Android) {
                Some(started) => {
                    multi.set(SourceKind::Emu, Some(started.source));
                    log::info!("device: the emu host is started");
                }
                None => log::warn!("device: the emu host cannot start in this build"),
            }
        }
        real.idle_since = None;
        if multi.is_empty() {
            self.src = None;
        }
    }

    /// No device panel for `IDLE_MS`: the hosts stop (each is told `quit`; its guard ends it either way).
    fn idle(&mut self, has_panels: bool) {
        let now = self.now();
        let running = matches!(self.src, Some(Src::Real(_)));
        let Some(real) = self.real.as_mut() else {
            return;
        };
        // Rule 1366 (PR A7): an emulator Flare booted must not idle out while Flare runs, so the emu host (whose
        // keep-alives hold it) stays while one is alive. Its entry goes when it is seen shut down or gone; then the
        // hosts stop as usual. Only a dead Flare lets it idle out.
        let keeps = self
            .booted
            .devices
            .iter()
            .any(|(u, _)| target::parse(u).is_some_and(|t| t.source() == SourceKind::Emu));
        if has_panels || !running || keeps {
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
        real.idle_since = None;
        self.stop_source();
        log::info!("device: no device panel for two minutes: the device hosts stop");
    }

    /// Drops the running sources (their `Drop` tells each host to quit) and what they held. The frame thread lets go
    /// of every Android region first, so their files can go.
    fn stop_source(&mut self) {
        #[cfg(target_os = "macos")]
        self.frames.borrow_mut().set_source(None);
        let emus: Vec<Target> = self
            .devs
            .keys()
            .filter(|t| t.source() == SourceKind::Emu)
            .cloned()
            .collect();
        for t in emus {
            self.frames.borrow_mut().region().release(&t);
        }
        self.src = None;
        self.hello_logged = false;
        self.pending.clear();
        self.devs.clear();
        self.devices = None;
        self.listing = false;
        self.relist_at = None;
        self.broken.clear();
        self.preparing.clear();
        if matches!(
            self.flow,
            Flow::Planning | Flow::Downloading { .. } | Flow::Cancelling
        ) {
            self.flow = Flow::Idle;
        }
    }

    /// The fake source, when it is the one in use (tests and the e2e probe).
    pub fn fake(&self) -> Option<&FakeSource> {
        match &self.src {
            Some(Src::Fake(f)) => Some(f),
            _ => None,
        }
    }

    /// Tests: the fake source becomes `fake` (the app's input tests on the Android fake, Android PR A7).
    #[cfg(test)]
    pub fn use_fake(&mut self, fake: FakeSource) {
        self.src = Some(Src::Fake(Box::new(fake)));
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
            // An Android region is let go by the frame thread before the emu host's client deletes its file.
            if t.source() == SourceKind::Emu {
                self.frames.borrow_mut().region().release(&t);
            }
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
        while let Some((kind, ev)) = self.src.as_mut().and_then(Src::next) {
            self.on_event(kind, ev);
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

    fn on_event(&mut self, kind: Option<SourceKind>, ev: SourceEvent) {
        // Only a frame (or the changed count) leaves the panel as it is: the notifier draws it.
        let quiet = match &ev {
            SourceEvent::Frame { target, .. } | SourceEvent::Region { target, .. } => self
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
                        // Only a kind whose source answered can say a device is gone.
                        for (u, _) in self.booted.devices.clone() {
                            let listed =
                                target::parse(&u).is_some_and(|t| self.lists_kind(t.source()));
                            let seen = d.iter().find(|x| x.target.name() == u);
                            if listed && seen.is_none_or(|x| x.state == DeviceState::Shutdown) {
                                forgot |= self.booted.forget(&u);
                            }
                        }
                        for x in &mut d {
                            x.booted_by_flare = self.booted.has(&x.target.name());
                            // A boot Flare asked for is under way until it answers: a list read meanwhile (simctl
                            // may still say shut down) does not take the panel back to Boot.
                            if self.booting.contains_key(&x.target)
                                && x.state == DeviceState::Shutdown
                            {
                                x.state = DeviceState::Booting;
                            }
                        }
                        if forgot {
                            self.save_booted();
                        }
                        // Rule 1366 (PR A7): every emulator Flare booted that runs is kept alive while Flare runs,
                        // attached or not (one from an earlier run of Flare too).
                        let keep: Vec<Target> = d
                            .iter()
                            .filter(|x| {
                                x.booted_by_flare
                                    && x.target.source() == SourceKind::Emu
                                    && x.state == DeviceState::Booted
                            })
                            .map(|x| x.target.clone())
                            .collect();
                        if let Some(src) = self.source() {
                            for t in &keep {
                                src.keep(t);
                            }
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
                        self.preparing.remove(&t);
                        match r {
                            Err(e) => {
                                self.agent_event(&AgentEvent::Booted(
                                    t.clone(),
                                    Err(flare_ctl::CtlError::new(e.fail.code(), e.message.clone())),
                                ));
                                // Not left "Starting ...": the list says what the device is now.
                                self.settled(&t, DeviceState::Shutdown);
                                self.fail(&t, e.message);
                            }
                            Ok(a) => {
                                // Flare booted it: shut down at quit (rule 1319), also after a crash; only such a
                                // device may the agent shut down (rule 1322). The file is the real source's. An
                                // emulator that already ran (Android Studio's) is waited for but never Flare's
                                // (rule 1366). An emulator's entry takes this boot's time: quit checks it against
                                // the running emulator's start, so a stale entry never ends another one.
                                let flares = !flare_device::emu_client::is_started_elsewhere(&a);
                                if flares {
                                    let name = t.name();
                                    if t.source() == SourceKind::Emu {
                                        self.booted.forget(&name);
                                    }
                                    if self.booted.record(&name, unix_ms()) && self.real.is_some() {
                                        self.save_booted();
                                    }
                                }
                                self.agent_event(&AgentEvent::Booted(t.clone(), Ok(())));
                                if flares
                                    && let Some(x) = self
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
                    (Pending::Setup(step), r) => self.on_setup_answer(step, r),
                }
            }
            SourceEvent::Frames { target, screen, .. } => {
                self.agent_event(&AgentEvent::NewSession(target.clone()));
                let d = self.devs.entry(target).or_default();
                d.screen = Some(screen);
                // A new attach starts on the normal path; the emu host says again if it falls back.
                d.mode = FrameMode::Live;
                d.mode_why.clear();
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
            // A frame in an Android device's region (rule 1364): the frame thread already copied it into the ring and
            // woke the window; here the panel only learns that the device has a picture.
            // The emulator sends a frame only when its screen changed, so each record is also the device's "changed"
            // count (the sim host counts its own): the agent's input calls answer `changed: true` from it.
            SourceEvent::Region { target, .. } => {
                if let Some(d) = self.devs.get_mut(&target)
                    && d.attached
                {
                    d.ring_frame = true;
                    d.changed += 1;
                    let count = d.changed;
                    self.agent_event(&AgentEvent::Changed(target, count));
                }
            }
            SourceEvent::Setup(e) => self.on_setup_event(e),
            // A ring frame: the notifier takes the newest in its next render (`DeviceFrames`).
            SourceEvent::Frame { target, .. } => {
                if let Some(d) = self.devs.get_mut(&target)
                    && d.attached
                {
                    d.ring_frame = true;
                    self.new_frame = true;
                }
            }
            // The sim host's count is its own running count. An Android device's count is Flare's (one per frame
            // record), so a change the emu host saw while paused (the agent helper's window events, PR A7) adds one:
            // the count only ever grows, and input calls answer `changed: true` while the stream is paused too.
            SourceEvent::Changed { target, count } => {
                let count = match self.devs.get_mut(&target) {
                    Some(d) if target.source() == SourceKind::Emu => {
                        d.changed += 1;
                        d.changed
                    }
                    Some(d) => {
                        d.changed = count;
                        count
                    }
                    None => count,
                };
                self.agent_event(&AgentEvent::Changed(target, count));
            }
            SourceEvent::Mode { target, mode, why } => {
                if let Some(d) = self.devs.get_mut(&target) {
                    if d.mode != mode {
                        log::info!(
                            "device: {target}: frames {mode:?}{}",
                            if why.is_empty() {
                                String::new()
                            } else {
                                format!(": {why}")
                            }
                        );
                    }
                    d.mode = mode;
                    d.mode_why = why;
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
                        self.preparing.remove(&target);
                        // Booted by someone else meanwhile: a boot call waiting for it answers too.
                        self.agent_event(&AgentEvent::Booted(target.clone(), Ok(())));
                    }
                    DeviceState::Booting => {
                        let now = self.now();
                        self.booting.entry(target).or_insert(now);
                    }
                    DeviceState::Shutdown | DeviceState::ShuttingDown => {
                        self.booting.remove(&target);
                        if state == DeviceState::Shutdown && self.booted.forget(&target.name()) {
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
                self.broken.insert(kind, why);
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
        if let Some(gate) = self.gate(target::parse(&t.target).map(|t| t.source())) {
            return Some(gate);
        }
        if let Some(why) = t.failed.as_ref().or(self.list_error.as_ref()) {
            // An emulator started without access for other apps (spec state 6): its own line, no button.
            if why == flare_device::emu::discovery::NO_ACCESS {
                return Some(Setup::NoAccess);
            }
            return Some(Setup::Failed { why: why.clone() });
        }
        let Some(devices) = &self.devices else {
            return Some(Setup::Reading);
        };
        let target = target::parse(&t.target);
        if t.choosing || target.is_none() {
            // An empty list on a computer with only Android: setup makes its device (NoRuntime is iOS's line).
            let android_only = self.real.as_ref().is_some_and(|r| r.android && !r.ios);
            return Some(if devices.is_empty() && android_only {
                Setup::SdkMissing
            } else if devices.is_empty() {
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
        if self.preparing.contains(&target) {
            return Some(Setup::Preparing { name });
        }
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

    /// The setup state that comes before any device of `kind` (None: the panel shows the device list), or None when
    /// that kind is ready.
    fn gate(&self, kind: Option<SourceKind>) -> Option<Setup> {
        let Some(real) = &self.real else {
            // The fake or a test's own source: the iOS states, as before.
            return self.xcode_gate();
        };
        let ios = || {
            if real.xcode.is_none() {
                Some(Setup::Reading)
            } else {
                self.xcode_gate()
            }
        };
        match kind {
            Some(SourceKind::Sim) => ios(),
            Some(SourceKind::Emu) => self.android_gate(),
            // Android setup started from the chooser's row on a Mac (PR A7): its states show until it ends.
            None if real.ios && self.flow != Flow::Idle => self.android_gate(),
            // The list: one with both kinds on a Mac (iPhones first), Android only on Windows (spec "Setup", 5).
            None if real.ios => match ios() {
                Some(Setup::Reading) => Some(Setup::Reading),
                Some(s) if !(self.android_offered() && self.android_gate().is_none()) => Some(s),
                _ => None,
            },
            None if real.android => self.android_gate(),
            None => ios(),
        }
    }

    fn xcode_gate(&self) -> Option<Setup> {
        match self.xcode {
            Xcode::Missing => Some(Setup::NoXcode),
            Xcode::NotSetUp => Some(Setup::NotSetUp),
            Xcode::Ready => None,
        }
    }

    /// Android's setup states (spec "Setup", states 1 to 4), or None once its emulator can run here.
    fn android_gate(&self) -> Option<Setup> {
        let real = self.real.as_ref()?;
        match &self.flow {
            Flow::Planning | Flow::Cancelling | Flow::TurningOn => return Some(Setup::Reading),
            Flow::Licence {
                text, download_mb, ..
            } => {
                return Some(Setup::Licence {
                    text: text.clone(),
                    download_mb: *download_mb,
                });
            }
            Flow::Downloading { done_mb, total_mb } => {
                return Some(Setup::Downloading {
                    done_mb: *done_mb,
                    total_mb: *total_mb,
                });
            }
            Flow::Failed(why) => return Some(Setup::Failed { why: why.clone() }),
            Flow::RestartNeeded => return Some(Setup::RestartNeeded),
            Flow::Idle => {}
        }
        match real.sdk? {
            // No device yet: setup writes "Flare Pixel".
            AndroidSdk::Missing | AndroidSdk::TooOld | AndroidSdk::Ready { avds: 0 } => {
                Some(Setup::SdkMissing)
            }
            AndroidSdk::Ready { .. }
                if cfg!(windows) && self.emu_fact("accel_usable") == Some("false".into()) =>
            {
                Some(Setup::NoHypervisor)
            }
            AndroidSdk::Ready { .. } => None,
        }
    }

    /// One of the emu host's facts: the newest setup's answer first (setup finds the SDK and checks the hypervisor
    /// again), else its `hello`, when it runs.
    fn emu_fact(&self, key: &str) -> Option<String> {
        if let Some(v) = self.emu_facts.get(key) {
            return Some(v.clone());
        }
        let Some(Src::Real(m)) = &self.src else {
            return None;
        };
        m.hello_of(SourceKind::Emu)?.facts.get(key).cloned()
    }

    /// The source of `kind` answers lists (a real source of another kind that is not running says nothing).
    fn lists_kind(&self, kind: SourceKind) -> bool {
        match &self.src {
            Some(Src::Real(m)) => m.has(kind),
            Some(_) => true,
            None => false,
        }
    }

    /// Why `t`'s source gives no live frames (rule 1312), if it does not.
    fn broken_for(&self, t: Option<&Target>) -> Option<&String> {
        self.broken
            .get(&None)
            .or_else(|| self.broken.get(&Some(t?.source())))
    }

    /// The device the tab's panel shows, if it names one.
    fn target_of(&self, tab: &str) -> Option<Target> {
        target::parse(&self.tabs.get(tab)?.target)
    }

    /// The shown tab drives its device (not view only), and it is live with a picture.
    fn drives(&self, tab: &str) -> Option<Target> {
        let t = self.target_of(tab)?;
        let d = self.devs.get(&t)?;
        (d.driver.as_deref() == Some(tab) && d.attached && self.broken_for(Some(&t)).is_none())
            .then_some(t)
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
                app.set_panel_device_dim(false);
                app.set_panel_device_foot("".into());
                app.set_panel_device_detail("".into());
                app.set_panel_device_has_cancel(false);
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
        let broken = self.broken_for(target.as_ref()).cloned();
        if let Some(t) = &target
            && let Some(d) = self.devs.get_mut(t)
        {
            // The ring's frames: the notifier draws them; Slint turns the simulator's portrait picture (spike S5). An
            // Android frame comes turned already (a landscape frame is wider than high).
            if d.ring_frame && d.attached && !d.paused && broken.is_none() {
                ring = Some(t.clone());
                if t.source() == SourceKind::Sim {
                    turn = d.screen.map_or(0, |s| turn_degrees(s.orientation));
                }
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
        // The licence screen's second button and Google's licence text under its line (spec "Setup", 2).
        app.set_panel_device_has_cancel(view.cancel);
        let detail = match &setup {
            Setup::Licence { text, .. } => text.as_str(),
            _ => "",
        };
        app.set_panel_device_detail(detail.into());
        // An Android device's More menu has Back and Recents instead of Shake (rule 1371).
        app.set_panel_device_android(
            target
                .as_ref()
                .is_some_and(|t| t.source() == SourceKind::Emu),
        );
        app.set_panel_device_has_frame(pictured);
        if !pictured {
            app.set_panel_device_frame(Image::default());
            self.pushed.seq = None;
        }
        let pt = target.as_ref().and_then(|t| self.screen_pt(t));
        app.set_panel_device_pt_w(pt.map_or(0.0, |s| s.w as f32));
        app.set_panel_device_pt_h(pt.map_or(0.0, |s| s.h as f32));
        let radius = match target.as_ref().map(Target::source) {
            Some(SourceKind::Emu) => RADIUS_DP,
            _ => RADIUS_PT,
        };
        app.set_panel_device_radius_pt(radius as f32);
        let rows: Vec<(String, String, bool)> = if view.state == 2 {
            self.devices
                .iter()
                .flatten()
                .map(|d| (d.name.clone(), d.os.clone(), d.state == DeviceState::Booted))
                // Android's own rows after the devices, in the same style (PR A7).
                .chain(
                    self.chooser_rows()
                        .into_iter()
                        .map(|r| (r.label().to_string(), String::new(), false)),
                )
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
        let (mode, why) = target
            .as_ref()
            .and_then(|t| self.devs.get(t))
            .map(|d| (d.mode, d.mode_why.clone()))
            .unwrap_or_default();
        app.set_panel_device_still(still_line(broken.as_deref(), live, mode, &why).into());
        app.set_panel_device_slow(
            slow_line(live && broken.is_none(), ring_path, mode, &why).into(),
        );
        // The display is off: the last picture stays, dimmed (spec "Frames").
        app.set_panel_device_dim(live && pictured && mode == FrameMode::Asleep);
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
                // The header's Lock button is Android's Power button (spec "Input": the iOS panel's buttons).
                let b = if b == Button::Lock && t.source() == SourceKind::Emu {
                    Button::Power
                } else {
                    b
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
                if matches!(self.flow, Flow::Failed(_)) {
                    self.flow = Flow::Idle;
                }
                self.list_error = None;
                if let Some(t) = self.target_of(&tab)
                    && let Some(d) = self.devs.get_mut(&t)
                {
                    d.attaching = false;
                }
                self.list();
            }
            // Android setup (rule 1361): nothing is downloaded before [Accept and set up].
            SetupButton::SetUp => self.setup_step(SetupStep::Plan, SetupReq::Plan, Flow::Planning),
            SetupButton::AcceptAndSetUp => {
                if let Flow::Licence {
                    hashes,
                    download_mb,
                    ..
                } = self.flow.clone()
                {
                    self.setup_step(
                        SetupStep::Start { accepted: hashes },
                        SetupReq::Start,
                        Flow::Downloading {
                            done_mb: 0,
                            total_mb: download_mb,
                        },
                    );
                }
            }
            SetupButton::Cancel => self.setup_cancel(),
            SetupButton::TurnOn => {
                self.setup_step(
                    SetupStep::TurnOnHypervisor,
                    SetupReq::TurnOn,
                    Flow::TurningOn,
                );
            }
        }
        self.drain();
        Vec::new()
    }

    /// The licence screen's [Cancel] (the second button): back to "Set up", nothing downloaded.
    pub fn cancel_clicked(&mut self) -> Vec<Effect> {
        self.setup_cancel();
        self.drain();
        Vec::new()
    }

    /// Sends one step of Android setup to the emu host and moves the flow on; with no emu host it says why.
    fn setup_step(&mut self, step: SetupStep, req: SetupReq, flow: Flow) {
        match self.source().and_then(|s| s.setup(step)) {
            Some(id) => {
                self.pending.insert(id, Pending::Setup(req));
                self.flow = flow;
            }
            None => {
                self.flow =
                    Flow::Failed("Android setup is not available in this Flare.".to_string());
            }
        }
    }

    /// [Cancel]: the licence goes away; a download stops (its answer ends it, nothing half-unpacked is left).
    fn setup_cancel(&mut self) {
        match self.flow {
            Flow::Licence { .. } | Flow::Failed(_) => self.flow = Flow::Idle,
            Flow::Downloading { .. } => {
                if let Some(id) = self.source().and_then(|s| s.setup(SetupStep::Cancel)) {
                    self.pending.insert(id, Pending::Setup(SetupReq::Cancel));
                }
                self.flow = Flow::Cancelling;
            }
            _ => {}
        }
    }

    /// An answer to a step of Android setup.
    fn on_setup_answer(
        &mut self,
        req: SetupReq,
        r: Result<Answer, flare_device::source::SourceError>,
    ) {
        match (req, r) {
            (SetupReq::Cancel, _) => {}
            (SetupReq::Plan, Ok(Answer::Value(v))) => {
                self.flow = plan_flow(&v);
                if self.flow == Flow::Idle {
                    // Nothing to fetch: the SDK is there after all.
                    self.sdk_found(AndroidSdk::Ready { avds: 1 });
                }
            }
            (SetupReq::Start, Ok(Answer::Value(v))) => {
                let facts = &v["facts"];
                for (k, f) in facts.as_object().into_iter().flatten() {
                    if let Some(s) = f.as_str() {
                        self.emu_facts.insert(k.clone(), s.to_string());
                    }
                }
                log::info!("device: Android setup done: {v}");
                let sdk = facts["sdk"]
                    .as_str()
                    .and_then(|f| AndroidSdk::from_fact(f, 1))
                    .unwrap_or(AndroidSdk::Ready { avds: 1 });
                self.flow = Flow::Idle;
                self.sdk_found(sdk);
            }
            (SetupReq::TurnOn, Ok(Answer::Value(v))) => {
                self.flow = if v["restart"].as_bool().unwrap_or(true) {
                    Flow::RestartNeeded
                } else {
                    Flow::Idle
                };
            }
            (SetupReq::Start, Err(e)) if e.fail == flare_device::job::Fail::Interrupted => {
                self.flow = Flow::Idle;
            }
            (_, Err(e)) => {
                log::warn!("device: Android setup: {}", e.message);
                self.flow = Flow::Failed(e.message);
            }
            (_, Ok(_)) => self.flow = Flow::Idle,
        }
    }

    /// The Android SDK is known again (after setup): the list is read again with it.
    fn sdk_found(&mut self, sdk: AndroidSdk) {
        if let Some(r) = self.real.as_mut() {
            r.sdk = Some(sdk);
        }
        self.devices = None;
        self.list_error = None;
        self.list();
    }

    /// Android setup's progress, or a new device's first-boot compile.
    fn on_setup_event(&mut self, e: flare_device::wire::SetupEvent) {
        use flare_device::wire::SetupEvent as E;
        match e {
            E::Download {
                done_mb, total_mb, ..
            } => {
                if matches!(self.flow, Flow::Downloading { .. } | Flow::Planning) {
                    self.flow = Flow::Downloading { done_mb, total_mb };
                }
            }
            E::Unpack { .. } => {}
            E::Preparing { target } => {
                self.preparing.insert(target);
            }
        }
    }

    /// Boots a device the person asked for (never on restore, never on its own: rule 1317's half here).
    fn boot(&mut self, t: &Target) {
        let now = self.now();
        self.booting.insert(t.clone(), now);
        // The panel shows "Starting ..." from the click on: the sim host sends no state of its own until the boot
        // answers, which can take 10 s and more (Max, 2026-11-22: "nothing happens on click").
        if let Some(d) = self
            .devices
            .as_mut()
            .and_then(|l| l.iter_mut().find(|d| &d.target == t))
            && d.state != DeviceState::Booted
        {
            d.state = DeviceState::Booting;
        }
        if let Some(id) = self.source().map(|s| s.boot(t)) {
            self.pending.insert(id, Pending::Boot(t.clone()));
        }
    }

    /// The person picked row `i` of the list: the panel shows that device, booted if it is not. A row after the
    /// devices is one of Android's own (PR A7): it starts Android's setup, which plans first and shows Google's
    /// licence before anything is downloaded (rule 1361).
    pub fn pick(&mut self, i: i32) -> Vec<Effect> {
        let Some(tab) = self.shown.clone() else {
            return Vec::new();
        };
        let n = self.devices.as_ref().map_or(0, Vec::len);
        if let Some(row) = usize::try_from(i)
            .ok()
            .and_then(|i| i.checked_sub(n))
            .and_then(|k| self.chooser_rows().get(k).copied())
        {
            match row {
                ChooserRow::SetUpAndroid | ChooserRow::UpdateAndroid => {
                    // The emu host does the setup: started now if it is not running (a Mac with no SDK yet).
                    self.flow = Flow::Planning;
                    self.ensure_source();
                    self.setup_step(SetupStep::Plan, SetupReq::Plan, Flow::Planning);
                }
            }
            self.drain();
            return Vec::new();
        }
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
        // A ring frame (iOS), or a region frame the frame thread made ready (Android): a redraw draws it.
        let region = self.frames.borrow_mut().region().take_new();
        let framed = std::mem::take(&mut self.new_frame) || region;
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
    /// down. The Android emulators Flare booted are asked to shut down the same way (rule 1366): one detached
    /// `flare-emu-host --shutdown <avd>...`. Returns the udids and AVD names handed over.
    pub fn quit(&mut self, shutdown_on: bool) -> Vec<String> {
        let Some(real) = self.real.as_ref() else {
            return Vec::new();
        };
        let dev_dir = real.xcode.clone().flatten().map(|x| x.developer_dir());
        let spawn = Rc::clone(&real.spawn);
        let emu_spawn = real.emu_spawn.clone();
        let path = real.booted_path.clone();
        self.stop_source();
        if !shutdown_on {
            return Vec::new();
        }
        let udids: Vec<String> = self
            .booted
            .of(SourceKind::Sim)
            .iter()
            .map(|t| t.id().to_string())
            .collect();
        let avds: Vec<String> = self
            .booted
            .of(SourceKind::Emu)
            .iter()
            .map(|t| t.id().to_string())
            .collect();
        let mut handed = Vec::new();
        if let Some(dev_dir) = dev_dir
            && !udids.is_empty()
        {
            log::info!(
                "device: shutting down {} simulator(s) Flare booted",
                udids.len()
            );
            spawn(&dev_dir, &udids);
            handed.extend(udids);
        }
        if let Some(emu_spawn) = emu_spawn
            && !avds.is_empty()
        {
            log::info!(
                "device: shutting down {} Android emulator(s) Flare booted",
                avds.len()
            );
            emu_spawn(&self.booted.emu_shutdown_args());
            handed.extend(avds);
        }
        if handed.is_empty() {
            return handed;
        }
        self.booted = Booted::default();
        if let Some(p) = path {
            self.booted.save(&p);
        }
        handed
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
        let (copied, dropped, p50, p99) = fr.region_stats();
        let region =
            json!({"copied": copied, "dropped": dropped, "copy_ms_p50": p50, "copy_ms_p99": p99});
        // The shown device's own buttons (rule 1371) and its frame mode (Android PR A7).
        let buttons: Vec<&str> = target
            .as_ref()
            .map(|t| t.source().buttons().iter().map(|b| b.name()).collect())
            .unwrap_or_default();
        let mode = dev.map(|d| format!("{:?}", d.mode).to_lowercase());
        json!({
            "buttons": buttons,
            "frame_mode": mode,
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
            "region": region,
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

    /// Max, 2026-11-22: "nothing happens on click, it just eventually starts but it can take 10+ seconds". The
    /// device reads booting from the click on, and a list read while it boots does not take it back to shut down.
    #[test]
    fn a_boot_shows_starting_from_the_click_on() {
        let b = "00000000-0000-4000-8000-00000000000b";
        let mut fake = FakeSource::new(vec![
            flare_device::fake::iphone(b, "iPhone B", DeviceState::Shutdown).unwrap(),
        ])
        .without_state_events();
        // A slow boot that has not answered yet (the fake's clock is not moved on here).
        fake.set_boot_ms(60_000);
        let (_spawned, spawn) = recorder();
        let mut d = DeviceCtl::new(post(), Rc::new(|| 0), None);
        d.install_real(starter_of(fake), None, spawn);
        d.set_ios(true);
        d.on_xcode(XcodeAnswer {
            android: None,
            xcode: Some(found_xcode()),
            booted: Booted::default(),
        });
        d.ensure_source();
        d.list();
        let tb = Target::sim(b).unwrap();
        d.boot(&tb);
        d.drain();
        assert_eq!(d.state_of(&tb), Some(DeviceState::Booting), "at once");
        d.list();
        assert_eq!(
            d.state_of(&tb),
            Some(DeviceState::Booting),
            "a list read while it boots keeps it booting"
        );
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
        d.set_ios(true);
        d.on_xcode(XcodeAnswer {
            android: None,
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
        d.set_ios(true);
        assert!(
            d.offered(),
            "the kind is offered before the pick answers (restore keeps the panel)"
        );
        d.on_xcode(XcodeAnswer {
            android: None,
            xcode: Some(found_xcode()),
            booted: Booted {
                devices: vec![(format!("sim/{c}"), 1)],
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
        d.set_ios(true);
        d.on_xcode(XcodeAnswer {
            android: None,
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
        assert!(b.record("sim/00000000-0000-4000-8000-00000000000b", 7));
        assert!(
            !b.record("sim/00000000-0000-4000-8000-00000000000b", 8),
            "once"
        );
        let v = b.to_json();
        assert_eq!(Booted::from_json(&v), b);
        let odd = serde_json::json!({"devices": [{"udid": "../etc"}, {"udid": 3}, {"udid": "00000000-0000-4000-8000-00000000000c"}]});
        assert_eq!(Booted::from_json(&odd).devices.len(), 1);
        assert_eq!(
            Booted::from_json(&serde_json::json!("nonsense")),
            Booted::default()
        );
        assert!(b.forget("sim/00000000-0000-4000-8000-00000000000b"));
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
        d.set_ios(true);
        assert!(!d.menu_offered(), "before the pick answers");
        d.on_xcode(XcodeAnswer {
            android: None,
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
        none.set_ios(true);
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

    /// The Android source's place taken by the Android fake: what `install_real` starts for Android.
    fn android_starter(fake: FakeSource) -> Starter {
        let mut fake = Some(fake);
        Box::new(move |setup| match setup {
            SourceSetup::Android => Some(Started {
                source: Box::new(fake.take()?),
                #[cfg(target_os = "macos")]
                ring: None,
            }),
            SourceSetup::Xcode(_) => None,
        })
    }

    type Quits = Rc<RefCell<Vec<Vec<String>>>>;

    /// A controller with only the Android source (as on Windows), the SDK search answered with `sdk`.
    fn android_ctl(sdk: AndroidSdk) -> (DeviceCtl, Quits) {
        let mut d = DeviceCtl::new(post(), Rc::new(|| 0), None);
        d.install_real(android_starter(FakeSource::android()), None, recorder().1);
        d.set_ios(false);
        let quits: Quits = Rc::default();
        let q = Rc::clone(&quits);
        d.install_android(Rc::new(move |avds: &[String]| {
            q.borrow_mut().push(avds.to_vec());
        }));
        d.on_xcode(XcodeAnswer {
            xcode: None,
            android: Some(sdk),
            booted: Booted::default(),
        });
        (d, quits)
    }

    /// The panel on `target` is the front one, shown in `tab`.
    fn show_tab(d: &mut DeviceCtl, tab: &str, target: &str) {
        d.tabs.insert(
            tab.into(),
            TabDev {
                target: target.into(),
                ..TabDev::default()
            },
        );
        d.shown = Some(tab.into());
    }

    /// Rule 1360 and 1366: an Android device is listed, booted (and remembered as Flare's), and asked to shut down at
    /// quit by AVD name, detached.
    #[test]
    fn an_android_device_boots_and_shuts_down_at_quit() {
        let (mut d, quits) = android_ctl(AndroidSdk::Ready { avds: 1 });
        assert!(d.menu_offered());
        assert!(d.agent_offered(), "an emulator at the floor with a device");
        d.ensure_source();
        d.list();
        let t = Target::emu("Flare_Pixel").unwrap();
        assert_eq!(d.state_of(&t), Some(DeviceState::Shutdown));
        show_tab(&mut d, "t1", "emu/Flare_Pixel");
        assert_eq!(
            d.setup_of("t1"),
            Some(Setup::ShutDown {
                name: "Flare Pixel".into(),
                again: false
            })
        );
        d.boot(&t);
        d.drain();
        assert_eq!(d.state_of(&t), Some(DeviceState::Booted));
        assert!(d.booted.has("emu/Flare_Pixel"), "{:?}", d.booted);
        // Quit: one detached shutdown by AVD name with its boot time, and the list is emptied.
        let at = d.booted.devices[0].1;
        assert!(at > 0);
        let handed = d.quit(true);
        assert_eq!(handed, ["Flare_Pixel"]);
        assert_eq!(*quits.borrow(), [vec![format!("Flare_Pixel@{at}")]]);
        assert!(d.booted.devices.is_empty());
    }

    /// Rule 1366 (PR A7): an emulator Flare booted must not idle out while Flare runs. With no device panel left the
    /// hosts stay while such an emulator is alive (the emu host's keep-alives hold it); once it is seen shut down they
    /// stop two minutes later as before.
    #[test]
    fn the_emu_host_stays_while_an_emulator_flare_booted_runs() {
        let now = Rc::new(std::cell::Cell::new(1_000u64));
        let clock = {
            let now = Rc::clone(&now);
            Rc::new(move || now.get())
        };
        let mut d = DeviceCtl::new(post(), clock, None);
        d.install_real(android_starter(FakeSource::android()), None, recorder().1);
        d.set_ios(false);
        d.install_android(Rc::new(|_: &[String]| {}));
        d.on_xcode(XcodeAnswer {
            xcode: None,
            android: Some(AndroidSdk::Ready { avds: 1 }),
            booted: Booted::default(),
        });
        d.ensure_source();
        d.list();
        let t = Target::emu("Flare_Pixel").unwrap();
        d.boot(&t);
        d.drain();
        assert!(d.booted.has("emu/Flare_Pixel"));
        // No panel for much longer than two minutes: the hosts stay.
        d.idle(false);
        now.set(1_000 + 3 * IDLE_MS);
        d.idle(false);
        assert!(
            matches!(d.src, Some(Src::Real(_))),
            "kept for the emulator Flare booted"
        );
        // The emulator is seen shut down (it died, or the person shut it down): two minutes later the hosts stop.
        d.on_event(
            Some(SourceKind::Emu),
            SourceEvent::State {
                target: t.clone(),
                state: DeviceState::Shutdown,
            },
        );
        assert!(!d.booted.has("emu/Flare_Pixel"));
        d.idle(false);
        assert!(matches!(d.src, Some(Src::Real(_))), "not at once");
        now.set(1_000 + 4 * IDLE_MS + 1);
        d.idle(false);
        assert!(d.src.is_none(), "stopped two minutes after");
    }

    type AgentRx = std::sync::mpsc::Receiver<Result<serde_json::Value, flare_ctl::CtlError>>;

    /// One agent call from tab `t1`.
    fn agent_call(d: &mut DeviceCtl, req: AgentReq) -> AgentRx {
        let (tx, rx) = std::sync::mpsc::channel();
        d.start_agent(AgentCall {
            tab: "t1".into(),
            req,
            reply: tx,
            cancel: flare_ctl::CancelFlag::default(),
            label: "Claude".into(),
        });
        rx
    }

    /// The call's answer, the agent's tick run on `clock` until it comes (at most 4 s).
    fn agent_answer(
        d: &mut DeviceCtl,
        clock: &std::cell::Cell<u64>,
        rx: &AgentRx,
    ) -> Result<serde_json::Value, flare_ctl::CtlError> {
        for _ in 0..250 {
            if let Ok(a) = rx.try_recv() {
                return a;
            }
            clock.set(clock.get() + 16);
            d.agent_tick();
        }
        panic!("no answer within 4 s");
    }

    /// Android plan PR A5 (spec "The agent"): the tools are offered once Android's emulator is at the floor with a
    /// device, and each setup state before that answers `unavailable` with the panel's own line; the agent never starts
    /// setup. Then the same calls list, boot, look (in dp, with the device's own buttons), press Back, refuse Shake
    /// (rule 1371), hold a launch to a package name and an install to an `.apk`.
    #[test]
    fn the_agent_drives_an_android_device_once_its_emulator_is_ready() {
        use flare_device::job::{DeviceOp, Press};
        use flare_device::source::Button;
        for sdk in [
            AndroidSdk::Missing,
            AndroidSdk::TooOld,
            AndroidSdk::Ready { avds: 0 },
        ] {
            let (mut d, _) = android_ctl(sdk);
            assert!(!d.agent_offered(), "{sdk:?}");
            let e = agent_call(&mut d, AgentReq::List)
                .try_recv()
                .expect("answered at once")
                .unwrap_err();
            assert_eq!(e.code, "unavailable", "{sdk:?}");
            assert!(
                e.message.contains("Google's Android emulator")
                    && e.message.contains("only the user can do this"),
                "{}",
                e.message
            );
            assert_eq!(d.flow, Flow::Idle, "the agent never starts setup");
        }
        // The licence on screen: the same line, nothing accepted.
        let (mut d, _) = android_ctl(AndroidSdk::Ready { avds: 1 });
        d.flow = Flow::Licence {
            text: "terms".into(),
            hashes: vec!["h".into()],
            download_mb: 2500,
        };
        let e = agent_call(&mut d, AgentReq::List)
            .try_recv()
            .unwrap()
            .unwrap_err();
        assert!(e.message.starts_with("Google's licence"), "{}", e.message);

        let clock = Rc::new(std::cell::Cell::new(1_000u64));
        let c = Rc::clone(&clock);
        let mut d = DeviceCtl::new(post(), Rc::new(move || c.get()), None);
        d.install_real(android_starter(FakeSource::android()), None, recorder().1);
        d.set_ios(false);
        d.install_android(Rc::new(|_: &[String]| {}));
        d.on_xcode(XcodeAnswer {
            xcode: None,
            android: Some(AndroidSdk::Ready { avds: 1 }),
            booted: Booted::default(),
        });
        assert!(d.agent_offered());
        let rx = agent_call(&mut d, AgentReq::List);
        let list = agent_answer(&mut d, &clock, &rx).unwrap();
        assert_eq!(list["devices"][0]["target"], "emu/Flare_Pixel", "{list}");
        assert_eq!(list["devices"][0]["kind"], "Android");
        let boot = agent_call(
            &mut d,
            AgentReq::Boot {
                target: None,
                name: Some("flare pixel".into()),
            },
        );
        let v = agent_answer(&mut d, &clock, &boot).unwrap();
        assert_eq!(v["target"], "emu/Flare_Pixel");
        assert!(
            d.take_effects().contains(&Effect::SetTarget {
                tab: "t1".into(),
                target: "emu/Flare_Pixel".into()
            }),
            "shown in the tab's own panel"
        );
        show_tab(&mut d, "t1", "emu/Flare_Pixel");
        let look = agent_call(&mut d, AgentReq::Look { tree: true });
        let v = agent_answer(&mut d, &clock, &look).unwrap();
        assert_eq!(
            v["screen"],
            serde_json::json!({"w": 411.0, "h": 914.0}),
            "dp"
        );
        assert_eq!(
            v["buttons"],
            serde_json::json!([
                "home",
                "back",
                "recents",
                "power",
                "volume_up",
                "volume_down",
                "rotate_left",
                "rotate_right"
            ])
        );
        assert!(
            v["tree"]
                .as_array()
                .is_some_and(|t| t.iter().any(|e| e["label"] == "Tap me")),
            "{v}"
        );
        let press = |b| AgentReq::Act(DeviceOp::Press(Press::Button(b)));
        let back = agent_call(&mut d, press(Button::Back));
        assert!(
            agent_answer(&mut d, &clock, &back).is_ok(),
            "Back is Android's"
        );
        let e = agent_call(&mut d, press(Button::Shake))
            .try_recv()
            .unwrap()
            .unwrap_err();
        assert_eq!(e.code, "unavailable");
        assert!(e.message.contains("no shake button") && e.message.contains("back, recents"));
        // A launch takes a package name; an install an .apk.
        let e = agent_call(
            &mut d,
            AgentReq::Launch {
                app: "com.example.my-app".into(),
            },
        )
        .try_recv()
        .unwrap()
        .unwrap_err();
        assert_eq!(e.code, flare_ctl::code::BAD_PARAMS, "{}", e.message);
        let launch = agent_call(
            &mut d,
            AgentReq::Launch {
                app: "com.android.settings".into(),
            },
        );
        assert!(agent_answer(&mut d, &clock, &launch).is_ok());
        let e = agent_call(
            &mut d,
            AgentReq::Install {
                path: PathBuf::from("/p/Demo.app"),
            },
        )
        .try_recv()
        .unwrap()
        .unwrap_err();
        assert!(e.message.contains(".apk"), "{}", e.message);
        let install = agent_call(
            &mut d,
            AgentReq::Install {
                path: PathBuf::from("/p/app-debug.apk"),
            },
        );
        let v = agent_answer(&mut d, &clock, &install).unwrap();
        assert_eq!(v["installed"], "app-debug.apk");
    }

    /// Android plan PR A5's real check (`deviceRealAndroid`; opt-in, `FLARE_DEVICE_REAL=1`, `cargo build -p
    /// flare-emu-host` first; by hand only, since GitHub's runners cannot run the emulator's hypervisor): an AVD written
    /// for the test into its own folder (deleted after; the person's AVDs are never listed) boots headless with the
    /// person's SDK (read only), and the agent's own calls go through the controller over the real emu host: list,
    /// boot, a look in dp with a PNG of the same size, a launch of Settings, its tree, Back, Shake refused, an `.apk`
    /// (`FLARE_DEVICE_APK`, optional) installed, the state, and the agent's shutdown. Quit's own command ends the
    /// emulator whatever happens. `FLARE_DEVICE_IMAGE` names another image package.
    #[test]
    fn a_real_emulator_answers_the_agent_tools() {
        use flare_device::emu::discovery;
        use flare_device::emu::{Places, flare_avd};
        use flare_device::emu_client::{EmuConfig, EmuSource};
        use flare_device::job::{At, DeviceOp, Press};
        use flare_device::source::Button;
        use std::time::{Duration, Instant};
        if std::env::var("FLARE_DEVICE_REAL").as_deref() != Ok("1") {
            eprintln!(
                "skipped: set FLARE_DEVICE_REAL=1 to run against an emulator made for the test"
            );
            return;
        }
        let exe = std::env::current_exe()
            .unwrap()
            .parent()
            .and_then(|d| d.parent())
            .map(|d| d.join(flare_win::names::EMU_HOST_EXE))
            .unwrap();
        assert!(exe.exists(), "build the emu host first: {}", exe.display());
        let home = tempfile::tempdir().unwrap();
        let abi = if cfg!(target_arch = "aarch64") {
            "arm64-v8a"
        } else {
            "x86_64"
        };
        let image = std::env::var("FLARE_DEVICE_IMAGE")
            .unwrap_or_else(|_| format!("system-images;android-37.0;google_apis;{abi}"));
        let img = flare_avd::Image::from_path(&image).expect("an image package path");
        flare_avd::write(home.path(), &img).expect("the AVD is written");
        let mut places = Places::from_env();
        places.android_avd_home = Some(home.path().to_path_buf());
        let running_dir = places.discovery_dir().expect("a discovery folder");
        let logs = tempfile::tempdir().unwrap();
        /// Ends the emulator through quit's own command, whatever happens, and waits for it to go.
        struct Down(PathBuf, PathBuf, PathBuf);
        impl Drop for Down {
            fn drop(&mut self) {
                let started = Instant::now();
                let _ = std::process::Command::new(&self.0)
                    .args(["--shutdown", flare_avd::NAME])
                    .env("ANDROID_AVD_HOME", &self.1)
                    .status();
                while started.elapsed() < Duration::from_secs(120)
                    && discovery::of_avd(&discovery::read_all(&self.2), flare_avd::NAME).is_some()
                {
                    std::thread::sleep(Duration::from_millis(500));
                }
                eprintln!(
                    "--shutdown: gone after {:.1} s: {}",
                    started.elapsed().as_secs_f64(),
                    discovery::of_avd(&discovery::read_all(&self.2), flare_avd::NAME).is_none()
                );
            }
        }
        let _down = Down(exe.clone(), home.path().to_path_buf(), running_dir);
        let mut cfg = EmuConfig::new(exe, logs.path().join("emu-host.log"), places);
        cfg.logs_dir = Some(logs.path().to_path_buf());
        cfg.env = vec![(
            "ANDROID_AVD_HOME".into(),
            home.path().to_string_lossy().to_string(),
        )];
        let mut cfg = Some(cfg);
        let starter: Starter = Box::new(move |setup| match setup {
            SourceSetup::Android => Some(Started {
                source: Box::new(EmuSource::start(cfg.take()?)),
                #[cfg(target_os = "macos")]
                ring: None,
            }),
            SourceSetup::Xcode(_) => None,
        });
        let t0 = Instant::now();
        let mut d = DeviceCtl::new(
            post(),
            Rc::new(move || t0.elapsed().as_millis() as u64),
            None,
        );
        d.install_real(starter, None, recorder().1);
        d.set_ios(false);
        d.install_android(Rc::new(|_: &[String]| {}));
        d.on_xcode(XcodeAnswer {
            xcode: None,
            android: Some(AndroidSdk::Ready { avds: 1 }),
            booted: Booted::default(),
        });
        assert!(d.agent_offered());
        // Waits in real time, the agent's tick run as its timer would.
        let wait = |d: &mut DeviceCtl, rx: &AgentRx, secs: u64| {
            let end = Instant::now() + Duration::from_secs(secs);
            while Instant::now() < end {
                if let Ok(a) = rx.try_recv() {
                    return a;
                }
                d.agent_tick();
                std::thread::sleep(Duration::from_millis(16));
            }
            panic!("no answer in {secs} s");
        };
        let call = |d: &mut DeviceCtl, req, secs| {
            let started = Instant::now();
            let rx = agent_call(d, req);
            let r = wait(d, &rx, secs);
            (r, started.elapsed().as_millis())
        };
        let target = format!("emu/{}", flare_avd::NAME);
        let (list, _) = call(&mut d, AgentReq::List, 60);
        let list = list.expect("the list");
        assert!(
            list["devices"]
                .as_array()
                .is_some_and(|l| l.iter().any(|x| x["target"] == target.as_str())),
            "{list}"
        );
        let (boot, ms) = call(
            &mut d,
            AgentReq::Boot {
                target: flare_device::target::parse(&target),
                name: None,
            },
            600,
        );
        eprintln!("boot: {boot:?} in {:.0} s", ms as f64 / 1000.0);
        assert_eq!(boot.expect("booted")["booted"], true);
        show_tab(&mut d, "t1", &target);
        let (look, ms) = call(&mut d, AgentReq::Look { tree: true }, 60);
        let look = look.expect("a look");
        let (w, h) = (
            look["screen"]["w"].as_f64().unwrap(),
            look["screen"]["h"].as_f64().unwrap(),
        );
        let png = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            look["image"]["data"].as_str().unwrap(),
        )
        .unwrap();
        let px = |at: usize| u32::from_be_bytes(png[at..at + 4].try_into().unwrap()) as f64;
        eprintln!(
            "first look in {ms} ms: {w} x {h} dp, PNG {} x {}, {} elements, buttons {}",
            px(16),
            px(20),
            look["tree"].as_array().map_or(0, Vec::len),
            look["buttons"]
        );
        assert!(
            w > 200.0 && w < 600.0 && h > 400.0 && h < 1200.0,
            "dp, not pixels"
        );
        // The emulator keeps the screen's aspect, so a side may be a pixel short (411 x 913 for 411.4 x 914.3 dp).
        assert!(
            (px(16) - w).abs() <= 1.5 && (px(20) - h).abs() <= 1.5,
            "the PNG is at dp size"
        );
        assert_eq!(look["buttons"][1], "back");
        let (r, ms) = call(
            &mut d,
            AgentReq::Launch {
                app: "com.android.settings".into(),
            },
            60,
        );
        eprintln!("launch Settings: {r:?} in {ms} ms");
        r.expect("Settings opens");
        let mut tree = serde_json::Value::Null;
        for _ in 0..10 {
            let (ui, ms) = call(&mut d, AgentReq::Ui, 30);
            tree = ui.expect("a tree");
            eprintln!(
                "tree in {ms} ms: {} elements",
                tree["tree"].as_array().map_or(0, Vec::len)
            );
            if tree["tree"].as_array().is_some_and(|t| t.len() > 3) {
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        let first = tree["tree"]
            .as_array()
            .and_then(|t| {
                t.iter()
                    .find(|e| e["label"].as_str().is_some_and(|l| !l.is_empty()))
            })
            .cloned()
            .expect("Settings lists a labelled element");
        eprintln!("Settings: {first}");
        // The tree came from the agent helper this emu host started (its hello echoed the nonce), not from the slow
        // `uiautomator dump` (PR A7 review: a helper whose hello lacked the nonce was refused on every start).
        let log = std::fs::read_to_string(logs.path().join("emu-host.log")).unwrap_or_default();
        assert!(
            log.contains("agent helper started"),
            "the helper started: {log}"
        );
        assert!(
            !log.contains("the tree uses uiautomator dump"),
            "no fall back to the dump: {log}"
        );
        let (r, ms) = call(
            &mut d,
            AgentReq::Act(DeviceOp::Tap {
                at: At::Ref(first["ref"].as_str().unwrap().to_string()),
                hold_ms: None,
            }),
            30,
        );
        eprintln!("tap by ref: {r:?} in {ms} ms");
        r.expect("a tap by ref");
        let press = |b| AgentReq::Act(DeviceOp::Press(Press::Button(b)));
        let (r, ms) = call(&mut d, press(Button::Back), 30);
        eprintln!("back: {r:?} in {ms} ms");
        r.expect("Back");
        let (r, _) = call(&mut d, press(Button::Shake), 5);
        assert_eq!(r.unwrap_err().code, "unavailable");
        if let Ok(apk) = std::env::var("FLARE_DEVICE_APK") {
            // As `ctl_device::install_ok` hands it on: resolved, written without `\\?\`.
            let real = std::fs::canonicalize(apk).unwrap();
            let path = PathBuf::from(real.to_string_lossy().trim_start_matches(r"\\?\"));
            let (r, ms) = call(&mut d, AgentReq::Install { path }, 200);
            eprintln!("install: {r:?} in {ms} ms");
            r.expect("the .apk installs");
        }
        let (state, _) = call(&mut d, AgentReq::State, 5);
        let state = state.unwrap();
        eprintln!("state: {state}");
        assert_eq!(state["booted_by_flare"], true);
        let (r, ms) = call(&mut d, AgentReq::Shutdown, 120);
        eprintln!("shutdown: {r:?} in {ms} ms");
        r.expect("Flare booted it, so the agent shuts it down");
    }

    /// An iPhone simulator's device is held to Xcode only: an Android setup state on the same Mac never blocks it, and
    /// an `.apk` is refused for it.
    #[test]
    fn an_iphone_refuses_an_apk_and_keeps_its_own_buttons() {
        let mut d = DeviceCtl::new(post(), Rc::new(|| 0), Some(FakeSource::default()));
        d.list();
        let t = Target::sim("00000000-0000-4000-8000-000000000001").unwrap();
        d.boot(&t);
        d.drain();
        show_tab(&mut d, "t1", &t.name());
        let state = agent_call(&mut d, AgentReq::State)
            .try_recv()
            .unwrap()
            .unwrap();
        assert_eq!(state["buttons"][1], "lock", "{state}");
        let e = agent_call(
            &mut d,
            AgentReq::Install {
                path: PathBuf::from("/p/app-debug.apk"),
            },
        )
        .try_recv()
        .unwrap()
        .unwrap_err();
        assert_eq!(e.code, flare_ctl::code::BAD_PARAMS);
        assert!(e.message.contains("iPhone simulator"), "{}", e.message);
    }

    /// The emulator sends a frame only when its screen changed: each frame record of an attached device counts as a
    /// change, so the agent's input calls can answer `changed: true` on Android as on iOS.
    #[test]
    fn an_android_frame_record_counts_as_a_change() {
        let mut d = DeviceCtl::new(post(), Rc::new(|| 0), Some(FakeSource::android()));
        let t = Target::emu("Flare_Pixel").unwrap();
        let region = |seq| SourceEvent::Region {
            target: t.clone(),
            seq,
            t_us: 0,
            size_px: Size::new(1080.0, 2400.0),
            rotation: Orientation::Rot0,
        };
        // Not attached: no change counted.
        d.on_event(Some(SourceKind::Emu), region(1));
        assert_eq!(d.devs.get(&t).map_or(0, |x| x.changed), 0);
        d.devs.entry(t.clone()).or_default().attached = true;
        d.on_event(Some(SourceKind::Emu), region(2));
        d.on_event(Some(SourceKind::Emu), region(3));
        assert_eq!(d.devs[&t].changed, 2);
        assert!(d.devs[&t].ring_frame);
        // While the stream is paused the emu host sends the agent helper's window changes (PR A7): each one adds one
        // to Flare's count, whatever the helper's own count says, so the count only grows.
        d.devs.get_mut(&t).unwrap().paused = true;
        d.on_event(
            Some(SourceKind::Emu),
            SourceEvent::Changed {
                target: t.clone(),
                count: 1,
            },
        );
        assert_eq!(d.devs[&t].changed, 3);
        d.on_event(
            Some(SourceKind::Emu),
            SourceEvent::Changed {
                target: t.clone(),
                count: 0,
            },
        );
        assert_eq!(
            d.devs[&t].changed, 4,
            "a restarted helper's count never goes back"
        );
        // The simulator's count is its own.
        let s = Target::sim("00000000-0000-4000-8000-0000000000aa").unwrap();
        d.devs.entry(s.clone()).or_default();
        d.on_event(
            Some(SourceKind::Sim),
            SourceEvent::Changed {
                target: s.clone(),
                count: 9,
            },
        );
        assert_eq!(d.devs[&s].changed, 9);
    }

    /// The spec's fallbacks as the panel shows them (PR A7): the slow mark with its reason, the still line with its
    /// reason, the dimmed picture while the display is off, and nothing on the normal path.
    #[test]
    fn an_android_frame_modes_show_as_slow_still_and_dim() {
        assert_eq!(still_line(None, true, FrameMode::Live, ""), "");
        assert_eq!(
            slow_line(true, Some(FramePath::Gpu), FrameMode::Live, ""),
            ""
        );
        let slow = slow_line(true, Some(FramePath::Gpu), FrameMode::Slow, "MMAP refused");
        assert!(
            slow.contains("slow way") && slow.contains("MMAP refused"),
            "{slow}"
        );
        let still = still_line(None, true, FrameMode::Still, "no frame for 2 s");
        assert!(
            still.starts_with("Live view unavailable: no frame for 2 s.")
                && still.contains("three times a second"),
            "{still}"
        );
        assert_eq!(
            still_line(None, false, FrameMode::Still, "x"),
            "",
            "only over a live picture"
        );
        assert!(still_line(Some("gone"), true, FrameMode::Live, "").contains("every half second"));
        assert_eq!(
            slow_line(true, Some(FramePath::Cpu), FrameMode::Live, ""),
            CPU_SLOW
        );
        assert_eq!(slow_line(false, None, FrameMode::Slow, "x"), "");

        let mut d = DeviceCtl::new(post(), Rc::new(|| 0), Some(FakeSource::android()));
        let t = Target::emu("Flare_Pixel").unwrap();
        d.devs.entry(t.clone()).or_default().attached = true;
        d.on_event(
            Some(SourceKind::Emu),
            SourceEvent::Mode {
                target: t.clone(),
                mode: FrameMode::Asleep,
                why: String::new(),
            },
        );
        assert_eq!(d.devs[&t].mode, FrameMode::Asleep);
        // A new attach starts on the normal path again.
        d.on_event(
            Some(SourceKind::Emu),
            SourceEvent::Frames {
                target: t.clone(),
                ring: 1,
                screen: Screen::from_dpi(Size::new(1080.0, 2400.0), 420.0, Orientation::Rot0)
                    .unwrap(),
            },
        );
        assert_eq!(d.devs[&t].mode, FrameMode::Live);
    }

    /// Rule 1366: a boot of an emulator that already ran (Android Studio's, or a list that was out of date) waits
    /// for it but never makes it Flare's, so quit leaves it running. A boot Flare made takes a fresh time, also over
    /// an entry left from an emulator that idled out.
    #[test]
    fn an_emulator_that_already_ran_is_never_flares_to_shut_down() {
        let (mut d, quits) = android_ctl(AndroidSdk::Ready { avds: 1 });
        d.ensure_source();
        d.list();
        let t = Target::emu("Flare_Pixel").unwrap();
        show_tab(&mut d, "t1", "emu/Flare_Pixel");
        d.pending.insert(999, Pending::Boot(t.clone()));
        d.on_event(
            Some(SourceKind::Emu),
            SourceEvent::Answer {
                id: 999,
                result: Ok(flare_device::emu_client::started_elsewhere()),
            },
        );
        assert!(!d.booted.has("emu/Flare_Pixel"), "{:?}", d.booted);
        assert!(d.info_of(&t).is_some_and(|x| !x.booted_by_flare));
        // A stale entry, then a boot Flare made: one entry, with the new time.
        d.booted.record("emu/Flare_Pixel", 5);
        d.boot(&t);
        d.drain();
        assert_eq!(d.state_of(&t), Some(DeviceState::Booted));
        assert_eq!(d.booted.devices.len(), 1);
        assert!(d.booted.devices[0].1 > 5, "{:?}", d.booted);
        assert_eq!(
            d.booted.emu_shutdown_args(),
            [format!("Flare_Pixel@{}", d.booted.devices[0].1)]
        );
        d.quit(true);
        assert_eq!(quits.borrow().len(), 1);
    }

    /// The spec's "Input": an Android panel has the iOS panel's buttons (Lock sends Power) and its own menu buttons
    /// (Back, Recents), never iOS's Shake (rule 1371).
    #[test]
    fn an_android_panel_sends_power_for_lock_and_never_shake() {
        let mut d = DeviceCtl::new(post(), Rc::new(|| 0), Some(FakeSource::android()));
        d.list();
        let t = Target::emu("Flare_Pixel").unwrap();
        d.boot(&t);
        d.drain();
        show_tab(&mut d, "t1", "emu/Flare_Pixel");
        if let Some(id) = d.source().map(|s| s.attach(&t)) {
            d.pending.insert(id, Pending::Attach(t.clone()));
        }
        d.devs.entry(t.clone()).or_default().driver = Some("t1".into());
        d.drain();
        assert!(d.drives("t1").is_some());
        for name in ["lock", "back", "recents", "home", "shake"] {
            d.act(name);
        }
        let pressed: Vec<String> = d
            .fake()
            .unwrap()
            .inputs()
            .iter()
            .filter_map(|r| match r {
                flare_device::fake::Recorded::Button(_, b) => Some(b.name().to_string()),
                _ => None,
            })
            .collect();
        assert_eq!(
            pressed,
            ["power", "back", "recents", "home"],
            "Shake is iOS only"
        );
    }

    /// Spec "Setup", states 1 to 3 (rule 1361): [Set up] reads the plan and shows Google's licence with a second
    /// button; [Cancel] backs out with nothing downloaded; [Accept and set up] carries the licence's hash, shows the
    /// progress, and ends on the device list.
    #[cfg(windows)]
    #[test]
    fn android_setup_shows_the_licence_then_downloads_then_lists() {
        let (mut d, _) = android_ctl(AndroidSdk::Missing);
        assert!(
            d.menu_offered(),
            "Windows offers Android before it is set up"
        );
        d.ensure_source();
        show_tab(&mut d, "t1", CHOOSE);
        assert_eq!(d.setup_of("t1"), Some(Setup::SdkMissing));
        d.setup_clicked();
        let Some(Setup::Licence { text, download_mb }) = d.setup_of("t1") else {
            panic!("{:?}", d.setup_of("t1"))
        };
        assert_eq!(text, flare_device::fake::FAKE_LICENCE);
        assert_eq!(download_mb, 2538);
        assert!(d.setup_of("t1").unwrap().view(false).cancel);
        d.cancel_clicked();
        assert_eq!(
            d.setup_of("t1"),
            Some(Setup::SdkMissing),
            "nothing downloaded"
        );
        d.setup_clicked();
        d.setup_clicked();
        // The fake said its progress, then answered: the SDK is ready and the list is read.
        assert_eq!(d.flow, Flow::Idle);
        assert_eq!(
            d.real.as_ref().and_then(|r| r.sdk),
            Some(AndroidSdk::Ready { avds: 1 })
        );
        assert_eq!(d.setup_of("t1"), Some(Setup::Choose));
        // A licence that changed is refused: the panel says why and offers Try again.
        d.flow = Flow::Licence {
            text: "new".into(),
            hashes: vec!["other".into()],
            download_mb: 1,
        };
        d.setup_clicked();
        assert!(matches!(d.setup_of("t1"), Some(Setup::Failed { .. })));
        d.setup_clicked();
        assert_eq!(d.flow, Flow::Idle);
    }

    /// Android PR A7: the chooser's own rows, decided from the computer and Android's state alone (so this runs on
    /// every OS). A Mac with no Android emulator set up gets one row that starts the setup; a set-up emulator gets
    /// [Update Android emulator]; Windows shows the setup states before any list, so it gets no setup row.
    #[test]
    fn the_chooser_offers_android_setup_on_a_mac_and_an_update_once_set_up() {
        assert_eq!(
            chooser_rows(true, true, Some(&Setup::SdkMissing), false),
            [ChooserRow::SetUpAndroid]
        );
        assert_eq!(
            ChooserRow::SetUpAndroid.label(),
            "Android: set up (about 2.5 GB from Google)"
        );
        assert_eq!(
            chooser_rows(false, true, Some(&Setup::SdkMissing), false),
            []
        );
        assert_eq!(
            chooser_rows(true, true, None, true),
            [ChooserRow::UpdateAndroid]
        );
        assert_eq!(
            chooser_rows(false, true, None, true),
            [ChooserRow::UpdateAndroid]
        );
        assert_eq!(
            chooser_rows(true, true, None, false),
            [],
            "the person's own SDK: an update would install a second one"
        );
        assert_eq!(ChooserRow::UpdateAndroid.label(), "Update Android emulator");
        assert_eq!(chooser_rows(true, false, None, true), [], "no Android here");
        assert_eq!(
            chooser_rows(true, true, Some(&Setup::Reading), true),
            [],
            "setup runs"
        );
    }

    /// Android PR A7, a Mac (iPhones and Android): with no Android emulator set up the chooser lists the devices and
    /// one more row; picking it runs the same setup as on Windows (Google's licence first, then the download), and the
    /// list comes back with [Update Android emulator].
    #[test]
    fn a_mac_sets_android_up_from_the_chooser_row() {
        // The sim host's place taken by the iPhone fake, the emu host's by the Android one.
        let mut iphone = Some(FakeSource::default());
        let mut android = Some(FakeSource::android());
        let mut d = DeviceCtl::new(post(), Rc::new(|| 0), None);
        d.install_real(
            Box::new(move |s| match s {
                SourceSetup::Xcode(_) => Some(Started {
                    source: Box::new(iphone.take()?),
                    #[cfg(target_os = "macos")]
                    ring: None,
                }),
                SourceSetup::Android => Some(Started {
                    source: Box::new(android.take()?),
                    #[cfg(target_os = "macos")]
                    ring: None,
                }),
            }),
            None,
            recorder().1,
        );
        d.set_ios(true);
        d.install_android(Rc::new(|_: &[String]| {}));
        d.on_xcode(XcodeAnswer {
            xcode: Some(found_xcode()),
            android: Some(AndroidSdk::Missing),
            booted: Booted::default(),
        });
        d.xcode = Xcode::Ready;
        d.ensure_source();
        let Some(Src::Real(m)) = &d.src else {
            panic!("the real sources")
        };
        assert!(
            m.has(SourceKind::Sim) && !m.has(SourceKind::Emu),
            "an iPhone-only Mac starts no emu host"
        );
        d.list();
        show_tab(&mut d, "t1", CHOOSE);
        assert_eq!(d.setup_of("t1"), Some(Setup::Choose));
        assert_eq!(d.chooser_rows(), [ChooserRow::SetUpAndroid]);
        let n = d.devices.as_ref().map_or(0, Vec::len) as i32;
        assert!(d.pick(n).is_empty(), "the row sets no target");
        let Some(Setup::Licence { text, .. }) = d.setup_of("t1") else {
            panic!("{:?}", d.setup_of("t1"))
        };
        assert_eq!(text, flare_device::fake::FAKE_LICENCE);
        // [Cancel] goes back to the list with nothing downloaded.
        d.cancel_clicked();
        assert_eq!(d.setup_of("t1"), Some(Setup::Choose));
        d.pick(n);
        d.setup_clicked(); // Accept and set up
        assert_eq!(d.flow, Flow::Idle);
        assert_eq!(d.setup_of("t1"), Some(Setup::Choose));
        // Set up into Flare's own folder (the setup's answer says so): the chooser offers its update.
        d.emu_facts.insert("sdk_own".into(), "true".into());
        assert_eq!(d.chooser_rows(), [ChooserRow::UpdateAndroid]);
        // A row past the end does nothing.
        assert!(d.pick(n + 5).is_empty());
        assert_eq!(d.flow, Flow::Idle);
    }

    /// Spec state 6 (Android PR A7): an emulator started without access for other apps shows its own line and no
    /// button, not a failure to try again.
    #[test]
    fn an_emulator_started_without_access_shows_its_line() {
        let (mut d, _) = android_ctl(AndroidSdk::Ready { avds: 1 });
        d.ensure_source();
        d.list();
        show_tab(&mut d, "t1", "emu/Flare_Pixel");
        d.tabs.get_mut("t1").unwrap().failed =
            Some(flare_device::emu::discovery::NO_ACCESS.to_string());
        assert_eq!(d.setup_of("t1"), Some(Setup::NoAccess));
        let v = Setup::NoAccess.view(false);
        assert_eq!(v.line, flare_device::emu::discovery::NO_ACCESS);
        assert_eq!(v.button, None);
    }

    /// The setup plan reads as the licence (all of them, with their hashes) or as a line about disk space.
    #[test]
    fn a_setup_plan_reads_as_the_licence_or_a_space_line() {
        let v = serde_json::json!({
            "needed": true,
            "licences": [{"text": "A", "hash": "1"}, {"text": "B", "hash": "2"}],
            "download_mb": 2600,
            "enough_space": true,
        });
        assert_eq!(
            plan_flow(&v),
            Flow::Licence {
                text: "A\n\nB".into(),
                hashes: vec!["1".into(), "2".into()],
                download_mb: 2600
            }
        );
        let full = serde_json::json!({"needed": true, "enough_space": false, "disk_mb": 8000});
        assert_eq!(
            plan_flow(&full),
            Flow::Failed("Android setup needs about 8.0 GB of free disk space.".into())
        );
        assert_eq!(plan_flow(&serde_json::json!({"needed": false})), Flow::Idle);
    }

    /// Rule 1366: `booted.json` holds both kinds; an old file's simulator entries still read.
    #[test]
    fn booted_json_holds_both_kinds() {
        let mut b = Booted::default();
        b.record("sim/00000000-0000-4000-8000-00000000000a", 1);
        b.record("emu/Flare_Pixel", 2);
        let back = Booted::from_json(&b.to_json());
        assert_eq!(back, b);
        assert_eq!(
            back.of(SourceKind::Emu),
            [Target::emu("Flare_Pixel").unwrap()]
        );
        let old = serde_json::json!({"devices": [{"udid": "00000000-0000-4000-8000-00000000000A", "booted_ms": 5}]});
        assert!(Booted::from_json(&old).has("sim/00000000-0000-4000-8000-00000000000a"));
        let cmd = emu_shutdown_command(Path::new("flare-emu-host"), &["Flare_Pixel".into()]);
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert_eq!(args, ["--shutdown", "Flare_Pixel"]);
    }
}

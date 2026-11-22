//! What a source of devices can do: list, boot, attach, show frames, take input, read the UI tree. Implementations
//! are `SimSource` (talks to `flare-sim-host`, PR 2), [`crate::fake::FakeSource`] (tests and `--e2e`), and the
//! Android one, `EmuSource` (talks to `flare-emu-host`, Android plan PR A2). Every source sits behind the same panel
//! kind, tools and agent job (rule 1360); [`SourceKind`] says which one a target belongs to.
//!
//! Every method returns at once (rule 34): what takes time answers later as a [`SourceEvent::Answer`] with the
//! request's id, read from [`DeviceSource::events`] by the UI thread. Points are device points in the current
//! orientation (dp on Android, rule 1367); only the helper process converts them, the sim host to the digitizer's
//! portrait ratios (rule 1313) and the emu host to pixels.

use std::path::Path;
use std::sync::mpsc::Receiver;

use serde::{Deserialize, Serialize};

use crate::geom::{Orientation, Size};
use crate::job::Fail;
use crate::target::Target;
use crate::tree::UiNode;

/// A request's id, unique per source, matched by its [`SourceEvent::Answer`].
pub type ReqId = u64;

/// Where a touch is in its life.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Start,
    Move,
    End,
}

/// One touch event in device points. `second` is a second finger (a pinch: Alt + drag in the panel, spike S1 showed
/// the digitizer takes a second contact).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Touch {
    pub phase: Phase,
    pub x: f64,
    pub y: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub second: Option<(f64, f64)>,
}

impl Touch {
    pub fn one(phase: Phase, x: f64, y: f64) -> Touch {
        Touch {
            phase,
            x,
            y,
            second: None,
        }
    }
}

/// One keyboard key going down or up, as a USB HID keyboard usage (page 0x07). Shift is a key of its own (0xE1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Key {
    pub usage: u32,
    pub down: bool,
}

/// A hardware button or gesture of the device. Each is one whole press. Not every source has every button: each
/// lists its own ([`SourceKind::buttons`]), and a press of another source's button is refused with `unavailable`
/// (rule 1371).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Button {
    Home,
    /// The iPhone's side button (lock).
    Lock,
    VolumeUp,
    VolumeDown,
    /// The simulator's shake gesture.
    Shake,
    RotateLeft,
    RotateRight,
    /// Android's Back.
    Back,
    /// Android's Recents (the app switcher).
    Recents,
    /// Android's power button (the panel's Lock button sends it on Android).
    Power,
}

impl Button {
    pub const ALL: [Button; 10] = [
        Button::Home,
        Button::Lock,
        Button::VolumeUp,
        Button::VolumeDown,
        Button::Shake,
        Button::RotateLeft,
        Button::RotateRight,
        Button::Back,
        Button::Recents,
        Button::Power,
    ];

    /// The name `device_press` takes.
    pub fn name(self) -> &'static str {
        match self {
            Button::Home => "home",
            Button::Lock => "lock",
            Button::VolumeUp => "volume_up",
            Button::VolumeDown => "volume_down",
            Button::Shake => "shake",
            Button::RotateLeft => "rotate_left",
            Button::RotateRight => "rotate_right",
            Button::Back => "back",
            Button::Recents => "recents",
            Button::Power => "power",
        }
    }

    pub fn parse(s: &str) -> Option<Button> {
        Button::ALL.into_iter().find(|b| b.name() == s)
    }
}

/// Which source shows a device (`Target::source`): the iOS simulator (`sim/`) or the Android emulator (`emu/`). Both
/// sit behind the same panel kind, tools and agent job (rule 1360); only the buttons and the coordinate unit differ
/// (points on iOS, dp on Android: [`Screen::scale`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SourceKind {
    Sim,
    Emu,
}

impl SourceKind {
    /// The buttons this source's devices have, in the order `device_press` lists them (rule 1371). The panel's Lock
    /// button sends [`Button::Power`] on Android; Shake and Lock are iOS only.
    pub fn buttons(self) -> &'static [Button] {
        match self {
            SourceKind::Sim => &[
                Button::Home,
                Button::Lock,
                Button::VolumeUp,
                Button::VolumeDown,
                Button::Shake,
                Button::RotateLeft,
                Button::RotateRight,
            ],
            SourceKind::Emu => &[
                Button::Home,
                Button::Back,
                Button::Recents,
                Button::Power,
                Button::VolumeUp,
                Button::VolumeDown,
                Button::RotateLeft,
                Button::RotateRight,
            ],
        }
    }

    /// Whether this source's devices have `b` (rule 1371).
    pub fn has(self, b: Button) -> bool {
        self.buttons().contains(&b)
    }
}

/// A device's boot state, as `simctl list` names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceState {
    Shutdown,
    Booting,
    Booted,
    ShuttingDown,
}

impl DeviceState {
    /// The state `simctl list -j` prints, or None for one this build does not know.
    pub fn from_simctl(s: &str) -> Option<DeviceState> {
        Some(match s {
            "Shutdown" => DeviceState::Shutdown,
            "Booting" => DeviceState::Booting,
            "Booted" => DeviceState::Booted,
            "Shutting Down" => DeviceState::ShuttingDown,
            _ => return None,
        })
    }
}

/// A device this Mac can run.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub target: Target,
    /// "iPhone 18 Pro".
    pub name: String,
    /// The family: "iPhone", "iPad".
    pub kind: String,
    /// "iOS 27.2".
    pub os: String,
    pub state: DeviceState,
    /// Flare booted it (only such a device is shut down by Flare, rule 1319).
    pub booted_by_flare: bool,
}

/// What a device's screen is: its size in pixels and in the agent's units (both portrait), its scale and the
/// interface's orientation.
///
/// The agent's unit is the point on iOS and the dp on Android (rule 1367): `size_pt` is in that unit and `scale` is
/// pixels per unit (3 on an iPhone 18 Pro; dpi / 160 on Android, so 2.625 at 420 dpi). The simulator's framebuffer
/// never turns (spike S5); the emulator sends its frames already turned, and its source gives the portrait size here
/// with the turn in `orientation`, so both read the same way.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Screen {
    pub size_px: Size,
    pub size_pt: Size,
    pub scale: f64,
    pub orientation: Orientation,
}

impl Screen {
    /// An Android screen: its portrait size in pixels and its density. `size_pt` is in dp (pixels / (dpi / 160)),
    /// the unit the agent sees (rule 1367). None for an empty size or a density that is not positive.
    pub fn from_dpi(portrait_px: Size, dpi: f64, orientation: Orientation) -> Option<Screen> {
        let scale = crate::geom::dp_scale(dpi)?;
        (!portrait_px.is_empty()).then(|| Screen {
            size_px: portrait_px,
            size_pt: Size::new(portrait_px.w / scale, portrait_px.h / scale),
            scale,
            orientation,
        })
    }
}

/// The device's input connection, for the panel's "Connecting touch..." line and the agent's `retry` (rule 1313).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "input", rename_all = "snake_case")]
pub enum InputState {
    /// Waiting for the simulator's input service (up to 20 s after a cold boot): touches wait, then drop.
    Connecting,
    /// Touches, keys and buttons go through.
    Live,
    /// The connection dropped and is being made again: what was in flight is lost (the agent's call fails with
    /// `retry`).
    Lost,
    /// No input for this device (no liveness reply after five tries, or the private API is missing): one line why.
    Broken { why: String },
}

/// A request's answer.
#[derive(Clone, Debug, PartialEq)]
pub enum Answer {
    /// Done, nothing to say.
    Done,
    Devices(Vec<DeviceInfo>),
    /// A PNG at point size.
    Png(Vec<u8>),
    Tree(UiNode),
}

/// Why a request failed, in the channel's words, and one line for the person or the agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceError {
    pub fail: Fail,
    pub message: String,
}

/// What a source tells the UI thread.
#[derive(Clone, Debug, PartialEq)]
pub enum SourceEvent {
    /// A new ring of frames (attach or a new screen size): the ring's id and the screen. The sim host hands the
    /// ring's surfaces over the mach channel; `SimSource::ring(ring)` gives them (the id is the records' `device`).
    Frames {
        target: Target,
        ring: u32,
        screen: Screen,
    },
    /// The interface turned (the ring does not change, spike S5).
    Orientation {
        target: Target,
        orientation: Orientation,
    },
    /// A frame is ready in a ring slot.
    Frame {
        target: Target,
        slot: u8,
        seq: u64,
        t_us: u64,
    },
    /// A frame is in the device's frame region (Android, rule 1363): the emulator wrote it into the shared-memory
    /// region Flare owns and maps, so only its facts travel. `size_px` is the frame as written, already turned by
    /// `rotation` (unlike the simulator's portrait framebuffer). The region is single-buffered: the next frame
    /// overwrites it about 16 ms later, so Flare copies it out on the GPU at once (rule 1364, PR A4).
    Region {
        target: Target,
        seq: u64,
        t_us: u64,
        size_px: Size,
        rotation: Orientation,
    },
    /// A frame as a CPU picture (the fake source, and the still mode's screenshots): RGBA, `size_px` big.
    Picture {
        target: Target,
        seq: u64,
        size_px: Size,
        rgba: Vec<u8>,
    },
    /// The screen changed `count` times since attach (sent even while paused, so an agent job can wait for it).
    Changed {
        target: Target,
        count: u64,
    },
    State {
        target: Target,
        state: DeviceState,
    },
    /// The input connection changed (connecting, live, lost, broken).
    Input {
        target: Target,
        state: InputState,
    },
    Answer {
        id: ReqId,
        result: Result<Answer, SourceError>,
    },
    /// The source cannot go on (live view lost): one line why.
    Broken(String),
}

/// A source of devices. See the module doc: nothing here blocks.
pub trait DeviceSource {
    fn list(&mut self) -> ReqId;
    /// Boots headless and answers once it is booted.
    fn boot(&mut self, target: &Target) -> ReqId;
    fn shutdown(&mut self, target: &Target) -> ReqId;
    /// Starts the frames and the input connection of a booted device.
    fn attach(&mut self, target: &Target) -> ReqId;
    fn detach(&mut self, target: &Target);
    /// Stops (true) or restarts (false) the frames. Input, the UI tree, snapshots and the changed count go on.
    fn pause(&mut self, target: &Target, paused: bool);
    fn touch(&mut self, target: &Target, touch: Touch);
    fn key(&mut self, target: &Target, key: Key);
    fn button(&mut self, target: &Target, button: Button);
    /// Text the keymap cannot type: through the simulator's own pasteboard and Cmd-V, never the Mac's (rule 1314).
    fn text(&mut self, target: &Target, text: &str) -> ReqId;
    fn snapshot(&mut self, target: &Target) -> ReqId;
    fn ui_tree(&mut self, target: &Target) -> ReqId;
    fn launch(&mut self, target: &Target, bundle_id: &str) -> ReqId;
    fn install(&mut self, target: &Target, app: &Path) -> ReqId;
    fn events(&self) -> &Receiver<SourceEvent>;
    /// The sim host's facts (its `hello`), for the log and `device_state`; None for a source that has none.
    fn hello(&self) -> Option<crate::wire::Hello> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buttons_and_states_read_their_names() {
        for b in Button::ALL {
            assert_eq!(Button::parse(b.name()), Some(b));
        }
        assert_eq!(Button::parse("power"), Some(Button::Power));
        assert_eq!(Button::parse("menu"), None);
        assert_eq!(Button::parse("Back"), None, "names are lower case");
        assert_eq!(
            DeviceState::from_simctl("Shutting Down"),
            Some(DeviceState::ShuttingDown)
        );
        assert_eq!(DeviceState::from_simctl("Creating"), None);
    }

    /// Rule 1371: each source lists its own buttons, and every button belongs to at least one source.
    #[test]
    fn each_source_lists_its_own_buttons() {
        let sim = SourceKind::Sim;
        let emu = SourceKind::Emu;
        for b in [Button::Shake, Button::Lock] {
            assert!(sim.has(b) && !emu.has(b), "{b:?} is iOS only");
        }
        for b in [Button::Back, Button::Recents, Button::Power] {
            assert!(emu.has(b) && !sim.has(b), "{b:?} is Android only");
        }
        for b in [
            Button::Home,
            Button::VolumeUp,
            Button::VolumeDown,
            Button::RotateLeft,
            Button::RotateRight,
        ] {
            assert!(sim.has(b) && emu.has(b), "{b:?} is on both");
        }
        for b in Button::ALL {
            assert!(sim.has(b) || emu.has(b), "{b:?} belongs to no source");
        }
        // No list names a button twice.
        for k in [sim, emu] {
            let l = k.buttons();
            for (i, b) in l.iter().enumerate() {
                assert!(!l[i + 1..].contains(b), "{k:?} lists {b:?} twice");
            }
        }
        // The iOS list is the one `device_press` has offered since PR 5, in its order.
        assert_eq!(
            sim.buttons().iter().map(|b| b.name()).collect::<Vec<_>>(),
            [
                "home",
                "lock",
                "volume_up",
                "volume_down",
                "shake",
                "rotate_left",
                "rotate_right"
            ]
        );
    }

    /// Rule 1367: an Android screen's agent unit is the dp.
    #[test]
    fn an_android_screen_is_in_dp() {
        // A Pixel 8: 1080 x 2400 at 420 dpi is 411.4 x 914.3 dp.
        let s = Screen::from_dpi(Size::new(1080.0, 2400.0), 420.0, Orientation::Rot0).unwrap();
        assert_eq!(s.scale, 2.625);
        assert!((s.size_pt.w - 411.428_571).abs() < 1e-3, "{:?}", s.size_pt);
        assert!((s.size_pt.h - 914.285_714).abs() < 1e-3, "{:?}", s.size_pt);
        assert_eq!((s.size_pt.w.round(), s.size_pt.h.round()), (411.0, 914.0));
        // 160 dpi is one pixel per dp.
        let s = Screen::from_dpi(Size::new(320.0, 640.0), 160.0, Orientation::Rot90).unwrap();
        assert_eq!((s.scale, s.size_pt), (1.0, Size::new(320.0, 640.0)));
        assert_eq!(s.orientation, Orientation::Rot90);
        for (px, dpi) in [
            (Size::new(0.0, 2400.0), 420.0),
            (Size::new(1080.0, 2400.0), 0.0),
            (Size::new(1080.0, 2400.0), -1.0),
            (Size::new(1080.0, 2400.0), f64::NAN),
        ] {
            assert_eq!(
                Screen::from_dpi(px, dpi, Orientation::Rot0),
                None,
                "{px:?} {dpi}"
            );
        }
    }
}

//! What a source of devices can do: list, boot, attach, show frames, take input, read the UI tree. Implementations
//! are `SimSource` (talks to `flare-sim-host`, PR 2), [`crate::fake::FakeSource`] (tests and `--e2e`), and later an
//! Android one.
//!
//! Every method returns at once (rule 34): what takes time answers later as a [`SourceEvent::Answer`] with the
//! request's id, read from [`DeviceSource::events`] by the UI thread. Points are device points in the current
//! orientation; the sim host alone converts them to the digitizer's portrait ratios (rule 1313).

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

/// A hardware button or gesture of the device. Each is one whole press.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Button {
    Home,
    Lock,
    VolumeUp,
    VolumeDown,
    Shake,
    RotateLeft,
    RotateRight,
}

impl Button {
    pub const ALL: [Button; 7] = [
        Button::Home,
        Button::Lock,
        Button::VolumeUp,
        Button::VolumeDown,
        Button::Shake,
        Button::RotateLeft,
        Button::RotateRight,
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
        }
    }

    pub fn parse(s: &str) -> Option<Button> {
        Button::ALL.into_iter().find(|b| b.name() == s)
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

/// What a device's screen is: its size in pixels (always portrait, the framebuffer never turns, spike S5), in
/// points, its scale and the interface's orientation.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Screen {
    pub size_px: Size,
    pub size_pt: Size,
    pub scale: f64,
    pub orientation: Orientation,
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buttons_and_states_read_their_names() {
        for b in Button::ALL {
            assert_eq!(Button::parse(b.name()), Some(b));
        }
        assert_eq!(Button::parse("power"), None);
        assert_eq!(
            DeviceState::from_simctl("Shutting Down"),
            Some(DeviceState::ShuttingDown)
        );
        assert_eq!(DeviceState::from_simctl("Creating"), None);
    }
}

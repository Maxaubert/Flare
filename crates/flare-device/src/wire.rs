//! The sim host's own protocol (internal: both ends ship in one bundle, and Flare refuses a sim host whose
//! [`PROTOCOL`] does not match, which only catches a broken install).
//!
//! Control goes over the sim host's stdin and stdout as JSON lines: Flare sends a [`Control`] (an id and a
//! [`Request`]), the sim host answers with [`HostLine`]s (`ok` or `err` with the id, and events with none). The
//! first line it writes is its [`Hello`].
//!
//! Frames go over the mach channel the rendezvous sets up (PR 2), as fixed little-endian [`Record`]s: a frame is
//! ready in a slot (sim host to Flare), a slot is done (Flare to sim host), the screen changed (sim host to Flare).
//! `device` in a record is the small number the sim host gave the device in its `frames` event.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::geom::Orientation;
use crate::source::{Button, DeviceState, Key, Screen, Touch};
use crate::target::Target;
use crate::versions::InputPath;

/// The protocol's version, in `hello` both ways.
pub const PROTOCOL: u32 = 1;

/// One control line from Flare.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Control {
    pub id: u64,
    #[serde(flatten)]
    pub req: Request,
}

/// What Flare asks of the sim host.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Hello {
        protocol: u32,
    },
    List,
    Attach {
        target: Target,
    },
    Detach {
        target: Target,
    },
    Pause {
        target: Target,
    },
    Resume {
        target: Target,
    },
    Touch {
        target: Target,
        touch: Touch,
    },
    Key {
        target: Target,
        key: Key,
    },
    Button {
        target: Target,
        button: Button,
    },
    /// Text the keymap cannot type: the simulator's pasteboard (`simctl pbcopy`) and Cmd-V.
    Text {
        target: Target,
        text: String,
    },
    /// A PNG at point size of the current screen (read from the framebuffer itself, fresh while paused).
    Snapshot {
        target: Target,
    },
    UiTree {
        target: Target,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        at: Option<(f64, f64)>,
    },
    Launch {
        target: Target,
        bundle_id: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<String>,
    },
    Install {
        target: Target,
        path: String,
    },
    Boot {
        target: Target,
    },
    Shutdown {
        target: Target,
    },
    /// Unregister every callback, close every connection, exit.
    Quit,
}

/// The sim host's facts, its first line.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Hello {
    pub protocol: u32,
    /// The sim host's own version (Flare's).
    pub version: String,
    /// The loaded CoreSimulator's and SimulatorKit's `CFBundleVersion`.
    pub coresim_version: String,
    pub simkit_version: String,
    pub xcode_version: String,
    pub xcode_path: String,
    pub input_path: InputPath,
    /// The UI tree (`AXPTranslator`) loaded.
    pub ax: bool,
}

/// One line from the sim host.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HostLine {
    Hello(Hello),
    Ok {
        id: u64,
        #[serde(default)]
        result: Value,
    },
    Err {
        id: u64,
        /// A [`crate::job::Fail`] code.
        code: String,
        message: String,
    },
    /// A new ring for a device (attach, or a new screen size): `device` numbers its records.
    Frames {
        target: Target,
        device: u32,
        ring: u32,
        screen: Screen,
    },
    Orientation {
        target: Target,
        orientation: Orientation,
    },
    State {
        target: Target,
        state: DeviceState,
    },
    Broken {
        why: String,
    },
}

/// One JSON line (no newline inside; the caller writes the `\n`).
pub fn to_line<T: Serialize>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_default()
}

/// A line read back, None for anything not shaped right (logged by the caller, never guessed at).
pub fn from_line<T: for<'de> Deserialize<'de>>(line: &str) -> Option<T> {
    serde_json::from_str(line.trim_end()).ok()
}

/// A frame is ready in `slot` of the device's current ring.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameRecord {
    pub device: u32,
    pub slot: u32,
    pub seq: u64,
    /// When the framebuffer callback came, in the sim host's monotonic microseconds.
    pub t_us: u64,
}

/// Flare's GPU work that sampled `slot` is done (or Flare skipped it): the slot is free again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotDone {
    pub device: u32,
    pub slot: u32,
}

/// The device's screen changed `count` times since attach (sent while paused too).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Changed {
    pub device: u32,
    pub count: u64,
}

/// A record on the mach channel. Byte layout, little-endian, no other padding:
///
/// | record | bytes | fields |
/// |---|---|---|
/// | frame | 32 | kind u32 = 1, device u32, slot u32, zero u32, seq u64, t_us u64 |
/// | slot done | 16 | kind u32 = 2, device u32, slot u32, zero u32 |
/// | changed | 16 | kind u32 = 3, device u32, count u64 |
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record {
    Frame(FrameRecord),
    SlotDone(SlotDone),
    Changed(Changed),
}

pub const FRAME_LEN: usize = 32;
pub const SLOT_DONE_LEN: usize = 16;
pub const CHANGED_LEN: usize = 16;

fn u32_at(b: &[u8], at: usize) -> u32 {
    let mut a = [0u8; 4];
    a.copy_from_slice(&b[at..at + 4]);
    u32::from_le_bytes(a)
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[at..at + 8]);
    u64::from_le_bytes(a)
}

impl Record {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(FRAME_LEN);
        match self {
            Record::Frame(f) => {
                b.extend(1u32.to_le_bytes());
                b.extend(f.device.to_le_bytes());
                b.extend(f.slot.to_le_bytes());
                b.extend(0u32.to_le_bytes());
                b.extend(f.seq.to_le_bytes());
                b.extend(f.t_us.to_le_bytes());
            }
            Record::SlotDone(d) => {
                b.extend(2u32.to_le_bytes());
                b.extend(d.device.to_le_bytes());
                b.extend(d.slot.to_le_bytes());
                b.extend(0u32.to_le_bytes());
            }
            Record::Changed(c) => {
                b.extend(3u32.to_le_bytes());
                b.extend(c.device.to_le_bytes());
                b.extend(c.count.to_le_bytes());
            }
        }
        b
    }

    /// The record in `b`, or None for a wrong kind, a wrong length or a padding that is not zero.
    pub fn from_bytes(b: &[u8]) -> Option<Record> {
        if b.len() < 4 {
            return None;
        }
        match (u32_at(b, 0), b.len()) {
            (1, FRAME_LEN) if u32_at(b, 12) == 0 => Some(Record::Frame(FrameRecord {
                device: u32_at(b, 4),
                slot: u32_at(b, 8),
                seq: u64_at(b, 16),
                t_us: u64_at(b, 24),
            })),
            (2, SLOT_DONE_LEN) if u32_at(b, 12) == 0 => Some(Record::SlotDone(SlotDone {
                device: u32_at(b, 4),
                slot: u32_at(b, 8),
            })),
            (3, CHANGED_LEN) => Some(Record::Changed(Changed {
                device: u32_at(b, 4),
                count: u64_at(b, 8),
            })),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Size;
    use crate::source::Phase;

    #[test]
    fn every_message_round_trips() {
        let t = crate::target::parse("sim/0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0").unwrap();
        let reqs = vec![
            Request::Hello { protocol: PROTOCOL },
            Request::List,
            Request::Attach { target: t.clone() },
            Request::Detach { target: t.clone() },
            Request::Pause { target: t.clone() },
            Request::Resume { target: t.clone() },
            Request::Touch {
                target: t.clone(),
                touch: Touch {
                    phase: Phase::Move,
                    x: 1.5,
                    y: 2.25,
                    second: Some((3.0, 4.0)),
                },
            },
            Request::Touch {
                target: t.clone(),
                touch: Touch::one(Phase::End, 0.0, 874.0),
            },
            Request::Key {
                target: t.clone(),
                key: Key {
                    usage: 0x28,
                    down: true,
                },
            },
            Request::Button {
                target: t.clone(),
                button: Button::VolumeUp,
            },
            Request::Text {
                target: t.clone(),
                text: "caf\u{e9} \"q\"\n".into(),
            },
            Request::Snapshot { target: t.clone() },
            Request::UiTree {
                target: t.clone(),
                at: None,
            },
            Request::UiTree {
                target: t.clone(),
                at: Some((10.0, 20.0)),
            },
            Request::Launch {
                target: t.clone(),
                bundle_id: "com.apple.Preferences".into(),
                args: vec![],
            },
            Request::Launch {
                target: t.clone(),
                bundle_id: "a.b".into(),
                args: vec!["-x".into()],
            },
            Request::Install {
                target: t.clone(),
                path: "/tmp/A.app".into(),
            },
            Request::Boot { target: t.clone() },
            Request::Shutdown { target: t.clone() },
            Request::Quit,
        ];
        for (id, req) in reqs.into_iter().enumerate() {
            let c = Control { id: id as u64, req };
            let line = to_line(&c);
            assert!(!line.contains('\n'), "{line}");
            assert_eq!(
                from_line::<Control>(&format!("{line}\n")),
                Some(c),
                "{line}"
            );
        }
        // The names on the wire are the plan's.
        assert_eq!(
            to_line(&Control {
                id: 7,
                req: Request::Quit
            }),
            r#"{"id":7,"op":"quit"}"#
        );
        assert!(
            to_line(&Control {
                id: 1,
                req: Request::UiTree {
                    target: t.clone(),
                    at: None
                }
            })
            .contains(r#""op":"ui_tree""#)
        );
        let hello = Hello {
            protocol: PROTOCOL,
            version: "0.1.0".into(),
            coresim_version: "1174.21".into(),
            simkit_version: "1005.14".into(),
            xcode_version: "27.2".into(),
            xcode_path: "/Applications/Xcode.app/Contents/Developer".into(),
            input_path: InputPath::Dtuhid,
            ax: true,
        };
        let screen = Screen {
            size_px: Size::new(1206.0, 2622.0),
            size_pt: Size::new(402.0, 874.0),
            scale: 3.0,
            orientation: Orientation::Rot90,
        };
        let lines = vec![
            HostLine::Hello(hello),
            HostLine::Ok {
                id: 1,
                result: serde_json::json!({"devices": [], "n": 1.5}),
            },
            HostLine::Ok {
                id: 2,
                result: Value::Null,
            },
            HostLine::Err {
                id: 3,
                code: "busy".into(),
                message: "booting".into(),
            },
            HostLine::Frames {
                target: t.clone(),
                device: 1,
                ring: 2,
                screen,
            },
            HostLine::Orientation {
                target: t.clone(),
                orientation: Orientation::Rot270,
            },
            HostLine::State {
                target: t.clone(),
                state: DeviceState::Booted,
            },
            HostLine::Broken {
                why: "SimDevice missing in CoreSimulator 1174.21".into(),
            },
        ];
        for l in lines {
            let line = to_line(&l);
            assert_eq!(from_line::<HostLine>(&line), Some(l), "{line}");
        }
        // Anything not shaped right is refused, never guessed at.
        for bad in [
            "",
            "{}",
            r#"{"id":1}"#,
            r#"{"id":1,"op":"explode"}"#,
            r#"{"id":1,"op":"attach","target":"sim/nope"}"#,
            r#"{"id":1,"op":"attach","target":"sim/0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0","extra":1}"#,
        ] {
            assert_eq!(from_line::<Control>(bad), None, "{bad}");
        }
        // The mach records, by their byte layout.
        let recs = [
            Record::Frame(FrameRecord {
                device: 1,
                slot: 2,
                seq: u64::MAX - 1,
                t_us: 123_456_789,
            }),
            Record::SlotDone(SlotDone { device: 7, slot: 0 }),
            Record::Changed(Changed {
                device: u32::MAX,
                count: 42,
            }),
        ];
        for (r, len) in recs
            .into_iter()
            .zip([FRAME_LEN, SLOT_DONE_LEN, CHANGED_LEN])
        {
            let b = r.to_bytes();
            assert_eq!(b.len(), len);
            assert_eq!(Record::from_bytes(&b), Some(r));
            assert_eq!(Record::from_bytes(&b[..len - 1]), None, "short");
            let mut long = b.clone();
            long.push(0);
            assert_eq!(Record::from_bytes(&long), None, "long");
        }
        let f = Record::Frame(FrameRecord {
            device: 1,
            slot: 2,
            seq: 3,
            t_us: 4,
        })
        .to_bytes();
        assert_eq!(&f[..16], &[1, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(&f[16..24], &3u64.to_le_bytes());
        let mut bad = f.clone();
        bad[12] = 1;
        assert_eq!(Record::from_bytes(&bad), None, "padding must be zero");
        let mut bad = f;
        bad[0] = 9;
        assert_eq!(Record::from_bytes(&bad), None, "unknown kind");
        assert_eq!(Record::from_bytes(&[1, 0]), None);
    }
}

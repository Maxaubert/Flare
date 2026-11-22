//! The sim host's own protocol (internal: both ends ship in one bundle, and Flare refuses a sim host whose
//! [`PROTOCOL`] does not match, which only catches a broken install).
//!
//! Control goes over the sim host's stdin and stdout as JSON lines: Flare sends a [`Control`] (an id and a
//! [`Request`]), the sim host answers with [`HostLine`]s (`ok` or `err` with the id, and events with none). The
//! first line it writes is its [`Hello`].
//!
//! Frames go over the mach channel the rendezvous sets up (PR 2), as fixed little-endian [`Record`]s: a frame is
//! ready in a slot (sim host to Flare), a slot is done (Flare to sim host), the screen changed (sim host to Flare).
//! `device` in a record is the small number the sim host gave the ring in its `frames` event: a fresh one per ring
//! (attach, or a new screen size), so a slot-done record always names its ring while an old ring waits for its last
//! slots. The ring's surfaces come over the mach channel with the same number (`macos::rendezvous`).
//!
//! The emu host (Android, rule 1360) speaks the same protocol: its `hello` says `host: "emu"` and its facts, and since
//! the emulator writes each frame into a region Flare owns, a frame is one [`HostLine::Region`] line on its stdout
//! (rule 1363); there is no mach channel.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::geom::Orientation;
use crate::source::{Button, DeviceState, InputState, Key, Screen, Touch};
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

/// Which helper process speaks: the sim host (iOS) or the emu host (Android). Both use this protocol (rule 1360).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostKind {
    /// `flare-sim-host`. The default, so a sim host that does not say its kind still reads.
    #[default]
    Sim,
    /// `flare-emu-host`.
    Emu,
}

/// A helper's facts, its first line. The sim host fills the named fields; the emu host leaves them empty and says its
/// own facts in `facts` (`sdk_path`, `emulator_version`, `adb_version`, `accel`, `agent_jar`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Hello {
    pub protocol: u32,
    /// The helper's own version (Flare's).
    pub version: String,
    /// Which helper this is. Named `host` on the wire, since `kind` tags the line itself.
    #[serde(default)]
    pub host: HostKind,
    /// The loaded CoreSimulator's and SimulatorKit's `CFBundleVersion` (sim host).
    #[serde(default)]
    pub coresim_version: String,
    #[serde(default)]
    pub simkit_version: String,
    #[serde(default)]
    pub xcode_version: String,
    #[serde(default)]
    pub xcode_path: String,
    /// The simulator's input path (sim host); None from the emu host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_path: Option<InputPath>,
    /// The UI tree (`AXPTranslator`) loaded (sim host).
    #[serde(default)]
    pub ax: bool,
    /// Any other facts, by name, for the log and `device_state` (the emu host's). Never a secret: the emulator's
    /// gRPC token is never in it (rule 1362).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub facts: BTreeMap<String, String>,
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
    /// A new ring for a device (attach, or a new screen size): `device` numbers its records (fresh per ring).
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
    /// A frame is in the device's frame region (emu host, rule 1363): one small line per frame, at most 60 a second
    /// (`cap::FrameCap`). `w` and `h` are the frame's pixels as the emulator wrote them, already turned by
    /// `rotation`. Becomes [`crate::source::SourceEvent::Region`].
    Region {
        target: Target,
        seq: u64,
        t_us: u64,
        w: u32,
        h: u32,
        rotation: Orientation,
    },
    State {
        target: Target,
        state: DeviceState,
    },
    /// The device's input connection changed.
    Input {
        target: Target,
        state: InputState,
    },
    Broken {
        why: String,
    },
}

/// A snapshot's answer: `{"png": <base64>}`.
pub fn png_result(png: &[u8]) -> Value {
    use base64::Engine as _;
    serde_json::json!({ "png": base64::engine::general_purpose::STANDARD.encode(png) })
}

/// The PNG in a snapshot's answer, None when it is not one.
pub fn png_of(result: &Value) -> Option<Vec<u8>> {
    use base64::Engine as _;
    let b = base64::engine::general_purpose::STANDARD
        .decode(result.get("png")?.as_str()?)
        .ok()?;
    b.starts_with(b"\x89PNG\r\n\x1a\n").then_some(b)
}

/// A UI tree's answer: `{"tree": <UiNode>}`.
pub fn tree_result(tree: &crate::tree::UiNode) -> Value {
    serde_json::json!({ "tree": tree })
}

/// The tree in a UI tree's answer.
pub fn tree_of(result: &Value) -> Option<crate::tree::UiNode> {
    serde_json::from_value(result.get("tree")?.clone()).ok()
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
            host: HostKind::Sim,
            coresim_version: "1174.21".into(),
            simkit_version: "1005.14".into(),
            xcode_version: "27.2".into(),
            xcode_path: "/Applications/Xcode.app/Contents/Developer".into(),
            input_path: Some(InputPath::Dtuhid),
            ax: true,
            facts: BTreeMap::new(),
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
            HostLine::Input {
                target: t.clone(),
                state: InputState::Connecting,
            },
            HostLine::Input {
                target: t.clone(),
                state: InputState::Broken {
                    why: "dtuhidd gave no liveness reply in 5 tries".into(),
                },
            },
            HostLine::Broken {
                why: "SimDevice missing in CoreSimulator 1174.21".into(),
            },
        ];
        for l in lines {
            let line = to_line(&l);
            assert_eq!(from_line::<HostLine>(&line), Some(l), "{line}");
        }
        // A snapshot's PNG and a tree cross as JSON.
        let png = b"\x89PNG\r\n\x1a\nrest".to_vec();
        let v = png_result(&png);
        assert_eq!(
            png_of(&from_line::<Value>(&to_line(&v)).unwrap()),
            Some(png)
        );
        assert_eq!(
            png_of(&serde_json::json!({"png": "bm90IGEgcG5n"})),
            None,
            "not a PNG"
        );
        assert_eq!(png_of(&Value::Null), None);
        let tree = crate::tree::UiNode {
            role: "AXButton".into(),
            label: "OK".into(),
            ..Default::default()
        };
        assert_eq!(tree_of(&tree_result(&tree)), Some(tree));
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

    /// Rule 1360: the emu host speaks the same protocol, with its kind and facts in `hello` and a frame as one
    /// `region` line.
    #[test]
    fn the_emu_hosts_lines_round_trip() {
        let t = crate::target::parse("emu/Flare_Pixel").unwrap();
        let hello = Hello {
            protocol: PROTOCOL,
            version: "0.1.0".into(),
            host: HostKind::Emu,
            coresim_version: String::new(),
            simkit_version: String::new(),
            xcode_version: String::new(),
            xcode_path: String::new(),
            input_path: None,
            ax: false,
            facts: [
                (
                    "sdk_path",
                    "C:\\Users\\a\\AppData\\Local\\Flare\\android-sdk",
                ),
                ("emulator_version", "37.3.10"),
                ("adb_version", "37.0.3"),
                ("accel", "whpx"),
                ("agent_jar", "1"),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        };
        let line = to_line(&HostLine::Hello(hello.clone()));
        assert!(line.contains(r#""host":"emu""#), "{line}");
        assert!(
            !line.contains("input_path"),
            "no simulator input path: {line}"
        );
        assert_eq!(
            from_line::<HostLine>(&line),
            Some(HostLine::Hello(hello.clone()))
        );
        // The emu host may leave out every simulator field.
        let short = r#"{"kind":"hello","protocol":1,"version":"0.1.0","facts":{"emulator_version":"37.3.10"}}"#;
        let Some(HostLine::Hello(h)) = from_line::<HostLine>(short) else {
            panic!("{short}")
        };
        assert_eq!(h.host, HostKind::Sim, "no host is the sim host");
        assert_eq!(h.facts["emulator_version"], "37.3.10");
        assert!(h.xcode_version.is_empty() && h.input_path.is_none() && !h.ax);
        // A sim host's hello from before this field still reads, as the sim host.
        let old = r#"{"kind":"hello","protocol":1,"version":"0.1.0","coresim_version":"1174.21","simkit_version":"1005.14","xcode_version":"27.2","xcode_path":"/x","input_path":"dtuhid","ax":true}"#;
        let Some(HostLine::Hello(h)) = from_line::<HostLine>(old) else {
            panic!("{old}")
        };
        assert_eq!(
            (h.host, h.input_path, h.facts.len()),
            (HostKind::Sim, Some(InputPath::Dtuhid), 0)
        );
        // A frame record.
        let region = HostLine::Region {
            target: t.clone(),
            seq: 901,
            t_us: 15_016_000,
            w: 2400,
            h: 1080,
            rotation: Orientation::Rot90,
        };
        let line = to_line(&region);
        assert_eq!(
            line,
            r#"{"kind":"region","target":"emu/Flare_Pixel","seq":901,"t_us":15016000,"w":2400,"h":1080,"rotation":"rot90"}"#
        );
        assert_eq!(from_line::<HostLine>(&line), Some(region));
        // Requests name an emulator like a simulator.
        let c = Control {
            id: 3,
            req: Request::Button {
                target: t.clone(),
                button: Button::Back,
            },
        };
        assert_eq!(
            to_line(&c),
            r#"{"id":3,"op":"button","target":"emu/Flare_Pixel","button":"back"}"#
        );
        assert_eq!(from_line::<Control>(&to_line(&c)), Some(c));
        for bad in [
            r#"{"kind":"region","target":"emu/../x","seq":1,"t_us":1,"w":1,"h":1,"rotation":"rot0"}"#,
            r#"{"kind":"region","target":"emu/A","seq":1,"t_us":1,"w":-1,"h":1,"rotation":"rot0"}"#,
            r#"{"kind":"region","target":"emu/A","seq":1,"t_us":1,"w":1,"h":1,"rotation":"rot45"}"#,
            r#"{"kind":"region","target":"emu/A","seq":1,"t_us":1,"w":1,"h":1}"#,
            r#"{"kind":"hello","protocol":1,"version":"x","host":"emu","facts":{"a":1}}"#,
            r#"{"kind":"hello","protocol":1,"version":"x","host":"android"}"#,
        ] {
            assert_eq!(from_line::<HostLine>(bad), None, "{bad}");
        }
    }
}

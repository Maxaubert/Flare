//! Rotation (decision 11, spike S5): the device is turned headless with a Purple GSEvent 50 sent to the simulator's
//! `PurpleWorkspacePort`, and the interface's orientation is read from CoreDevice's `displayinfo` (the primary
//! display's `currentOrientation`). The framebuffer never turns (it stays portrait); Flare turns the picture by the
//! orientation the sim host reports, and only the sim host turns touch points back (`geom::to_portrait_ratio`).
//!
//! The panel follows `displayinfo`, not the device: an app that is portrait only keeps `rot0` while the device lies
//! on its side. Purple values: 1 portrait, 2 upside down, 3 landscape left (interface `rot270`), 4 landscape right
//! (interface `rot90`).
//!
//! Message shapes follow idb (`FBSimulatorControl/HID/SimulatorPurpleHID.swift`,
//! `CoreDevice/SimulatorDisplayProtocol.swift`, `CoreDevice/CoreDeviceEnvelope.swift`, facebook/idb, MIT, Meta
//! Platforms).

use std::path::Path;
use std::time::{Duration, Instant};

use objc2::runtime::AnyObject;

use super::guarded::{Broken, Why};
use super::xpc::{Conn, XVal, lookup};
use crate::geom::Orientation;

/// The orientation port inside the simulator.
pub const PURPLE_PORT: &str = "PurpleWorkspacePort";
/// CoreDevice's display service and action.
pub const DISPLAY_SERVICE: &str = "com.apple.coredevice.feature.getdisplayinfo";
pub const DISPLAY_ACTION: &str = "com.apple.coredevice.action.displayinfo";
/// CoreDevice, whose version every CoreDevice request carries.
pub const COREDEVICE: &str = "/Library/Developer/PrivateFrameworks/CoreDevice.framework";
/// How long a `displayinfo` read may take.
pub const READ_LIMIT: Duration = Duration::from_secs(2);
/// After a rotate, `displayinfo` is read every [`POLL_EVERY`] until it changes, at most [`POLL_FOR`] (S5: the
/// change shows 41 to 54 ms after the send; a portrait-only app never changes).
pub const POLL_EVERY: Duration = Duration::from_millis(10);
pub const POLL_FOR: Duration = Duration::from_millis(1500);
/// The longest wait for the Purple send.
const SEND_TIMEOUT_MS: u32 = 2000;

/// The GSEvent's type (device orientation changed), the host flag and the mach message id.
const GS_ORIENTATION: u32 = 50;
const GS_HOST_FLAG: u32 = 0x2_0000;
const GS_MSG_ID: u32 = 0x7B;
/// `MACH_MSG_TYPE_COPY_SEND` for the remote port, no local port.
const BITS_COPY_SEND: u32 = 0x13;
/// The message's size on the wire.
pub const PURPLE_LEN: u32 = 108;

/// The Purple value that turns the device so the interface shows `o` (when the app allows it).
pub fn purple_value(o: Orientation) -> u32 {
    match o {
        Orientation::Rot0 => 1,
        Orientation::Rot180 => 2,
        Orientation::Rot270 => 3,
        Orientation::Rot90 => 4,
    }
}

/// The device turned a quarter: left is anticlockwise (the picture turns anticlockwise, `rot270` from portrait),
/// right is clockwise (`rot90` from portrait).
pub fn turned(o: Orientation, left: bool) -> Orientation {
    use Orientation::*;
    match (o, left) {
        (Rot0, false) | (Rot180, true) => Rot90,
        (Rot90, false) | (Rot270, true) => Rot180,
        (Rot180, false) | (Rot0, true) => Rot270,
        (Rot270, false) | (Rot90, true) => Rot0,
    }
}

/// The GSEvent 50 mach message for Purple value `value`, addressed to `port` (112 bytes, 108 sent).
pub fn purple_message(port: u32, value: u32) -> [u8; 112] {
    let mut b = [0u8; 112];
    let mut put = |at: usize, v: u32| b[at..at + 4].copy_from_slice(&v.to_le_bytes());
    put(0x00, BITS_COPY_SEND);
    put(0x04, PURPLE_LEN);
    put(0x08, port);
    put(0x14, GS_MSG_ID);
    put(0x18, GS_ORIENTATION | GS_HOST_FLAG);
    put(0x48, 4);
    put(0x4C, value);
    b
}

/// Sends the GSEvent that turns the device to `o`. BLOCKING (at most 2 s): the device's input worker.
pub fn rotate(device: *mut AnyObject, o: Orientation) -> Result<(), Broken> {
    let port = lookup(device, PURPLE_PORT)?;
    let mut msg = purple_message(port, purple_value(o));
    let kr = super::rendezvous::send_raw(&mut msg, PURPLE_LEN, SEND_TIMEOUT_MS);
    if kr != 0 {
        return Err(Broken::nothing(
            PURPLE_PORT,
            format!("mach_msg kern_return {kr:#x}"),
        ));
    }
    Ok(())
}

/// CoreDevice's version as its requests carry it.
pub fn coredevice_version(version: &str) -> XVal {
    let parts: Vec<XVal> = version
        .split('.')
        .filter_map(|p| p.parse::<u64>().ok())
        .map(XVal::U64)
        .collect();
    XVal::dict([
        ("components", XVal::Arr(parts.clone())),
        ("originalComponentsCount", XVal::I64(parts.len() as i64)),
        ("stringValue", XVal::Str(version.into())),
    ])
}

/// A CoreDevice request for `action` on the device `udid`.
pub fn request(action: &str, udid: &str, version: &str, invocation: &str) -> XVal {
    XVal::dict([
        ("CoreDevice.actionIdentifier", XVal::Str(action.into())),
        ("CoreDevice.deviceIdentifier", XVal::Str(udid.into())),
        (
            "CoreDevice.invocationIdentifier",
            XVal::Str(invocation.into()),
        ),
        ("CoreDevice.CoreDeviceDDIProtocolVersion", XVal::I64(1)),
        ("CoreDevice.coreDeviceVersion", coredevice_version(version)),
        ("CoreDevice.input", XVal::Dict(Vec::new())),
    ])
}

/// Reads the interface's orientation (the primary display's `currentOrientation`). BLOCKING (1 to 4 ms, at most
/// [`READ_LIMIT`]): the sim host's workers.
pub fn read(device: *mut AnyObject, udid: &str) -> Result<Orientation, Broken> {
    let version = super::load::bundle_version(Path::new(COREDEVICE))
        .ok_or_else(|| Broken::missing("CoreDevice.framework"))?;
    let conn = Conn::open(device, DISPLAY_SERVICE)?;
    let invocation = objc2_foundation::NSUUID::new().UUIDString().to_string();
    let reply = conn.request(
        &request(
            DISPLAY_ACTION,
            &udid.to_ascii_uppercase(),
            &version,
            &invocation,
        ),
        READ_LIMIT,
    )?;
    let names: Vec<String> = reply
        .items(&["CoreDevice.output", "displays"])
        .into_iter()
        .filter_map(|d| {
            let r = super::xpc::Reply(d);
            let primary = r.at(&["primary"]).and_then(|v| v.value()) == Some(XVal::Bool(true));
            match r.at(&["currentOrientation"]).and_then(|v| v.value()) {
                Some(XVal::Str(s)) if primary => Some(s),
                _ => None,
            }
        })
        .collect();
    let name = names
        .first()
        .ok_or_else(|| Broken::nothing(DISPLAY_ACTION, "no primary display"))?;
    Orientation::parse(name).ok_or_else(|| Broken {
        symbol: DISPLAY_ACTION.into(),
        why: Why::Nothing(format!("an orientation this build does not know: {name}")),
    })
}

/// Reads until the orientation is no longer `before` (or the time is up): the last one read.
pub fn poll_change(
    before: Orientation,
    mut read_once: impl FnMut() -> Result<Orientation, Broken>,
) -> Result<Orientation, Broken> {
    let end = Instant::now() + POLL_FOR;
    loop {
        let now = read_once()?;
        if now != before || Instant::now() >= end {
            return Ok(now);
        }
        std::thread::sleep(POLL_EVERY); // sync-ok: the input worker's own pacing, at most 1.5 s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_turns_a_quarter_and_writes_the_purple_event() {
        use Orientation::*;
        // Right is clockwise, left the other way, four quarters make a turn.
        assert_eq!(turned(Rot0, false), Rot90);
        assert_eq!(turned(Rot0, true), Rot270);
        for o in Orientation::ALL {
            assert_eq!(turned(turned(o, false), true), o);
            let mut x = o;
            for _ in 0..4 {
                x = turned(x, false);
            }
            assert_eq!(x, o);
        }
        // S5's values: 1 portrait, 2 upside down, 3 interface rot270, 4 interface rot90.
        assert_eq!(Orientation::ALL.map(purple_value), [1, 4, 2, 3]);
        let m = purple_message(0x1234, 4);
        let u32_at = |at: usize| u32::from_le_bytes(m[at..at + 4].try_into().unwrap());
        assert_eq!(u32_at(0x00), 0x13, "copy-send remote, no local port");
        assert_eq!(u32_at(0x04), 108);
        assert_eq!(u32_at(0x08), 0x1234);
        assert_eq!(u32_at(0x0C), 0);
        assert_eq!(u32_at(0x14), 0x7B);
        assert_eq!(u32_at(0x18), 50 | 0x2_0000);
        assert_eq!(u32_at(0x48), 4);
        assert_eq!(u32_at(0x4C), 4);
        // The CoreDevice request carries the framework's version part by part.
        let r = request(DISPLAY_ACTION, "ABC", "651.32", "id");
        let v = r.get("CoreDevice.coreDeviceVersion").unwrap();
        assert_eq!(
            v.get("components"),
            Some(&XVal::Arr(vec![XVal::U64(651), XVal::U64(32)]))
        );
        assert_eq!(v.get("originalComponentsCount"), Some(&XVal::I64(2)));
        assert_eq!(
            r.get("CoreDevice.actionIdentifier"),
            Some(&XVal::Str(DISPLAY_ACTION.into()))
        );
        // A poll stops at the first change.
        let mut reads = vec![Ok(Rot90), Ok(Rot0), Ok(Rot0)];
        assert_eq!(poll_change(Rot0, || reads.pop().unwrap()), Ok(Rot90));
        assert!(reads.is_empty());
    }
}

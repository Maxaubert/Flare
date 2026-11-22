//! The panel's buttons, keys and text as the emulator's input (spec builder's calls 8 and 9, rule 1368).
//!
//! - Keys from the person or the agent are USB HID usages (`keymap.rs`); the emulator's `KeyboardEvent` takes them as
//!   `codeType: Usb` with the page in the high bits (`0x07_0004` is A, Chromium's `keycode_converter_data.inc`, which
//!   the emulator translates with), key down and key up apart.
//! - Buttons: Home, Back, Recents and Power are the W3C key names the emulator knows (`GoHome`, `GoBack`, `AppSwitch`,
//!   `Power`), volume is evdev 115 and 114, and a rotation turns the virtual device (`setPhysicalModel(ROTATION)`).
//! - Text the keymap cannot type goes through the device's own clipboard (gRPC `setClipboard`, which is the guest's)
//!   and Ctrl+V. The computer's clipboard is never touched (rule 1368). The agent helper's set-text (PR A3) comes
//!   first once it is there.

use crate::geom::{Orientation, Size};
use crate::keymap::{self, Typing, usage};
use crate::source::{Button, Key};

/// The USB HID keyboard page, in the emulator's `Usb` key codes.
pub const USB_PAGE: u32 = 0x07_0000;
/// Left Ctrl, for Ctrl+V.
pub const LEFT_CTRL: u32 = 0xE0;
/// evdev `KEY_VOLUMEUP` and `KEY_VOLUMEDOWN`.
pub const EVDEV_VOLUME_UP: i32 = 115;
pub const EVDEV_VOLUME_DOWN: i32 = 114;

/// The emulator's `Usb` key code of a HID usage.
pub fn usb_code(usage: u32) -> i32 {
    i32::try_from(USB_PAGE | (usage & 0xFFFF)).unwrap_or(0)
}

/// What a button press is on the emulator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Press {
    /// A key by its W3C name, pressed and let go.
    Named(&'static str),
    /// An evdev key code, pressed and let go.
    Evdev(i32),
    /// Turn the device a quarter: +1 clockwise (right), -1 anticlockwise (left).
    Turn(i32),
}

/// The emulator's press for a button, None for a button the emulator has no match for (Lock and Shake are iOS only;
/// the panel's Lock button sends Power on Android, rule 1371).
pub fn press(b: Button) -> Option<Press> {
    Some(match b {
        Button::Home => Press::Named("GoHome"),
        Button::Back => Press::Named("GoBack"),
        Button::Recents => Press::Named("AppSwitch"),
        Button::Power => Press::Named("Power"),
        Button::VolumeUp => Press::Evdev(EVDEV_VOLUME_UP),
        Button::VolumeDown => Press::Evdev(EVDEV_VOLUME_DOWN),
        Button::RotateRight => Press::Turn(1),
        Button::RotateLeft => Press::Turn(-1),
        Button::Lock | Button::Shake => return None,
    })
}

/// The orientation after a quarter turn (`delta` +1 or -1) from `o`.
pub fn turned(o: Orientation, delta: i32) -> Orientation {
    let i = Orientation::ALL.iter().position(|x| *x == o).unwrap_or(0) as i32;
    Orientation::ALL[(i + delta).rem_euclid(4) as usize]
}

/// The virtual device's rotation for `setPhysicalModel(ROTATION)`: degrees about x, y and z. A quarter turn of the
/// screen is the device turned about z (`[0, 0, 90]` turns it to landscape, the emulator's own rotate step).
pub fn rotation_degrees(o: Orientation) -> [f32; 3] {
    match o {
        Orientation::Rot0 => [0.0, 0.0, 0.0],
        Orientation::Rot90 => [0.0, 0.0, 90.0],
        Orientation::Rot180 => [0.0, 0.0, 180.0],
        Orientation::Rot270 => [0.0, 0.0, -90.0],
    }
}

/// One step of typing on the emulator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextStep {
    /// HID keys, in order.
    Keys(Vec<Key>),
    /// Put the text on the device's own clipboard (never the computer's, rule 1368).
    DeviceClipboard(String),
}

/// Ctrl+V: paste the device's clipboard.
pub fn paste_keys() -> Vec<Key> {
    let k = |usage, down| Key { usage, down };
    vec![
        k(LEFT_CTRL, true),
        k(usage::V, true),
        k(usage::V, false),
        k(LEFT_CTRL, false),
    ]
}

/// How `text` is typed (rule 1368): as HID keys with the US keymap, or, when a character has no key, all of it through
/// the device's clipboard and Ctrl+V (so its order is kept). Empty text types nothing.
pub fn text_steps(text: &str) -> Vec<TextStep> {
    if text.is_empty() {
        return Vec::new();
    }
    match keymap::typing(text) {
        Typing::Keys(keys) => vec![TextStep::Keys(
            keys.into_iter().flat_map(keymap::events).collect(),
        )],
        Typing::Paste(t) => vec![TextStep::DeviceClipboard(t), TextStep::Keys(paste_keys())],
    }
}

/// A point in dp, in the current orientation, as the emulator's touch pixels: dp times `scale` (pixels per dp,
/// rule 1367), clamped to the screen as shown (`portrait_px` turned by `o`), rounded.
pub fn touch_px(x: f64, y: f64, scale: f64, portrait_px: Size, o: Orientation) -> (i32, i32) {
    let shown = o.size(portrait_px);
    let (px, py) = crate::geom::dp_to_px(x, y, scale);
    let clamp = |v: f64, max: f64| v.clamp(0.0, (max - 1.0).max(0.0)).round() as i32;
    (clamp(px, shown.w), clamp(py, shown.h))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rule 1371 on the emulator: every Android button has a press, the iOS-only ones none.
    #[test]
    fn android_buttons_are_named_keys_volume_codes_and_turns() {
        use crate::source::SourceKind;
        for b in SourceKind::Emu.buttons() {
            assert!(press(*b).is_some(), "{b:?}");
        }
        assert_eq!(press(Button::Lock), None);
        assert_eq!(press(Button::Shake), None);
        assert_eq!(press(Button::Home), Some(Press::Named("GoHome")));
        assert_eq!(press(Button::Back), Some(Press::Named("GoBack")));
        assert_eq!(press(Button::Recents), Some(Press::Named("AppSwitch")));
        assert_eq!(press(Button::Power), Some(Press::Named("Power")));
        assert_eq!(press(Button::VolumeUp), Some(Press::Evdev(115)));
        assert_eq!(press(Button::VolumeDown), Some(Press::Evdev(114)));
        assert_eq!(usb_code(0x04), 0x07_0004, "A on the USB page");
        assert_eq!(usb_code(usage::RETURN), 0x07_0028);
        // Turns go round both ways.
        assert_eq!(turned(Orientation::Rot0, 1), Orientation::Rot90);
        assert_eq!(turned(Orientation::Rot0, -1), Orientation::Rot270);
        assert_eq!(turned(Orientation::Rot270, 1), Orientation::Rot0);
        for o in Orientation::ALL {
            assert_eq!(turned(turned(o, 1), -1), o);
        }
        assert_eq!(rotation_degrees(Orientation::Rot0), [0.0, 0.0, 0.0]);
        assert_eq!(rotation_degrees(Orientation::Rot90)[2], 90.0);
    }

    /// Rule 1368: text without keys goes through the device's clipboard and Ctrl+V, never the computer's.
    #[test]
    fn text_without_keys_goes_through_the_devices_clipboard() {
        assert_eq!(text_steps(""), vec![]);
        let TextStep::Keys(keys) = &text_steps("ok")[0] else {
            panic!()
        };
        assert_eq!(keys.len(), 4, "o and k, down and up");
        assert_eq!(
            text_steps("bl\u{e5}b\u{e6}r"),
            vec![
                TextStep::DeviceClipboard("bl\u{e5}b\u{e6}r".into()),
                TextStep::Keys(vec![
                    Key { usage: LEFT_CTRL, down: true },
                    Key { usage: usage::V, down: true },
                    Key { usage: usage::V, down: false },
                    Key { usage: LEFT_CTRL, down: false },
                ])
            ]
        );
    }

    #[test]
    fn a_dp_point_is_a_touch_pixel_on_the_screen_as_shown() {
        let p = Size::new(1080.0, 2400.0);
        assert_eq!(touch_px(0.0, 0.0, 2.625, p, Orientation::Rot0), (0, 0));
        assert_eq!(touch_px(205.7, 457.1, 2.625, p, Orientation::Rot0), (540, 1200));
        assert_eq!(touch_px(500.0, 2000.0, 2.625, p, Orientation::Rot0), (1079, 2399), "clamped");
        // Landscape: the screen as shown is 2400 wide.
        assert_eq!(touch_px(800.0, 10.0, 2.625, p, Orientation::Rot90), (2100, 26));
        assert_eq!(touch_px(-5.0, 10.0, 2.625, p, Orientation::Rot90), (0, 26));
    }
}

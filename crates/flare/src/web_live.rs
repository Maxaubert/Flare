//! The page as a picture (macOS, CEF, port phase 5): where WebView2 puts its own child window over the panel on
//! Windows, the macOS backend draws off screen, so the window shows the backend's latest picture in the panel's page
//! box (`panel-web-frame`, `panel-web-live`) and hands the pointer, the wheel and the keys over that box to the
//! backend ([`WebBackend::pointer`], [`WebBackend::key`]). On Windows the backend has no picture, `panel-web-live`
//! stays false and the box takes no input, so nothing here runs.

use std::time::Instant;

use flare_web::{KeyInput, Mods, Pointer, PointerAct, PointerButton, Slot, ViewFrame};
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

use super::{Phase, WebCtl, is_typing_key};
use crate::AppWindow;

/// Two presses closer than this (and a few pixels) are a double click.
const DOUBLE_MS: u128 = 500;
const DOUBLE_PX: f32 = 5.0;

/// What the live picture last showed, and the last press (for the click count).
#[derive(Default)]
pub(super) struct Live {
    serial: Option<u64>,
    shown: bool,
    last_down: Option<(Instant, f32, f32, u8)>,
}

/// The modifier bits the panel sends: 1 shift, 2 ctrl, 4 alt, 8 cmd.
pub fn mods_of(bits: i32) -> Mods {
    Mods {
        shift: bits & 1 != 0,
        ctrl: bits & 2 != 0,
        alt: bits & 4 != 0,
        cmd: bits & 8 != 0,
    }
}

/// A key as Slint names it (`KeyEvent.text`: a character, or one of its private-use codes for the keys that type
/// nothing): the Win32 virtual key, the macOS key code and what it types. None for a modifier alone.
pub fn key_of(text: &str) -> Option<(u32, u16, String)> {
    let mut chars = text.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        // Several characters at once (an input method's or a dead key's result): text only.
        return Some((0, 0, text.to_string()));
    }
    let named = |vk: u32, mac: u16, typed: &str| Some((vk, mac, typed.to_string()));
    match c {
        '\u{8}' => named(0x08, 0x33, ""),
        '\t' => named(0x09, 0x30, "\t"),
        '\n' | '\r' => named(0x0D, 0x24, "\r"),
        '\u{1b}' => named(0x1B, 0x35, ""),
        '\u{7f}' => named(0x2E, 0x75, ""),
        ' ' => named(0x20, 0x31, " "),
        '\u{F700}' => named(0x26, 0x7E, ""),
        '\u{F701}' => named(0x28, 0x7D, ""),
        '\u{F702}' => named(0x25, 0x7B, ""),
        '\u{F703}' => named(0x27, 0x7C, ""),
        '\u{F729}' => named(0x24, 0x73, ""),
        '\u{F72B}' => named(0x23, 0x77, ""),
        '\u{F72C}' => named(0x21, 0x74, ""),
        '\u{F72D}' => named(0x22, 0x79, ""),
        '\u{F704}'..='\u{F70F}' => {
            const MAC_F: [u16; 12] = [
                0x7A, 0x78, 0x63, 0x76, 0x60, 0x61, 0x62, 0x64, 0x65, 0x6D, 0x67, 0x6F,
            ];
            let n = (c as u32 - 0xF704) as usize;
            named(0x70 + n as u32, MAC_F[n], "")
        }
        // Shift, Control, Alt, AltGr, CapsLock, the right-hand ones, Meta: no key of their own for the page.
        '\u{10}'..='\u{18}' => None,
        c if ('\u{F700}'..='\u{F8FF}').contains(&c) => None,
        c if c.is_ascii_alphabetic() => {
            let up = c.to_ascii_uppercase();
            named(u32::from(up), mac_letter(up), &c.to_string())
        }
        c if c.is_ascii_digit() => named(u32::from(c), mac_digit(c), &c.to_string()),
        c if c.is_control() => None,
        c => named(0, 0, &c.to_string()),
    }
}

/// The macOS key code (`kVK_ANSI_*`) of a letter on the ANSI layout.
fn mac_letter(c: char) -> u16 {
    const CODES: [u16; 26] = [
        0x00, 0x0B, 0x08, 0x02, 0x0E, 0x03, 0x05, 0x04, 0x22, 0x26, 0x28, 0x25, 0x2E, 0x2D, 0x1F,
        0x23, 0x0C, 0x0F, 0x01, 0x11, 0x20, 0x09, 0x0D, 0x07, 0x10, 0x06,
    ];
    CODES
        .get((c as u32).wrapping_sub(u32::from('A')) as usize)
        .copied()
        .unwrap_or(0)
}

fn mac_digit(c: char) -> u16 {
    const CODES: [u16; 10] = [0x1D, 0x12, 0x13, 0x14, 0x15, 0x17, 0x16, 0x1A, 0x1C, 0x19];
    CODES
        .get((c as u32).wrapping_sub(u32::from('0')) as usize)
        .copied()
        .unwrap_or(0)
}

/// The backend's picture (BGRA, premultiplied) as a Slint image.
pub fn image_of(f: &ViewFrame) -> Image {
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(f.width, f.height);
    for (d, s) in buf
        .make_mut_slice()
        .iter_mut()
        .zip(f.bgra.as_chunks::<4>().0)
    {
        *d = Rgba8Pixel {
            r: s[2],
            g: s[1],
            b: s[0],
            a: s[3],
        };
    }
    Image::from_rgba8_premultiplied(buf)
}

impl WebCtl {
    /// The person pressed a button over the page or typed into it: while an agent works in that tab this is the
    /// person taking the page over (the stop-on-input monitor). The agent's own input is DevTools events and never
    /// comes through here, as on Windows it moves neither the pointer nor a key.
    fn person_input(&mut self, tab: &str) {
        if !self.splitting
            && self.phase == Phase::Live
            && self.tabs.get(tab).is_some_and(|t| t.agent.is_some())
        {
            self.user_input();
        }
    }

    /// Shows the shown tab's latest picture in the page box (after the backend's turn and every sync), or takes the
    /// box's picture away when there is none.
    pub(super) fn show_live_frame(&mut self, app: &AppWindow) {
        let frame = self
            .shown_tab
            .as_deref()
            .and_then(|t| self.backend.as_ref()?.frame(&Slot::panel(t)));
        match frame {
            Some(f) => {
                if self.live.serial != Some(f.serial) {
                    self.live.serial = Some(f.serial);
                    app.set_panel_web_frame(image_of(&f));
                }
                if !self.live.shown {
                    self.live.shown = true;
                    app.set_panel_web_live(true);
                }
            }
            None => {
                if self.live.shown {
                    self.live.shown = false;
                    self.live.serial = None;
                    app.set_panel_web_live(false);
                    app.set_panel_web_frame(Image::default());
                }
            }
        }
    }

    /// The pointer over the page box (`kind` 0 down, 1 up, 2 move, 3 leave; logical pixels of the box).
    pub fn page_pointer(&mut self, kind: i32, x: f32, y: f32, button: i32, mods: i32) {
        let Some(tab) = self.shown_tab.clone() else {
            return;
        };
        let scale = self.scale;
        let (px, py) = (x * scale, y * scale);
        let button = match button {
            1 => PointerButton::Middle,
            2 => PointerButton::Right,
            _ => PointerButton::Left,
        };
        let act = match kind {
            0 => {
                let now = Instant::now();
                let clicks = match self.live.last_down {
                    Some((t, lx, ly, n))
                        if now.duration_since(t).as_millis() < DOUBLE_MS
                            && (lx - px).abs() < DOUBLE_PX * scale
                            && (ly - py).abs() < DOUBLE_PX * scale =>
                    {
                        n.saturating_add(1).min(3)
                    }
                    _ => 1,
                };
                self.live.last_down = Some((now, px, py, clicks));
                self.focus_page(&tab);
                self.person_input(&tab);
                PointerAct::Down(button, clicks)
            }
            1 => PointerAct::Up(button, self.live.last_down.map_or(1, |d| d.3)),
            2 => PointerAct::Move,
            _ => PointerAct::Leave,
        };
        if let Some(b) = self.backend.as_mut() {
            b.pointer(
                &Slot::panel(&tab),
                Pointer {
                    x: px,
                    y: py,
                    act,
                    mods: mods_of(mods),
                },
            );
        }
    }

    /// A wheel or trackpad scroll over the page box (Slint's deltas: positive `dy` toward the top).
    pub fn page_wheel(&mut self, x: f32, y: f32, dx: f32, dy: f32, mods: i32) {
        let Some(tab) = self.shown_tab.clone() else {
            return;
        };
        let scale = self.scale;
        if let Some(b) = self.backend.as_mut() {
            b.pointer(
                &Slot::panel(&tab),
                Pointer {
                    x: x * scale,
                    y: y * scale,
                    act: PointerAct::Wheel {
                        dx: -dx * scale,
                        dy: -dy * scale,
                    },
                    mods: mods_of(mods),
                },
            );
        }
    }

    /// A key while the page box has the keyboard.
    pub fn page_key(&mut self, down: bool, text: &str, mods: i32, repeat: bool) {
        let Some(tab) = self.shown_tab.clone() else {
            return;
        };
        let Some((vk, mac_code, typed)) = key_of(text) else {
            return;
        };
        let typing = match u8::try_from(vk) {
            Ok(0) | Err(_) => !typed.is_empty(),
            Ok(v) => is_typing_key(v),
        };
        let m = mods_of(mods);
        // Cmd or Ctrl with a key is a shortcut, not typing (as Alt or Win with one on Windows).
        if down && typing && !m.cmd && !m.ctrl {
            self.person_input(&tab);
        }
        if let Some(b) = self.backend.as_mut() {
            b.key(
                &Slot::panel(&tab),
                KeyInput {
                    down,
                    repeat,
                    vk,
                    mac_code,
                    text: typed,
                    mods: mods_of(mods),
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn slints_keys_become_the_keys_chromium_knows() {
        assert_eq!(key_of("a"), Some((0x41, 0x00, "a".into())));
        assert_eq!(key_of("T"), Some((0x54, 0x11, "T".into())));
        assert_eq!(key_of("7"), Some((0x37, 0x1A, "7".into())));
        assert_eq!(key_of("\u{8}"), Some((0x08, 0x33, String::new())));
        assert_eq!(key_of("\n"), Some((0x0D, 0x24, "\r".into())));
        assert_eq!(key_of("\u{F702}"), Some((0x25, 0x7B, String::new())));
        assert_eq!(key_of("\u{F704}"), Some((0x70, 0x7A, String::new())));
        assert_eq!(key_of("\u{F70F}"), Some((0x7B, 0x6F, String::new())));
        assert_eq!(key_of("é"), Some((0, 0, "é".into())));
        assert_eq!(key_of("你好"), Some((0, 0, "你好".into())));
        // A modifier alone and an unknown private-use key give the page nothing.
        assert_eq!(key_of("\u{10}"), None);
        assert_eq!(key_of("\u{17}"), None);
        assert_eq!(key_of("\u{F8A0}"), None);
        assert_eq!(key_of(""), None);
    }

    #[test]
    fn the_modifier_bits_are_the_panels() {
        let m = mods_of(1 | 8);
        assert!(m.shift && m.cmd && !m.ctrl && !m.alt);
        assert_eq!(mods_of(0), Mods::default());
    }

    #[test]
    fn a_picture_is_drawn_with_its_colours_in_place() {
        let f = ViewFrame {
            width: 2,
            height: 1,
            bgra: Arc::new(vec![0xcc, 0x66, 0x33, 0xff, 0, 0, 0x80, 0x80]),
            serial: 1,
        };
        let img = image_of(&f);
        assert_eq!(img.size().width, 2);
        assert_eq!(img.size().height, 1);
    }
}

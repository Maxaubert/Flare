//! The page as a picture (macOS, CEF, port phase 5): where WebView2 puts its own child window over the panel on
//! Windows, the macOS backend draws off screen, so the window shows the backend's latest picture in the panel's page
//! box (`panel-web-frame`, `panel-web-live`) and hands the pointer, the wheel and the keys over that box to the
//! backend ([`WebBackend::pointer`], [`WebBackend::key`]). On Windows the backend has no picture, `panel-web-live`
//! stays false and the box takes no input, so nothing here runs.

use std::collections::HashMap;
use std::time::Instant;

use flare_web::{
    ImeInput, KeyInput, Mods, Nav, Pointer, PointerAct, PointerButton, Slot, ViewFrame,
};
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

use super::{Phase, WebCtl, is_typing_key};
use crate::AppWindow;

/// Two presses closer than this (and a few pixels) are a double click.
const DOUBLE_MS: u128 = 500;
const DOUBLE_PX: f32 = 5.0;

/// Chromium's zoom steps (Ctrl+plus and Ctrl+minus in Edge and Chrome), 100 % among them.
pub const ZOOM_STEPS: [f64; 17] = [
    0.25, 0.33, 0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 4.0, 5.0,
];

/// A wheel turned this far (logical pixels) with Cmd held is one zoom step.
const WHEEL_ZOOM_PX: f32 = 50.0;

/// What the live picture last showed, and the last press (for the click count).
#[derive(Default)]
pub(super) struct Live {
    serial: Option<u64>,
    shown: bool,
    last_down: Option<(Instant, f32, f32, u8)>,
    /// The person's zoom of each tab's page (absent: 100 %).
    zoom: HashMap<String, f64>,
    /// Cmd+wheel turned over the page and not yet a whole step.
    wheel_zoom: f32,
    /// The status text and whose page it is.
    status: Option<(String, String)>,
    /// The find bar over the page: whose page, its address when it opened, the count and the active match.
    find: Option<PageFind>,
    /// What the window shows: the status text, and the find bar's open, readout and steps.
    pushed: (String, bool, String, bool),
}

#[derive(Clone, Debug, Default)]
struct PageFind {
    tab: String,
    url: String,
    query: String,
    /// The browser counted the matches of `query` (till then the readout is empty).
    counted: bool,
    /// The step that selects a new search's first match was sent.
    stepped: bool,
    count: u32,
    active: u32,
}

/// The modifier bits the panel sends: 1 shift, 2 ctrl, 4 alt, 8 cmd. On a Mac Slint calls Cmd `control` and Ctrl
/// `meta` (it follows Qt, rule 1110), so there bit 2 is Cmd and bit 8 is Ctrl.
pub fn mods_of(bits: i32) -> Mods {
    let (ctrl, cmd) = if cfg!(target_vendor = "apple") {
        (bits & 8 != 0, bits & 2 != 0)
    } else {
        (bits & 2 != 0, bits & 8 != 0)
    };
    Mods {
        shift: bits & 1 != 0,
        ctrl,
        alt: bits & 4 != 0,
        cmd,
    }
}

/// The panel's bit for Cmd (see [`mods_of`]).
#[cfg(test)]
pub const CMD_BIT: i32 = if cfg!(target_vendor = "apple") { 2 } else { 8 };

/// The zoom one step up (`step` 1) or down (-1) from `now`, along [`ZOOM_STEPS`]; 0 is 100 %.
pub fn zoom_step(now: f64, step: i32) -> f64 {
    match step.signum() {
        0 => 1.0,
        1 => ZOOM_STEPS
            .into_iter()
            .find(|z| *z > now + 0.001)
            .unwrap_or(ZOOM_STEPS[ZOOM_STEPS.len() - 1]),
        _ => ZOOM_STEPS
            .into_iter()
            .rev()
            .find(|z| *z < now - 0.001)
            .unwrap_or(ZOOM_STEPS[0]),
    }
}

/// The find bar's readout, as the terminal's: empty with no text, "none", or "3 of 12".
pub fn find_readout(query: &str, count: u32, active: u32) -> String {
    if query.is_empty() {
        String::new()
    } else if count == 0 {
        "none".to_string()
    } else {
        format!("{} of {count}", active.max(1))
    }
}

/// The UTF-16 offset of byte `at` in `text` (winit's preedit cursor is in bytes, CEF's in UTF-16 units).
pub fn utf16_at(text: &str, at: usize) -> u32 {
    let at = at.min(text.len());
    let mut end = at;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    u32::try_from(text[..end].encode_utf16().count()).unwrap_or(u32::MAX)
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
        c => match punct_key(c) {
            Some((vk, mac)) => named(vk, mac, &c.to_string()),
            None => named(0, 0, &c.to_string()),
        },
    }
}

/// The Win32 virtual key (`VK_OEM_*`) and the macOS key code (`kVK_ANSI_*`) of a punctuation key on the US layout,
/// shifted or not: the chord table reads Cmd+= and Cmd+[ by them, and the page's `keyCode` is the key's.
fn punct_key(c: char) -> Option<(u32, u16)> {
    Some(match c {
        ';' | ':' => (0xBA, 0x29),
        '=' | '+' => (0xBB, 0x18),
        ',' | '<' => (0xBC, 0x2B),
        '-' | '_' => (0xBD, 0x1B),
        '.' | '>' => (0xBE, 0x2F),
        '/' | '?' => (0xBF, 0x2C),
        '`' | '~' => (0xC0, 0x32),
        '[' | '{' => (0xDB, 0x21),
        '\\' | '|' => (0xDC, 0x2A),
        ']' | '}' => (0xDD, 0x1E),
        '\'' | '"' => (0xDE, 0x27),
        _ => return None,
    })
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
        self.push_page_bits(app);
    }

    /// The pointer over the page box (`kind` 0 down, 1 up, 2 move, 3 leave; logical pixels of the box).
    pub fn page_pointer(&mut self, kind: i32, x: f32, y: f32, button: i32, mods: i32) {
        let Some(tab) = self.shown_tab.clone() else {
            return;
        };
        let scale = self.scale;
        let (px, py) = (x * scale, y * scale);
        let button = match (button, kind) {
            // A move or a leave carries no button of its own.
            (_, 2 | 3) | (0, _) => PointerButton::Left,
            (1, _) => PointerButton::Middle,
            (2, _) => PointerButton::Right,
            // A mouse's back and forward buttons step through the history (as WebView2's XButtons do); any other
            // button is nothing the page knows, and never a left click.
            (3 | 4, _) => {
                if kind == 0 {
                    self.nav_by_button(&tab, button == 3);
                }
                return;
            }
            _ => return,
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
        // Cmd+wheel zooms the page, as Ctrl+wheel does in WebView2.
        if mods_of(mods).cmd {
            self.live.wheel_zoom += dy;
            let steps = (self.live.wheel_zoom / WHEEL_ZOOM_PX).trunc();
            if steps != 0.0 {
                self.live.wheel_zoom -= steps * WHEEL_ZOOM_PX;
                let z = (0..steps.abs() as i32).fold(self.page_zoom(&tab), |z, _| {
                    zoom_step(z, steps.signum() as i32)
                });
                self.set_page_zoom(&tab, z);
            }
            return;
        }
        self.live.wheel_zoom = 0.0;
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

impl WebCtl {
    /// A mouse's back (true) or forward button over the page.
    fn nav_by_button(&mut self, tab: &str, back: bool) {
        let can = self
            .tabs
            .get(tab)
            .is_some_and(|t| if back { t.can_back } else { t.can_forward });
        if can {
            self.nav(tab, if back { Nav::Back } else { Nav::Forward });
        }
    }

    /// The input method's text for the page while it has the keys (macOS): being composed, the caret at byte
    /// `cursor` of it (an empty one ends the composition), or chosen (`commit`).
    pub fn page_ime(&mut self, text: &str, cursor: Option<usize>, commit: bool) {
        let Some(tab) = self.shown_tab.clone() else {
            return;
        };
        let ime = if commit {
            ImeInput::Commit(text.to_string())
        } else if text.is_empty() {
            ImeInput::Cancel
        } else {
            ImeInput::Preedit {
                text: text.to_string(),
                cursor: utf16_at(text, cursor.unwrap_or(text.len())),
            }
        };
        if let Some(b) = self.backend.as_mut() {
            b.ime(&Slot::panel(&tab), ime);
        }
    }

    /// The person's zoom of the tab's page (1.0 is 100 %).
    pub fn page_zoom(&self, tab: &str) -> f64 {
        self.live.zoom.get(tab).copied().unwrap_or(1.0)
    }

    /// The zoom keys over the page (macOS): one step up (1), down (-1) or back to 100 % (0). False when the tab has
    /// no page in front.
    pub fn zoom_page(&mut self, tab: &str, step: i32) -> bool {
        if !self.live.shown
            || self.shown_tab.as_deref() != Some(tab)
            || !self.tabs.contains_key(tab)
        {
            return false;
        }
        let z = zoom_step(self.page_zoom(tab), step);
        self.set_page_zoom(tab, z);
        true
    }

    fn set_page_zoom(&mut self, tab: &str, z: f64) {
        if (z - 1.0).abs() < 0.001 {
            self.live.zoom.remove(tab);
        } else {
            self.live.zoom.insert(tab.to_string(), z);
        }
        self.set_zoom(tab, z);
    }

    /// Opens the find bar over the page in front (macOS Cmd+F while the page has the keys). False when no page
    /// is drawn here.
    pub fn open_page_find(&mut self, app: &AppWindow) -> bool {
        let Some(tab) = self.shown_tab.clone().filter(|_| self.live.shown) else {
            return false;
        };
        let url = self
            .tabs
            .get(&tab)
            .map(|t| t.url.clone())
            .unwrap_or_default();
        let query = app.get_panel_web_find_query().to_string();
        let again = self.live.find.as_ref().is_some_and(|f| f.tab == tab);
        if !again {
            self.live.find = Some(PageFind {
                tab: tab.clone(),
                url,
                query: query.clone(),
                ..PageFind::default()
            });
            if !query.is_empty()
                && let Some(b) = self.backend.as_mut()
            {
                b.find(&tab, &query, true, false);
            }
        }
        // The find field takes the keys: the page lets them go.
        let hwnd = self.hwnd;
        self.release_focus(hwnd);
        self.push_page_bits(app);
        app.invoke_focus_web_find();
        true
    }

    /// The find field's text changed, or Enter (next) or Shift+Enter (previous) was pressed.
    pub fn page_find(&mut self, app: &AppWindow, query: &str, forward: bool, again: bool) {
        let Some(f) = self.live.find.as_mut() else {
            return;
        };
        if !again || f.query != query {
            f.counted = false;
            f.stepped = false;
        }
        f.query = query.to_string();
        let tab = f.tab.clone();
        if query.is_empty() {
            f.count = 0;
            f.active = 0;
            if let Some(b) = self.backend.as_mut() {
                b.stop_find(&tab);
            }
        } else if let Some(b) = self.backend.as_mut() {
            b.find(&tab, query, forward, again);
        }
        self.push_page_bits(app);
    }

    /// Closes the find bar (its close button, Escape, or the page went elsewhere): the matches are unmarked, and
    /// `refocus` gives the page the keys back.
    pub fn close_page_find(&mut self, app: &AppWindow, refocus: bool) {
        let Some(f) = self.live.find.take() else {
            return;
        };
        if let Some(b) = self.backend.as_mut() {
            b.stop_find(&f.tab);
        }
        self.push_page_bits(app);
        if refocus && self.shown_tab.as_deref() == Some(f.tab.as_str()) && self.focus_page(&f.tab) {
            app.invoke_focus_web_page();
        }
    }

    /// Whether the find bar is open over the page (the tests read it).
    #[cfg(test)]
    pub fn page_find_open(&self) -> bool {
        self.live.find.is_some()
    }

    /// A find counted its matches.
    pub(super) fn page_found(&mut self, app: &AppWindow, tab: &str, count: u32, active: u32) {
        let Some(f) = self.live.find.as_mut().filter(|f| f.tab == tab) else {
            return;
        };
        f.count = count;
        f.active = active;
        // CEF counts a new search's matches and selects none of them: one step selects the first, as typing in
        // Chrome's find bar does (the readout waits for it).
        if !f.stepped && count > 0 && active == 0 {
            f.stepped = true;
            let (tab, query) = (f.tab.clone(), f.query.clone());
            if let Some(b) = self.backend.as_mut() {
                b.find(&tab, &query, true, true);
            }
            return;
        }
        f.counted = true;
        self.push_page_bits(app);
    }

    /// The page's status text changed.
    pub(super) fn page_status(&mut self, app: &AppWindow, tab: &str, text: String) {
        self.live.status = if text.is_empty() {
            None
        } else {
            Some((tab.to_string(), text))
        };
        self.push_page_bits(app);
    }

    /// The status text and the find bar as the window shows them, for the page in front only. A find whose page went
    /// elsewhere, or is no longer in front, is closed.
    fn push_page_bits(&mut self, app: &AppWindow) {
        let front = self.shown_tab.clone().filter(|_| self.live.shown);
        if let Some(f) = self.live.find.as_ref() {
            let url = self.tabs.get(&f.tab).map(|t| t.url.as_str());
            if front.as_deref() != Some(f.tab.as_str()) || url != Some(f.url.as_str()) {
                let tab = f.tab.clone();
                self.live.find = None;
                if let Some(b) = self.backend.as_mut() {
                    b.stop_find(&tab);
                }
                // The find field had the keys: the page takes them back if it is still in front.
                if app.get_find_input_focused()
                    && front.as_deref() == Some(tab.as_str())
                    && self.focus_page(&tab)
                {
                    app.invoke_focus_web_page();
                }
            }
        }
        let status = match (&self.live.status, &front) {
            (Some((t, text)), Some(f)) if t == f => text.clone(),
            _ => String::new(),
        };
        let find = self.live.find.as_ref();
        let now = (
            status,
            find.is_some(),
            find.filter(|f| f.counted)
                .map(|f| find_readout(&f.query, f.count, f.active))
                .unwrap_or_default(),
            find.is_some_and(|f| f.count > 0),
        );
        if now != self.live.pushed {
            app.set_panel_web_status(now.0.as_str().into());
            app.set_panel_web_find_open(now.1);
            app.set_panel_web_find_readout(now.2.as_str().into());
            app.set_panel_web_find_can_step(now.3);
            self.live.pushed = now;
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
        let m = mods_of(1 | CMD_BIT);
        assert!(m.shift && m.cmd && !m.ctrl && !m.alt);
        assert_eq!(mods_of(0), Mods::default());
        // Slint's `control` is Cmd on a Mac and Ctrl elsewhere (rule 1110).
        assert_eq!(mods_of(2).cmd, cfg!(target_vendor = "apple"));
        assert_eq!(mods_of(8).ctrl, cfg!(target_vendor = "apple"));
        assert!(mods_of(4).alt);
    }

    #[test]
    fn punctuation_keys_carry_their_virtual_key_and_mac_key_code() {
        assert_eq!(key_of("="), Some((0xBB, 0x18, "=".into())));
        assert_eq!(key_of("+"), Some((0xBB, 0x18, "+".into())));
        assert_eq!(key_of("-"), Some((0xBD, 0x1B, "-".into())));
        assert_eq!(key_of("["), Some((0xDB, 0x21, "[".into())));
        assert_eq!(key_of("}"), Some((0xDD, 0x1E, "}".into())));
        assert_eq!(key_of(","), Some((0xBC, 0x2B, ",".into())));
        assert_eq!(key_of("\\"), Some((0xDC, 0x2A, "\\".into())));
    }

    #[test]
    fn the_zoom_steps_are_chromiums_and_stop_at_the_ends() {
        assert_eq!(zoom_step(1.0, 1), 1.1);
        assert_eq!(zoom_step(1.0, -1), 0.9);
        assert_eq!(zoom_step(1.5, 1), 1.75);
        assert_eq!(
            zoom_step(1.3, -1),
            1.25,
            "an odd zoom goes to the step below"
        );
        assert_eq!(zoom_step(5.0, 1), 5.0);
        assert_eq!(zoom_step(0.25, -1), 0.25);
        assert_eq!(zoom_step(2.5, 0), 1.0);
    }

    #[test]
    fn the_find_readout_is_the_terminals() {
        assert_eq!(find_readout("", 4, 1), "");
        assert_eq!(find_readout("x", 0, 0), "none");
        assert_eq!(find_readout("x", 12, 3), "3 of 12");
        assert_eq!(find_readout("x", 2, 0), "1 of 2");
    }

    #[test]
    fn a_preedit_caret_in_bytes_is_counted_in_utf16_units() {
        assert_eq!(utf16_at("ni", 2), 2);
        assert_eq!(utf16_at("你好", 3), 1);
        assert_eq!(utf16_at("你好", 4), 1, "inside a character: its start");
        assert_eq!(utf16_at("a😀b", 5), 3);
        assert_eq!(utf16_at("ab", 9), 2);
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

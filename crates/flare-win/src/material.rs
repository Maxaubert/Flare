//! acrylic_ok, system_backdrop_ok and the material state machine (Task 17).
//!
//! Port of Prism Terminal's `material.ts`. The order of calls is regression rule 73 in its native
//! meaning (decision D7): the backdrop is changed only on a change, and a window that cannot
//! have one (an old build, a refused call) makes no call at all and stays solid (rule 74).

use crate::os_build::{OsVersion, parse_release};
use flare_theme::ground::DEFAULT_GROUND;

/// Windows 11: the build where the acrylic row exists (Prism Terminal's rule).
pub const WIN11_BUILD: u32 = 22000;
/// Windows 11 22H2: the build where `DWMWA_SYSTEMBACKDROP_TYPE` exists (decision D6).
pub const SYSTEM_BACKDROP_BUILD: u32 = 22621;

/// Prism Terminal's `acrylicOk`: Windows, major >= 10, build >= 22000; an unreadable release is false.
pub fn acrylic_ok(release: &str, is_windows: bool) -> bool {
    match parse_release(release) {
        Some(v) => is_windows && v.major >= 10 && v.build >= WIN11_BUILD,
        None => false,
    }
}

pub fn acrylic_ok_for(v: OsVersion) -> bool {
    v.build >= WIN11_BUILD
}

/// Whether the DWM backdrop call exists on this build.
pub fn system_backdrop_ok(v: OsVersion) -> bool {
    v.build >= SYSTEM_BACKDROP_BUILD
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Support {
    Ok,
    NeedsWindows11,
    NeedsWindows11_22H2,
}

pub fn support(v: OsVersion) -> Support {
    if v.build < WIN11_BUILD {
        Support::NeedsWindows11
    } else if v.build < SYSTEM_BACKDROP_BUILD {
        Support::NeedsWindows11_22H2
    } else {
        Support::Ok
    }
}

pub fn support_text(s: Support) -> Option<&'static str> {
    match s {
        Support::Ok => None,
        Support::NeedsWindows11 => Some("Needs Windows 11."),
        Support::NeedsWindows11_22H2 => Some("Needs Windows 11 22H2."),
    }
}

/// `^#[0-9a-f]{6}$`, either case, nothing else.
pub fn valid_bg(hex: &str) -> bool {
    let b = hex.as_bytes();
    b.len() == 7 && b[0] == b'#' && b[1..].iter().all(u8::is_ascii_hexdigit)
}

/// The frost DWM picks: Rec.709 luma over 0.5. Deliberately not the 0.4 WCAG measure.
pub fn is_light_bg(hex: &str) -> bool {
    let n = u32::from_str_radix(hex.trim_start_matches('#'), 16).unwrap_or(0);
    let (r, g, b) = ((n >> 16) & 255, (n >> 8) & 255, n & 255);
    (0.2126 * f64::from(r) + 0.7152 * f64::from(g) + 0.0722 * f64::from(b)) / 255.0 > 0.5
}

pub trait Surface {
    /// False means the OS refused; the window stays solid.
    fn set_backdrop(&mut self, on: bool) -> bool;
    /// The frost's light or dark.
    fn set_dark(&mut self, dark: bool);
    fn is_fullscreen(&self) -> bool;
}

pub struct Material<S: Surface> {
    surface: S,
    supported: bool,
    wanted: bool,
    solid: String,
    fs_pending: bool,
    worn: Option<bool>,
    refused: Option<bool>,
    glass: bool,
}

impl<S: Surface> Material<S> {
    pub fn new(surface: S, supported: bool) -> Self {
        Self {
            surface,
            supported,
            wanted: false,
            solid: DEFAULT_GROUND.to_string(),
            fs_pending: false,
            worn: None,
            refused: None,
            glass: false,
        }
    }

    /// The last valid ground.
    pub fn solid(&self) -> &str {
        &self.solid
    }

    /// What `apply` last decided.
    pub fn glass_on(&self) -> bool {
        self.glass
    }

    /// Returns `supported` (false means the row is disabled); the wish is kept anyway.
    pub fn set_acrylic(&mut self, on: bool) -> bool {
        if self.wanted != on {
            self.refused = None;
        }
        self.wanted = on;
        self.apply();
        self.supported
    }

    /// Ignores anything but `#rrggbb`.
    pub fn set_bg(&mut self, hex: &str) {
        if !valid_bg(hex) {
            return;
        }
        self.solid = hex.to_string();
        if self.supported {
            self.surface.set_dark(!is_light_bg(hex));
        }
        self.apply();
    }

    pub fn before_fullscreen(&mut self) {
        self.fs_pending = true;
        self.apply();
    }

    pub fn settled(&mut self) {
        self.fs_pending = false;
        self.apply();
    }

    pub fn surface(&mut self) -> &mut S {
        &mut self.surface
    }

    fn apply(&mut self) {
        let mut on =
            self.wanted && self.supported && !self.surface.is_fullscreen() && !self.fs_pending;
        if self.supported && self.refused != Some(on) && self.worn != Some(on) {
            if self.surface.set_backdrop(on) {
                self.worn = Some(on);
            } else {
                log::warn!("the window backdrop call was refused; the window stays solid");
                self.worn = Some(false);
                self.refused = Some(on);
                on = false;
            }
        } else if self.refused == Some(on) {
            on = false;
        }
        self.glass = on;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Call {
        Dark(bool),
        Backdrop(bool),
    }

    struct Rec {
        log: Rc<RefCell<Vec<Call>>>,
        full: Rc<RefCell<bool>>,
        refuse_on: bool,
    }

    type Log = Rc<RefCell<Vec<Call>>>;

    fn rec() -> (Rec, Log, Rc<RefCell<bool>>) {
        let log: Log = Rc::new(RefCell::new(Vec::new()));
        let full = Rc::new(RefCell::new(false));
        (
            Rec {
                log: log.clone(),
                full: full.clone(),
                refuse_on: false,
            },
            log,
            full,
        )
    }

    impl Surface for Rec {
        fn set_backdrop(&mut self, on: bool) -> bool {
            self.log.borrow_mut().push(Call::Backdrop(on));
            !(on && self.refuse_on)
        }
        fn set_dark(&mut self, dark: bool) {
            self.log.borrow_mut().push(Call::Dark(dark));
        }
        fn is_fullscreen(&self) -> bool {
            *self.full.borrow()
        }
    }

    fn v(build: u32) -> OsVersion {
        OsVersion {
            major: 10,
            minor: 0,
            build,
        }
    }

    #[test]
    fn acrylic_ok_is_windows_11_and_later_by_build() {
        for r in ["10.0.22631", "10.0.22000", "10.0.26200"] {
            assert!(acrylic_ok(r, true), "{r}");
        }
    }

    #[test]
    fn acrylic_ok_is_not_windows_10() {
        for r in ["10.0.19045", "10.0.17763"] {
            assert!(!acrylic_ok(r, true), "{r}");
        }
    }

    #[test]
    fn acrylic_ok_is_not_another_platform_or_an_unreadable_release() {
        assert!(!acrylic_ok("24.1.0", false));
        assert!(!acrylic_ok("", true));
    }

    #[test]
    fn the_window_ground_takes_rrggbb_and_nothing_else() {
        assert!(valid_bg("#0b0b0f"));
        assert!(valid_bg("#FFFFFF"));
        for s in ["#fff", "#0b0b0f80", "red", ""] {
            assert!(!valid_bg(s), "{s}");
        }
    }

    #[test]
    fn a_pale_ground_is_light_for_the_frost() {
        assert!(is_light_bg("#fdf6e3"));
        assert!(!is_light_bg("#0b0b0f"));
        assert!(is_light_bg("#aaaaaa"));
        assert!(!is_light_bg("#777777"));
    }

    #[test]
    fn two_gates_and_their_words() {
        assert_eq!(support(v(19045)), Support::NeedsWindows11);
        assert_eq!(support_text(support(v(19045))), Some("Needs Windows 11."));
        for b in [22000, 22620] {
            assert_eq!(support(v(b)), Support::NeedsWindows11_22H2);
            assert_eq!(support_text(support(v(b))), Some("Needs Windows 11 22H2."));
        }
        assert_eq!(support(v(22621)), Support::Ok);
        assert_eq!(support_text(Support::Ok), None);
        assert!(!system_backdrop_ok(v(22620)));
        assert!(system_backdrop_ok(v(22621)));
        assert!(acrylic_ok_for(v(22000)));
        assert!(!acrylic_ok_for(v(21999)));
    }

    #[test]
    fn going_glass_sets_the_frost_then_the_backdrop() {
        let (s, log, _) = rec();
        let mut m = Material::new(s, true);
        m.set_bg("#171717");
        m.set_acrylic(true);
        assert_eq!(
            *log.borrow(),
            vec![
                Call::Dark(true),
                Call::Backdrop(false),
                Call::Backdrop(true)
            ]
        );
        assert!(m.glass_on());
    }

    #[test]
    fn leaving_glass_removes_the_backdrop() {
        let (s, log, _) = rec();
        let mut m = Material::new(s, true);
        m.set_bg("#171717");
        m.set_acrylic(true);
        m.set_acrylic(false);
        assert_eq!(log.borrow().last(), Some(&Call::Backdrop(false)));
        assert!(!m.glass_on());
    }

    #[test]
    fn the_backdrop_is_set_only_on_a_change() {
        let (s, log, _) = rec();
        let mut m = Material::new(s, true);
        m.set_acrylic(true);
        m.set_bg("#171717");
        m.set_bg("#202020");
        m.set_bg("#fdf6e3");
        let n = log
            .borrow()
            .iter()
            .filter(|c| **c == Call::Backdrop(true))
            .count();
        assert_eq!(n, 1);
        assert_eq!(log.borrow().last(), Some(&Call::Dark(false)));
    }

    #[test]
    fn glass_is_on_only_when_wanted_supported_not_fullscreen_and_no_change_pending() {
        for bits in 0..16u8 {
            let (wanted, supported, full, pending) =
                (bits & 1 != 0, bits & 2 != 0, bits & 4 != 0, bits & 8 != 0);
            let (s, _, f) = rec();
            *f.borrow_mut() = full;
            let mut m = Material::new(s, supported);
            m.set_acrylic(wanted);
            if pending {
                m.before_fullscreen();
            }
            let expect = wanted && supported && !full && !pending;
            assert_eq!(m.glass_on(), expect, "bits {bits:04b}");
        }
    }

    #[test]
    fn full_screen_goes_opaque_before_the_os_is_asked_and_back_on_settle() {
        let (s, log, _) = rec();
        let mut m = Material::new(s, true);
        m.set_acrylic(true);
        m.before_fullscreen();
        assert_eq!(log.borrow().last(), Some(&Call::Backdrop(false)));
        assert!(!m.glass_on());
        m.settled();
        assert_eq!(log.borrow().last(), Some(&Call::Backdrop(true)));
        assert!(m.glass_on());
    }

    #[test]
    fn an_unsupported_build_makes_no_surface_call_at_all() {
        let (s, log, _) = rec();
        let mut m = Material::new(s, false);
        m.set_bg("#171717");
        assert!(!m.set_acrylic(true));
        m.before_fullscreen();
        m.settled();
        assert!(log.borrow().is_empty());
    }

    #[test]
    fn a_failed_backdrop_call_leaves_the_window_solid() {
        let (mut s, log, _) = rec();
        s.refuse_on = true;
        let mut m = Material::new(s, true);
        m.set_acrylic(true);
        assert!(!m.glass_on());
        m.set_bg("#171717");
        let on = |l: &Vec<Call>| l.iter().filter(|c| **c == Call::Backdrop(true)).count();
        assert_eq!(on(&log.borrow()), 1);
        m.set_acrylic(false);
        m.set_acrylic(true);
        assert_eq!(on(&log.borrow()), 2);
        assert!(!m.glass_on());
    }

    #[test]
    fn set_bg_ignores_anything_but_rrggbb() {
        let (s, _, _) = rec();
        let mut m = Material::new(s, true);
        m.set_bg("#171717");
        for bad in ["javascript:1", "#fff", "#aabbcc80"] {
            m.set_bg(bad);
            assert_eq!(m.solid(), "#171717");
        }
    }
}

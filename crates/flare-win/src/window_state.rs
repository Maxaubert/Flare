//! WindowState, parse_window_state, NormalBounds, WindowStateStore (Task 14).
//!
//! The window's size, position and maximised flag, kept in `window.json`. Width and height are logical
//! pixels; x and y are device pixels (physical, the only unambiguous thing across monitors of different
//! scales). A remembered position is used only when the window would overlap a monitor that is there now.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use flare_settings::debounce::Debounced;
use serde_json::Value;

/// The smallest the window may be, and the floor a remembered size is held to.
pub const MIN_WINDOW: (i32, i32) = (560, 400);
/// The size of a window with no usable state.
pub const DEFAULT_WINDOW: (i32, i32) = (1180, 780);

/// A monitor's work area, in device pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Area {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WindowState {
    pub width: i32,
    pub height: i32,
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub maximised: bool,
    pub scale: f64,
}

/// The state a window opens with when there is nothing usable on disk.
fn fallback() -> WindowState {
    WindowState {
        width: DEFAULT_WINDOW.0,
        height: DEFAULT_WINDOW.1,
        x: None,
        y: None,
        maximised: false,
        scale: 1.0,
    }
}

/// Safe to open a window with. Width and height (logical) must be finite numbers, rounded and at least
/// 560 x 400. A broken or foreign file gives 1180 x 780 with no position. x and y (device pixels) are
/// kept only when both are numbers AND the window (logical size times scale, in device pixels) overlaps
/// some work area. `scale` is a number above 0, clamped to 0.5..=4.0, else 1.0.
pub fn parse_window_state(raw: &str, areas: &[Area]) -> WindowState {
    let raw = raw.trim_start_matches('\u{feff}');
    let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(raw) else {
        return fallback();
    };
    let num = |key: &str| {
        obj.get(key)
            .and_then(Value::as_f64)
            .filter(|v| v.is_finite())
    };
    let (Some(w), Some(h)) = (num("width"), num("height")) else {
        return fallback();
    };
    let scale = match num("scale") {
        Some(s) if s > 0.0 => s.clamp(0.5, 4.0),
        _ => 1.0,
    };
    // No larger than the virtual screen (the box round all monitors), in device pixels, so a corrupt
    // or hand-edited file cannot ask for a window nothing can show (audit L16). Never below the floor.
    let (max_w, max_h) = max_logical_size(areas, scale);
    let width = (w.min(f64::from(max_w)).round() as i32).max(MIN_WINDOW.0);
    let height = (h.min(f64::from(max_h)).round() as i32).max(MIN_WINDOW.1);
    let maximised = obj.get("maximised").and_then(Value::as_bool) == Some(true);
    let (x, y) = match (num("x"), num("y")) {
        (Some(x), Some(y)) => {
            let (x, y) = (i64::from(x.round() as i32), i64::from(y.round() as i32));
            let dw = (f64::from(width) * scale).round() as i64;
            let dh = (f64::from(height) * scale).round() as i64;
            let visible = areas.iter().any(|a| {
                let (ax, ay) = (i64::from(a.x), i64::from(a.y));
                let (aw, ah) = (i64::from(a.width), i64::from(a.height));
                x + dw > ax && x < ax + aw && y + dh > ay && y < ay + ah
            });
            if visible {
                (Some(x as i32), Some(y as i32))
            } else {
                (None, None)
            }
        }
        _ => (None, None),
    };
    WindowState {
        width,
        height,
        x,
        y,
        maximised,
        scale,
    }
}

/// The cap when Windows reported no monitor at all, in device pixels.
const NO_MONITOR_CAP: i64 = 16_384;

/// The largest window, in logical pixels, that fits the virtual screen at `scale`.
fn max_logical_size(areas: &[Area], scale: f64) -> (i32, i32) {
    let (vw, vh) = if areas.is_empty() {
        (NO_MONITOR_CAP, NO_MONITOR_CAP)
    } else {
        let left = areas.iter().map(|a| i64::from(a.x)).min().unwrap_or(0);
        let top = areas.iter().map(|a| i64::from(a.y)).min().unwrap_or(0);
        let right = areas
            .iter()
            .map(|a| i64::from(a.x) + i64::from(a.width))
            .max()
            .unwrap_or(0);
        let bottom = areas
            .iter()
            .map(|a| i64::from(a.y) + i64::from(a.height))
            .max()
            .unwrap_or(0);
        // A quarter over: a window saved a little past a work area (the taskbar is not in it) keeps
        // its size; only the absurd is cut.
        (
            ((right - left) * 5 / 4).max(1),
            ((bottom - top) * 5 / 4).max(1),
        )
    };
    let to_logical = |device: i64| {
        (device as f64 / scale)
            .floor()
            .clamp(1.0, f64::from(i32::MAX)) as i32
    };
    (to_logical(vw), to_logical(vh))
}

/// `window.json` text, x and y omitted when None.
pub fn window_json(s: &WindowState) -> String {
    let mut out = format!("{{\"width\":{},\"height\":{}", s.width, s.height);
    if let Some(x) = s.x {
        let _ = write!(out, ",\"x\":{x}");
    }
    if let Some(y) = s.y {
        let _ = write!(out, ",\"y\":{y}");
    }
    let _ = write!(
        out,
        ",\"maximised\":{},\"scale\":{:?}}}",
        s.maximised, s.scale
    );
    out
}

/// What the window reports about itself: size in device pixels, position, scale and the window modes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Observed {
    pub width_phys: i32,
    pub height_phys: i32,
    pub x: i32,
    pub y: i32,
    pub scale: f64,
    pub maximised: bool,
    pub minimised: bool,
    pub fullscreen: bool,
}

/// Remembers the last NORMAL bounds: maximised, minimised and full-screen observations are ignored, so
/// a maximised size is never what comes back on restore.
#[derive(Clone, Debug, Default)]
pub struct NormalBounds {
    last: Option<WindowState>,
}

impl NormalBounds {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn observe(&mut self, o: &Observed) {
        if o.maximised || o.minimised || o.fullscreen {
            return;
        }
        let scale = if o.scale.is_finite() && o.scale > 0.0 {
            o.scale
        } else {
            1.0
        };
        self.last = Some(WindowState {
            width: (f64::from(o.width_phys) / scale).round() as i32,
            height: (f64::from(o.height_phys) / scale).round() as i32,
            x: Some(o.x),
            y: Some(o.y),
            maximised: false,
            scale,
        });
    }

    /// The last normal bounds, with the maximised flag the caller gives (None before any normal sighting).
    pub fn state(&self, maximised: bool) -> Option<WindowState> {
        self.last.as_ref().map(|s| WindowState {
            maximised,
            ..s.clone()
        })
    }
}

/// The window state file, saved on a delay and flushed at the close.
pub struct WindowStateStore {
    debounced: Debounced,
}

impl WindowStateStore {
    pub fn new(file: impl Into<PathBuf>, delay: Duration) -> Self {
        Self {
            debounced: Debounced::new(file, delay),
        }
    }

    pub fn save(&self, s: &WindowState) {
        self.debounced.save(window_json(s));
    }

    pub fn flush(&self) {
        self.debounced.flush();
    }

    /// The startup read: a missing or broken file gives the fallback.
    pub fn read(file: &Path, areas: &[Area]) -> WindowState {
        let raw = std::fs::read_to_string(file).unwrap_or_default(); // sync-ok: the startup read
        parse_window_state(&raw, areas)
    }
    /// The startup read where the work areas depend on the saved scale (`monitors::work_areas_at`:
    /// macOS gives positions as points times the window's scale). Gives the state and the areas it
    /// was checked against.
    pub fn read_at(file: &Path, areas_at: impl Fn(f64) -> Vec<Area>) -> (WindowState, Vec<Area>) {
        let raw = std::fs::read_to_string(file).unwrap_or_default(); // sync-ok: the startup read
        parse_window_state_at(&raw, areas_at)
    }
}

/// `parse_window_state` against the areas of the saved scale (the scale never depends on the areas).
pub fn parse_window_state_at(
    raw: &str,
    areas_at: impl Fn(f64) -> Vec<Area>,
) -> (WindowState, Vec<Area>) {
    let areas = areas_at(parse_window_state(raw, &[]).scale);
    (parse_window_state(raw, &areas), areas)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE: [Area; 1] = [Area {
        x: 0,
        y: 0,
        width: 1920,
        height: 1040,
    }];

    fn fallback_state() -> WindowState {
        WindowState {
            width: 1180,
            height: 780,
            x: None,
            y: None,
            maximised: false,
            scale: 1.0,
        }
    }

    fn observed(w: i32, h: i32, x: i32, y: i32, scale: f64, maximised: bool) -> Observed {
        Observed {
            width_phys: w,
            height_phys: h,
            x,
            y,
            scale,
            maximised,
            minimised: false,
            fullscreen: false,
        }
    }

    #[test]
    fn the_areas_are_asked_for_in_the_saved_scale() {
        // Two screens in points: a 2x one at 0..1512 and a 1x one at 1512..3432 (macOS).
        let at = |s: f64| {
            vec![
                Area {
                    x: 0,
                    y: 0,
                    width: (1512.0 * s) as i32,
                    height: (982.0 * s) as i32,
                },
                Area {
                    x: (1512.0 * s) as i32,
                    y: 0,
                    width: (1920.0 * s) as i32,
                    height: (1080.0 * s) as i32,
                },
            ]
        };
        // Saved at 2x, 2000 pt in: on the second screen, 4000 in that space.
        let raw = r#"{"width":800,"height":600,"x":4000,"y":100,"scale":2}"#;
        let (s, areas) = parse_window_state_at(raw, at);
        assert_eq!(areas, at(2.0));
        assert_eq!((s.x, s.y), (Some(4000), Some(100)));
        // The same file checked against 1x areas would lose the position.
        assert_eq!(parse_window_state(raw, &at(1.0)).x, None);
        let (s, areas) = parse_window_state_at("", at);
        assert_eq!((s.x, areas), (None, at(1.0)));
    }

    #[test]
    fn window_state_falls_back_for_no_broken_or_foreign_file() {
        for raw in ["", "not json", "null", r#"{"width":"wide","height":700}"#] {
            assert_eq!(
                parse_window_state(raw, &ONE),
                fallback_state(),
                "raw: {raw}"
            );
        }
    }

    #[test]
    fn window_state_brings_back_size_position_and_maximised() {
        let raw = r#"{"width":1400.4,"height":900,"x":100,"y":50.6,"maximised":true}"#;
        assert_eq!(
            parse_window_state(raw, &ONE),
            WindowState {
                width: 1400,
                height: 900,
                x: Some(100),
                y: Some(51),
                maximised: true,
                scale: 1.0,
            }
        );
    }

    #[test]
    fn window_state_never_opens_below_560_by_400() {
        let s = parse_window_state(r#"{"width":10,"height":10}"#, &ONE);
        assert_eq!((s.width, s.height), (560, 400));
    }

    #[test]
    fn window_state_drops_the_position_off_every_monitor() {
        let raw = r#"{"width":1200,"height":800,"x":2500,"y":100}"#;
        assert_eq!(
            parse_window_state(raw, &ONE),
            WindowState {
                width: 1200,
                height: 800,
                x: None,
                y: None,
                maximised: false,
                scale: 1.0,
            }
        );
    }

    #[test]
    fn window_state_keeps_a_negative_position_on_a_second_monitor() {
        let areas = [
            Area {
                x: -1920,
                y: 0,
                width: 1920,
                height: 1040,
            },
            ONE[0],
        ];
        let raw = r#"{"width":1200,"height":800,"x":-1800,"y":100}"#;
        let s = parse_window_state(raw, &areas);
        assert_eq!((s.x, s.y), (Some(-1800), Some(100)));
    }

    #[test]
    fn the_overlap_check_uses_device_pixels() {
        let raw = r#"{"width":1000,"height":700,"x":-1000,"y":0}"#;
        let s = parse_window_state(raw, &ONE);
        assert_eq!(s.x, None);
        let raw = r#"{"width":1000,"height":700,"x":-1000,"y":0,"scale":2.0}"#;
        let s = parse_window_state(raw, &ONE);
        assert_eq!(s.x, Some(-1000));
    }

    #[test]
    fn scale_is_clamped_and_junk_reads_one() {
        let base = |extra: &str| format!("{{\"width\":1200,\"height\":800{extra}}}");
        assert_eq!(parse_window_state(&base(""), &ONE).scale, 1.0);
        assert_eq!(
            parse_window_state(&base(",\"scale\":\"big\""), &ONE).scale,
            1.0
        );
        assert_eq!(parse_window_state(&base(",\"scale\":0"), &ONE).scale, 1.0);
        assert_eq!(parse_window_state(&base(",\"scale\":-2"), &ONE).scale, 1.0);
        assert_eq!(parse_window_state(&base(",\"scale\":0.1"), &ONE).scale, 0.5);
        assert_eq!(parse_window_state(&base(",\"scale\":9"), &ONE).scale, 4.0);
        assert_eq!(parse_window_state(&base(",\"scale\":1.5"), &ONE).scale, 1.5);
    }

    #[test]
    fn window_json_round_trips_and_omits_a_missing_position() {
        let s = WindowState {
            width: 1180,
            height: 780,
            x: Some(10),
            y: Some(20),
            maximised: false,
            scale: 1.5,
        };
        let text = window_json(&s);
        assert_eq!(
            text,
            r#"{"width":1180,"height":780,"x":10,"y":20,"maximised":false,"scale":1.5}"#
        );
        assert_eq!(parse_window_state(&text, &ONE), s);
        let no_pos = WindowState {
            x: None,
            y: None,
            ..s
        };
        let text = window_json(&no_pos);
        assert!(!text.contains("\"x\""));
        assert!(!text.contains("\"y\""));
    }

    #[test]
    fn a_maximised_size_is_never_remembered() {
        let mut nb = NormalBounds::new();
        nb.observe(&observed(1500, 900, 100, 50, 1.5, false));
        nb.observe(&observed(3000, 1700, 0, 0, 1.5, true));
        assert_eq!(
            nb.state(true),
            Some(WindowState {
                width: 1000,
                height: 600,
                x: Some(100),
                y: Some(50),
                maximised: true,
                scale: 1.5,
            })
        );
    }

    #[test]
    fn a_minimised_window_is_not_saved() {
        let mut nb = NormalBounds::new();
        nb.observe(&observed(1500, 900, 100, 50, 1.5, false));
        let minimised = Observed {
            minimised: true,
            ..observed(0, 0, -32000, -32000, 1.5, false)
        };
        nb.observe(&minimised);
        let s = nb.state(false).unwrap();
        assert_eq!((s.x, s.y), (Some(100), Some(50)));
    }

    #[test]
    fn physical_size_becomes_logical_by_the_scale() {
        let mut nb = NormalBounds::new();
        assert_eq!(nb.state(false), None);
        nb.observe(&observed(2250, 1350, 0, 0, 2.25, false));
        let s = nb.state(false).unwrap();
        assert_eq!((s.width, s.height), (1000, 600));
    }

    #[test]
    fn the_store_writes_and_flushes_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("window.json");
        let store = WindowStateStore::new(&file, Duration::from_secs(60));
        let saved = WindowState {
            width: 1200,
            height: 800,
            x: Some(100),
            y: Some(100),
            maximised: false,
            scale: 1.0,
        };
        store.save(&saved);
        store.flush();
        assert_eq!(WindowStateStore::read(&file, &ONE), saved);
    }

    /// L16 (audit 2026-10-20): a hand-edited or corrupt `window.json` must not ask for a window
    /// larger than all the monitors together, at any scaling.
    #[test]
    fn window_state_clamps_the_size_to_the_virtual_screen_at_every_scale() {
        for scale in [1.0, 1.25, 1.5, 2.0, 2.25, 3.0] {
            let raw = format!(r#"{{"width":100000,"height":99999,"scale":{scale}}}"#);
            let s = parse_window_state(&raw, &ONE);
            let (dw, dh) = (f64::from(s.width) * scale, f64::from(s.height) * scale);
            assert!(
                dw <= 2400.5 && dh <= 1300.5,
                "scale {scale}: {dw} x {dh} device px"
            );
            assert!(
                s.width >= MIN_WINDOW.0 && s.height >= MIN_WINDOW.1,
                "never below the floor"
            );
        }
    }

    #[test]
    fn window_state_lets_a_window_span_two_monitors_but_no_more() {
        let areas = [
            Area {
                x: -1920,
                y: 0,
                width: 1920,
                height: 1040,
            },
            ONE[0],
        ];
        let s = parse_window_state(r#"{"width":3500,"height":900}"#, &areas);
        assert_eq!(
            (s.width, s.height),
            (3500, 900),
            "inside the virtual screen: kept"
        );
        let s = parse_window_state(r#"{"width":9000,"height":900}"#, &areas);
        assert_eq!(s.width, 4800, "a quarter over the virtual screen's width");
    }

    #[test]
    fn window_state_with_no_monitor_info_still_caps_the_size() {
        let s = parse_window_state(r#"{"width":1e9,"height":1e9}"#, &[]);
        assert!(s.width <= 16384 && s.height <= 16384, "{s:?}");
    }
}

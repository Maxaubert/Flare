//! Coordinates: the panel's logical points to device points (the picture is letterboxed to keep its aspect), device
//! points to the portrait ratios the digitizer takes, and a swipe's path.
//!
//! Panel points are Slint logical points and device points are the device's, so neither the Mac's backing scale nor
//! the device's scale enters the map. Device points are always in the current orientation (rule 1313): the size of a
//! landscape screen is the portrait one turned. On Android a device point is a dp (pixels / (dpi / 160), rule 1367):
//! the same letterbox and swipe code serves both, and only the emu host turns dp into pixels ([`dp_to_px`]).

use serde::{Deserialize, Serialize};

/// How far apart the positions of a swipe are, in device points.
pub const SWIPE_STEP_PT: f64 = 8.0;

/// A size (points or pixels).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Size {
    pub w: f64,
    pub h: f64,
}

impl Size {
    pub fn new(w: f64, h: f64) -> Size {
        Size { w, h }
    }

    /// Turned a quarter: width and height swap.
    pub fn turned(self) -> Size {
        Size {
            w: self.h,
            h: self.w,
        }
    }

    pub fn is_empty(self) -> bool {
        !(self.w > 0.0 && self.h > 0.0)
    }
}

/// How the interface is rotated inside the portrait framebuffer, as CoreDevice `displayinfo` reads it
/// (`currentOrientation`, spike S5). The framebuffer never turns: Flare turns the picture. `Rot90` shows the
/// portrait picture turned a quarter clockwise (EXIF 6), `Rot270` a quarter anticlockwise (EXIF 8).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Orientation {
    #[default]
    Rot0,
    Rot90,
    Rot180,
    Rot270,
}

impl Orientation {
    pub const ALL: [Orientation; 4] = [
        Orientation::Rot0,
        Orientation::Rot90,
        Orientation::Rot180,
        Orientation::Rot270,
    ];

    /// The name `displayinfo` gives (`rot0` ...), or None for one this build does not know.
    pub fn parse(s: &str) -> Option<Orientation> {
        Some(match s {
            "rot0" => Orientation::Rot0,
            "rot90" => Orientation::Rot90,
            "rot180" => Orientation::Rot180,
            "rot270" => Orientation::Rot270,
            _ => return None,
        })
    }

    /// The screen's size in this orientation, from its portrait size.
    pub fn size(self, portrait: Size) -> Size {
        match self {
            Orientation::Rot0 | Orientation::Rot180 => portrait,
            Orientation::Rot90 | Orientation::Rot270 => portrait.turned(),
        }
    }
}

/// Where the device's picture sits in the panel's body: as large as fits, centred, its aspect kept.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Letterbox {
    /// The picture's top left corner in panel points.
    pub x: f64,
    pub y: f64,
    /// Panel points per device point.
    pub scale: f64,
    /// The device's screen in points (current orientation).
    pub device: Size,
}

impl Letterbox {
    /// The picture of a `device` sized screen in a `panel` sized body. None while either is empty.
    pub fn fit(panel: Size, device: Size) -> Option<Letterbox> {
        if panel.is_empty() || device.is_empty() {
            return None;
        }
        let scale = (panel.w / device.w).min(panel.h / device.h);
        Some(Letterbox {
            x: (panel.w - device.w * scale) / 2.0,
            y: (panel.h - device.h * scale) / 2.0,
            scale,
            device,
        })
    }

    /// The picture's size in panel points.
    pub fn shown(&self) -> Size {
        Size::new(self.device.w * self.scale, self.device.h * self.scale)
    }

    /// The device point under a panel point, or None outside the screen (in the bars, or past the panel).
    pub fn to_device(&self, px: f64, py: f64) -> Option<(f64, f64)> {
        let x = (px - self.x) / self.scale;
        let y = (py - self.y) / self.scale;
        ((0.0..=self.device.w).contains(&x) && (0.0..=self.device.h).contains(&y)).then_some((x, y))
    }

    /// A device point in panel points (where the indicators are drawn).
    pub fn to_panel(&self, x: f64, y: f64) -> (f64, f64) {
        (self.x + x * self.scale, self.y + y * self.scale)
    }
}

/// A device point in the current orientation as the 0..1 ratios of the portrait screen the digitizer takes. The
/// digitizer stays fixed to the portrait panel (idb), settled on the real device 2026-11-19 by PR 3's
/// `flare-sim-host --input-bench`: taps in rot90 and rot270 turned back this way landed 20 of 20 within 0.17 pt.
/// `portrait` is the screen's portrait size in points. Out-of-screen points clamp.
pub fn to_portrait_ratio(x: f64, y: f64, portrait: Size, o: Orientation) -> (f64, f64) {
    let (w, h) = (portrait.w, portrait.h);
    // Where the point is on the portrait screen (the picture is turned back the other way).
    let (px, py) = match o {
        Orientation::Rot0 => (x, y),
        // Shown turned clockwise: the shown top edge is the portrait's left edge.
        Orientation::Rot90 => (y, h - x),
        Orientation::Rot180 => (w - x, h - y),
        Orientation::Rot270 => (w - y, x),
    };
    ((px / w).clamp(0.0, 1.0), (py / h).clamp(0.0, 1.0))
}

/// The density at which one Android dp is one pixel.
pub const DP_DPI: f64 = 160.0;

/// Android's pixels per dp at `dpi` (dpi / 160), or None for a density that is not a positive number. The dp is the
/// unit the agent sees on Android, as the point is on iOS (rule 1367).
pub fn dp_scale(dpi: f64) -> Option<f64> {
    (dpi.is_finite() && dpi > 0.0).then(|| dpi / DP_DPI)
}

/// A point in dp as pixels, both in the current orientation. Only the emu host calls it (rule 1367): the agent, the
/// tree and the panel's touch map stay in dp, as they stay in points for iOS.
pub fn dp_to_px(x: f64, y: f64, scale: f64) -> (f64, f64) {
    (x * scale, y * scale)
}

/// A point in pixels as dp, both in the current orientation (the UI tree's bounds, in the emu host).
pub fn px_to_dp(x: f64, y: f64, scale: f64) -> (f64, f64) {
    (x / scale, y / scale)
}

impl Orientation {
    /// The orientation of a number of clockwise quarter turns, 0 to 3 ([`Orientation::ALL`]'s order). None for anything
    /// else. Not the emulator's `SkinRotation`: its LANDSCAPE turns the other way (checked on a real frame in Android
    /// plan PR A2), so the emu host reads it with `emu::keys::orientation_of`.
    pub fn from_quarter_turns(n: u32) -> Option<Orientation> {
        Orientation::ALL.get(n as usize).copied()
    }
}

/// The positions of a swipe from `from` to `to` over `duration_ms`: one every [`SWIPE_STEP_PT`] along the line,
/// the end point exactly, each with its time from the start. The first is the start (time 0); a zero-length swipe
/// is the start and the end at one spot.
pub fn swipe_path(from: (f64, f64), to: (f64, f64), duration_ms: u64) -> Vec<(f64, f64, u64)> {
    let d = (to.0 - from.0).hypot(to.1 - from.1);
    let steps = ((d / SWIPE_STEP_PT).ceil() as usize).max(1);
    (0..=steps)
        .map(|i| {
            let t = i as f64 / steps as f64;
            (
                from.0 + (to.0 - from.0) * t,
                from.1 + (to.1 - from.1) * t,
                (duration_ms as f64 * t).round() as u64,
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An iPhone 18 Pro: 402 x 874 points.
    const PHONE: Size = Size { w: 402.0, h: 874.0 };

    fn close(a: (f64, f64), b: (f64, f64)) -> bool {
        (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9
    }

    #[test]
    fn panel_points_map_to_device_points_through_the_letterbox() {
        // A tall panel: bars above and below, the width is filled.
        let lb = Letterbox::fit(Size::new(201.0, 600.0), PHONE).unwrap();
        assert_eq!(lb.scale, 0.5);
        assert_eq!((lb.x, lb.y), (0.0, (600.0 - 437.0) / 2.0));
        assert_eq!(lb.shown(), Size::new(201.0, 437.0));
        assert!(close(
            lb.to_device(100.5, 81.5 + 218.5).unwrap(),
            (201.0, 437.0)
        ));
        assert!(close(lb.to_device(0.0, 81.5).unwrap(), (0.0, 0.0)));
        assert_eq!(lb.to_device(100.0, 10.0), None, "in the bar above");
        assert_eq!(lb.to_device(100.0, 590.0), None, "in the bar below");
        // A wide panel: bars left and right, the height is filled.
        let lb = Letterbox::fit(Size::new(1000.0, 437.0), PHONE).unwrap();
        assert_eq!(lb.scale, 0.5);
        assert_eq!(lb.x, (1000.0 - 201.0) / 2.0);
        assert_eq!(lb.to_device(lb.x - 1.0, 100.0), None, "left bar");
        assert_eq!(lb.to_device(lb.x + 202.0, 100.0), None, "right bar");
        let (dx, dy) = lb.to_device(lb.x + 50.0, 100.0).unwrap();
        assert!(close((dx, dy), (100.0, 200.0)));
        assert!(close(lb.to_panel(dx, dy), (lb.x + 50.0, 100.0)), "and back");
        // Landscape: the device's size is the portrait one turned.
        let land = Orientation::Rot90.size(PHONE);
        assert_eq!(land, Size::new(874.0, 402.0));
        let lb = Letterbox::fit(Size::new(874.0, 1000.0), land).unwrap();
        assert_eq!(lb.scale, 1.0);
        assert!(close(
            lb.to_device(874.0, lb.y + 402.0).unwrap(),
            (874.0, 402.0)
        ));
        // Nothing to fit in, or nothing to fit.
        assert_eq!(Letterbox::fit(Size::new(0.0, 100.0), PHONE), None);
        assert_eq!(
            Letterbox::fit(Size::new(100.0, 100.0), Size::new(0.0, 0.0)),
            None
        );
    }

    #[test]
    fn points_map_to_portrait_ratios_in_every_orientation() {
        // Each orientation's four shown corners and centre land on the right portrait corners.
        let land = PHONE.turned();
        let cases = [
            (
                Orientation::Rot0,
                PHONE,
                [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)],
            ),
            // Turned clockwise: the shown top left is the portrait's bottom left.
            (
                Orientation::Rot90,
                land,
                [(0.0, 1.0), (0.0, 0.0), (1.0, 0.0), (1.0, 1.0)],
            ),
            (
                Orientation::Rot180,
                PHONE,
                [(1.0, 1.0), (0.0, 1.0), (0.0, 0.0), (1.0, 0.0)],
            ),
            (
                Orientation::Rot270,
                land,
                [(1.0, 0.0), (1.0, 1.0), (0.0, 1.0), (0.0, 0.0)],
            ),
        ];
        for (o, shown, want) in cases {
            assert_eq!(o.size(PHONE), shown, "{o:?}");
            let corners = [
                (0.0, 0.0),
                (shown.w, 0.0),
                (shown.w, shown.h),
                (0.0, shown.h),
            ];
            for (c, w) in corners.into_iter().zip(want) {
                let got = to_portrait_ratio(c.0, c.1, PHONE, o);
                assert!(close(got, w), "{o:?} {c:?}: {got:?} want {w:?}");
            }
            let mid = to_portrait_ratio(shown.w / 2.0, shown.h / 2.0, PHONE, o);
            assert!(close(mid, (0.5, 0.5)), "{o:?} centre {mid:?}");
        }
        // A point a quarter across a landscape screen's top edge.
        let got = to_portrait_ratio(874.0 / 4.0, 0.0, PHONE, Orientation::Rot90);
        assert!(close(got, (0.0, 0.75)), "{got:?}");
        // Off the screen: clamped to its edge.
        assert!(close(
            to_portrait_ratio(-5.0, 900.0, PHONE, Orientation::Rot0),
            (0.0, 1.0)
        ));
        for name in ["rot0", "rot90", "rot180", "rot270"] {
            assert!(Orientation::parse(name).is_some());
        }
        assert_eq!(Orientation::parse("portrait"), None);
    }

    /// Rule 1367: Android's agent unit is the dp. In each of the four orientations, the panel maps to dp through the
    /// same letterbox as iOS points, and only the dp-to-pixel step (the emu host's) knows the density.
    #[test]
    fn dp_map_to_pixels_in_every_orientation() {
        // A Pixel 8: 1080 x 2400 pixels at 420 dpi.
        let scale = dp_scale(420.0).unwrap();
        assert_eq!(scale, 2.625);
        let px = Size::new(1080.0, 2400.0);
        let dp = Size::new(px.w / scale, px.h / scale);
        for o in Orientation::ALL {
            let shown_dp = o.size(dp);
            let shown_px = o.size(px);
            // The screen's corners and centre in dp land on the same corners and centre in pixels.
            let pts = [
                (0.0, 0.0),
                (1.0, 0.0),
                (1.0, 1.0),
                (0.0, 1.0),
                (0.5, 0.5),
                (0.25, 0.75),
            ];
            for (fx, fy) in pts {
                let got = dp_to_px(shown_dp.w * fx, shown_dp.h * fy, scale);
                let want = (shown_px.w * fx, shown_px.h * fy);
                assert!(
                    (got.0 - want.0).abs() < 1e-9 && (got.1 - want.1).abs() < 1e-9,
                    "{o:?} ({fx}, {fy}): {got:?} want {want:?}"
                );
                // And back.
                let back = px_to_dp(got.0, got.1, scale);
                assert!(close(back, (shown_dp.w * fx, shown_dp.h * fy)), "{o:?}");
            }
            // A panel's click goes through the letterbox in dp, as for points: the panel's middle is the screen's.
            let lb = Letterbox::fit(Size::new(500.0, 500.0), shown_dp).unwrap();
            let (x, y) = lb.to_device(250.0, 250.0).unwrap();
            assert!(close(
                dp_to_px(x, y, scale),
                (shown_px.w / 2.0, shown_px.h / 2.0)
            ));
        }
        // A landscape screen is the portrait one turned, in dp too.
        assert_eq!(Orientation::Rot90.size(dp), Size::new(dp.h, dp.w));
        // 160 dpi is one pixel per dp; no density is no map.
        assert_eq!(dp_scale(160.0), Some(1.0));
        assert_eq!(dp_scale(0.0), None);
        assert_eq!(dp_scale(-420.0), None);
        assert_eq!(dp_scale(f64::INFINITY), None);
        // The emulator's quarter turns, in order.
        assert_eq!(
            (0..4)
                .map(|n| Orientation::from_quarter_turns(n).unwrap())
                .collect::<Vec<_>>(),
            Orientation::ALL
        );
        assert_eq!(Orientation::from_quarter_turns(4), None);
    }

    #[test]
    fn a_swipe_is_sampled_every_eight_points() {
        let path = swipe_path((10.0, 100.0), (10.0, 500.0), 400);
        assert_eq!(path.len(), 51, "400 pt is 50 steps of 8, plus the start");
        assert_eq!(path[0], (10.0, 100.0, 0));
        assert_eq!(path[1], (10.0, 108.0, 8));
        assert_eq!(*path.last().unwrap(), (10.0, 500.0, 400), "ends exactly");
        for w in path.windows(2) {
            let step = (w[1].0 - w[0].0).hypot(w[1].1 - w[0].1);
            assert!(step <= SWIPE_STEP_PT + 1e-9, "{step}");
            assert!(w[1].2 >= w[0].2, "time never goes back");
        }
        // A length that is not a multiple of 8: no step is longer than 8.
        let path = swipe_path((0.0, 0.0), (30.0, 40.0), 100);
        assert_eq!(path.len(), 8, "50 pt is 7 steps");
        assert!(close((path[7].0, path[7].1), (30.0, 40.0)));
        // Nowhere to go: start and end at one spot.
        let path = swipe_path((5.0, 5.0), (5.0, 5.0), 100);
        assert_eq!(path, vec![(5.0, 5.0, 0), (5.0, 5.0, 100)]);
    }
}

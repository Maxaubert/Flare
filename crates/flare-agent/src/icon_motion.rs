//! Agent icon motion (owner decision 2026-10-26, PR 2): how an agent's tab icon moves while its tab works, as pure
//! functions of the mark clock, and which motion each agent wears. A plugin picks it (`agents.motion.*`, permission
//! `agent-looks`) from Flare's fixed list; Flare draws it. Without a pick the icon stands still (today's look).
//!
//! The timings are the mocks the owner picked: Claude H1 (`docs/surfaces/agent-motion-mock.html`, class `hb1`), Codex
//! O6 (`spinpulse`), agy A1 (`docs/surfaces/agy-motion-mock.html`, `flow`), and for any agent the hop of H1 without its
//! blink, O1's turn (`rot`) and O4's breath (`breathe`). Nothing here reads a clock: the caller passes the animation
//! time in, as for `mark_motion` (decision D5), and the window pushes it with the marks (`push_frame`).

use crate::icon::AGY_COLOURS;
use crate::kinds::AgentKind;
use crate::mark_motion::{cubic_bezier, ease_in_out};

/// How an agent's icon moves while its tab works.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IconMotion {
    /// Still, as without a pick (a plugin may ask for it to hold an icon still over another plugin's pick).
    None,
    /// H1: a slow hop with a squash on landing, and a blink every other landing (the eyes are Clawd's).
    HopBlink,
    /// O6: a turn every 3.5 s while fading from 0.55 to 1 and back.
    SpinFade,
    /// A1: agy's rainbow flows along the arch. agy only: it needs the colours.
    RainbowFlow,
    /// The hop of H1, no blink.
    Hop,
    /// O1: a steady turn every 4 s.
    Spin,
    /// O4: grows and shrinks softly while it fades a little.
    Breathe,
}

impl IconMotion {
    /// Every motion, in documentation order.
    pub const ALL: [IconMotion; 7] = [
        IconMotion::None,
        IconMotion::HopBlink,
        IconMotion::SpinFade,
        IconMotion::RainbowFlow,
        IconMotion::Hop,
        IconMotion::Spin,
        IconMotion::Breathe,
    ];

    /// The wire name.
    pub fn name(self) -> &'static str {
        match self {
            IconMotion::None => "none",
            IconMotion::HopBlink => "hop-blink",
            IconMotion::SpinFade => "spin-fade",
            IconMotion::RainbowFlow => "rainbow-flow",
            IconMotion::Hop => "hop",
            IconMotion::Spin => "spin",
            IconMotion::Breathe => "breathe",
        }
    }

    pub fn parse(name: &str) -> Option<IconMotion> {
        IconMotion::ALL.into_iter().find(|m| m.name() == name)
    }

    /// What `AgentIcon` reads (`motion`): 0 still, then one number per motion.
    pub fn code(self) -> i32 {
        match self {
            IconMotion::None => 0,
            IconMotion::HopBlink => 1,
            IconMotion::SpinFade => 2,
            IconMotion::RainbowFlow => 3,
            IconMotion::Hop => 4,
            IconMotion::Spin => 5,
            IconMotion::Breathe => 6,
        }
    }

    /// Whether `agent`'s icon can wear it: the rainbow flow is agy's, the rest fit every agent.
    pub fn fits(self, agent: AgentKind) -> bool {
        self != IconMotion::RainbowFlow || agent == AgentKind::Agy
    }

    /// The names `agent` may be given, in documentation order.
    pub fn names_for(agent: AgentKind) -> Vec<&'static str> {
        IconMotion::ALL
            .into_iter()
            .filter(|m| m.fits(agent))
            .map(IconMotion::name)
            .collect()
    }
}

/// One lap of the hop (H1 and `hop`).
pub const HOP_MS: u64 = 1800;
/// The hop's timing curve between its keyframes (`cubic-bezier(.3, 0, .4, 1)`).
const HOP_CURVE: (f64, f64, f64, f64) = (0.3, 0.0, 0.4, 1.0);
/// The hop's keyframes: (fraction of a lap, rise as a fraction of the icon's size with up negative, scale x, scale y).
const HOP_KEYS: [(f32, f32, f32, f32); 5] = [
    (0.0, 0.0, 1.0, 1.0),
    (0.12, 0.0, 1.08, 0.88),
    (0.45, -0.22, 0.96, 1.06),
    (0.78, 0.0, 1.06, 0.92),
    (1.0, 0.0, 1.0, 1.0),
];
/// The blink's lap: two hops, the eyes shut from 47 % to 51 % of it (`lid2`, `steps(1)`).
pub const BLINK_MS: u64 = 2 * HOP_MS;
const BLINK_FROM: u64 = BLINK_MS * 47 / 100;
const BLINK_TO: u64 = BLINK_MS * 51 / 100;
/// O6: one turn, and one way of the fade (it runs there and back, `alternate`).
pub const SPIN_FADE_MS: u64 = 3500;
pub const FADE_MS: u64 = 1800;
pub const FADE_LOW: f32 = 0.55;
/// O1: one turn.
pub const SPIN_MS: u64 = 4000;
/// O4: one way of the breath, from 0.82 times the size at 0.7 opacity to 1.05 times at full.
pub const BREATHE_MS: u64 = 2600;
const BREATHE_SCALE: (f32, f32) = (0.82, 1.05);
const BREATHE_ALPHA: (f32, f32) = (0.7, 1.0);
/// A1: one lap of the colours, and how far ahead each path of the arch is (the mock's `--i * -0.3s`, one step a
/// path here: the icon is one path per colour).
pub const FLOW_MS: u64 = 3000;
pub const FLOW_STEP_MS: u64 = 300;
/// A1's keyframes: (fraction of a lap, index into `AGY_COLOURS`), linear between them.
const FLOW_KEYS: [(f32, usize); 7] = [
    (0.0, 0),
    (0.16, 2),
    (0.33, 3),
    (0.5, 5),
    (0.66, 7),
    (0.83, 9),
    (1.0, 0),
];

/// A colour as red, green and blue.
pub type Rgb = (u8, u8, u8);

/// `#rrggbb` as red, green and blue (black for anything else; the inputs are this crate's own constants).
pub const fn rgb_of(hex: &str) -> Rgb {
    const fn nib(b: u8) -> u8 {
        match b {
            b'0'..=b'9' => b - b'0',
            b'a'..=b'f' => b - b'a' + 10,
            b'A'..=b'F' => b - b'A' + 10,
            _ => 0,
        }
    }
    let b = hex.as_bytes();
    if b.len() != 7 || b[0] != b'#' {
        return (0, 0, 0);
    }
    (
        nib(b[1]) * 16 + nib(b[2]),
        nib(b[3]) * 16 + nib(b[4]),
        nib(b[5]) * 16 + nib(b[6]),
    )
}

/// agy's colours as red, green and blue, path by path (the still arch).
pub const AGY_RGB: [Rgb; 10] = {
    let mut out = [(0, 0, 0); 10];
    let mut i = 0;
    while i < 10 {
        out[i] = rgb_of(AGY_COLOURS[i]);
        i += 1;
    }
    out
};

/// Where every moving icon is at one instant. The window draws the one its motion names; a still icon reads none.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IconPhase {
    pub still: bool,
    /// The hop: the rise as a fraction of the icon's size (up is negative) and the squash.
    pub hop_y: f32,
    pub hop_sx: f32,
    pub hop_sy: f32,
    /// Clawd's eyes are shut (H1).
    pub blink: bool,
    /// O6: the turn and the opacity.
    pub spin_fade_deg: f32,
    pub fade: f32,
    /// O1: the turn.
    pub spin_deg: f32,
    /// O4: the size and the opacity.
    pub grow: f32,
    pub grow_alpha: f32,
    /// A1: each path's colour, in `AGY_COLOURS` order.
    pub flow: [Rgb; 10],
}

/// Animation effects off, or no tab working: every icon stands as it is drawn still.
pub const ICON_STILL: IconPhase = IconPhase {
    still: true,
    hop_y: 0.0,
    hop_sx: 1.0,
    hop_sy: 1.0,
    blink: false,
    spin_fade_deg: 0.0,
    fade: 1.0,
    spin_deg: 0.0,
    grow: 1.0,
    grow_alpha: 1.0,
    flow: AGY_RGB,
};

/// Every motion's frame at `ms` (animation time).
pub fn icon_phase(ms: u64) -> IconPhase {
    let (hop_y, hop_sx, hop_sy) = hop_at(ms);
    let grow = there_and_back(ms, BREATHE_MS);
    let mut flow = AGY_RGB;
    for (k, c) in flow.iter_mut().enumerate() {
        *c = flow_colour(ms + FLOW_STEP_MS * k as u64);
    }
    IconPhase {
        still: false,
        hop_y,
        hop_sx,
        hop_sy,
        blink: blink_at(ms),
        spin_fade_deg: turn(ms, SPIN_FADE_MS),
        fade: FADE_LOW + (1.0 - FADE_LOW) * there_and_back(ms, FADE_MS),
        spin_deg: turn(ms, SPIN_MS),
        grow: BREATHE_SCALE.0 + (BREATHE_SCALE.1 - BREATHE_SCALE.0) * grow,
        grow_alpha: BREATHE_ALPHA.0 + (BREATHE_ALPHA.1 - BREATHE_ALPHA.0) * grow,
        flow,
    }
}

/// The hop at `ms`: rise, scale x, scale y.
pub fn hop_at(ms: u64) -> (f32, f32, f32) {
    let t = (ms % HOP_MS) as f32 / HOP_MS as f32;
    let i = HOP_KEYS
        .windows(2)
        .position(|w| t < w[1].0)
        .unwrap_or(HOP_KEYS.len() - 2);
    let (a, b) = (HOP_KEYS[i], HOP_KEYS[i + 1]);
    let (x1, y1, x2, y2) = HOP_CURVE;
    let e = cubic_bezier(x1, y1, x2, y2, (t - a.0) / (b.0 - a.0));
    let lerp = |p: f32, q: f32| p + (q - p) * e;
    (lerp(a.1, b.1), lerp(a.2, b.2), lerp(a.3, b.3))
}

/// Where `AgentIcon` draws its moving box, in whole device pixels of the window: left, top, width, height.
///
/// This is the rule `AgentIcon`'s `body` box follows in `tabmark.slint` (owner 2026-11-13, "keep squash, snap to
/// pixels"): `origin` is the icon's place in the window and `size` its size, in logical px, and `scale` the device
/// pixels per logical px. The rest size, the squashed width and height and the rise each round to whole device
/// pixels (`Px.snap`), and the box sits on whole device pixels of the window, so no frame lands on a fraction and
/// smears. The box keeps its centre across. A hop (`feet` given: where the art's feet sit, as a fraction of the box's
/// height from its top, `icon::CLAWD_FEET` and the like) squashes about its feet, which stay on the rest line to half a
/// device pixel (plus the whole-pixel rise); any other motion keeps its centre down.
pub fn icon_box(
    origin: (f32, f32),
    size: f32,
    scale: f32,
    (sx, sy, rise): (f32, f32, f32),
    feet: Option<f32>,
) -> (f32, f32, f32, f32) {
    let snap = |l: f32| (l * scale).round();
    let rest = snap(size);
    let (w, h) = (snap(size * sx), snap(size * sy));
    let (ax, ay) = (snap(origin.0), snap(origin.1));
    let left = (ax + (rest - w) / 2.0).round();
    let top = match feet {
        Some(f) => ay + (f * rest).round() - (f * h).round() + snap(rise * size),
        None => (ay + (rest - h) / 2.0).round(),
    };
    (left, top, w, h)
}

/// Whether Clawd's eyes are shut at `ms` (every other landing of the hop).
pub fn blink_at(ms: u64) -> bool {
    (BLINK_FROM..BLINK_TO).contains(&(ms % BLINK_MS))
}

/// A turn of `lap` ms at `ms`, in degrees.
fn turn(ms: u64, lap: u64) -> f32 {
    (ms % lap) as f32 / lap as f32 * 360.0
}

/// An `ease-in-out` there and back (`alternate`), `one_way` ms each way: 0 at the start, 1 at the far end.
fn there_and_back(ms: u64, one_way: u64) -> f32 {
    let p = ms % (2 * one_way);
    let t = if p < one_way {
        p as f32 / one_way as f32
    } else {
        1.0 - (p - one_way) as f32 / one_way as f32
    };
    ease_in_out(t)
}

/// A1's colour at `ms` of its lap.
pub fn flow_colour(ms: u64) -> Rgb {
    let t = (ms % FLOW_MS) as f32 / FLOW_MS as f32;
    let i = FLOW_KEYS
        .windows(2)
        .position(|w| t < w[1].0)
        .unwrap_or(FLOW_KEYS.len() - 2);
    let (a, b) = (FLOW_KEYS[i], FLOW_KEYS[i + 1]);
    let f = (t - a.0) / (b.0 - a.0);
    let (p, q) = (AGY_RGB[a.1], AGY_RGB[b.1]);
    let mix = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * f).round() as u8;
    (mix(p.0, q.0), mix(p.1, q.1), mix(p.2, q.2))
}

/// One plugin's pick for one agent.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Pick {
    agent: AgentKind,
    plugin: String,
    motion: IconMotion,
}

/// Which motion each agent wears: the plugins' picks, the last one set winning. A plugin that clears its pick, or is
/// turned off, takes it away, and the pick set before it shows again (the decor rule).
#[derive(Clone, Debug, Default)]
pub struct MotionPicks {
    /// Oldest first; at most one per agent and plugin.
    picks: Vec<Pick>,
}

impl MotionPicks {
    pub fn new() -> Self {
        Self::default()
    }

    /// The motion `agent` wears now.
    pub fn of(&self, agent: AgentKind) -> IconMotion {
        self.picks
            .iter()
            .rev()
            .find(|p| p.agent == agent)
            .map_or(IconMotion::None, |p| p.motion)
    }

    /// `plugin` picks `motion` for `agent` (it becomes the newest). True when what `agent` wears changed.
    pub fn set(&mut self, agent: AgentKind, plugin: &str, motion: IconMotion) -> bool {
        let before = self.of(agent);
        self.picks
            .retain(|p| !(p.agent == agent && p.plugin == plugin));
        self.picks.push(Pick {
            agent,
            plugin: plugin.to_string(),
            motion,
        });
        self.of(agent) != before
    }

    /// `plugin`'s pick for `agent` goes. True when it had one.
    pub fn clear(&mut self, agent: AgentKind, plugin: &str) -> bool {
        let n = self.picks.len();
        self.picks
            .retain(|p| !(p.agent == agent && p.plugin == plugin));
        self.picks.len() != n
    }

    /// Every pick of `plugin` goes (it was turned off or removed). True when it had any.
    pub fn clear_plugin(&mut self, plugin: &str) -> bool {
        let n = self.picks.len();
        self.picks.retain(|p| p.plugin != plugin);
        self.picks.len() != n
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kinds::RESUMABLE_AGENTS;

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn names_round_trip_and_the_codes_are_unique() {
        let mut codes = std::collections::HashSet::new();
        for m in IconMotion::ALL {
            assert_eq!(IconMotion::parse(m.name()), Some(m));
            assert!(codes.insert(m.code()), "{m:?}");
        }
        assert_eq!(IconMotion::None.code(), 0);
        for bad in ["", "Hop", "hop ", "flow", "rainbow", "glow"] {
            assert_eq!(IconMotion::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_rainbow_flow_is_agys_alone() {
        for a in RESUMABLE_AGENTS {
            for m in IconMotion::ALL {
                assert_eq!(
                    m.fits(a),
                    m != IconMotion::RainbowFlow || a == AgentKind::Agy,
                    "{m:?} {a:?}"
                );
            }
        }
        assert_eq!(IconMotion::names_for(AgentKind::Agy).len(), 7);
        assert!(!IconMotion::names_for(AgentKind::Claude).contains(&"rainbow-flow"));
    }

    #[test]
    fn the_hop_rises_a_fifth_of_the_icon_and_squashes_on_landing() {
        assert_eq!(hop_at(0), (0.0, 1.0, 1.0));
        // The keyframes, exactly.
        let (y, sx, sy) = hop_at(HOP_MS * 45 / 100);
        assert!(
            near(y, -0.22) && near(sx, 0.96) && near(sy, 1.06),
            "{y} {sx} {sy}"
        );
        let (y, sx, sy) = hop_at(HOP_MS * 12 / 100);
        assert!(near(y, 0.0) && near(sx, 1.08) && near(sy, 0.88));
        let (y, sx, sy) = hop_at(HOP_MS * 78 / 100);
        assert!(near(y, 0.0) && near(sx, 1.06) && near(sy, 0.92));
        // It never goes higher than the top keyframe, never below the ground, and each lap is the same.
        for ms in (0..HOP_MS).step_by(10) {
            let (y, _, _) = hop_at(ms);
            assert!((-0.2201..=0.0).contains(&y), "{ms}: {y}");
            assert_eq!(hop_at(ms), hop_at(ms + HOP_MS));
        }
        // Rising between the squash and the top: the curve is the mock's, half way is above a straight line's.
        let (mid, _, _) = hop_at(HOP_MS * 285 / 1000);
        assert!(mid < -0.11, "{mid}");
    }

    /// Owner 2026-11-13 ("keep squash, snap to pixels"): every frame of the hop, at every shipped scale and wherever
    /// the icon sits, is a box on whole device pixels whose art keeps its feet on the rest line (to half a device
    /// pixel, lifted only by the whole-pixel rise), and it is never narrower than the narrowest key squashes it.
    #[test]
    fn every_frame_of_the_hop_is_a_whole_device_pixel_box_standing_on_its_feet() {
        use crate::icon::{AGY_FEET, CLAWD_FEET, OPENAI_FEET};
        let size = 18.0;
        let min_sx = HOP_KEYS.iter().map(|k| k.2).fold(f32::MAX, f32::min);
        let max_sx = HOP_KEYS.iter().map(|k| k.2).fold(f32::MIN, f32::max);
        assert_eq!((min_sx, max_sx), (0.96, 1.08));
        let whole = |v: f32| v.fract() == 0.0;
        for scale in [1.0f32, 1.25, 1.5, 2.0, 2.25] {
            for origin in [(340.0, 9.0), (10.0, 10.5), (14.0, 14.0), (7.3, 3.7)] {
                for feet in [CLAWD_FEET, AGY_FEET, OPENAI_FEET] {
                    let rest = icon_box(origin, size, scale, (1.0, 1.0, 0.0), Some(feet));
                    assert_eq!(
                        rest,
                        icon_box(origin, size, scale, (1.0, 1.0, 0.0), None),
                        "at rest a hop and a breath draw the same box ({scale}, {origin:?})"
                    );
                    let ground = rest.1 + (feet * rest.3).round();
                    let mut last = rest;
                    for ms in 0..HOP_MS {
                        let (rise, sx, sy) = hop_at(ms);
                        let b = icon_box(origin, size, scale, (sx, sy, rise), Some(feet));
                        let at = format!(
                            "{ms} ms at {scale}, {origin:?}, feet {feet}: {b:?}, rest {rest:?}"
                        );
                        assert!(whole(b.0) && whole(b.1) && whole(b.2) && whole(b.3), "{at}");
                        // The squash never takes more width than its narrowest key, nor gives more than its widest.
                        assert!(b.2 >= (size * scale * min_sx).round(), "{at}");
                        assert!(b.2 <= (size * scale * max_sx).round(), "{at}");
                        // Centred across on the rest box, to half a device pixel.
                        assert!(
                            ((b.0 + b.2 / 2.0) - (rest.0 + rest.2 / 2.0)).abs() <= 0.5,
                            "{at}"
                        );
                        // The feet: on the ground line, lifted only by the whole-pixel rise.
                        let lift = (rise * size * scale).round();
                        assert!(
                            (b.1 + feet * b.3 - (ground + lift)).abs() <= 0.5 + 1e-4,
                            "{at}"
                        );
                        // A frame at a time the size steps by at most one device pixel: no jitter.
                        if ms % 16 == 0 {
                            assert!((b.2 - last.2).abs() <= 1.0, "{at} after {last:?}");
                            assert!((b.3 - last.3).abs() <= 1.0, "{at} after {last:?}");
                            last = b;
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn the_breath_keeps_its_box_centred_on_whole_device_pixels() {
        for scale in [1.0f32, 1.25, 1.5, 2.0, 2.25] {
            let rest = icon_box((10.0, 10.5), 18.0, scale, (1.0, 1.0, 0.0), None);
            for g in [0.82f32, 0.9, 1.0, 1.05] {
                let b = icon_box((10.0, 10.5), 18.0, scale, (g, g, 0.0), None);
                assert!(b.0.fract() == 0.0 && b.1.fract() == 0.0 && b.2.fract() == 0.0);
                assert!(((b.1 + b.3 / 2.0) - (rest.1 + rest.3 / 2.0)).abs() <= 0.5);
            }
        }
    }

    #[test]
    fn the_eyes_shut_every_other_landing() {
        assert!(!blink_at(0));
        assert!(!blink_at(1691));
        assert!(blink_at(1692));
        assert!(blink_at(1835));
        assert!(!blink_at(1836));
        // Once in two hops: the second lap's landing does not blink.
        assert!(!blink_at(1692 + HOP_MS));
        assert!(blink_at(1692 + BLINK_MS));
        let shut = (0..BLINK_MS).filter(|ms| blink_at(*ms)).count();
        assert_eq!(shut, 144);
    }

    #[test]
    fn spin_fade_turns_in_three_and_a_half_seconds_and_fades_from_055() {
        let p = |ms| icon_phase(ms);
        assert_eq!(p(0).spin_fade_deg, 0.0);
        assert!(near(p(875).spin_fade_deg, 90.0));
        assert!(near(p(1750).spin_fade_deg, 180.0));
        assert_eq!(p(3500).spin_fade_deg, 0.0);
        assert!(near(p(0).fade, 0.55));
        assert!(near(p(900).fade, 0.775));
        assert!(near(p(1800).fade, 1.0));
        assert!(near(p(2700).fade, 0.775));
        assert!(near(p(3600).fade, 0.55));
    }

    #[test]
    fn spin_turns_in_four_seconds_and_breathe_grows_from_082_to_105() {
        assert!(near(icon_phase(1000).spin_deg, 90.0));
        assert_eq!(icon_phase(4000).spin_deg, 0.0);
        let b0 = icon_phase(0);
        assert!(near(b0.grow, 0.82) && near(b0.grow_alpha, 0.7));
        let b1 = icon_phase(BREATHE_MS);
        assert!(near(b1.grow, 1.05) && near(b1.grow_alpha, 1.0));
        assert!(near(icon_phase(2 * BREATHE_MS).grow, 0.82));
    }

    #[test]
    fn the_rainbow_flows_through_the_banners_colours_each_path_ahead_of_the_last() {
        // The keyframes are agy's own colours.
        assert_eq!(flow_colour(0), AGY_RGB[0]);
        assert_eq!(flow_colour(480), AGY_RGB[2]);
        assert_eq!(flow_colour(1500), AGY_RGB[5]);
        assert_eq!(flow_colour(2490), AGY_RGB[9]);
        assert_eq!(flow_colour(3000), AGY_RGB[0]);
        // Half way between two keyframes is half way between their colours.
        let (a, b) = (AGY_RGB[0], AGY_RGB[2]);
        let mid = flow_colour(240);
        assert_eq!(
            mid,
            (
                (u16::from(a.0) + u16::from(b.0)).div_ceil(2) as u8,
                (u16::from(a.1) + u16::from(b.1)).div_ceil(2) as u8,
                (u16::from(a.2) + u16::from(b.2)).div_ceil(2) as u8,
            )
        );
        // Path k is 0.3 s ahead of path 0, so the colours run along the arch.
        let p = icon_phase(1000);
        for k in 0..10 {
            assert_eq!(p.flow[k], flow_colour(1000 + 300 * k as u64), "path {k}");
        }
        assert_eq!(icon_phase(0).flow[0], AGY_RGB[0]);
        assert_ne!(icon_phase(0).flow, icon_phase(600).flow);
    }

    #[test]
    fn the_still_frame_is_the_icon_as_drawn() {
        let s = ICON_STILL;
        assert!(s.still && !s.blink);
        assert_eq!((s.hop_y, s.hop_sx, s.hop_sy), (0.0, 1.0, 1.0));
        assert_eq!((s.spin_fade_deg, s.spin_deg), (0.0, 0.0));
        assert_eq!((s.fade, s.grow, s.grow_alpha), (1.0, 1.0, 1.0));
        assert_eq!(s.flow, AGY_RGB);
        assert_eq!(AGY_RGB[0], (0x15, 0xaa, 0xbf));
        assert_eq!(rgb_of("#4c6ef5"), (0x4c, 0x6e, 0xf5));
        assert_eq!(rgb_of("4c6ef5"), (0, 0, 0));
        assert!(!icon_phase(0).still);
        let src = include_str!("icon_motion.rs");
        assert!(!src.contains(concat!("Inst", "ant")));
        assert!(!src.contains(concat!("System", "Time")));
    }

    #[test]
    fn the_last_pick_wins_and_a_plugin_takes_its_picks_with_it() {
        let mut p = MotionPicks::new();
        let claude = AgentKind::Claude;
        assert_eq!(p.of(claude), IconMotion::None);
        assert!(p.set(claude, "a", IconMotion::HopBlink));
        assert!(
            !p.set(claude, "a", IconMotion::HopBlink),
            "the same pick again"
        );
        assert!(p.set(claude, "b", IconMotion::Spin));
        assert_eq!(p.of(claude), IconMotion::Spin);
        // a sets again: it is the newest now.
        assert!(p.set(claude, "a", IconMotion::Breathe));
        assert_eq!(p.of(claude), IconMotion::Breathe);
        assert!(p.clear(claude, "a"));
        assert!(!p.clear(claude, "a"));
        assert_eq!(p.of(claude), IconMotion::Spin);
        assert!(p.set(AgentKind::Agy, "b", IconMotion::RainbowFlow));
        assert!(p.clear_plugin("b"));
        assert!(!p.clear_plugin("b"));
        assert_eq!(p.of(claude), IconMotion::None);
        assert_eq!(p.of(AgentKind::Agy), IconMotion::None);
        // A pick of none holds an agent still over an older one.
        p.set(AgentKind::Codex, "a", IconMotion::SpinFade);
        p.set(AgentKind::Codex, "b", IconMotion::None);
        assert_eq!(p.of(AgentKind::Codex), IconMotion::None);
    }
}

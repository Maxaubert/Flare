//! The sidebar ground (Task 7). Port of PT `src/renderer/src/lib/sidebarGround.ts`.

/// One even OKLab lightness step darker than the terminal ground.
pub const SIDEBAR_STEP: f64 = 0.045;

/// Below this luminance a ground is black and keeps itself.
pub const BLACK_GROUND: f64 = 0.0045;

use crate::ansi::luminance;
use crate::oklab::{from_lab, to_lab};

/// The sidebar's own ground for a terminal ground (a 6-digit hex).
pub fn sidebar_ground(bg: &str) -> String {
    if luminance(bg) < BLACK_GROUND {
        return bg.to_string();
    }
    let [l, a, b] = to_lab(bg);
    let step = SIDEBAR_STEP.min(l - 0.08);
    let k = (l - step) / l;
    from_lab([l - step, a * k, b * k])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oklab::ok_lightness;
    use crate::preset::presets;
    use crate::testutil::fixture;

    #[test]
    fn the_sidebar_ground_is_one_even_step_darker_or_black_kept() {
        for p in presets() {
            let side = sidebar_ground(p.bg);
            if luminance(p.bg) < BLACK_GROUND {
                assert_eq!(side, p.bg, "{} is black and keeps its ground", p.id);
                continue;
            }
            let step = ok_lightness(p.bg) - ok_lightness(&side);
            assert!((step - 0.045).abs() <= 0.005, "{}: step {step}", p.id);
        }
    }

    #[test]
    fn oracle_sidebar_grounds() {
        let v = fixture("sidebar_grounds.json");
        let map = v.as_object().expect("an object");
        assert_eq!(map.len(), 39);
        for p in presets() {
            assert_eq!(
                sidebar_ground(p.bg),
                map[p.id].as_str().expect("hex"),
                "{}",
                p.id
            );
        }
    }
}

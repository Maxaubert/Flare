//! on_device_pixels: device-pixel helpers for the tab layout (Task 6).

/// Snaps a CSS length to a whole number of device pixels: `round(px * ratio) / ratio`.
/// A ratio that is zero, negative or NaN leaves the length alone.
pub fn on_device_pixels(px: f64, ratio: f64) -> f64 {
    if ratio.is_nan() || ratio <= 0.0 {
        return px;
    }
    (px * ratio).round() / ratio
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_edge_ends_on_a_whole_device_pixel_at_any_scale() {
        for ratio in [1.0, 1.25, 1.5, 1.75, 2.0] {
            for px in [150.0, 220.0, 235.0, 241.3, 420.0] {
                let w = on_device_pixels(px, ratio);
                assert!((w * ratio - (w * ratio).round()).abs() < 1e-9);
                assert!((w - px).abs() <= 0.5 / ratio + 1e-9);
            }
        }
    }

    #[test]
    fn a_whole_pixel_width_is_left_alone() {
        assert_eq!(on_device_pixels(220.0, 1.25), 220.0);
        assert_eq!(on_device_pixels(235.0, 1.0), 235.0);
    }

    #[test]
    fn a_bad_ratio_leaves_the_length_alone() {
        for ratio in [0.0, -1.0, f64::NAN] {
            assert_eq!(on_device_pixels(241.3, ratio), 241.3);
        }
    }
}

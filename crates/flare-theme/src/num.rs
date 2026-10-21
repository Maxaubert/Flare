//! Number helpers that behave like JavaScript's where Prism Terminal's code relies on it.

/// `Math.round`: a tie goes toward +infinity (Rust's `f64::round` sends it away from zero).
pub fn js_round(x: f64) -> f64 {
    let f = x.floor();
    if x - f >= 0.5 {
        f + 1.0
    } else {
        f
    }
}

/// `Math.min(hi, Math.max(lo, n))` for a number that is not NaN.
pub fn clamp(n: f64, lo: f64, hi: f64) -> f64 {
    hi.min(lo.max(n))
}

/// Two lower-case hex digits of `n`, clamped to 0..=255 and rounded: PT's `byte` and `c` helpers.
pub fn hex_byte(n: f64) -> String {
    format!("{:02x}", js_round(clamp(n, 0.0, 255.0)) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn js_round_ties_go_up() {
        assert_eq!(js_round(2.5), 3.0);
        assert_eq!(js_round(0.5), 1.0);
        assert_eq!(js_round(-1.5), -1.0);
        assert_eq!(js_round(-0.5), 0.0);
        assert_eq!(js_round(0.49999999999999994), 0.0);
        assert_eq!(js_round(127.5), 128.0);
    }

    #[test]
    fn clamp_orders_like_math_min_max() {
        assert_eq!(clamp(5.0, 0.0, 3.0), 3.0);
        assert_eq!(clamp(-5.0, 0.0, 3.0), 0.0);
        assert_eq!(clamp(2.0, 0.0, 3.0), 2.0);
    }

    #[test]
    fn hex_byte_pads_clamps_and_rounds() {
        assert_eq!(hex_byte(-5.0), "00");
        assert_eq!(hex_byte(0.4), "00");
        assert_eq!(hex_byte(127.5), "80");
        assert_eq!(hex_byte(255.4), "ff");
        assert_eq!(hex_byte(300.0), "ff");
        assert_eq!(hex_byte(10.0), "0a");
    }
}

//! The one blend formula both painters use, in encoded sRGB byte space.

use crate::color::Rgba;

/// Blends a mask coverage of `fg` over `dst` (`dst*(255-c) + fg*c`, rounded as `(x + 127) / 255`).
pub fn blend_mask(dst: [u8; 4], fg: Rgba, coverage: u8) -> [u8; 4] {
    let c = u32::from(coverage);
    let inv = 255 - c;
    let src = [fg.r, fg.g, fg.b, 255];
    let mut out = [0u8; 4];
    for (i, o) in out.iter_mut().enumerate() {
        let d = u32::from(dst[i]);
        let s = u32::from(src[i]);
        *o = ((d * inv + s * c + 127) / 255) as u8;
    }
    out
}

/// Premultiplied source over `dst`: `src + dst*(255-a)/255`, rounded the same way, clamped to 255.
pub fn blend_premul(dst: [u8; 4], src: [u8; 4]) -> [u8; 4] {
    let inv = 255 - u32::from(src[3]);
    let mut out = [0u8; 4];
    for (i, o) in out.iter_mut().enumerate() {
        let d = u32::from(dst[i]);
        let s = u32::from(src[i]);
        *o = (s + (d * inv + 127) / 255).min(255) as u8;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHITE: Rgba = Rgba::rgb(255, 255, 255);

    #[test]
    fn zero_coverage_keeps_dst() {
        let dst = [10, 20, 30, 255];
        assert_eq!(blend_mask(dst, WHITE, 0), dst);
    }

    #[test]
    fn full_coverage_is_fg() {
        assert_eq!(blend_mask([0, 0, 0, 255], WHITE, 255), [255, 255, 255, 255]);
    }

    #[test]
    fn half_coverage_rounds() {
        assert_eq!(blend_mask([0, 0, 0, 255], WHITE, 128), [128, 128, 128, 255]);
    }

    #[test]
    fn premul_opaque_replaces() {
        let src = [12, 34, 56, 255];
        assert_eq!(blend_premul([200, 200, 200, 255], src), src);
    }

    #[test]
    fn premul_half() {
        assert_eq!(
            blend_premul([0, 0, 255, 255], [128, 0, 0, 128]),
            [128, 0, 127, 255]
        );
    }
}

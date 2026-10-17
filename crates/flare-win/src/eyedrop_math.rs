//! The eyedropper's pure maths: the magnifier and the pixel under the pointer (M7 Task 30).

/// The virtual screen in physical pixels: top-down rows, 4 bytes a pixel in B, G, R, A order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub origin: (i32, i32),
    pub size: (u32, u32),
    pub bgra: Vec<u8>,
}

impl Snapshot {
    /// The colour at a screen coordinate, or None outside the snapshot.
    pub fn colour_at(&self, x: i32, y: i32) -> Option<[u8; 3]> {
        let dx = i64::from(x) - i64::from(self.origin.0);
        let dy = i64::from(y) - i64::from(self.origin.1);
        if dx < 0 || dy < 0 || dx >= i64::from(self.size.0) || dy >= i64::from(self.size.1) {
            return None;
        }
        let at = ((dy as usize) * (self.size.0 as usize) + dx as usize) * 4;
        let px = self.bgra.get(at..at + 3)?;
        Some([px[2], px[1], px[0]])
    }
}

/// `#rrggbb`, lower case.
pub fn hex_of(rgb: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
}

/// The magnifier shows this many cells on a side.
pub const LOUPE_CELLS: u32 = 9;
/// Each cell is this many device pixels square.
pub const LOUPE_CELL_PX: u32 = 10;

/// The top-left of the 9 by 9 block around `centre`, clamped so the block stays inside the
/// snapshot (the pointer pixel is the centre cell except at an edge).
pub fn loupe_source(
    centre: (i32, i32),
    snap_origin: (i32, i32),
    snap_size: (u32, u32),
) -> (i32, i32) {
    let half = (LOUPE_CELLS / 2) as i32;
    let clamp = |c: i32, o: i32, s: u32| {
        let lo = o;
        let hi = (o + s as i32 - LOUPE_CELLS as i32).max(lo);
        (c - half).clamp(lo, hi)
    };
    (
        clamp(centre.0, snap_origin.0, snap_size.0),
        clamp(centre.1, snap_origin.1, snap_size.1),
    )
}

/// Where the loupe box goes: 24 px right and below the pointer, flipped left or up when it
/// would leave the screen `(x, y, w, h)`.
pub fn loupe_origin(
    pointer: (i32, i32),
    loupe: (u32, u32),
    screen: (i32, i32, u32, u32),
) -> (i32, i32) {
    const GAP: i32 = 24;
    let (sx, sy, sw, sh) = screen;
    let (lw, lh) = (loupe.0 as i32, loupe.1 as i32);
    let mut x = pointer.0 + GAP;
    if x + lw > sx + sw as i32 {
        x = pointer.0 - GAP - lw;
    }
    let mut y = pointer.1 + GAP;
    if y + lh > sy + sh as i32 {
        y = pointer.1 - GAP - lh;
    }
    (x.max(sx), y.max(sy))
}

/// Black or white text for a caption over `rgb`: black when the relative luminance is above 0.4.
pub fn caption_ink(rgb: [u8; 3]) -> [u8; 3] {
    let lin = |c: u8| {
        let c = f64::from(c) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let y = 0.2126 * lin(rgb[0]) + 0.7152 * lin(rgb[1]) + 0.0722 * lin(rgb[2]);
    if y > 0.4 { [0, 0, 0] } else { [255, 255, 255] }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap() -> Snapshot {
        // 3 by 2 at origin (-5, -3); pixel i has B=i, G=i+10, R=i+20.
        let mut bgra = Vec::new();
        for i in 0..6u8 {
            bgra.extend_from_slice(&[i, i + 10, i + 20, 255]);
        }
        Snapshot {
            origin: (-5, -3),
            size: (3, 2),
            bgra,
        }
    }

    #[test]
    fn colour_at_reads_bgra_as_rgb() {
        let s = snap();
        assert_eq!(s.colour_at(-5, -3), Some([20, 10, 0]));
        assert_eq!(s.colour_at(-3, -3), Some([22, 12, 2]));
        assert_eq!(s.colour_at(-5, -2), Some([23, 13, 3]));
        assert_eq!(s.colour_at(-3, -2), Some([25, 15, 5]));
    }

    #[test]
    fn outside_the_snapshot_is_none() {
        let s = snap();
        for (x, y) in [(-6, -3), (-5, -4), (-2, -3), (-5, -1), (0, 0)] {
            assert_eq!(s.colour_at(x, y), None, "{x},{y}");
        }
    }

    #[test]
    fn hex_is_lower_case_and_padded() {
        assert_eq!(hex_of([1, 2, 255]), "#0102ff");
        assert_eq!(hex_of([0, 0, 0]), "#000000");
        assert_eq!(hex_of([171, 205, 239]), "#abcdef");
    }

    #[test]
    fn the_loupe_block_stays_inside_the_snapshot() {
        let o = (-100, 0);
        let sz = (200, 100);
        assert_eq!(loupe_source((0, 50), o, sz), (-4, 46));
        assert_eq!(loupe_source((-100, 0), o, sz), (-100, 0));
        assert_eq!(loupe_source((99, 99), o, sz), (91, 91));
        assert_eq!(loupe_source((-98, 99), o, sz), (-100, 91));
        assert_eq!(loupe_source((5, 5), (0, 0), (4, 4)), (0, 0));
    }

    #[test]
    fn the_loupe_flips_at_the_screen_edge() {
        let scr = (0, 0, 1920, 1080);
        assert_eq!(loupe_origin((100, 100), (92, 114), scr), (124, 124));
        assert_eq!(
            loupe_origin((1900, 100), (92, 114), scr),
            (1900 - 24 - 92, 124)
        );
        assert_eq!(
            loupe_origin((100, 1070), (92, 114), scr),
            (124, 1070 - 24 - 114)
        );
        assert_eq!(loupe_origin((1900, 1070), (92, 114), scr), (1784, 932));
    }

    #[test]
    fn caption_ink_is_black_on_light_and_white_on_dark() {
        assert_eq!(caption_ink([255, 255, 255]), [0, 0, 0]);
        assert_eq!(caption_ink([255, 255, 0]), [0, 0, 0]);
        assert_eq!(caption_ink([0, 0, 0]), [255, 255, 255]);
        assert_eq!(caption_ink([20, 30, 120]), [255, 255, 255]);
    }
}

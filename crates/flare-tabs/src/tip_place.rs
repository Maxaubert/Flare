//! place_tip: where a tooltip goes (Task 9).

/// The gap between the item and its tip, and the tip's distance from the window's edge.
pub const TIP_GAP: f64 = 6.0;
/// See [`TIP_GAP`].
pub const TIP_MARGIN: f64 = 6.0;
/// Core Tooltips SHOW_MS: the pointer must rest this long before a tip shows.
pub const TIP_SHOW_MS: u64 = 1000;

/// The side a tip is placed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TipSide {
    /// Under the item.
    Below,
    /// Over the item.
    Above,
    /// Beside the item, on its right.
    Right,
    /// Beside the item, on its left.
    Left,
}

/// A rectangle in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoxF {
    /// Left edge.
    pub left: f64,
    /// Top edge.
    pub top: f64,
    /// Width.
    pub width: f64,
    /// Height.
    pub height: f64,
}

/// Where the tip goes and the side it ended on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TipSpot {
    /// Left edge of the tip.
    pub x: f64,
    /// Top edge of the tip.
    pub y: f64,
    /// Where it ended up, after any flip.
    pub side: TipSide,
}

/// Port of placeTip: centred under the item, above when there is no room below, beside it where asked
/// (flipping to the other side, else below), always inside the view by [`TIP_MARGIN`].
pub fn place_tip(item: BoxF, tip: (f64, f64), view: (f64, f64), prefer: TipSide) -> TipSpot {
    let (tw, th) = tip;
    let (vw, vh) = view;
    let fits_below = item.top + item.height + TIP_GAP + th <= vh - TIP_MARGIN;
    let fits_above = item.top - TIP_GAP - th >= TIP_MARGIN;
    let fits_right = item.left + item.width + TIP_GAP + tw <= vw - TIP_MARGIN;
    let fits_left = item.left - TIP_GAP - tw >= TIP_MARGIN;
    let side = match prefer {
        TipSide::Below if !fits_below && fits_above => TipSide::Above,
        TipSide::Above if !fits_above && fits_below => TipSide::Below,
        TipSide::Right if !fits_right => {
            if fits_left {
                TipSide::Left
            } else {
                TipSide::Below
            }
        }
        TipSide::Left if !fits_left => {
            if fits_right {
                TipSide::Right
            } else {
                TipSide::Below
            }
        }
        other => other,
    };
    let max_x = (vw - TIP_MARGIN - tw).max(TIP_MARGIN);
    let max_y = (vh - TIP_MARGIN - th).max(TIP_MARGIN);
    let clamp = |v: f64, hi: f64| v.clamp(TIP_MARGIN, hi);
    match side {
        TipSide::Below | TipSide::Above => {
            let x = clamp(item.left + item.width / 2.0 - tw / 2.0, max_x);
            let y = if side == TipSide::Below {
                item.top + item.height + TIP_GAP
            } else {
                item.top - TIP_GAP - th
            };
            TipSpot {
                x: x.round(),
                y: clamp(y, max_y).round(),
                side,
            }
        }
        TipSide::Right | TipSide::Left => {
            let x = if side == TipSide::Right {
                item.left + item.width + TIP_GAP
            } else {
                item.left - TIP_GAP - tw
            };
            let y = clamp(item.top + item.height / 2.0 - th / 2.0, max_y);
            TipSpot {
                x: clamp(x, max_x).round(),
                y: y.round(),
                side,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEW: (f64, f64) = (1200.0, 800.0);
    const TIP: (f64, f64) = (120.0, 26.0);

    #[test]
    fn centres_the_tip_under_the_item() {
        let spot = place_tip(
            BoxF {
                left: 500.0,
                top: 4.0,
                width: 32.0,
                height: 32.0,
            },
            TIP,
            VIEW,
            TipSide::Below,
        );
        assert_eq!(spot.side, TipSide::Below);
        assert_eq!(spot.x + 60.0, 516.0);
        assert_eq!(spot.y, 42.0);
    }

    #[test]
    fn keeps_it_inside_at_the_right_edge() {
        let spot = place_tip(
            BoxF {
                left: 1160.0,
                top: 4.0,
                width: 36.0,
                height: 32.0,
            },
            TIP,
            VIEW,
            TipSide::Below,
        );
        assert_eq!(spot.x + 120.0, 1194.0);
    }

    #[test]
    fn goes_above_when_there_is_no_room_below() {
        let spot = place_tip(
            BoxF {
                left: 500.0,
                top: 770.0,
                width: 32.0,
                height: 24.0,
            },
            TIP,
            VIEW,
            TipSide::Below,
        );
        assert_eq!(spot.side, TipSide::Above);
        assert_eq!(spot.y, 738.0);
    }

    #[test]
    fn sits_beside_and_flips_at_the_edge() {
        let row = BoxF {
            left: 8.0,
            top: 200.0,
            width: 220.0,
            height: 34.0,
        };
        let right = place_tip(row, TIP, VIEW, TipSide::Right);
        assert_eq!(right.side, TipSide::Right);
        assert_eq!(right.x, 234.0);
        assert_eq!(right.y + 13.0, 217.0);

        let at_edge = BoxF {
            left: 1000.0,
            ..row
        };
        assert_eq!(
            place_tip(at_edge, TIP, VIEW, TipSide::Right).side,
            TipSide::Left
        );
    }
}

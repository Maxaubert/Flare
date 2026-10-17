//! Lane and drop_slot: where a dragged tab lands (Task 7).

/// One tab's place along the strip, frozen when a drag starts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lane {
    pub left: f64,
    pub width: f64,
    pub mid: f64,
}

/// The slot a carried tab asks for, in `reorder_tabs` terms (the index in the list as it was,
/// so passing lane j going right is j + 1). The LEADING EDGE decides, not the centre (#125):
/// going left the carried tab's left edge must pass a lane's middle, going right its right
/// edge must. An unknown `from` answers `from`.
pub fn drop_slot(lanes: &[Lane], from: usize, dx: f64) -> usize {
    let Some(lane) = lanes.get(from) else {
        return from;
    };
    let left = lane.left + dx;
    let right = left + lane.width;
    let mut slot = from;
    let mut i = from;
    while i > 0 && left < lanes[i - 1].mid {
        i -= 1;
        slot = i;
    }
    if slot != from {
        return slot;
    }
    let mut i = from + 1;
    while i < lanes.len() && right > lanes[i].mid {
        slot = i + 1;
        i += 1;
    }
    slot
}

#[cfg(test)]
pub(crate) fn lanes(widths: &[f64]) -> Vec<Lane> {
    let mut left = 0.0;
    widths
        .iter()
        .map(|&width| {
            let lane = Lane {
                left,
                width,
                mid: left + width / 2.0,
            };
            left += width;
            lane
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stays_put_without_a_move() {
        assert_eq!(drop_slot(&lanes(&[100.0, 100.0, 100.0]), 1, 0.0), 1);
    }

    #[test]
    fn reaches_first_place_when_the_carried_tab_is_wider() {
        assert_eq!(drop_slot(&lanes(&[90.0, 120.0, 100.0]), 1, -90.0), 0);
    }

    #[test]
    fn reaches_first_place_with_equal_widths() {
        assert_eq!(drop_slot(&lanes(&[100.0, 100.0, 100.0]), 1, -100.0), 0);
    }

    #[test]
    fn passes_a_lane_going_left_past_its_middle() {
        let l = lanes(&[100.0, 100.0, 100.0]);
        assert_eq!(drop_slot(&l, 2, -49.0), 2);
        assert_eq!(drop_slot(&l, 2, -51.0), 1);
        assert_eq!(drop_slot(&l, 2, -151.0), 0);
    }

    #[test]
    fn passes_a_lane_going_right_past_its_middle() {
        let l = lanes(&[100.0, 100.0, 100.0]);
        assert_eq!(drop_slot(&l, 0, 49.0), 0);
        assert_eq!(drop_slot(&l, 0, 51.0), 2);
        assert_eq!(drop_slot(&l, 0, 151.0), 3);
    }

    #[test]
    fn reaches_last_place_when_the_carried_tab_is_wider() {
        assert_eq!(drop_slot(&lanes(&[120.0, 100.0, 90.0]), 0, 190.0), 3);
    }

    #[test]
    fn unknown_lane_answers_the_start() {
        assert_eq!(drop_slot(&[], 0, 40.0), 0);
    }
}

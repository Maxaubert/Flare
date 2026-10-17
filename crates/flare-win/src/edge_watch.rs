//! REVEAL_ZONE, edge_near and EdgeWatch: the cursor at the window edge (Task 17).
//!
//! The page cannot see the pointer reliably at the top or left edge of a frameless window: the
//! outermost pixels are Windows' own resize border, and over a drag region the page gets no move
//! at all. So the glue reads the cursor itself, a few times a second and only while the UI has
//! asked, and reports where it is against the content box. All lengths here are DIPs: the glue
//! divides device pixels by the scale.

/// How far inside the content edge counts as "at" it, and how far outside it (the resize border's
/// own band). The ONE table: flare-tabs' reveal rules read the same numbers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Zone {
    pub top: f64,
    pub left: f64,
    pub outside: f64,
}

pub const REVEAL_ZONE: Zone = Zone {
    top: 16.0,
    left: 24.0,
    outside: 8.0,
};

/// How often the glue reads the cursor while the UI has asked.
pub const EDGE_EVERY_MS: u64 = 50;
/// How far outside the content the cursor is "far": past the resize border's band and anything the
/// UI reads (the slid-out sidebar lies inside the content).
pub const EDGE_FAR_DIP: f64 = 48.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointF {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RectF {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Where the cursor is against the content box: `x`, `y` relative to the content origin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EdgeNear {
    pub top: bool,
    pub left: bool,
    pub x: f64,
    pub y: f64,
}

/// The reading for a cursor far from the window: at no edge, and up and to the left of everything,
/// where no reveal zone reaches.
pub const FAR: EdgeNear = EdgeNear {
    top: false,
    left: false,
    x: -100000.0,
    y: -100000.0,
};

/// Pure: where `cursor` is against the content box `content`.
pub fn edge_near(cursor: PointF, content: RectF, zone: Zone) -> EdgeNear {
    let x = cursor.x - content.x;
    let y = cursor.y - content.y;
    let across = x >= -zone.outside && x < content.width + zone.outside;
    let down = y >= -zone.outside && y < content.height + zone.outside;
    EdgeNear {
        top: across && y >= -zone.outside && y < zone.top,
        left: down && x >= -zone.outside && x < zone.left,
        x,
        y,
    }
}

/// Reads the cursor once per `tick` while on, and tells the UI about it only when it changed.
pub struct EdgeWatch {
    content: Box<dyn FnMut() -> Option<RectF>>,
    cursor: Box<dyn FnMut() -> PointF>,
    send: Box<dyn FnMut(EdgeNear)>,
    last: String,
    on: bool,
}

impl EdgeWatch {
    pub fn new(
        content: Box<dyn FnMut() -> Option<RectF>>,
        cursor: Box<dyn FnMut() -> PointF>,
        send: Box<dyn FnMut(EdgeNear)>,
    ) -> Self {
        Self {
            content,
            cursor,
            send,
            last: String::new(),
            on: false,
        }
    }

    /// Turning the watch off clears the last reading, so turning it on again reports the cursor.
    pub fn set(&mut self, on: bool) {
        self.on = on;
        if !on {
            self.last.clear();
        }
    }

    /// One reading. No content rect (minimised, hidden): nothing. Far from the window the UI hears
    /// `FAR` once, then nothing until the cursor comes back; near it, every change is sent.
    pub fn tick(&mut self) {
        if !self.on {
            return;
        }
        let Some(content) = (self.content)() else {
            return;
        };
        let near = edge_near((self.cursor)(), content, REVEAL_ZONE);
        let far = near.x < -EDGE_FAR_DIP
            || near.y < -EDGE_FAR_DIP
            || near.x >= content.width + EDGE_FAR_DIP
            || near.y >= content.height + EDGE_FAR_DIP;
        let key = if far {
            "far".to_string()
        } else {
            format!("{}|{}", near.x.round(), near.y.round())
        };
        if key == self.last {
            return;
        }
        self.last = key;
        (self.send)(if far { FAR } else { near });
    }
}

/// The cursor in physical pixels (GetCursorPos), or None when the call fails.
pub fn real_cursor_phys() -> Option<(i32, i32)> {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut point = POINT { x: 0, y: 0 };
    // SAFETY: `point` is a valid, writable POINT for the duration of the call.
    let ok = unsafe { GetCursorPos(&mut point) };
    (ok != 0).then_some((point.x, point.y))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    const CONTENT: RectF = RectF {
        x: 100.0,
        y: 200.0,
        width: 800.0,
        height: 600.0,
    };

    fn p(x: f64, y: f64) -> PointF {
        PointF { x, y }
    }

    #[test]
    fn at_the_top_inside_the_band_and_in_the_border_above() {
        let at = |x, y| edge_near(p(x, y), CONTENT, REVEAL_ZONE).top;
        assert!(at(500.0, 200.0)); // content-relative y = 0
        assert!(at(500.0, 215.0)); // y = 15, inside the 16 px zone
        assert!(at(500.0, 192.0)); // y = -8, the resize border's band
        assert!(!at(500.0, 216.0)); // y = 16, below the zone
        assert!(!at(500.0, 191.0)); // y = -9, outside the band
    }

    #[test]
    fn not_at_the_top_beside_the_window() {
        assert!(!edge_near(p(91.0, 202.0), CONTENT, REVEAL_ZONE).top);
        assert!(!edge_near(p(908.0, 202.0), CONTENT, REVEAL_ZONE).top);
    }

    #[test]
    fn at_the_left_edge_along_the_window_only() {
        let at = |x, y| edge_near(p(x, y), CONTENT, REVEAL_ZONE).left;
        assert!(at(100.0, 500.0));
        assert!(at(92.0, 500.0));
        assert!(!at(124.0, 500.0));
        assert!(!at(100.0, 808.0));
    }

    #[test]
    fn says_where_the_cursor_is() {
        let near = edge_near(p(350.0, 203.0), CONTENT, REVEAL_ZONE);
        assert_eq!(near.x, 250.0);
        assert_eq!(near.y, 3.0);
    }

    /// A watch whose readings come from a queue; `sent` collects what the UI hears.
    fn watch_over(
        readings: Rc<RefCell<Vec<PointF>>>,
        sent: Rc<RefCell<Vec<EdgeNear>>>,
    ) -> EdgeWatch {
        let content_src = Rc::new(RefCell::new(Some(CONTENT)));
        let cursor_src = readings;
        EdgeWatch::new(
            Box::new(move || *content_src.borrow()),
            Box::new(move || {
                let mut q = cursor_src.borrow_mut();
                if q.len() > 1 { q.remove(0) } else { q[0] }
            }),
            Box::new(move |near| sent.borrow_mut().push(near)),
        )
    }

    #[test]
    fn far_from_the_window_is_told_once() {
        let mut readings = vec![p(500.0, 202.0)];
        for i in 0..20 {
            readings.push(p(2000.0 + f64::from(i) * 7.0, 900.0 + f64::from(i)));
        }
        readings.push(p(500.0, 203.0));
        let sent = Rc::new(RefCell::new(Vec::new()));
        let mut watch = watch_over(Rc::new(RefCell::new(readings)), sent.clone());
        watch.set(true);
        for _ in 0..22 {
            watch.tick();
        }
        assert_eq!(
            *sent.borrow(),
            vec![
                edge_near(p(500.0, 202.0), CONTENT, REVEAL_ZONE),
                FAR,
                edge_near(p(500.0, 203.0), CONTENT, REVEAL_ZONE),
            ]
        );
    }

    #[test]
    fn near_moves_are_all_followed() {
        let readings = vec![p(53.0, 400.0), p(92.0, 400.0), p(300.0, 198.0)];
        let sent = Rc::new(RefCell::new(Vec::new()));
        let mut watch = watch_over(Rc::new(RefCell::new(readings.clone())), sent.clone());
        watch.set(true);
        for _ in 0..readings.len() {
            watch.tick();
        }
        assert_eq!(
            *sent.borrow(),
            readings
                .iter()
                .map(|&r| edge_near(r, CONTENT, REVEAL_ZONE))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    #[allow(clippy::assertions_on_constants)] // the table is const: the point is the values
    fn far_is_at_no_edge_and_in_no_zone() {
        assert!(!FAR.top && !FAR.left);
        assert!(FAR.x < -REVEAL_ZONE.outside);
        assert!(FAR.y < -REVEAL_ZONE.outside);
    }

    #[test]
    fn no_content_rect_sends_nothing_and_off_clears() {
        let content = Rc::new(RefCell::new(None));
        let sent = Rc::new(RefCell::new(Vec::new()));
        let content_src = content.clone();
        let mut watch = EdgeWatch::new(
            Box::new(move || *content_src.borrow()),
            Box::new(|| PointF { x: 500.0, y: 202.0 }),
            {
                let sent = sent.clone();
                Box::new(move |near| sent.borrow_mut().push(near))
            },
        );
        watch.set(true);
        watch.tick();
        assert!(sent.borrow().is_empty());

        *content.borrow_mut() = Some(CONTENT);
        watch.tick();
        assert_eq!(sent.borrow().len(), 1);

        // Off clears the last reading, so the same reading is sent again after turning on.
        watch.set(false);
        watch.set(true);
        watch.tick();
        assert_eq!(sent.borrow().len(), 2);
    }

    #[test]
    fn the_edge_every_ms_is_fifty() {
        assert_eq!(EDGE_EVERY_MS, 50);
    }
}

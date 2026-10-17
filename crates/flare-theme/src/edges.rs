//! Window edge words and the edge alpha and border step tables (Task 6).
//! flare-theme, see the roadmap crate table for its role. Pure: no runtime dependencies.

/// How strongly the window draws its edges. Hairline is the default.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WindowEdges {
    None,
    Faint,
    Hairline,
    Solid,
}

/// Weakest to strongest: the order the control offers.
pub const WINDOW_EDGES: [WindowEdges; 4] = [
    WindowEdges::None,
    WindowEdges::Faint,
    WindowEdges::Hairline,
    WindowEdges::Solid,
];

pub const DEFAULT_WINDOW_EDGES: WindowEdges = WindowEdges::Hairline;

impl WindowEdges {
    /// The stored word: "none", "faint", "hairline" or "solid".
    pub fn word(self) -> &'static str {
        match self {
            WindowEdges::None => "none",
            WindowEdges::Faint => "faint",
            WindowEdges::Hairline => "hairline",
            WindowEdges::Solid => "solid",
        }
    }

    /// The edge for one of the four words, or None for anything else.
    pub fn from_word(w: &str) -> Option<WindowEdges> {
        WINDOW_EDGES.iter().copied().find(|e| e.word() == w)
    }
}

/// Anything that is not one of the four words (including empty or absent) is the default.
pub fn valid_window_edges(v: Option<&str>) -> WindowEdges {
    v.and_then(WindowEdges::from_word)
        .unwrap_or(DEFAULT_WINDOW_EDGES)
}

/// A dark and a light value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shades {
    pub dark: f64,
    pub light: f64,
}

/// Percent of the ink: the chrome's divider line and the list's line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EdgeAlpha {
    pub divider: Shades,
    pub line: Shades,
}

pub fn edge_alpha(e: WindowEdges) -> EdgeAlpha {
    let (dd, dl, ld, ll) = match e {
        WindowEdges::Hairline => (7.0, 15.0, 9.0, 17.0),
        WindowEdges::Faint => (2.2, 5.0, 3.0, 6.5),
        WindowEdges::Solid => (16.0, 26.0, 18.0, 28.0),
        WindowEdges::None => (0.0, 0.0, 0.0, 0.0),
    };
    EdgeAlpha {
        divider: Shades {
            dark: dd,
            light: dl,
        },
        line: Shades {
            dark: ld,
            light: ll,
        },
    }
}

/// The DWM border step, or None for no border (only WindowEdges::None).
pub fn edge_border_step(e: WindowEdges) -> Option<Shades> {
    match e {
        WindowEdges::Hairline => Some(Shades {
            dark: 0.13,
            light: 0.22,
        }),
        WindowEdges::Faint => Some(Shades {
            dark: 0.045,
            light: 0.075,
        }),
        WindowEdges::Solid => Some(Shades {
            dark: 0.30,
            light: 0.44,
        }),
        WindowEdges::None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Border step as a number, None counting as 0 (PT's `?? 0`).
    fn step_or_zero(e: WindowEdges, dark: bool) -> f64 {
        match edge_border_step(e) {
            Some(s) => {
                if dark {
                    s.dark
                } else {
                    s.light
                }
            }
            None => 0.0,
        }
    }

    #[test]
    fn edges_are_offered_weakest_to_strongest_by_every_number() {
        for pair in WINDOW_EDGES.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let (ca, cb) = (edge_alpha(a), edge_alpha(b));
            assert!(
                ca.divider.dark < cb.divider.dark,
                "{a:?} < {b:?} divider dark"
            );
            assert!(
                ca.divider.light < cb.divider.light,
                "{a:?} < {b:?} divider light"
            );
            assert!(ca.line.dark < cb.line.dark, "{a:?} < {b:?} line dark");
            assert!(ca.line.light < cb.line.light, "{a:?} < {b:?} line light");
            for dark in [true, false] {
                assert!(
                    step_or_zero(a, dark) < step_or_zero(b, dark),
                    "{a:?} < {b:?} border dark={dark}"
                );
            }
        }
    }

    #[test]
    fn the_first_edge_is_none_and_only_none_has_no_border() {
        assert_eq!(WINDOW_EDGES[0], WindowEdges::None);
        for e in WINDOW_EDGES {
            assert_eq!(
                edge_border_step(e).is_none(),
                e == WindowEdges::None,
                "{e:?}"
            );
        }
    }

    #[test]
    fn default_to_hairline_and_read_anything_else_as_it() {
        assert_eq!(DEFAULT_WINDOW_EDGES, WindowEdges::Hairline);
        for v in [
            Some("strong"),
            Some(""),
            None,
            Some("dotted"),
            Some("HAIRLINE"),
        ] {
            assert_eq!(valid_window_edges(v), WindowEdges::Hairline, "{v:?}");
        }
        for e in WINDOW_EDGES {
            assert_eq!(valid_window_edges(Some(e.word())), e);
            assert_eq!(WindowEdges::from_word(e.word()), Some(e));
        }
        assert_eq!(WindowEdges::from_word("strong"), None);
    }

    #[test]
    fn numbers_match_the_prism_terminal_tables() {
        let h = edge_alpha(WindowEdges::Hairline);
        assert_eq!((h.divider.dark, h.divider.light), (7.0, 15.0));
        assert_eq!((h.line.dark, h.line.light), (9.0, 17.0));
        let f = edge_alpha(WindowEdges::Faint);
        assert_eq!((f.divider.dark, f.divider.light), (2.2, 5.0));
        assert_eq!((f.line.dark, f.line.light), (3.0, 6.5));
        let s = edge_alpha(WindowEdges::Solid);
        assert_eq!((s.divider.dark, s.divider.light), (16.0, 26.0));
        assert_eq!((s.line.dark, s.line.light), (18.0, 28.0));
        let n = edge_alpha(WindowEdges::None);
        assert_eq!(
            (n.divider.dark, n.divider.light, n.line.dark, n.line.light),
            (0.0, 0.0, 0.0, 0.0)
        );
        let steps = [
            (WindowEdges::Hairline, 0.13, 0.22),
            (WindowEdges::Faint, 0.045, 0.075),
            (WindowEdges::Solid, 0.30, 0.44),
        ];
        for (e, dark, light) in steps {
            assert_eq!(edge_border_step(e), Some(Shades { dark, light }), "{e:?}");
        }
    }
}

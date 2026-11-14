//! The settings icons, as path data (M7 Task 8).
//!
//! Source: Prism Terminal's `core/renderer/settings/layout/icons.ts`. Every path is a 24 unit
//! stroke path; a row tile draws it at 16 px with `ICON_STROKE`, the rail at 17 px.

/// The stroke width of a row tile icon, in logical px at 16 px.
pub const ICON_STROKE: f32 = 1.7;

/// Every settings icon as `(name, svg path data on a 24 unit box)`, in Prism Terminal's order.
pub const SETTING_ICONS: &[(&str, &str)] = &[
    (
        "appearance",
        "M12 3a9 9 0 1 0 0 18 3 3 0 0 0 0-6 3 3 0 0 1 0-6h3a6 6 0 0 0-3-6ZM7.5 10.5h.01M10 7h.01M14 7h.01",
    ),
    ("terminal", "M4 5h16v14H4zM7.5 9.5l3 2.5-3 2.5M13 15h4"),
    (
        "agents",
        "M12 3l1.9 5.1L19 10l-5.1 1.9L12 17l-1.9-5.1L5 10l5.1-1.9zM18.5 15.5l.8 2.2 2.2.8-2.2.8-.8 2.2-.8-2.2-2.2-.8 2.2-.8z",
    ),
    (
        "dictation",
        "M12 3a3 3 0 0 0-3 3v5a3 3 0 0 0 6 0V6a3 3 0 0 0-3-3ZM5.5 11a6.5 6.5 0 0 0 13 0M12 17.5V21",
    ),
    (
        "about",
        "M12 8h.01M11 12h1v4h1M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18Z",
    ),
    ("brush", "M4 20h4L19 9a2.8 2.8 0 0 0-4-4L4 16zM13.5 6.5l4 4"),
    ("viewer", "M4 5h16v14H4zM8 15l3-3 2 2 3-3 2 2"),
    (
        "accent",
        "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18zM12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8z",
    ),
    (
        "font",
        "M4 19l5.5-14h1L16 19M6.3 14h7.4M17 19v-6a2.5 2.5 0 0 1 4 0v6",
    ),
    (
        "size",
        "M3 8V6h10v2M8 6v12M6 18h4M14 13v-1h7v1M17.5 12v6M16 18h3",
    ),
    ("titlebar", "M4 5h16v14H4zM4 9h16"),
    ("tabs", "M3 10h18v9H3zM4 10V7h6v3M12 10V7h7v3"),
    (
        "sidebar",
        "M4 5h16v14H4zM9.5 5v14M6 9h1.5M6 12h1.5M6 15h1.5",
    ),
    ("edges", "M4 9V4h5M15 4h5v5M20 15v5h-5M9 20H4v-5"),
    (
        "newtab",
        "M3 6.5A1.5 1.5 0 0 1 4.5 5H9l2 2h8.5A1.5 1.5 0 0 1 21 8.5v9a1.5 1.5 0 0 1-1.5 1.5h-15A1.5 1.5 0 0 1 3 17.5zM12 10v6M9 13h6",
    ),
    ("menu", "M5 4h14v16H5zM8 8h8M8 12h8M8 16h5"),
    ("shell", "M5 7l5 5-5 5M12 17h7"),
    (
        "glass",
        "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18zM12 3v18M12 7.5h6.5M12 12h9M12 16.5h6.5",
    ),
    (
        "help",
        "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18zM9.5 9.5a2.5 2.5 0 1 1 3.5 2.3c-.6.3-1 .8-1 1.5v.4M12 16.5h.01",
    ),
    ("search", "M11 4a7 7 0 1 0 0 14 7 7 0 0 0 0-14zM20 20l-4-4"),
    ("working", "M3 12h4l3-7 4 14 3-7h4"),
    (
        "done",
        "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18zM8 12.5l3 3 5-6",
    ),
    (
        "ask",
        "M21 12a8 8 0 0 1-11.6 7.1L4 20.5l1.4-4.9A8 8 0 1 1 21 12zM10 10a2 2 0 1 1 2.8 1.8c-.5.2-.8.7-.8 1.2M12 15.5h.01",
    ),
    (
        "fail",
        "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18zM12 7.5v5.5M12 16.5h.01",
    ),
    (
        "taskbar",
        "M3 15h18v5H3zM6.5 17.5h.01M10 17.5h4M17 4.5a2.5 2.5 0 1 0 0 5 2.5 2.5 0 0 0 0-5z",
    ),
    ("hook", "M8 3v6a4 4 0 0 0 8 0V3M12 13v3a5 5 0 0 1-5 5"),
    (
        "rainbow",
        "M3 17a9 9 0 0 1 18 0M6.5 17a5.5 5.5 0 0 1 11 0M10 17a2 2 0 0 1 4 0",
    ),
    ("prompt", "M3 6h11l5 6-5 6H3l5-6z"),
    (
        "hand",
        "M8 13V5.5a1.5 1.5 0 0 1 3 0V12M11 11.5V4.5a1.5 1.5 0 0 1 3 0V12M14 11.5V6.5a1.5 1.5 0 0 1 3 0V14a6 6 0 0 1-6 6h-1a5 5 0 0 1-4.2-2.3L4.5 14a1.5 1.5 0 0 1 2.5-1.6L8 14",
    ),
    ("key", "M3 6h18v12H3zM7 10h.01M11 10h.01M15 10h.01M7 14h10"),
    (
        "mic",
        "M12 3a3 3 0 0 0-3 3v5a3 3 0 0 0 6 0V6a3 3 0 0 0-3-3ZM5.5 11a6.5 6.5 0 0 0 13 0M12 17.5V21",
    ),
    (
        "globe",
        "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18zM3 12h18M12 3c2.5 2.7 3.8 5.7 3.8 9s-1.3 6.3-3.8 9c-2.5-2.7-3.8-5.7-3.8-9S9.5 5.7 12 3z",
    ),
    (
        "pause",
        "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18zM10 9v6M14 9v6",
    ),
    (
        "sound",
        "M4 9.5v5h4l5 4v-13l-5 4zM16.5 9a4 4 0 0 1 0 6M19 6.5a7.5 7.5 0 0 1 0 11",
    ),
    ("download", "M12 4v11M7 10.5l5 5 5-5M5 20h14"),
    (
        "chip",
        "M7 7h10v10H7zM10 3v4M14 3v4M10 17v4M14 17v4M3 10h4M3 14h4M17 10h4M17 14h4",
    ),
    ("version", "M20 12l-8 8-9-9V3h8zM7.5 7.5h.01"),
    ("diagnostics", "M4 5h16v14H4zM6.5 12h3l1.5-3 2 6 1.5-3h3"),
    ("log", "M6 3h9l4 4v14H6zM14 3v5h5M9 12h7M9 16h7"),
    (
        "folder",
        "M3 6.5A1.5 1.5 0 0 1 4.5 5H9l2 2h8.5A1.5 1.5 0 0 1 21 8.5v9a1.5 1.5 0 0 1-1.5 1.5h-15A1.5 1.5 0 0 1 3 17.5z",
    ),
    ("flag", "M5 21V4M5 4h12l-2.5 4.5L17 13H5"),
    ("warn", "M12 4l9 16H3zM12 10v4M12 17h.01"),
    ("x", "M6 6l12 12M18 6L6 18"),
];

/// Icons only an icon button draws (not in Prism Terminal's set), same 24 unit stroke paths.
pub const BUTTON_ICONS: &[(&str, &str)] = &[
    (
        "trash",
        "M4 7h16M10 11v6M14 11v6M6 7l1 12a2 2 0 0 0 2 2h6a2 2 0 0 0 2-2l1-12M9 7V4h6v3",
    ),
    (
        "info",
        "M12 8h.01M11 12h1v4h1M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18Z",
    ),
];

/// The path data of an icon button's icon: a button icon, else a settings icon.
pub fn button_icon_path(name: &str) -> Option<&'static str> {
    BUTTON_ICONS
        .iter()
        .find(|(icon, _)| *icon == name)
        .map(|(_, path)| *path)
        .or_else(|| icon_path(name))
}

/// Flare's own icons beyond Prism Terminal's set, same 24 unit stroke paths: the Machines page's rail icon (a screen
/// on its stand) and the plus of its "Add a machine" row (remote sessions PR 7).
pub const FLARE_ICONS: &[(&str, &str)] = &[
    ("machines", "M3 5h18v11H3zM12 16v4M8 20h8"),
    ("plus", "M12 5v14M5 12h14"),
    // The app icon picker's row (rule 1230): an app tile with a flame in it.
    (
        "app-icon",
        "M7 3h10a4 4 0 0 1 4 4v10a4 4 0 0 1-4 4H7a4 4 0 0 1-4-4V7a4 4 0 0 1 4-4zM12 7.5c2.2 2.3 3.5 3.9 3.5 6a3.5 3.5 0 0 1-7 0c0-1.5.8-2.6 1.8-3.6.2 1 .7 1.6 1.3 1.9-.4-1.4-.3-2.8.4-4.3z",
    ),
    // The caret rows (rule 1233): a terminal with a caret bar after its prompt.
    ("caret", "M4 5h16v14H4zM7.5 9.5l3 2.5-3 2.5M14 9v6"),
    // The Computer sharing row (rule 1252, the approved mockup's icon): a screen on its stand and a phone beside it.
    (
        "share",
        "M3 4h11a1 1 0 0 1 1 1v8a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1zM5 18h7M8.5 14v4M18 8h3a1 1 0 0 1 1 1v8a1 1 0 0 1-1 1h-3a1 1 0 0 1-1-1V9a1 1 0 0 1 1-1z",
    ),
];

/// The path data of a named icon, or `None` when the name is not in the set.
pub fn icon_path(name: &str) -> Option<&'static str> {
    SETTING_ICONS
        .iter()
        .chain(FLARE_ICONS.iter())
        .find(|(icon, _)| *icon == name)
        .map(|(_, path)| *path)
}

/// Whether a name is in the set. Lists name their icons as plain strings.
pub fn is_setting_icon(name: &str) -> bool {
    icon_path(name).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Prism Terminal's `SETTING_ICONS`, copied entry for entry from `icons.ts`.
    const TS_ICONS: &[(&str, &str)] = &[
        (
            "appearance",
            "M12 3a9 9 0 1 0 0 18 3 3 0 0 0 0-6 3 3 0 0 1 0-6h3a6 6 0 0 0-3-6ZM7.5 10.5h.01M10 7h.01M14 7h.01",
        ),
        ("terminal", "M4 5h16v14H4zM7.5 9.5l3 2.5-3 2.5M13 15h4"),
        (
            "agents",
            "M12 3l1.9 5.1L19 10l-5.1 1.9L12 17l-1.9-5.1L5 10l5.1-1.9zM18.5 15.5l.8 2.2 2.2.8-2.2.8-.8 2.2-.8-2.2-2.2-.8 2.2-.8z",
        ),
        (
            "dictation",
            "M12 3a3 3 0 0 0-3 3v5a3 3 0 0 0 6 0V6a3 3 0 0 0-3-3ZM5.5 11a6.5 6.5 0 0 0 13 0M12 17.5V21",
        ),
        (
            "about",
            "M12 8h.01M11 12h1v4h1M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18Z",
        ),
        ("brush", "M4 20h4L19 9a2.8 2.8 0 0 0-4-4L4 16zM13.5 6.5l4 4"),
        ("viewer", "M4 5h16v14H4zM8 15l3-3 2 2 3-3 2 2"),
        (
            "accent",
            "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18zM12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8z",
        ),
        (
            "font",
            "M4 19l5.5-14h1L16 19M6.3 14h7.4M17 19v-6a2.5 2.5 0 0 1 4 0v6",
        ),
        (
            "size",
            "M3 8V6h10v2M8 6v12M6 18h4M14 13v-1h7v1M17.5 12v6M16 18h3",
        ),
        ("titlebar", "M4 5h16v14H4zM4 9h16"),
        ("tabs", "M3 10h18v9H3zM4 10V7h6v3M12 10V7h7v3"),
        (
            "sidebar",
            "M4 5h16v14H4zM9.5 5v14M6 9h1.5M6 12h1.5M6 15h1.5",
        ),
        ("edges", "M4 9V4h5M15 4h5v5M20 15v5h-5M9 20H4v-5"),
        (
            "newtab",
            "M3 6.5A1.5 1.5 0 0 1 4.5 5H9l2 2h8.5A1.5 1.5 0 0 1 21 8.5v9a1.5 1.5 0 0 1-1.5 1.5h-15A1.5 1.5 0 0 1 3 17.5zM12 10v6M9 13h6",
        ),
        ("menu", "M5 4h14v16H5zM8 8h8M8 12h8M8 16h5"),
        ("shell", "M5 7l5 5-5 5M12 17h7"),
        (
            "glass",
            "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18zM12 3v18M12 7.5h6.5M12 12h9M12 16.5h6.5",
        ),
        (
            "help",
            "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18zM9.5 9.5a2.5 2.5 0 1 1 3.5 2.3c-.6.3-1 .8-1 1.5v.4M12 16.5h.01",
        ),
        ("search", "M11 4a7 7 0 1 0 0 14 7 7 0 0 0 0-14zM20 20l-4-4"),
        ("working", "M3 12h4l3-7 4 14 3-7h4"),
        (
            "done",
            "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18zM8 12.5l3 3 5-6",
        ),
        (
            "ask",
            "M21 12a8 8 0 0 1-11.6 7.1L4 20.5l1.4-4.9A8 8 0 1 1 21 12zM10 10a2 2 0 1 1 2.8 1.8c-.5.2-.8.7-.8 1.2M12 15.5h.01",
        ),
        (
            "fail",
            "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18zM12 7.5v5.5M12 16.5h.01",
        ),
        (
            "taskbar",
            "M3 15h18v5H3zM6.5 17.5h.01M10 17.5h4M17 4.5a2.5 2.5 0 1 0 0 5 2.5 2.5 0 0 0 0-5z",
        ),
        ("hook", "M8 3v6a4 4 0 0 0 8 0V3M12 13v3a5 5 0 0 1-5 5"),
        (
            "rainbow",
            "M3 17a9 9 0 0 1 18 0M6.5 17a5.5 5.5 0 0 1 11 0M10 17a2 2 0 0 1 4 0",
        ),
        ("prompt", "M3 6h11l5 6-5 6H3l5-6z"),
        (
            "hand",
            "M8 13V5.5a1.5 1.5 0 0 1 3 0V12M11 11.5V4.5a1.5 1.5 0 0 1 3 0V12M14 11.5V6.5a1.5 1.5 0 0 1 3 0V14a6 6 0 0 1-6 6h-1a5 5 0 0 1-4.2-2.3L4.5 14a1.5 1.5 0 0 1 2.5-1.6L8 14",
        ),
        ("key", "M3 6h18v12H3zM7 10h.01M11 10h.01M15 10h.01M7 14h10"),
        (
            "mic",
            "M12 3a3 3 0 0 0-3 3v5a3 3 0 0 0 6 0V6a3 3 0 0 0-3-3ZM5.5 11a6.5 6.5 0 0 0 13 0M12 17.5V21",
        ),
        (
            "globe",
            "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18zM3 12h18M12 3c2.5 2.7 3.8 5.7 3.8 9s-1.3 6.3-3.8 9c-2.5-2.7-3.8-5.7-3.8-9S9.5 5.7 12 3z",
        ),
        (
            "pause",
            "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18zM10 9v6M14 9v6",
        ),
        (
            "sound",
            "M4 9.5v5h4l5 4v-13l-5 4zM16.5 9a4 4 0 0 1 0 6M19 6.5a7.5 7.5 0 0 1 0 11",
        ),
        ("download", "M12 4v11M7 10.5l5 5 5-5M5 20h14"),
        (
            "chip",
            "M7 7h10v10H7zM10 3v4M14 3v4M10 17v4M14 17v4M3 10h4M3 14h4M17 10h4M17 14h4",
        ),
        ("version", "M20 12l-8 8-9-9V3h8zM7.5 7.5h.01"),
        ("diagnostics", "M4 5h16v14H4zM6.5 12h3l1.5-3 2 6 1.5-3h3"),
        ("log", "M6 3h9l4 4v14H6zM14 3v5h5M9 12h7M9 16h7"),
        (
            "folder",
            "M3 6.5A1.5 1.5 0 0 1 4.5 5H9l2 2h8.5A1.5 1.5 0 0 1 21 8.5v9a1.5 1.5 0 0 1-1.5 1.5h-15A1.5 1.5 0 0 1 3 17.5z",
        ),
        ("flag", "M5 21V4M5 4h12l-2.5 4.5L17 13H5"),
        ("warn", "M12 4l9 16H3zM12 10v4M12 17h.01"),
        ("x", "M6 6l12 12M18 6L6 18"),
    ];

    /// The number of keys in Prism Terminal's record.
    const TS_COUNT: usize = 43;

    #[test]
    fn every_name_of_the_ts_record_is_here_with_its_exact_path() {
        assert_eq!(SETTING_ICONS.len(), TS_ICONS.len());
        for (i, (name, path)) in TS_ICONS.iter().enumerate() {
            assert_eq!(SETTING_ICONS[i], (*name, *path), "entry {i} ({name})");
            assert_eq!(icon_path(name), Some(*path), "lookup of {name}");
        }
    }

    #[test]
    fn names_are_unique_and_the_count_is_43() {
        assert_eq!(SETTING_ICONS.len(), TS_COUNT);
        let mut names: Vec<&str> = SETTING_ICONS.iter().map(|(n, _)| *n).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(names.len(), before, "a duplicate icon name");
    }

    #[test]
    fn a_name_that_is_not_listed_is_not_an_icon() {
        assert!(!is_setting_icon("nope"));
        assert_eq!(icon_path("nope"), None);
        assert!(is_setting_icon("warn"));
        assert!(is_setting_icon("appearance"));
    }

    /// Flare's own icons are found like the set's, never take a name of the set, and hold only path characters.
    #[test]
    fn flare_icons_add_to_the_set_without_shadowing_it() {
        for (name, path) in FLARE_ICONS {
            assert!(!SETTING_ICONS.iter().any(|(n, _)| n == name), "{name}");
            assert_eq!(icon_path(name), Some(*path));
            assert!(
                path.chars()
                    .all(|c| "MmLlHhVvCcSsQqTtAaZz0123456789 .,-".contains(c))
            );
        }
        assert!(is_setting_icon("machines") && is_setting_icon("plus"));
    }

    #[test]
    fn paths_hold_only_svg_path_characters() {
        let allowed = "MmLlHhVvCcSsQqTtAaZz0123456789 .,-";
        for (name, path) in SETTING_ICONS {
            assert!(!path.is_empty(), "{name} has an empty path");
            for ch in path.chars() {
                assert!(allowed.contains(ch), "{name} holds {ch:?}");
            }
        }
    }

    #[test]
    fn the_stroke_is_the_row_tile_stroke() {
        assert_eq!(ICON_STROKE, 1.7);
    }
}

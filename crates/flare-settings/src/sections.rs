//! The settings sections, by id and heading, as Prism Terminal's core records them (M7 Task 9).
//! A section is drawn the same way everywhere; which page holds it is decided in `pages`.

/// The core's sections (`sectionIds.ts`): id and heading. `dictation` and `diagnostics`
/// have no heading; their page's own title says what they are.
pub const SECTIONS: &[(&str, &str)] = &[
    ("shell", "Shell"),
    ("text", "Text"),
    ("theme", "Theme"),
    ("help", "Command help"),
    ("marks", "Tab marks"),
    ("claude", "Claude Code"),
    ("colours", "Mark colours"),
    ("dictation", ""),
    ("listening", "Listening"),
    ("while", "While dictating"),
    ("models", "Speech models"),
    ("gpu", "GPU acceleration"),
    ("anywhere", "From any app"),
    ("diagnostics", ""),
];

/// This app's own sections (`appOptions.ts`, `APP_SECTIONS`). `about` has no heading.
pub const APP_SECTIONS: &[(&str, &str)] = &[
    ("window", "Window"),
    ("opening", "Opening terminals"),
    ("keys", "Keys"),
    ("links", "Links"),
    ("about", ""),
];

/// The heading of a section, core or app. `None` for an id that is no section; an empty
/// heading (`dictation`, `diagnostics`, `about`) is `Some("")`.
pub fn section_title(id: &str) -> Option<&'static str> {
    SECTIONS
        .iter()
        .chain(APP_SECTIONS.iter())
        .find(|(key, _)| *key == id)
        .map(|(_, title)| *title)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_titles_are_the_ts_record() {
        assert_eq!(
            SECTIONS,
            &[
                ("shell", "Shell"),
                ("text", "Text"),
                ("theme", "Theme"),
                ("help", "Command help"),
                ("marks", "Tab marks"),
                ("claude", "Claude Code"),
                ("colours", "Mark colours"),
                ("dictation", ""),
                ("listening", "Listening"),
                ("while", "While dictating"),
                ("models", "Speech models"),
                ("gpu", "GPU acceleration"),
                ("anywhere", "From any app"),
                ("diagnostics", ""),
            ]
        );
        assert_eq!(section_title("dictation"), Some(""));
        assert_eq!(section_title("diagnostics"), Some(""));
        assert_eq!(section_title("help"), Some("Command help"));
        assert_eq!(section_title("window"), Some("Window"));
        assert_eq!(section_title("opening"), Some("Opening terminals"));
        assert_eq!(section_title("keys"), Some("Keys"));
        assert_eq!(section_title("about"), Some(""));
        assert_eq!(section_title("nope"), None);
    }

    #[test]
    fn section_ids_are_unique() {
        let mut ids: Vec<&str> = SECTIONS
            .iter()
            .chain(APP_SECTIONS.iter())
            .map(|(id, _)| *id)
            .collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len());
    }
}

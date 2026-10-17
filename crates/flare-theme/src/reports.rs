//! Theme reports, theme push and the width nudge (Task 11b). Source: PT `core/renderer/lib/termReplies.ts`
//! (`themeReports`, `noteThemeMode`, `themePush`, `nudgeWidths`) and its tests. The bytes are not built
//! here: `theme_push` returns the mode to report, and the caller turns it into bytes with
//! `flare_term::replies::dsr_theme_reply`.

use crate::marks::Mode;

/// Whether the program asked for theme reports (?2031h), and the last mode it was told.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThemeReports {
    pub on: bool,
    pub last: Option<Mode>,
}

pub const THEME_REPORTS_OFF: ThemeReports = ThemeReports {
    on: false,
    last: None,
};

/// The program turned reports on or off; turning on remembers `now` when nothing was told yet.
pub fn theme_reports(s: ThemeReports, on: bool, now: Mode) -> ThemeReports {
    ThemeReports {
        on,
        last: if on { s.last.or(Some(now)) } else { s.last },
    }
}

/// A query answered (OSC 11, `?996n`): the program now knows `mode`.
pub fn note_theme_mode(s: ThemeReports, mode: Mode) -> ThemeReports {
    ThemeReports {
        last: Some(mode),
        ..s
    }
}

/// After a theme change: `Some(mode)` to report when reports are on and `mode` differs from the last told.
pub fn theme_push(s: ThemeReports, mode: Mode) -> (Option<Mode>, ThemeReports) {
    if !s.on || s.last == Some(mode) {
        return (None, s);
    }
    (
        Some(mode),
        ThemeReports {
            last: Some(mode),
            ..s
        },
    )
}

/// After a push, the resize nudge waits this long for the program to re-query and switch.
pub const NUDGE_AFTER_MS: u64 = 250;
/// How long the one-column-narrower size stands.
pub const NUDGE_HOLD_MS: u64 = 80;

/// The two widths of a nudge (one narrower, then back), or None for a terminal under 3 columns.
pub fn nudge_widths(cols: i64) -> Option<(i64, i64)> {
    (cols >= 3).then_some((cols - 1, cols))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ON_DARK: ThemeReports = ThemeReports {
        on: true,
        last: Some(Mode::Dark),
    };

    #[test]
    fn push_sends_nothing_while_the_program_has_not_asked() {
        let (send, s) = theme_push(THEME_REPORTS_OFF, Mode::Light);
        assert_eq!(send, None);
        assert_eq!(s, THEME_REPORTS_OFF);
    }

    #[test]
    fn push_sends_a_change_once_and_nothing_for_the_same_mode() {
        let s = theme_reports(THEME_REPORTS_OFF, true, Mode::Dark);
        assert_eq!(s, ON_DARK);
        let (send, s) = theme_push(s, Mode::Dark);
        assert_eq!(send, None);
        let (send, s) = theme_push(s, Mode::Light);
        assert_eq!(send, Some(Mode::Light));
        assert_eq!(s.last, Some(Mode::Light));
        let (send, s) = theme_push(s, Mode::Light);
        assert_eq!(send, None);
        let (send, _) = theme_push(s, Mode::Dark);
        assert_eq!(send, Some(Mode::Dark));
    }

    #[test]
    fn push_stops_after_2031l_and_keeps_what_was_told() {
        let s = theme_reports(ON_DARK, false, Mode::Light);
        assert!(!s.on);
        assert_eq!(s.last, Some(Mode::Dark));
        let (send, s2) = theme_push(s, Mode::Light);
        assert_eq!(send, None);
        assert_eq!(s2, s);
        // Turning it on again: `last` is still Dark, so a Light ground pushes Light.
        let s = theme_reports(s, true, Mode::Light);
        let (send, _) = theme_push(s, Mode::Light);
        assert_eq!(send, Some(Mode::Light));
    }

    #[test]
    fn an_answered_query_counts_as_told() {
        let s = note_theme_mode(ON_DARK, Mode::Light);
        assert_eq!(s.last, Some(Mode::Light));
        let (send, _) = theme_push(s, Mode::Light);
        assert_eq!(send, None);
    }

    #[test]
    fn the_nudge_is_one_column_narrower_and_back() {
        assert_eq!(nudge_widths(120), Some((119, 120)));
        assert_eq!(NUDGE_AFTER_MS, 250);
        assert_eq!(NUDGE_HOLD_MS, 80);
    }

    #[test]
    fn the_nudge_is_nothing_for_a_terminal_too_narrow_or_unmeasured() {
        assert_eq!(nudge_widths(2), None);
        assert_eq!(nudge_widths(0), None);
        assert_eq!(nudge_widths(-1), None);
    }
}

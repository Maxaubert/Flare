//! M9 Task 30: the update chip and window copy, one function per string.
//!
//! Ported from Prism Terminal v0.55.6 `UpdateChip.tsx` and `UpdateDialog.tsx`. Pure: the controller
//! builds its view from these and nothing else says a word in the update UI.

use crate::types::Phase;

/// The heading over the release notes (the notes list's accessible label).
pub const NOTES_LABEL: &str = "What is new";

/// The chip's text.
pub fn chip_label(v: &str) -> String {
    format!("Update {v}")
}

/// The chip's accessible name.
pub fn chip_aria(v: &str) -> String {
    format!("Update to {v}")
}

/// The chip's tooltip: busy, a preview, or a real offer.
pub fn chip_tip(v: &str, mock: bool, phase: Phase) -> String {
    if phase != Phase::Idle {
        format!("Updating to {v}")
    } else if mock {
        format!("Preview: see what an update to {v} would look like")
    } else {
        format!("See what is new in {v}")
    }
}

/// The window's title.
pub fn window_title(v: &str, running: bool) -> String {
    if running {
        format!("Updating to {v}")
    } else {
        format!("Update to {v}")
    }
}

/// The quiet line under the title.
pub fn window_sub(current: &str) -> String {
    format!("You have {current}")
}

/// The status line: what is happening, or how the last install ended.
pub fn status_line(phase: Phase, mock: bool, aborting: bool, notice: Option<&str>) -> String {
    if aborting {
        return "Cancelling\u{2026}".to_string();
    }
    match phase {
        Phase::Downloading => "Downloading the update".to_string(),
        Phase::Installing if mock => "Installing (preview: nothing is installed)".to_string(),
        Phase::Installing => "Installing. The app will close and reopen.".to_string(),
        Phase::Idle => notice.unwrap_or("").to_string(),
    }
}

/// The left button: Close once an ending is said, else Cancel.
pub fn cancel_label(running: bool, notice: Option<&str>) -> &'static str {
    if !running && notice.is_some() {
        "Close"
    } else {
        "Cancel"
    }
}

/// Only a download that has not been cancelled yet can be stopped.
pub fn can_abort(phase: Phase, aborting: bool) -> bool {
    phase == Phase::Downloading && !aborting
}

/// The left button works when nothing runs, or when the running download can be stopped.
pub fn cancel_enabled(running: bool, can_abort: bool) -> bool {
    !running || can_abort
}

/// Why the left button is disabled, while it is (empty when it has nothing to say).
pub fn cancel_tip(running: bool, phase: Phase, aborting: bool) -> &'static str {
    if running && !can_abort(phase, aborting) && !aborting {
        if phase == Phase::Installing {
            "The installer has started"
        } else {
            "This download cannot be cancelled"
        }
    } else {
        ""
    }
}

/// The percentage beside the status line.
pub fn pct_text(p: u8) -> String {
    format!("{p}%")
}

// ---- install when idle (2026-10-29): a build the agent staged ------------------------------------

/// The staged build's chip.
pub const STAGED_CHIP: &str = "Build ready";
/// The chip's menu.
pub const STAGED_INSTALL_NOW: &str = "Install now";
pub const STAGED_WHEN_IDLE: &str = "Install when idle";
pub const STAGED_NOT_TODAY: &str = "Not today";
pub const STAGED_DISCARD: &str = "Discard";
/// The toast before the install, and its button.
pub const STAGED_COUNTDOWN: &str = "Installing the new build and restarting";
pub const STAGED_CANCEL: &str = "Cancel";
/// A swap that failed and put everything back.
pub const STAGED_FAILED: &str = "The build could not be installed; nothing changed";

/// The staged chip's accessible name.
pub fn staged_chip_aria(when_idle: bool) -> String {
    if when_idle {
        "Build ready, installs when idle".to_string()
    } else {
        STAGED_CHIP.to_string()
    }
}

/// The staged chip's tooltip.
pub fn staged_chip_tip(commit: &str, when_idle: bool) -> String {
    if when_idle {
        format!("Build {commit} installs once no agent has worked for a minute")
    } else {
        format!("Build {commit} is ready to install")
    }
}

/// Said once by the first start of a new build.
pub fn staged_installed(commit: &str) -> String {
    format!("Installed {commit}")
}

/// Said once by the old build after the last-good guard put it back.
pub fn staged_rolled_back(commit: &str) -> String {
    format!("Build {commit} did not start, so the last build is back")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_staged_build_words() {
        assert_eq!(staged_installed("abc1234"), "Installed abc1234");
        assert_eq!(
            staged_rolled_back("abc1234"),
            "Build abc1234 did not start, so the last build is back"
        );
        for s in [
            STAGED_CHIP,
            STAGED_COUNTDOWN,
            STAGED_FAILED,
            &staged_chip_tip("abc1234", true),
            &staged_chip_tip("abc1234", false),
        ] {
            assert!(!s.contains('\u{2014}'), "no em-dash: {s}");
        }
    }

    #[test]
    fn chip_label_names_the_version() {
        assert_eq!(chip_label("0.56.0"), "Update 0.56.0");
    }

    #[test]
    fn chip_aria_says_update_to() {
        assert_eq!(chip_aria("0.56.0"), "Update to 0.56.0");
    }

    #[test]
    fn chip_tip_while_not_idle() {
        assert_eq!(
            chip_tip("1.0.0", false, Phase::Downloading),
            "Updating to 1.0.0"
        );
        assert_eq!(
            chip_tip("1.0.0", true, Phase::Installing),
            "Updating to 1.0.0"
        );
    }

    #[test]
    fn chip_tip_for_a_preview_when_idle() {
        assert_eq!(
            chip_tip("1.0.0", true, Phase::Idle),
            "Preview: see what an update to 1.0.0 would look like"
        );
    }

    #[test]
    fn chip_tip_for_a_real_offer_when_idle() {
        assert_eq!(
            chip_tip("1.0.0", false, Phase::Idle),
            "See what is new in 1.0.0"
        );
    }

    #[test]
    fn window_title_follows_running() {
        assert_eq!(window_title("1.0.0", true), "Updating to 1.0.0");
        assert_eq!(window_title("1.0.0", false), "Update to 1.0.0");
    }

    #[test]
    fn window_sub_names_the_running_version() {
        assert_eq!(window_sub("0.55.6"), "You have 0.55.6");
    }

    #[test]
    fn status_line_cancelling_beats_every_phase() {
        assert_eq!(
            status_line(Phase::Downloading, false, true, None),
            "Cancelling\u{2026}"
        );
    }

    #[test]
    fn status_line_downloading() {
        assert_eq!(
            status_line(Phase::Downloading, false, false, None),
            "Downloading the update"
        );
    }

    #[test]
    fn status_line_installing_a_preview() {
        assert_eq!(
            status_line(Phase::Installing, true, false, None),
            "Installing (preview: nothing is installed)"
        );
    }

    #[test]
    fn status_line_installing_for_real() {
        assert_eq!(
            status_line(Phase::Installing, false, false, None),
            "Installing. The app will close and reopen."
        );
    }

    #[test]
    fn status_line_idle_is_the_notice_or_nothing() {
        assert_eq!(
            status_line(Phase::Idle, false, false, Some("It broke")),
            "It broke"
        );
        assert_eq!(status_line(Phase::Idle, false, false, None), "");
    }

    #[test]
    fn cancel_label_is_close_only_once_an_ending_is_said() {
        assert_eq!(cancel_label(false, Some("x")), "Close");
        assert_eq!(cancel_label(false, None), "Cancel");
        assert_eq!(cancel_label(true, Some("x")), "Cancel");
    }

    #[test]
    fn cancel_enabled_when_idle_or_abortable() {
        assert!(cancel_enabled(false, false));
        assert!(cancel_enabled(true, true));
        assert!(!cancel_enabled(true, false));
        assert!(can_abort(Phase::Downloading, false));
        assert!(!can_abort(Phase::Downloading, true));
        assert!(!can_abort(Phase::Installing, false));
    }

    #[test]
    fn cancel_tip_when_the_installer_has_started() {
        assert_eq!(
            cancel_tip(true, Phase::Installing, false),
            "The installer has started"
        );
    }

    #[test]
    fn cancel_tip_when_the_download_cannot_be_cancelled() {
        assert_eq!(
            cancel_tip(true, Phase::Idle, false),
            "This download cannot be cancelled"
        );
        assert_eq!(cancel_tip(true, Phase::Downloading, false), "");
    }

    #[test]
    fn cancel_tip_is_empty_when_there_is_nothing_to_say() {
        assert_eq!(cancel_tip(false, Phase::Idle, false), "");
        assert_eq!(cancel_tip(true, Phase::Installing, true), "");
    }

    #[test]
    fn pct_text_and_the_notes_label() {
        assert_eq!(pct_text(42), "42%");
        assert_eq!(NOTES_LABEL, "What is new");
    }
}

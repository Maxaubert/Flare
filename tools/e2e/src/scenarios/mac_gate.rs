//! Which scenarios a macOS run does not start, and why (rule 1105). A scenario is gated only when what it
//! checks is Windows' own (WebView2, Velopack's Windows swap, the Windows speech host and keyboard hook,
//! PrintWindow captures, the system menu, WM_NCHITTEST, the Windows DPI steps) or when its Mac port is not
//! written yet (the web panel, the update, dictation, the `.cmd` Claude stand-in); the reason says which. Everything else runs on the macOS runner. `docs/e2e.md` ("macOS E2E") lists both.

/// Why `name` is not run on macOS, None when it runs there.
pub fn reason(name: &str) -> Option<&'static str> {
    GATED.iter().find(|(n, _)| *n == name).map(|(_, why)| *why)
}

const WEB: &str = "Mac port not written (phase 8): these drive the WebView2 controller (its child window, virtual host and controller probes); the Mac web panel is CEF, which runs only from the Flare.app bundle, and its scenarios are still to write";
const UPDATE: &str = "Mac port not written (phase 8): these drive Velopack's Windows install and swap; the Mac update is a whole-bundle swap, and its scenarios are still to write";
const DICTATION: &str = "Mac port not written (phase 7): these drive the Windows speech host processes, the WASAPI seam and the Right Alt keyboard hook; Mac dictation and its scenarios are still to write";
const DPI: &str = "Windows only: drives cmd.exe tabs through WM_DPICHANGED at the eight Windows scale steps (100 to 300 percent) and per-monitor moves; a Mac has no cmd, its backing scale is 1 or 2 and changes only with the screen, and the runner has one 1x display (a Mac launch at another scale is Slint's override, rule 1114)";
const FAKE_CLAUDE: &str = "Mac port not written: the Claude stand-in is a claude.cmd running a PowerShell script (agent_cmd.rs)";

/// Every gated scenario and its reason. A name here that is not a scenario fails a unit test.
pub const GATED: &[(&str, &str)] = &[
    ("webPanel", WEB),
    ("webVideo", WEB),
    ("webNoRuntime", WEB),
    ("agentBrowser", WEB),
    ("agentCoveredClick", WEB),
    ("mcpBrowser", WEB),
    ("miniApp", WEB),
    ("diagram", WEB),
    ("mcpDiagram", WEB),
    ("pluginBrand", WEB),
    ("modsSample", WEB),
    ("pluginFill", WEB),
    ("devPreview", WEB),
    ("model", WEB),
    ("updateWindow", UPDATE),
    ("updateGuard", UPDATE),
    ("updateNotes", UPDATE),
    ("idleInstall", UPDATE),
    ("dictation", DICTATION),
    ("dictationPage", DICTATION),
    ("dictationParakeet", DICTATION),
    ("dictateAnywhere", DICTATION),
    ("dictateTwoCopies", DICTATION),
    ("dpiMatrix", DPI),
    ("monitorMove", DPI),
    ("launchSkeleton", FAKE_CLAUDE),
    ("sonaraQuestion", FAKE_CLAUDE),
    ("imagePaste", FAKE_CLAUDE),
    ("closeAsk", FAKE_CLAUDE),
    (
        "systemMenu",
        "Windows only: the window's system menu (Alt+Space, the strip's right click); a Mac window has none, and Option+Space types (chords.rs holds the Mac side)",
    ),
    (
        "resizeEdges",
        "Windows only: the frameless window's WM_NCHITTEST codes through its window procedure; the Mac window is titled and AppKit owns its edges (flare-win macos resize_edges holds the Mac side)",
    ),
    (
        "visualTour",
        "Windows only: an opt-in tour that captures the reference terminal beside Flare with PrintWindow",
    ),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenarios::{ALL, OPT_IN};

    #[test]
    fn every_gated_name_is_a_scenario_once_with_a_reason() {
        for (i, (name, why)) in GATED.iter().enumerate() {
            assert!(
                ALL.iter().chain(OPT_IN).any(|s| s.name == *name),
                "{name} is not a scenario"
            );
            assert!(why.len() > 20, "{name} needs a real reason");
            assert!(
                GATED[i + 1..].iter().all(|(n, _)| n != name),
                "{name} is gated twice"
            );
        }
        assert_eq!(reason("spawn"), None);
        assert!(reason("webPanel").is_some());
    }
}

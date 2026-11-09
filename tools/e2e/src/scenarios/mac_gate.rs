//! Which scenarios a macOS run does not start, and why (rule 332). A scenario is gated only when what it
//! checks is Windows' own (WebView2, Velopack's Windows swap, the Windows speech host and keyboard hook,
//! `.cmd` stand-ins, PrintWindow captures) or when its Mac port is not written yet; the reason says
//! which. Everything else runs on the macOS runner. `docs/e2e.md` ("macOS E2E") lists both.

/// Why `name` is not run on macOS, None when it runs there.
pub fn reason(name: &str) -> Option<&'static str> {
    GATED.iter().find(|(n, _)| *n == name).map(|(_, why)| *why)
}

const WEB: &str = "Windows only: drives the WebView2 controller (its child window, virtual host and controller probes); the Mac web panel is CEF, which runs only from the Flare.app bundle (port phase 8)";
const UPDATE: &str = "Windows only: Velopack's Windows install and swap (the Mac update is a whole-bundle swap, port phase 8)";
const DICTATION: &str = "Windows only: the Windows speech host processes, the WASAPI seam and the Right Alt keyboard hook (Mac dictation is port phase 7)";
const FAKE_CLAUDE: &str = "Mac port not written: the Claude stand-in is a claude.cmd running a PowerShell script (agent_cmd.rs)";

/// Every gated scenario and its reason. A name here that is not a scenario fails a unit test.
pub const GATED: &[(&str, &str)] = &[
    ("webPanel", WEB),
    ("webVideo", WEB),
    ("webNoRuntime", WEB),
    ("agentBrowser", WEB),
    ("mcpBrowser", WEB),
    ("miniApp", WEB),
    ("diagram", WEB),
    ("mcpDiagram", WEB),
    ("pluginBrand", WEB),
    ("modsSample", WEB),
    ("devPreview", WEB),
    ("updateWindow", UPDATE),
    ("updateGuard", UPDATE),
    ("updateNotes", UPDATE),
    ("idleInstall", UPDATE),
    ("dictation", DICTATION),
    ("dictationPage", DICTATION),
    ("dictationParakeet", DICTATION),
    ("dictateAnywhere", DICTATION),
    ("dictateTwoCopies", DICTATION),
    ("launchSkeleton", FAKE_CLAUDE),
    ("sonaraQuestion", FAKE_CLAUDE),
    ("imagePaste", FAKE_CLAUDE),
    ("closeAsk", FAKE_CLAUDE),
    (
        "visualTour",
        "Windows only: an opt-in tour that captures Prism Terminal beside Flare with PrintWindow",
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

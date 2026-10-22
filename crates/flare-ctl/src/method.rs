//! The method names of protocol v1, as one enum. A name on the wire that is not here is
//! `unknown_method`. Adding a method is adding a variant: the grant table and the timeout table below
//! are exhaustive matches, so the compiler names every place that needs a decision.

use std::time::Duration;

/// Default time a request may take before the server answers `timeout`.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);
/// Time for the `browser.*` methods (page loads, waits).
pub const BROWSER_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Method {
    Hello,
    TabsList,
    PanelOpen,
    PanelClose,
    PanelState,
    ViewImage,
    ViewFile,
    ViewText,
    /// A mini-app: a page the agent wrote (Surfaces stage 5).
    ViewHtml,
    /// A Mermaid diagram (Surfaces stage 5).
    ViewDiagram,
    BrowserOpen,
    BrowserNavigate,
    BrowserLook,
    BrowserClick,
    BrowserType,
    BrowserPress,
    BrowserScroll,
    BrowserWait,
    BrowserEval,
    /// Reserved: what Flare knows about the conversation of the tab.
    ConvoState,
    /// Reserved, UI only: a mod answers a tool call the agent made.
    ToolsRespond,
    /// Reserved, UI only (never over the pipe): bytes into a shell. Rules 1 and 2.
    ShellInput,
    /// Reserved, UI only: change a setting.
    SettingsSet,
    /// Reserved, UI only: enable or disable a mod.
    ModsEnable,
}

/// What a method belongs to, which decides which grants may call it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Namespace {
    Hello,
    Tabs,
    Panel,
    View,
    Browser,
    Convo,
    Tools,
    /// Methods only the app's own UI may call (settings, shell input, mod management).
    Ui,
}

impl Method {
    /// Every method, in documentation order.
    pub const ALL: [Method; 24] = [
        Method::Hello,
        Method::TabsList,
        Method::PanelOpen,
        Method::PanelClose,
        Method::PanelState,
        Method::ViewImage,
        Method::ViewFile,
        Method::ViewText,
        Method::ViewHtml,
        Method::ViewDiagram,
        Method::BrowserOpen,
        Method::BrowserNavigate,
        Method::BrowserLook,
        Method::BrowserClick,
        Method::BrowserType,
        Method::BrowserPress,
        Method::BrowserScroll,
        Method::BrowserWait,
        Method::BrowserEval,
        Method::ConvoState,
        Method::ToolsRespond,
        Method::ShellInput,
        Method::SettingsSet,
        Method::ModsEnable,
    ];

    /// The wire name.
    pub fn name(self) -> &'static str {
        match self {
            Method::Hello => "hello",
            Method::TabsList => "tabs.list",
            Method::PanelOpen => "panel.open",
            Method::PanelClose => "panel.close",
            Method::PanelState => "panel.state",
            Method::ViewImage => "view.image",
            Method::ViewFile => "view.file",
            Method::ViewText => "view.text",
            Method::ViewHtml => "view.html",
            Method::ViewDiagram => "view.diagram",
            Method::BrowserOpen => "browser.open",
            Method::BrowserNavigate => "browser.navigate",
            Method::BrowserLook => "browser.look",
            Method::BrowserClick => "browser.click",
            Method::BrowserType => "browser.type",
            Method::BrowserPress => "browser.press",
            Method::BrowserScroll => "browser.scroll",
            Method::BrowserWait => "browser.wait",
            Method::BrowserEval => "browser.eval",
            Method::ConvoState => "convo.state",
            Method::ToolsRespond => "tools.respond",
            Method::ShellInput => "shell.input",
            Method::SettingsSet => "settings.set",
            Method::ModsEnable => "mods.enable",
        }
    }

    /// The method for a wire name, if the protocol has it.
    pub fn parse(name: &str) -> Option<Method> {
        Method::ALL.into_iter().find(|m| m.name() == name)
    }

    pub fn namespace(self) -> Namespace {
        match self {
            Method::Hello => Namespace::Hello,
            Method::TabsList => Namespace::Tabs,
            Method::PanelOpen | Method::PanelClose | Method::PanelState => Namespace::Panel,
            Method::ViewImage
            | Method::ViewFile
            | Method::ViewText
            | Method::ViewHtml
            | Method::ViewDiagram => Namespace::View,
            Method::BrowserOpen
            | Method::BrowserNavigate
            | Method::BrowserLook
            | Method::BrowserClick
            | Method::BrowserType
            | Method::BrowserPress
            | Method::BrowserScroll
            | Method::BrowserWait
            | Method::BrowserEval => Namespace::Browser,
            Method::ConvoState => Namespace::Convo,
            Method::ToolsRespond => Namespace::Tools,
            Method::ShellInput | Method::SettingsSet | Method::ModsEnable => Namespace::Ui,
        }
    }

    /// How long the server lets a request run before it answers `timeout`.
    pub fn timeout(self) -> Duration {
        match self.namespace() {
            // A page or a diagram is answered when it has loaded (and drawn): as long as a browser call.
            Namespace::Browser => BROWSER_TIMEOUT,
            Namespace::View if matches!(self, Method::ViewHtml | Method::ViewDiagram) => {
                BROWSER_TIMEOUT
            }
            _ => DEFAULT_TIMEOUT,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn names_are_unique_and_round_trip() {
        let mut seen = HashSet::new();
        for m in Method::ALL {
            assert!(seen.insert(m.name()), "duplicate {}", m.name());
            assert_eq!(Method::parse(m.name()), Some(m));
        }
        assert_eq!(seen.len(), Method::ALL.len());
    }

    #[test]
    fn names_follow_their_namespace() {
        for m in Method::ALL {
            let prefix = m.name().split('.').next().unwrap();
            let ok = match m.namespace() {
                Namespace::Hello => prefix == "hello",
                Namespace::Tabs => prefix == "tabs",
                Namespace::Panel => prefix == "panel",
                Namespace::View => prefix == "view",
                Namespace::Browser => prefix == "browser",
                Namespace::Convo => prefix == "convo",
                Namespace::Tools => prefix == "tools",
                Namespace::Ui => matches!(prefix, "shell" | "settings" | "mods"),
            };
            assert!(ok, "{} is filed under the wrong namespace", m.name());
        }
    }

    #[test]
    fn unknown_names_do_not_parse() {
        for n in [
            "",
            "Hello",
            "panel",
            "panel.",
            "browser.fly",
            "view.image ",
            "tabs.list.x",
        ] {
            assert_eq!(Method::parse(n), None, "{n:?}");
        }
    }

    #[test]
    fn browser_calls_get_the_long_timeout() {
        assert_eq!(Method::BrowserWait.timeout(), BROWSER_TIMEOUT);
        assert_eq!(Method::ViewImage.timeout(), DEFAULT_TIMEOUT);
        // A mini-app is answered when its page has loaded, a diagram when it has drawn.
        assert_eq!(Method::ViewHtml.timeout(), BROWSER_TIMEOUT);
        assert_eq!(Method::ViewDiagram.timeout(), BROWSER_TIMEOUT);
        assert!(BROWSER_TIMEOUT > DEFAULT_TIMEOUT);
    }
}

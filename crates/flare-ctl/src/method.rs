//! The method names of protocol v1, as one enum. A name on the wire that is not here is
//! `unknown_method`. Adding a method is adding a variant: the grant table and the timeout table below
//! are exhaustive matches, so the compiler names every place that needs a decision.

use std::time::Duration;

use crate::grant::Permission;

/// Default time a request may take before the server answers `timeout`.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);
/// Time for the `browser.*` methods (page loads, waits).
pub const BROWSER_TIMEOUT: Duration = Duration::from_secs(60);
/// Time for `mods.call`: the mod's page has 30 s, and a page that has to be opened first needs a few more.
pub const MOD_CALL_TIMEOUT: Duration = Duration::from_secs(40);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Method {
    Hello,
    TabsList,
    /// A badge on a tab: a small glyph in Flare's outlined chip (plugins phase 1). The agent's own tab only.
    TabsBadgeSet,
    TabsBadgeClear,
    /// A progress chip on a tab: a static pill, never at the tab's foot (the agent's mark lives there).
    TabsProgressSet,
    TabsProgressClear,
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
    /// The tools the enabled mods offer the agent (`flare-mcp` lists them). The agent's, not a mod's.
    ModsTools,
    /// The agent calls one mod tool by its agent-facing name (`mod_<id>_<tool>`) and waits for the mod's page
    /// to answer.
    ModsCall,
}

/// What a method belongs to, which decides which grants may call it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Namespace {
    Hello,
    Tabs,
    /// What a caller draws on a tab (badges and progress), under the `decorate` permission. Its own namespace, so
    /// nothing of it rides on the open `Tabs` one.
    Decor,
    Panel,
    View,
    Browser,
    Convo,
    Tools,
    /// The agent's view of the mods' tools (`mods.tools`, `mods.call`): the agent and the app, never a mod.
    AgentTools,
    /// Methods only the app's own UI may call (settings, shell input, mod management).
    Ui,
    /// A plugin's sidebar sections (`sidebar.set`), under `sidebar`. Its methods come with the sidebar phase.
    Sidebar,
    /// A plugin's buttons and menu items (`actions.set`), under `actions`.
    Actions,
    /// A plugin's strip under a terminal (`bar.show`, `bar.hide`), under `bar`.
    Bar,
    /// What is said in Claude sessions (`session.read`), under `sessions`.
    Session,
    /// Typing into a tab's shell (`tabs.input`), under `input`. Its own namespace, so it never rides on the open
    /// `Tabs` one.
    Input,
}

impl Namespace {
    /// Every namespace.
    pub const ALL: [Namespace; 15] = [
        Namespace::Hello,
        Namespace::Tabs,
        Namespace::Decor,
        Namespace::Panel,
        Namespace::View,
        Namespace::Browser,
        Namespace::Convo,
        Namespace::Tools,
        Namespace::AgentTools,
        Namespace::Ui,
        Namespace::Sidebar,
        Namespace::Actions,
        Namespace::Bar,
        Namespace::Session,
        Namespace::Input,
    ];

    /// The one permission that opens this namespace to a plugin. `None` for the open ones (`hello`, `tabs.list`),
    /// for the agent's tools and for the app's own: no permission reaches those.
    pub fn permission(self) -> Option<Permission> {
        match self {
            Namespace::Hello | Namespace::Tabs | Namespace::AgentTools | Namespace::Ui => None,
            Namespace::Decor => Some(Permission::Decorate),
            Namespace::Panel => Some(Permission::Panel),
            Namespace::View => Some(Permission::View),
            Namespace::Browser => Some(Permission::Browser),
            Namespace::Convo => Some(Permission::Convo),
            Namespace::Tools => Some(Permission::Tools),
            Namespace::Sidebar => Some(Permission::Sidebar),
            Namespace::Actions => Some(Permission::Actions),
            Namespace::Bar => Some(Permission::Bar),
            Namespace::Session => Some(Permission::Sessions),
            Namespace::Input => Some(Permission::Input),
        }
    }
}

impl Method {
    /// Every method, in documentation order.
    pub const ALL: [Method; 30] = [
        Method::Hello,
        Method::TabsList,
        Method::TabsBadgeSet,
        Method::TabsBadgeClear,
        Method::TabsProgressSet,
        Method::TabsProgressClear,
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
        Method::ModsTools,
        Method::ModsCall,
    ];

    /// The wire name.
    pub fn name(self) -> &'static str {
        match self {
            Method::Hello => "hello",
            Method::TabsList => "tabs.list",
            Method::TabsBadgeSet => "tabs.badge.set",
            Method::TabsBadgeClear => "tabs.badge.clear",
            Method::TabsProgressSet => "tabs.progress.set",
            Method::TabsProgressClear => "tabs.progress.clear",
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
            Method::ModsTools => "mods.tools",
            Method::ModsCall => "mods.call",
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
            Method::TabsBadgeSet
            | Method::TabsBadgeClear
            | Method::TabsProgressSet
            | Method::TabsProgressClear => Namespace::Decor,
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
            Method::ModsTools | Method::ModsCall => Namespace::AgentTools,
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
            // The mod's page has 30 s to answer (`flare_mods::hub::CALL_TIMEOUT`); the channel lets the
            // handler run a little longer, so the answer is the app's own, with the mod's name in it.
            Namespace::AgentTools if self == Method::ModsCall => MOD_CALL_TIMEOUT,
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
                Namespace::Decor => prefix == "tabs",
                Namespace::Panel => prefix == "panel",
                Namespace::View => prefix == "view",
                Namespace::Browser => prefix == "browser",
                Namespace::Convo => prefix == "convo",
                Namespace::Tools => prefix == "tools",
                Namespace::AgentTools => prefix == "mods",
                Namespace::Ui => matches!(prefix, "shell" | "settings" | "mods"),
                Namespace::Sidebar => prefix == "sidebar",
                Namespace::Actions => prefix == "actions",
                Namespace::Bar => prefix == "bar",
                Namespace::Session => prefix == "session",
                Namespace::Input => prefix == "tabs",
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
            "tabs.badge",
            "tabs.badge.set.x",
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

    #[test]
    fn a_mod_tool_call_waits_for_the_mods_30_seconds_and_a_listing_does_not() {
        assert_eq!(Method::ModsCall.timeout(), MOD_CALL_TIMEOUT);
        assert!(MOD_CALL_TIMEOUT > Duration::from_secs(30));
        assert_eq!(Method::ModsTools.timeout(), DEFAULT_TIMEOUT);
    }
}

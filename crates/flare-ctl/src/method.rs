//! The method names of protocol v1, as one enum. A name on the wire that is not here is
//! `unknown_method`. Adding a method is adding a variant: the grant table and the timeout table below
//! are exhaustive matches, so the compiler names every place that needs a decision.

use std::time::Duration;

use crate::grant::Permission;

/// Default time a request may take before the server answers `timeout`.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);
/// Time for the `browser.*` methods (page loads, waits).
pub const BROWSER_TIMEOUT: Duration = Duration::from_secs(60);
/// Time for `browser.fill` and `secret.ask`: the person answers Flare's own dialog (it waits at most a minute), then
/// the page is filled.
pub const FILL_TIMEOUT: Duration = Duration::from_secs(90);
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
    /// The items of one of a plugin's sidebar sections (plugins phase 4), drawn by Flare in the sidebar's look.
    SidebarSet,
    /// The state of one of a plugin's actions, on one tab or on every tab (plugins phase 4).
    ActionsSet,
    /// The last entries of a Claude session's conversation in a tab (plugins phase 5), under `sessions`.
    SessionRead,
    /// Text typed into a tab's shell as one paste, and an Enter after it when asked (plugins phase 5), under `input`.
    /// The only way a caller outside Flare's own interface writes into a shell (rule 2's plugin exception).
    TabsInput,
    /// A plugin's own small store (plugins phase 5): what it keeps, read back. No permission, never the agent's.
    StorageGet,
    StorageSet,
    /// A plugin's strip under a tab's terminal (plugins phase 6), shown and hidden, under `bar`. One bar a tab.
    BarShow,
    BarHide,
    /// How one agent's tab icon moves while it works, a motion from Flare's fixed list (owner decision 2026-10-26),
    /// under `agent-looks`. Last writer wins; a plugin turned off takes its motions with it.
    AgentsMotionSet,
    AgentsMotionClear,
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
    /// A sound file or a public https address in Flare's audio player (plugins phase 6).
    ViewAudio,
    /// A .glb, .gltf or .stl file in Flare's 3D viewer (issue #36).
    ViewModel,
    BrowserOpen,
    BrowserNavigate,
    BrowserLook,
    BrowserClick,
    BrowserType,
    BrowserPress,
    BrowserScroll,
    BrowserWait,
    BrowserEval,
    /// An arrow over the page at what the person must do themselves (2026-11-10, rule 254): it never clicks and cannot
    /// type, and it is not a job.
    BrowserPoint,
    /// A plugin fills a saved login into a tab's web page (under `fill`): only a page whose address matches the
    /// login's, and only after the person allowed it in Flare's own dialog. The values go to the page only.
    BrowserFill,
    /// The address of a tab's web page, for a plugin that is to pick a login for it (under `fill`).
    BrowserFillPage,
    /// A plugin asks the person for a secret (a master password) in Flare's own masked field (under `fill`); the
    /// text goes back to that plugin only and is never kept or logged.
    SecretAsk,
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
    /// A plugin's sidebar sections (`sidebar.set`), under `sidebar`.
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
    /// A plugin's own store (`storage.get`, `storage.set`): every plugin's, with no permission, and never the agent's
    /// (design 13). It holds only what the plugin put there.
    Storage,
    /// How the agents' tab icons move while they work (`agents.motion.*`), under `agent-looks`. Never the agent's.
    Agents,
    /// A login filled into a web page and a secret asked of the person (`browser.fill`, `browser.fill.page`,
    /// `secret.ask`), under `fill`. Never the agent's.
    Fill,
}

impl Namespace {
    /// Every namespace.
    pub const ALL: [Namespace; 18] = [
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
        Namespace::Storage,
        Namespace::Agents,
        Namespace::Fill,
    ];

    /// The one permission that opens this namespace to a plugin. `None` for the open ones (`hello`, `tabs.list`), for a
    /// plugin's own store (every plugin's, see `Grant::may_enter`), for the agent's tools and for the app's own: no
    /// permission reaches those.
    pub fn permission(self) -> Option<Permission> {
        match self {
            Namespace::Hello
            | Namespace::Tabs
            | Namespace::AgentTools
            | Namespace::Ui
            | Namespace::Storage => None,
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
            Namespace::Agents => Some(Permission::AgentLooks),
            Namespace::Fill => Some(Permission::Fill),
        }
    }
}

impl Method {
    /// Every method, in documentation order.
    pub const ALL: [Method; 46] = [
        Method::Hello,
        Method::TabsList,
        Method::TabsBadgeSet,
        Method::TabsBadgeClear,
        Method::TabsProgressSet,
        Method::TabsProgressClear,
        Method::SidebarSet,
        Method::ActionsSet,
        Method::SessionRead,
        Method::TabsInput,
        Method::StorageGet,
        Method::StorageSet,
        Method::BarShow,
        Method::BarHide,
        Method::AgentsMotionSet,
        Method::AgentsMotionClear,
        Method::PanelOpen,
        Method::PanelClose,
        Method::PanelState,
        Method::ViewImage,
        Method::ViewFile,
        Method::ViewText,
        Method::ViewHtml,
        Method::ViewDiagram,
        Method::ViewAudio,
        Method::ViewModel,
        Method::BrowserOpen,
        Method::BrowserNavigate,
        Method::BrowserLook,
        Method::BrowserClick,
        Method::BrowserType,
        Method::BrowserPress,
        Method::BrowserScroll,
        Method::BrowserWait,
        Method::BrowserEval,
        Method::BrowserPoint,
        Method::BrowserFill,
        Method::BrowserFillPage,
        Method::SecretAsk,
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
            Method::SidebarSet => "sidebar.set",
            Method::ActionsSet => "actions.set",
            Method::SessionRead => "session.read",
            Method::TabsInput => "tabs.input",
            Method::StorageGet => "storage.get",
            Method::StorageSet => "storage.set",
            Method::BarShow => "bar.show",
            Method::BarHide => "bar.hide",
            Method::AgentsMotionSet => "agents.motion.set",
            Method::AgentsMotionClear => "agents.motion.clear",
            Method::PanelOpen => "panel.open",
            Method::PanelClose => "panel.close",
            Method::PanelState => "panel.state",
            Method::ViewImage => "view.image",
            Method::ViewFile => "view.file",
            Method::ViewText => "view.text",
            Method::ViewHtml => "view.html",
            Method::ViewDiagram => "view.diagram",
            Method::ViewAudio => "view.audio",
            Method::ViewModel => "view.model",
            Method::BrowserOpen => "browser.open",
            Method::BrowserNavigate => "browser.navigate",
            Method::BrowserLook => "browser.look",
            Method::BrowserClick => "browser.click",
            Method::BrowserType => "browser.type",
            Method::BrowserPress => "browser.press",
            Method::BrowserScroll => "browser.scroll",
            Method::BrowserWait => "browser.wait",
            Method::BrowserEval => "browser.eval",
            Method::BrowserPoint => "browser.point",
            Method::BrowserFill => "browser.fill",
            Method::BrowserFillPage => "browser.fill.page",
            Method::SecretAsk => "secret.ask",
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
            Method::SidebarSet => Namespace::Sidebar,
            Method::ActionsSet => Namespace::Actions,
            Method::SessionRead => Namespace::Session,
            Method::TabsInput => Namespace::Input,
            Method::StorageGet | Method::StorageSet => Namespace::Storage,
            Method::BarShow | Method::BarHide => Namespace::Bar,
            Method::AgentsMotionSet | Method::AgentsMotionClear => Namespace::Agents,
            Method::PanelOpen | Method::PanelClose | Method::PanelState => Namespace::Panel,
            Method::ViewImage
            | Method::ViewFile
            | Method::ViewText
            | Method::ViewHtml
            | Method::ViewDiagram
            | Method::ViewAudio
            | Method::ViewModel => Namespace::View,
            Method::BrowserOpen
            | Method::BrowserNavigate
            | Method::BrowserLook
            | Method::BrowserClick
            | Method::BrowserType
            | Method::BrowserPress
            | Method::BrowserScroll
            | Method::BrowserWait
            | Method::BrowserEval
            | Method::BrowserPoint => Namespace::Browser,
            Method::BrowserFill | Method::BrowserFillPage | Method::SecretAsk => Namespace::Fill,
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
            // The person answers Flare's dialog, then the page is filled; the page's address is answered at once.
            Namespace::Fill if self != Method::BrowserFillPage => FILL_TIMEOUT,
            Namespace::View
                if matches!(
                    self,
                    Method::ViewHtml | Method::ViewDiagram | Method::ViewModel
                ) =>
            {
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
                Namespace::Storage => prefix == "storage",
                Namespace::Agents => prefix == "agents",
                Namespace::Fill => matches!(prefix, "browser" | "secret"),
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
        assert_eq!(Method::ViewModel.timeout(), BROWSER_TIMEOUT);
        assert!(BROWSER_TIMEOUT > DEFAULT_TIMEOUT);
    }

    #[test]
    fn a_fill_waits_for_the_persons_answer_and_the_page_address_does_not() {
        assert_eq!(Method::BrowserFill.timeout(), FILL_TIMEOUT);
        assert_eq!(Method::SecretAsk.timeout(), FILL_TIMEOUT);
        assert_eq!(Method::BrowserFillPage.timeout(), DEFAULT_TIMEOUT);
        assert!(FILL_TIMEOUT > Duration::from_secs(60));
    }

    #[test]
    fn a_mod_tool_call_waits_for_the_mods_30_seconds_and_a_listing_does_not() {
        assert_eq!(Method::ModsCall.timeout(), MOD_CALL_TIMEOUT);
        assert!(MOD_CALL_TIMEOUT > Duration::from_secs(30));
        assert_eq!(Method::ModsTools.timeout(), DEFAULT_TIMEOUT);
    }
}

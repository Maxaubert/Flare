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
/// Time for the long `remote.*` calls a viewer makes over ssh (`remote.open`, `remote.ls`, `remote.fetch`, remote
/// sessions PR 2 on): the host starts a shell, lists a folder or reads a file before it answers.
pub const REMOTE_CALL: Duration = Duration::from_secs(60);

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
    /// The first line of a remote link (remote sessions, 2026-11-12): both ends say their remote protocol, their
    /// machine and their Flare version. Only a connection that came in on `flare --serve-remote`'s own stdio may
    /// send it, and it is what gives that connection the remote grant; on the pipe it is `forbidden`.
    RemoteHello,
    /// A new tab on the host in a host folder, with an optional Claude session to resume (remote sessions PR 2).
    RemoteOpen,
    /// One host folder's names, bounded (the folder picker of a later PR).
    RemoteLs,
    /// The host's shell tabs: id, folder, title, the agent's state word and session.
    RemoteTabs,
    /// Starts following a host tab's output: its size and the sequence number its output has reached.
    RemoteAttach,
    /// Stops following a host tab. Never ends its session (rule 280).
    RemoteDetach,
    /// Typing on a host tab as a list of typed intents (key, text, paste), never raw bytes (rule 279).
    RemoteInput,
    /// The viewer's size for a host tab: the host tab takes it and its window stops refitting it (rule 282).
    RemoteResize,
    /// A host tab's output since a sequence number, polled (rule 281).
    RemoteOutput,
    /// A host tab's screen and scrollback tail as text, for a viewer whose `since` fell out of the ring.
    RemoteSnapshot,
    /// Closes a host tab the way a person's close does: an agent at work asks first (rule 48).
    RemoteClose,
    /// The host's tabs as the host's own agent marks say them: a state word, the agent, its session, folder, title and
    /// the question's words (remote sessions PR 4, rule 294). Polled by the viewer every second or two.
    RemoteStatus,
    /// What is said in a host tab's Claude session, as numbered entries after a number (remote sessions PR 4, rule
    /// 297). The host finds and reads the transcript; no host path crosses the link.
    RemoteConvo,
    /// One chunk of a file a host tab's link names (remote sessions PR 6, rule 303): read on the host from the tab's
    /// folder or the host's temp folder only, by the path as the host resolves it.
    RemoteFetch,
    /// One chunk of a file the viewer copies to the host (a dropped file, a pasted image; remote sessions PR 6, rule
    /// 304): written into the tab's own upload folder on the host, under a name the host cleans.
    RemotePut,
    /// Opens a tab in this Flare that shows a session on another machine (remote sessions PR 5): a machine named in
    /// `machines.json`, a folder there and an optional Claude session to resume. The agent's (owner decision: no
    /// prompt) and the `flare open --on` command's. The opened tab takes no typing from either (rule 299).
    OpenRemoteSession,
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
    /// A remote link's methods (`remote.*`, remote sessions): only the remote grant, which only the serve stdio path
    /// issues, enters it. No permission opens it, so no mod or plugin can ask for it.
    Remote,
    /// What a caller opens in this Flare for the person (`open.remote`, remote sessions PR 5): the agent's and the
    /// app's. No permission opens it, so no mod or plugin reaches it, and neither does a remote link or the bridge.
    Open,
}

impl Namespace {
    /// Every namespace.
    pub const ALL: [Namespace; 20] = [
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
        Namespace::Remote,
        Namespace::Open,
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
            | Namespace::Storage
            | Namespace::Remote
            | Namespace::Open => None,
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
    pub const ALL: [Method; 62] = [
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
        Method::RemoteHello,
        Method::RemoteOpen,
        Method::RemoteLs,
        Method::RemoteTabs,
        Method::RemoteAttach,
        Method::RemoteDetach,
        Method::RemoteInput,
        Method::RemoteResize,
        Method::RemoteOutput,
        Method::RemoteSnapshot,
        Method::RemoteClose,
        Method::RemoteStatus,
        Method::RemoteConvo,
        Method::RemoteFetch,
        Method::RemotePut,
        Method::OpenRemoteSession,
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
            Method::RemoteHello => "remote.hello",
            Method::RemoteOpen => "remote.open",
            Method::RemoteLs => "remote.ls",
            Method::RemoteTabs => "remote.tabs",
            Method::RemoteAttach => "remote.attach",
            Method::RemoteDetach => "remote.detach",
            Method::RemoteInput => "remote.input",
            Method::RemoteResize => "remote.resize",
            Method::RemoteOutput => "remote.output",
            Method::RemoteSnapshot => "remote.snapshot",
            Method::RemoteClose => "remote.close",
            Method::RemoteStatus => "remote.status",
            Method::RemoteConvo => "remote.convo",
            Method::RemoteFetch => "remote.fetch",
            Method::RemotePut => "remote.put",
            Method::OpenRemoteSession => "open.remote",
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
            Method::RemoteHello
            | Method::RemoteOpen
            | Method::RemoteLs
            | Method::RemoteTabs
            | Method::RemoteAttach
            | Method::RemoteDetach
            | Method::RemoteInput
            | Method::RemoteResize
            | Method::RemoteOutput
            | Method::RemoteSnapshot
            | Method::RemoteClose
            | Method::RemoteStatus
            | Method::RemoteConvo
            | Method::RemoteFetch
            | Method::RemotePut => Namespace::Remote,
            Method::OpenRemoteSession => Namespace::Open,
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
            // The host starts a shell, reads a folder or moves a file's chunk before it answers: `REMOTE_CALL`. A remote
            // link's short calls (the hello, the polls, typing) get the default.
            Namespace::Remote
                if matches!(
                    self,
                    Method::RemoteOpen | Method::RemoteLs | Method::RemoteFetch | Method::RemotePut
                ) =>
            {
                REMOTE_CALL
            }
            Namespace::Remote => DEFAULT_TIMEOUT,
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
                Namespace::Remote => prefix == "remote",
                Namespace::Open => prefix == "open",
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
    fn every_method_and_namespace_is_listed_once() {
        assert_eq!(Method::ALL.len(), 62);
        assert_eq!(Namespace::ALL.len(), 20);
        for (i, ns) in Namespace::ALL.iter().enumerate() {
            assert!(!Namespace::ALL[..i].contains(ns), "{ns:?} twice");
        }
        for m in Method::ALL {
            assert!(Namespace::ALL.contains(&m.namespace()), "{}", m.name());
        }
    }

    #[test]
    fn remote_hello_is_a_short_call_and_the_long_remote_calls_have_their_own_time() {
        assert_eq!(Method::RemoteHello.namespace(), Namespace::Remote);
        assert_eq!(Method::parse("remote.hello"), Some(Method::RemoteHello));
        assert_eq!(Method::RemoteHello.timeout(), DEFAULT_TIMEOUT);
        assert!(REMOTE_CALL > DEFAULT_TIMEOUT);
        assert_eq!(Namespace::Remote.permission(), None);
        // The host starts a shell or reads a folder before it answers; the polls and typing are short.
        for m in [
            Method::RemoteOpen,
            Method::RemoteLs,
            Method::RemoteFetch,
            Method::RemotePut,
        ] {
            assert_eq!(m.timeout(), REMOTE_CALL, "{}", m.name());
        }
        for m in [
            Method::RemoteTabs,
            Method::RemoteAttach,
            Method::RemoteDetach,
            Method::RemoteInput,
            Method::RemoteResize,
            Method::RemoteOutput,
            Method::RemoteSnapshot,
            Method::RemoteClose,
            Method::RemoteStatus,
            Method::RemoteConvo,
        ] {
            assert_eq!(m.timeout(), DEFAULT_TIMEOUT, "{}", m.name());
            assert_eq!(m.namespace(), Namespace::Remote, "{}", m.name());
            assert_eq!(Method::parse(m.name()), Some(m));
        }
    }

    /// Remote sessions PR 5: opening a remote tab is answered as soon as the tab is there (its link connects on its own
    /// worker), so it takes the default time; it is no `remote.*` method and no permission opens it.
    #[test]
    fn open_remote_is_a_short_call_of_its_own_namespace() {
        assert_eq!(
            Method::parse("open.remote"),
            Some(Method::OpenRemoteSession)
        );
        assert_eq!(Method::OpenRemoteSession.namespace(), Namespace::Open);
        assert_eq!(Method::OpenRemoteSession.timeout(), DEFAULT_TIMEOUT);
        assert_eq!(Namespace::Open.permission(), None);
    }

    #[test]
    fn a_mod_tool_call_waits_for_the_mods_30_seconds_and_a_listing_does_not() {
        assert_eq!(Method::ModsCall.timeout(), MOD_CALL_TIMEOUT);
        assert!(MOD_CALL_TIMEOUT > Duration::from_secs(30));
        assert_eq!(Method::ModsTools.timeout(), DEFAULT_TIMEOUT);
    }
}

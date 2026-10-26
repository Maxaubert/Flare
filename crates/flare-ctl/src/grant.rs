//! Grants: who a connection is, and the pure rule that decides whether it may call a method.
//!
//! Three kinds. The **agent** grant is what a caller with no token gets (Claude Code through
//! `flare-mcp`): it can show content, drive its own tab's browser panel and put a badge or a progress
//! chip on its own tab, and nothing else (rule S3): it never types into a shell, reads a session or keeps a store.
//! A **mod** grant is exactly the permissions its manifest declared and the user accepted (S5), plus its own store.
//! The **ui** grant is the app's own code, called in-process; it is never issued over the pipe, so
//! no token can mean ui.

use crate::method::{Method, Namespace};
use crate::proto::{CtlError, code};
use serde::{Deserialize, Serialize};

/// A manifest permission of a mod.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    Panel,
    View,
    Browser,
    /// `browser.eval`, on top of `browser`. Off for the agent unless the user turns it on (S4).
    Eval,
    Convo,
    Tools,
    /// Badges and progress on a tab (`tabs.badge.*`, `tabs.progress.*`). The agent holds it for its own tab;
    /// a protocol 1 mod cannot ask for it (`flare_mods::manifest::ASKABLE_V1` leaves it out), a plugin can.
    Decorate,
    /// Every tab, not only the one that shows the plugin: see them all and act on any of them.
    Tabs,
    /// The plugin's own sections in the sidebar (`sidebar.*`).
    Sidebar,
    /// The plugin's buttons and menu items, and their click events (`actions.*`).
    Actions,
    /// The plugin's strip under a tab's terminal (`bar.*`).
    Bar,
    /// Read the conversation of Claude sessions (`session.*`, and the `session` event).
    Sessions,
    /// Type into a tab's shell (`tabs.input`). Full control, and never the agent's (rules 2 and 188).
    Input,
    /// The plugin's pages may reach the internet. No method of its own: it changes the pages' policy.
    Network,
    /// Flare starts the plugin's helper program. Full control, and no method of its own.
    Process,
}

impl Permission {
    /// Every permission, in documentation order.
    pub const ALL: [Permission; 15] = [
        Permission::Panel,
        Permission::View,
        Permission::Browser,
        Permission::Eval,
        Permission::Convo,
        Permission::Tools,
        Permission::Decorate,
        Permission::Tabs,
        Permission::Sidebar,
        Permission::Actions,
        Permission::Bar,
        Permission::Sessions,
        Permission::Input,
        Permission::Network,
        Permission::Process,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Permission::Panel => "panel",
            Permission::View => "view",
            Permission::Browser => "browser",
            Permission::Eval => "eval",
            Permission::Convo => "convo",
            Permission::Tools => "tools",
            Permission::Decorate => "decorate",
            Permission::Tabs => "tabs",
            Permission::Sidebar => "sidebar",
            Permission::Actions => "actions",
            Permission::Bar => "bar",
            Permission::Sessions => "sessions",
            Permission::Input => "input",
            Permission::Network => "network",
            Permission::Process => "process",
        }
    }

    pub fn parse(name: &str) -> Option<Permission> {
        Permission::ALL.into_iter().find(|p| p.name() == name)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Grant {
    /// No token. `eval` is the user's switch for `browser.eval` (default off).
    Agent { eval: bool },
    /// A mod's token: exactly these permissions. `plugin` is true for a protocol 2 plugin, false for a protocol 1
    /// mod, which keeps exactly what it had (no store).
    Mod {
        id: String,
        permissions: Vec<Permission>,
        plugin: bool,
    },
    /// The app's own UI, in-process only.
    Ui,
}

impl Grant {
    /// The agent grant with the default switches.
    pub fn agent() -> Grant {
        Grant::Agent { eval: false }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Grant::Agent { .. } => "agent",
            Grant::Mod { .. } => "mod",
            Grant::Ui => "ui",
        }
    }

    /// The permission names this grant holds, for the `hello` answer.
    pub fn permission_names(&self) -> Vec<&'static str> {
        match self {
            Grant::Agent { eval } => {
                let mut v = vec!["panel", "view", "browser", "convo", "decorate"];
                if *eval {
                    v.push("eval");
                }
                v
            }
            Grant::Mod { permissions, .. } => permissions.iter().map(|p| p.name()).collect(),
            Grant::Ui => Permission::ALL
                .iter()
                .map(|p| p.name())
                .chain(["ui"])
                .collect(),
        }
    }

    /// Whether this grant may name a tab other than its own in a call: the app, and a plugin that holds `tabs`.
    /// The agent and every other plugin act on the tab they sit in only (a plugin's background page, which sits in
    /// none, then acts on no tab). The app's dispatcher asks it for the decor methods, a plugin's own panel and the
    /// wide `tabs.list` (plugins phase 3).
    pub fn may_act_on_any_tab(&self) -> bool {
        match self {
            Grant::Ui => true,
            Grant::Agent { .. } => false,
            Grant::Mod { permissions, .. } => permissions.contains(&Permission::Tabs),
        }
    }

    /// Whether this grant may call `method`. The whole of rule S3 lives here.
    pub fn may_call(&self, method: Method) -> bool {
        let eval_ok = match self {
            Grant::Ui => true,
            Grant::Agent { eval } => *eval,
            Grant::Mod { permissions, .. } => permissions.contains(&Permission::Eval),
        };
        self.may_enter(method.namespace()) && (method != Method::BrowserEval || eval_ok)
    }

    /// Whether this grant reaches anything in the namespace (`browser.eval` asks for more, see [`Grant::may_call`]).
    /// A namespace with no method yet is decided here all the same, so the rule is set before its methods come.
    pub fn may_enter(&self, ns: Namespace) -> bool {
        use Namespace as N;
        match self {
            Grant::Ui => true,
            // `hello` and `tabs.list` are open to every connection; the rest depends on the grant.
            _ if matches!(ns, N::Hello | N::Tabs) => true,
            // Nothing in the Ui namespace (shell input, settings, mod management) is reachable
            // over the pipe by anyone, whatever they hold.
            _ if ns == N::Ui => false,
            Grant::Agent { .. } => match ns {
                // Decorating is for the caller's own tab only: the dispatcher holds it to that.
                N::Panel | N::View | N::Convo | N::Decor | N::Browser => true,
                // Answering a tool call is a mod's job, not the agent's.
                N::Tools => false,
                // Listing and calling the mods' tools is the agent's job.
                N::AgentTools => true,
                // The plugin surfaces and a plugin's store are a plugin's; reading a session is not the agent's, and
                // typing into a shell is never the agent's (rules 2 and 188).
                N::Sidebar | N::Actions | N::Bar | N::Session | N::Input | N::Storage => false,
                N::Hello | N::Tabs | N::Ui => unreachable!("handled above"),
            },
            Grant::Mod {
                permissions,
                plugin,
                ..
            } => match ns {
                // A mod cannot call another mod's tools, or its own: only the agent calls tools.
                N::AgentTools => false,
                // A plugin's own store needs no permission: it holds only what the plugin put there. A protocol 1
                // mod has none (the dispatcher checks that again, and keeps a plugin to its own store).
                N::Storage => *plugin,
                // Every other namespace is exactly its one permission.
                _ => ns.permission().is_some_and(|p| permissions.contains(&p)),
            },
        }
    }

    /// `Ok` when allowed, else the `forbidden` error the server sends.
    pub fn check(&self, method: Method) -> Result<(), CtlError> {
        if self.may_call(method) {
            return Ok(());
        }
        let why = match (self, method) {
            (Grant::Agent { .. }, Method::BrowserEval) => {
                "browser.eval is off; the user turns it on in Settings".to_string()
            }
            (_, m) if m.namespace() == Namespace::Ui => {
                format!("{} is for Flare's own interface only", m.name())
            }
            (g, m) => format!("the {} grant does not allow {}", g.kind(), m.name()),
        };
        Err(CtlError::new(code::FORBIDDEN, why))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Permission as P;

    fn mod_with(perms: &[Permission]) -> Grant {
        Grant::Mod {
            id: "demo".into(),
            permissions: perms.to_vec(),
            plugin: true,
        }
    }

    fn v1_mod_with(perms: &[Permission]) -> Grant {
        Grant::Mod {
            id: "demo".into(),
            permissions: perms.to_vec(),
            plugin: false,
        }
    }

    #[test]
    fn agent_can_show_and_drive_but_not_eval() {
        let g = Grant::agent();
        for m in [
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
            Method::ConvoState,
            Method::ModsTools,
            Method::ModsCall,
            Method::TabsBadgeSet,
            Method::TabsBadgeClear,
            Method::TabsProgressSet,
            Method::TabsProgressClear,
        ] {
            assert!(g.may_call(m), "agent should call {}", m.name());
        }
        assert!(!g.may_call(Method::BrowserEval));
        assert!(Grant::Agent { eval: true }.may_call(Method::BrowserEval));
    }

    /// Rules 2 and 188: the agent never types into a shell, by the app's own `shell.input` or by a plugin's
    /// `tabs.input`; it changes no setting, turns on no mod, reads no session and keeps no plugin store.
    #[test]
    fn rule_s3_the_agent_cannot_type_into_shells_change_settings_or_enable_mods() {
        for g in [Grant::agent(), Grant::Agent { eval: true }] {
            for m in [
                Method::ShellInput,
                Method::TabsInput,
                Method::SettingsSet,
                Method::ModsEnable,
                Method::ToolsRespond,
                Method::SessionRead,
                Method::StorageGet,
                Method::StorageSet,
            ] {
                let err = g.check(m).unwrap_err();
                assert_eq!(err.code, code::FORBIDDEN, "{}", m.name());
            }
        }
    }

    #[test]
    fn nothing_over_the_pipe_reaches_the_ui_namespace() {
        let all = Permission::ALL;
        for g in [Grant::agent(), Grant::Agent { eval: true }, mod_with(&all)] {
            for m in Method::ALL {
                if m.namespace() == Namespace::Ui {
                    assert!(!g.may_call(m), "{} {}", g.kind(), m.name());
                }
            }
        }
        for m in Method::ALL {
            assert!(Grant::Ui.may_call(m));
        }
    }

    #[test]
    fn a_mod_gets_exactly_its_permissions() {
        let g = mod_with(&[P::View]);
        assert!(g.may_call(Method::ViewImage));
        assert!(g.may_call(Method::ViewText));
        assert!(g.may_call(Method::Hello));
        assert!(g.may_call(Method::TabsList));
        for m in [
            Method::PanelOpen,
            Method::PanelClose,
            Method::BrowserOpen,
            Method::BrowserLook,
            Method::ConvoState,
            Method::ToolsRespond,
            Method::ShellInput,
        ] {
            assert!(!g.may_call(m), "{}", m.name());
        }
    }

    #[test]
    fn an_empty_mod_can_only_hello_list_and_keep_its_own_store() {
        let g = mod_with(&[]);
        for m in Method::ALL {
            let open = matches!(
                m,
                Method::Hello | Method::TabsList | Method::StorageGet | Method::StorageSet
            );
            assert_eq!(g.may_call(m), open, "{}", m.name());
        }
    }

    /// A protocol 1 mod keeps exactly what it had: the same methods as a plugin with the same permissions, less the
    /// store, so its `hello` lists no `storage.*` method it would only be refused.
    #[test]
    fn a_protocol_1_mod_has_no_store_and_otherwise_what_a_plugin_has() {
        let all = Permission::ALL;
        for perms in [&[][..], &[P::View, P::Tools][..], &all[..]] {
            let v1 = v1_mod_with(perms);
            let v2 = mod_with(perms);
            assert!(!v1.may_enter(Namespace::Storage));
            for m in Method::ALL {
                let want = v2.may_call(m) && m.namespace() != Namespace::Storage;
                assert_eq!(v1.may_call(m), want, "{}", m.name());
            }
            for m in [Method::StorageGet, Method::StorageSet] {
                assert_eq!(v1.check(m).unwrap_err().code, code::FORBIDDEN);
            }
        }
        let empty: Vec<Method> = Method::ALL
            .into_iter()
            .filter(|m| v1_mod_with(&[]).may_call(*m))
            .collect();
        assert_eq!(empty, [Method::Hello, Method::TabsList]);
    }

    /// The rule plugins keep: a caller with no permission reaches `hello`, its own-tab `tabs.list` and its own
    /// store (which holds only what it put there), and no method of any other namespace, the decor, session and
    /// input ones included.
    #[test]
    fn a_grant_with_no_permission_may_call_only_hello_tabs_list_and_its_own_store() {
        let g = mod_with(&[]);
        let open: Vec<Method> = Method::ALL.into_iter().filter(|m| g.may_call(*m)).collect();
        assert_eq!(
            open,
            [
                Method::Hello,
                Method::TabsList,
                Method::StorageGet,
                Method::StorageSet
            ]
        );
        for m in [Method::SessionRead, Method::TabsInput] {
            assert_eq!(
                g.check(m).unwrap_err().code,
                code::FORBIDDEN,
                "{}",
                m.name()
            );
        }
        for m in Method::ALL {
            if m.namespace() == Namespace::Decor {
                assert_eq!(
                    g.check(m).unwrap_err().code,
                    code::FORBIDDEN,
                    "{}",
                    m.name()
                );
            }
        }
    }

    #[test]
    fn decorate_unlocks_the_decor_methods_and_nothing_else() {
        let g = mod_with(&[P::Decorate]);
        for m in Method::ALL {
            let want = matches!(
                m.namespace(),
                Namespace::Hello | Namespace::Tabs | Namespace::Decor | Namespace::Storage
            );
            assert_eq!(g.may_call(m), want, "{}", m.name());
        }
        // The decor methods never ride on another permission.
        let others = mod_with(&[P::Panel, P::View, P::Browser, P::Eval, P::Convo, P::Tools]);
        assert!(!others.may_call(Method::TabsBadgeSet));
        assert!(!others.may_call(Method::TabsProgressSet));
    }

    #[test]
    fn browser_eval_needs_both_browser_and_eval() {
        assert!(!mod_with(&[P::Browser]).may_call(Method::BrowserEval));
        assert!(!mod_with(&[P::Eval]).may_call(Method::BrowserEval));
        assert!(!mod_with(&[P::Eval]).may_call(Method::BrowserLook));
        assert!(mod_with(&[P::Browser, P::Eval]).may_call(Method::BrowserEval));
        assert!(mod_with(&[P::Browser]).may_call(Method::BrowserClick));
    }

    #[test]
    fn tools_permission_unlocks_only_tools_respond() {
        let g = mod_with(&[P::Tools]);
        assert!(g.may_call(Method::ToolsRespond));
        assert!(!g.may_call(Method::ViewImage));
        // Not even the mods' own tools: only the agent calls a tool.
        assert!(!g.may_call(Method::ModsCall));
        assert!(!g.may_call(Method::ModsTools));
    }

    #[test]
    fn a_mod_without_browser_is_refused_every_browser_method_with_forbidden() {
        let g = mod_with(&[P::Panel, P::Tools]);
        for m in Method::ALL {
            if m.namespace() == Namespace::Browser {
                let e = g.check(m).unwrap_err();
                assert_eq!(e.code, code::FORBIDDEN, "{}", m.name());
            }
        }
    }

    #[test]
    fn forbidden_messages_say_why() {
        let e = Grant::agent().check(Method::BrowserEval).unwrap_err();
        assert!(e.message.contains("Settings"), "{}", e.message);
        let e = Grant::agent().check(Method::ShellInput).unwrap_err();
        assert!(e.message.contains("own interface"), "{}", e.message);
        let e = mod_with(&[]).check(Method::ViewFile).unwrap_err();
        assert!(e.message.contains("view.file"), "{}", e.message);
        assert!(Grant::agent().check(Method::ViewFile).is_ok());
    }

    #[test]
    fn permission_names_round_trip() {
        for p in Permission::ALL {
            assert_eq!(Permission::parse(p.name()), Some(p));
        }
        assert_eq!(Permission::parse("root"), None);
        assert_eq!(Permission::ALL.len(), 15);
        let mut names: Vec<&str> = Permission::ALL.iter().map(|p| p.name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), Permission::ALL.len(), "names are unique");
        assert_eq!(
            Grant::agent().permission_names(),
            ["panel", "view", "browser", "convo", "decorate"]
        );
        assert_eq!(
            Grant::Agent { eval: true }.permission_names(),
            ["panel", "view", "browser", "convo", "decorate", "eval"]
        );
    }

    /// What a grant says it holds must match what it can call (the `hello` answer shows both).
    #[test]
    fn permission_names_agree_with_what_each_grant_can_call() {
        let all = Permission::ALL;
        let grants = [
            Grant::agent(),
            Grant::Agent { eval: true },
            mod_with(&[]),
            mod_with(&[P::View, P::Convo]),
            mod_with(&[P::Decorate]),
            mod_with(&all),
            v1_mod_with(&[]),
            v1_mod_with(&all),
            Grant::Ui,
        ];
        for g in grants {
            let names = g.permission_names();
            for m in Method::ALL {
                let need = match m.namespace() {
                    Namespace::Hello | Namespace::Tabs => continue,
                    Namespace::Panel => "panel",
                    Namespace::View => "view",
                    Namespace::Browser if m == Method::BrowserEval => "eval",
                    Namespace::Browser => "browser",
                    Namespace::Convo => "convo",
                    Namespace::Tools => "tools",
                    Namespace::Decor => "decorate",
                    Namespace::Sidebar => "sidebar",
                    Namespace::Actions => "actions",
                    Namespace::Bar => "bar",
                    Namespace::Session => "sessions",
                    Namespace::Input => "input",
                    // Every plugin keeps its own store and the app reaches it; a protocol 1 mod and the agent never do.
                    Namespace::Storage => {
                        assert_eq!(
                            g.may_call(m),
                            matches!(g, Grant::Mod { plugin: true, .. } | Grant::Ui),
                            "{} {}",
                            g.kind(),
                            m.name()
                        );
                        continue;
                    }
                    // The agent and the app call mod tools; no permission name stands for it.
                    Namespace::AgentTools => {
                        assert_eq!(
                            g.may_call(m),
                            matches!(g, Grant::Agent { .. } | Grant::Ui),
                            "{} {}",
                            g.kind(),
                            m.name()
                        );
                        continue;
                    }
                    Namespace::Ui => "ui",
                };
                assert_eq!(
                    g.may_call(m),
                    names.contains(&need),
                    "{} {} needs {need}",
                    g.kind(),
                    m.name()
                );
            }
        }
    }

    /// The plugin permissions: each namespace a plugin may enter is opened by exactly one permission, the open
    /// `Tabs` namespace by none, and no permission opens the agent's tools or the app's own.
    #[test]
    fn each_namespace_is_opened_by_exactly_its_own_permission() {
        for ns in Namespace::ALL {
            for p in Permission::ALL {
                let g = mod_with(&[p]);
                let want = match ns {
                    Namespace::Hello | Namespace::Tabs | Namespace::Storage => true,
                    Namespace::AgentTools | Namespace::Ui => false,
                    _ => ns.permission() == Some(p),
                };
                assert_eq!(g.may_enter(ns), want, "{ns:?} with {}", p.name());
            }
            // Holding every permission but the namespace's own never opens it.
            if let Some(own) = ns.permission() {
                let rest: Vec<Permission> =
                    Permission::ALL.into_iter().filter(|p| *p != own).collect();
                assert!(!mod_with(&rest).may_enter(ns), "{ns:?}");
                assert!(mod_with(&[own]).may_enter(ns), "{ns:?}");
            }
        }
        assert_eq!(Namespace::Tabs.permission(), None);
        assert_eq!(Namespace::Storage.permission(), None);
        assert_eq!(Namespace::Input.permission(), Some(P::Input));
        assert_eq!(Namespace::Session.permission(), Some(P::Sessions));
        assert_eq!(Namespace::Sidebar.permission(), Some(P::Sidebar));
        assert_eq!(Namespace::Actions.permission(), Some(P::Actions));
        assert_eq!(Namespace::Bar.permission(), Some(P::Bar));
        assert_eq!(Namespace::Decor.permission(), Some(P::Decorate));
    }

    /// Three permissions open no namespace: `network` changes a page's policy, `process` starts the helper, and
    /// `tabs` is to widen the tab a call may name (a later phase). None of them is a key to a method of its own.
    #[test]
    fn network_process_and_tabs_open_no_namespace() {
        for p in [P::Network, P::Process, P::Tabs] {
            let g = mod_with(&[p]);
            for m in Method::ALL {
                assert_eq!(
                    g.may_call(m),
                    matches!(
                        m,
                        Method::Hello | Method::TabsList | Method::StorageGet | Method::StorageSet
                    ),
                    "{} with {}",
                    m.name(),
                    p.name()
                );
            }
            assert!(Namespace::ALL.iter().all(|ns| ns.permission() != Some(p)));
        }
    }

    /// With every permission a plugin can hold, nothing of the app's own or of the agent's tools opens; with none,
    /// a plugin reaches only `hello`, `tabs.list` and its own store in every namespace there is.
    #[test]
    fn the_widest_plugin_still_cannot_reach_the_ui_or_the_agents_tools() {
        let all = mod_with(&Permission::ALL);
        for ns in [Namespace::Ui, Namespace::AgentTools] {
            assert!(!all.may_enter(ns), "{ns:?}");
        }
        let none = mod_with(&[]);
        for ns in Namespace::ALL {
            assert_eq!(
                none.may_enter(ns),
                matches!(ns, Namespace::Hello | Namespace::Tabs | Namespace::Storage),
                "{ns:?}"
            );
        }
    }

    /// The agent never gets a plugin surface or a plugin's store, never reads a session and never types into a
    /// shell (rules 2 and 188).
    #[test]
    fn the_agent_enters_no_plugin_namespace() {
        for g in [Grant::agent(), Grant::Agent { eval: true }] {
            for ns in [
                Namespace::Sidebar,
                Namespace::Actions,
                Namespace::Bar,
                Namespace::Session,
                Namespace::Input,
                Namespace::Storage,
            ] {
                assert!(!g.may_enter(ns), "{ns:?}");
            }
        }
        for ns in Namespace::ALL {
            assert!(Grant::Ui.may_enter(ns), "{ns:?}");
        }
    }

    #[test]
    fn only_tabs_lets_a_plugin_name_another_tab() {
        assert!(!Grant::agent().may_act_on_any_tab());
        assert!(!Grant::Agent { eval: true }.may_act_on_any_tab());
        assert!(Grant::Ui.may_act_on_any_tab());
        assert!(mod_with(&[P::Tabs]).may_act_on_any_tab());
        let rest: Vec<Permission> = Permission::ALL
            .into_iter()
            .filter(|p| *p != P::Tabs)
            .collect();
        assert!(!mod_with(&rest).may_act_on_any_tab());
    }
}

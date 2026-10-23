//! Grants: who a connection is, and the pure rule that decides whether it may call a method.
//!
//! Three kinds. The **agent** grant is what a caller with no token gets (Claude Code through
//! `flare-mcp`): it can show content and drive its own tab's browser panel, and nothing else (rule
//! S3). A **mod** grant is exactly the permissions its manifest declared and the user accepted (S5).
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
}

impl Permission {
    pub fn name(self) -> &'static str {
        match self {
            Permission::Panel => "panel",
            Permission::View => "view",
            Permission::Browser => "browser",
            Permission::Eval => "eval",
            Permission::Convo => "convo",
            Permission::Tools => "tools",
        }
    }

    pub fn parse(name: &str) -> Option<Permission> {
        [
            Permission::Panel,
            Permission::View,
            Permission::Browser,
            Permission::Eval,
            Permission::Convo,
            Permission::Tools,
        ]
        .into_iter()
        .find(|p| p.name() == name)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Grant {
    /// No token. `eval` is the user's switch for `browser.eval` (default off).
    Agent { eval: bool },
    /// A mod's token: exactly these permissions.
    Mod {
        id: String,
        permissions: Vec<Permission>,
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
                let mut v = vec!["panel", "view", "browser", "convo"];
                if *eval {
                    v.push("eval");
                }
                v
            }
            Grant::Mod { permissions, .. } => permissions.iter().map(|p| p.name()).collect(),
            Grant::Ui => vec!["panel", "view", "browser", "eval", "convo", "tools", "ui"],
        }
    }

    /// Whether this grant may call `method`. The whole of rule S3 lives here.
    pub fn may_call(&self, method: Method) -> bool {
        use Namespace as N;
        let ns = method.namespace();
        match self {
            Grant::Ui => true,
            // `hello` and `tabs.list` are open to every connection; the rest depends on the grant.
            _ if matches!(ns, N::Hello | N::Tabs) => true,
            // Nothing in the Ui namespace (shell input, settings, mod management) is reachable
            // over the pipe by anyone, whatever they hold.
            _ if ns == N::Ui => false,
            Grant::Agent { eval } => match ns {
                N::Panel | N::View | N::Convo => true,
                N::Browser => method != Method::BrowserEval || *eval,
                // Answering a tool call is a mod's job, not the agent's.
                N::Tools => false,
                N::Hello | N::Tabs | N::Ui => unreachable!("handled above"),
            },
            Grant::Mod { permissions, .. } => {
                let has = |p: Permission| permissions.contains(&p);
                match ns {
                    N::Panel => has(Permission::Panel),
                    N::View => has(Permission::View),
                    N::Browser => {
                        has(Permission::Browser)
                            && (method != Method::BrowserEval || has(Permission::Eval))
                    }
                    N::Convo => has(Permission::Convo),
                    N::Tools => has(Permission::Tools),
                    N::Hello | N::Tabs | N::Ui => unreachable!("handled above"),
                }
            }
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
        ] {
            assert!(g.may_call(m), "agent should call {}", m.name());
        }
        assert!(!g.may_call(Method::BrowserEval));
        assert!(Grant::Agent { eval: true }.may_call(Method::BrowserEval));
    }

    #[test]
    fn rule_s3_the_agent_cannot_type_into_shells_change_settings_or_enable_mods() {
        for g in [Grant::agent(), Grant::Agent { eval: true }] {
            for m in [
                Method::ShellInput,
                Method::SettingsSet,
                Method::ModsEnable,
                Method::ToolsRespond,
            ] {
                let err = g.check(m).unwrap_err();
                assert_eq!(err.code, code::FORBIDDEN, "{}", m.name());
            }
        }
    }

    #[test]
    fn nothing_over_the_pipe_reaches_the_ui_namespace() {
        let all = [P::Panel, P::View, P::Browser, P::Eval, P::Convo, P::Tools];
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
    fn an_empty_mod_can_only_hello_and_list() {
        let g = mod_with(&[]);
        for m in Method::ALL {
            let open = matches!(m, Method::Hello | Method::TabsList);
            assert_eq!(g.may_call(m), open, "{}", m.name());
        }
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
        for p in [P::Panel, P::View, P::Browser, P::Eval, P::Convo, P::Tools] {
            assert_eq!(Permission::parse(p.name()), Some(p));
        }
        assert_eq!(Permission::parse("root"), None);
        assert_eq!(
            Grant::agent().permission_names(),
            ["panel", "view", "browser", "convo"]
        );
    }

    /// What a grant says it holds must match what it can call (the `hello` answer shows both).
    #[test]
    fn permission_names_agree_with_what_each_grant_can_call() {
        let all = [P::Panel, P::View, P::Browser, P::Eval, P::Convo, P::Tools];
        let grants = [
            Grant::agent(),
            Grant::Agent { eval: true },
            mod_with(&[]),
            mod_with(&[P::View, P::Convo]),
            mod_with(&all),
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
}

//! A plugin's question in Flare's own dialog (the `fill` permission, 2026-10-28), centred in the plugin's own look
//! (2026-11-08): a login to fill into a tab's web page, approved with Fill, Deny or "Deny for this session", and a
//! secret the plugin asks for, typed into a masked field. One question at a time; it waits at most [`fill::ASK_WAIT`]
//! and then counts as a no. A plugin the person set to "Always allow" on its details sheet has its fill typed with
//! no question; every other check (the site, the tab, a waiting call, the page that moved) is made just the same.
//!
//! The call's thread checked the plugin, the params, the tab and the plugin's asks; here the window checks the page's
//! site against the login's addresses (again after Allow, since the page may have moved), and only Allow's pointer
//! press sends the values to the page, through `ExecuteScript` with the values as JSON data
//! (`flare_mods::fill::fill_script`). A secret goes back to the plugin that asked and nowhere else. The one log line
//! of each has the plugin, the page's host and the outcome: never a value, a login's name or the typed text.

use std::sync::Arc;
use std::sync::mpsc::Sender;
use std::time::Duration;

use flare_ctl::{CtlError, code};
use flare_mods::fill::{self, Secret, Site, Values};
use serde_json::{Value, json};
use slint::SharedString;

use super::{AppCtl, UiMsg};
use crate::ctl_app::CtlOp;
use crate::mods_ctl::{ModsMsg, ModsMsgBox};

type Reply = Sender<Result<Value, CtlError>>;

const ASK_TIMER: &str = "plugins.ask";
/// How long a fill's script may take once Allow is pressed.
const FILL_RUN_WAIT: Duration = Duration::from_secs(10);

/// What a plugin asks.
pub(crate) enum AskWhat {
    /// A login for the page of `tab`, which was on `site` when the dialog went up. `via_call`: the plugin may fill
    /// the tab only while the agent's call it answers still waits.
    Fill {
        tab: String,
        site: Site,
        values: Values,
        via_call: bool,
    },
    Secret,
}

/// The plugin's question on the screen (or, once Allow is pressed, its fill running in the page).
pub(crate) struct PluginAsk {
    pub(crate) plugin: String,
    generation: u64,
    what: AskWhat,
    reply: Reply,
    seq: u64,
    /// The fill's script runs (Allow was pressed): its id. The dialog is down by then.
    running: Option<u64>,
}

impl PluginAsk {
    fn host(&self) -> &str {
        match &self.what {
            AskWhat::Fill { site, .. } => &site.host,
            AskWhat::Secret => "",
        }
    }
}

/// The plain words of what a fill did, for its log line: kinds of field, never a value.
fn filled_words(filled: &[&str], missing: &[&str]) -> String {
    let mut s = if filled.is_empty() {
        "no field filled".to_string()
    } else {
        format!("filled {}", filled.join(", "))
    };
    if !missing.is_empty() {
        s.push_str(&format!("; no field for {}", missing.join(", ")));
    }
    s
}

impl AppCtl {
    /// `browser.fill.page`: the address of the tab's web page, `{tab, url, host}`.
    pub(super) fn ctl_fill_page(&mut self, tab: &str) -> Result<Value, CtlError> {
        let url = self.fill_page_url(tab, false)?;
        let host = fill::page_site(&url).map(|s| s.host);
        Ok(json!({"tab": tab, "url": url, "host": host}))
    }

    /// The address of the tab's web page now (the browser's own when it is up). A mini-app, a plugin's page and a
    /// panel of another kind are no web page. `loaded` asks for a page that is up in a browser (a fill types into it).
    fn fill_page_url(&self, tab: &str, loaded: bool) -> Result<String, CtlError> {
        if !self.is_shell_tab(tab) {
            return Err(CtlError::new(code::NO_TAB, "that tab is closed"));
        }
        if self.panels.is_app(tab) || self.panels.mod_of(tab).is_some() {
            return Err(CtlError::new(
                code::NOT_FOUND,
                "the panel in this tab is not a web page",
            ));
        }
        let Some(target) = self.panels.web_target(tab) else {
            return Err(CtlError::new(
                code::NOT_FOUND,
                "no web page is open in this tab",
            ));
        };
        if self.web.has_tab(tab) {
            let now = self.web.snapshot(tab).url;
            if !now.is_empty() {
                return Ok(now);
            }
        }
        if loaded {
            return Err(CtlError::new(
                code::NOT_FOUND,
                "the tab's web page is not loaded; bring the tab to the front",
            ));
        }
        Ok(target)
    }

    /// The plugin is on, in the generation the call was checked in.
    fn plugin_live(&self, plugin: &str, generation: u64) -> bool {
        self.mods.as_ref().is_some_and(|m| {
            m.hub().grant_of_mod(plugin).is_some() && m.hub().generation(plugin) == generation
        })
    }

    /// `browser.fill` and `secret.ask` on the window's side: the question goes up, unless one is up already (`busy`).
    pub(super) fn ctl_plugin_ask(&mut self, op: CtlOp, reply: Reply) {
        if self.plugin_ask.is_some() {
            let _ = reply.send(Err(CtlError::new(
                code::BUSY,
                "another plugin question is open; Flare asks one at a time",
            )));
            return;
        }
        match op {
            CtlOp::Fill {
                plugin,
                plugin_name,
                tab,
                login_name,
                login_uris,
                values,
                via_call,
                generation,
            } => {
                if !self.plugin_live(&plugin, generation) {
                    let _ = reply.send(Err(CtlError::new(
                        code::UNAUTHORIZED,
                        "the plugin was turned off",
                    )));
                    return;
                }
                let url = match self.fill_page_url(&tab, true) {
                    Ok(u) => u,
                    Err(e) => {
                        let _ = reply.send(Err(e));
                        return;
                    }
                };
                let site = match fill::check_login_for(&url, &login_uris) {
                    Ok(s) => s,
                    Err(e) => {
                        let host = fill::page_site(&url).map_or_else(String::new, |s| s.host);
                        log::info!(
                            "{}",
                            fill::log_line(&plugin, &host, "refused, not this login's site")
                        );
                        let _ = reply.send(Err(e));
                        return;
                    }
                };
                let tab_title = self
                    .tab_facts()
                    .into_iter()
                    .find(|f| f.id == tab)
                    .map(|f| f.title)
                    .unwrap_or_default();
                let view = crate::plugin_ask::fill_view(
                    &plugin_name,
                    &login_name,
                    &site.host,
                    &tab_title,
                    via_call,
                );
                let always = self
                    .mods
                    .as_ref()
                    .is_some_and(|m| m.hub().fills_without_asking(&plugin));
                let ask = PluginAsk {
                    plugin,
                    generation,
                    what: AskWhat::Fill {
                        tab,
                        site,
                        values,
                        via_call,
                    },
                    reply,
                    seq: 0,
                    running: None,
                };
                if always {
                    // "Always allow": no question, the fill runs as if Fill were pressed now. The person's choice is
                    // theirs: the log says what the fill did, as for any fill, and nothing of the choice.
                    self.plugin_ask_seq += 1;
                    let mut ask = ask;
                    ask.seq = self.plugin_ask_seq;
                    self.run_fill(ask);
                    return;
                }
                self.open_plugin_ask(ask, &view);
            }
            CtlOp::SecretAsk {
                plugin,
                plugin_name,
                title,
                label,
                sub,
                button,
                generation,
            } => {
                if !self.plugin_live(&plugin, generation) {
                    let _ = reply.send(Err(CtlError::new(
                        code::UNAUTHORIZED,
                        "the plugin was turned off",
                    )));
                    return;
                }
                let view =
                    crate::plugin_ask::secret_view(&plugin_name, &title, &label, &sub, &button);
                let ask = PluginAsk {
                    plugin,
                    generation,
                    what: AskWhat::Secret,
                    reply,
                    seq: 0,
                    running: None,
                };
                self.open_plugin_ask(ask, &view);
            }
            _ => {
                let _ = reply.send(Err(CtlError::new(code::INTERNAL, "not a plugin question")));
            }
        }
    }

    fn open_plugin_ask(&mut self, mut ask: PluginAsk, v: &crate::plugin_ask::AskView) {
        self.plugin_ask_seq += 1;
        ask.seq = self.plugin_ask_seq;
        let seq = ask.seq;
        let ask_plugin = ask.plugin.clone();
        self.plugin_ask = Some(ask);
        if let Some(app) = self.app() {
            self.clear_tip();
            self.close_menu_quietly();
            let brand = self
                .mods
                .as_ref()
                .and_then(|m| m.hub().found(&ask_plugin))
                .and_then(|f| f.manifest.brand);
            app.set_pask_kind(v.kind);
            app.set_pask_title(v.title.as_str().into());
            app.set_pask_sub(v.sub.as_str().into());
            app.set_pask_label(v.label.as_str().into());
            app.set_pask_button(v.button.as_str().into());
            app.set_pask_site(v.site.as_str().into());
            app.set_pask_asker(v.asker.as_str().into());
            app.set_pask_letter(v.letter.as_str().into());
            app.set_pask_foot(v.foot.as_str().into());
            app.set_pask_look(crate::plugin_look::look_of(brand.as_ref()));
            // The question has the keys: a page that holds them gives them back (Escape must reach the question).
            self.web.release_focus(crate::window::hwnd_of(&app));
            app.set_pask_open(true);
        }
        self.refresh();
        self.arm_ask_timer(seq, fill::ASK_WAIT);
    }

    fn arm_ask_timer(&self, seq: u64, after: Duration) {
        let post = Arc::clone(&self.deps.post);
        let ms = u64::try_from(after.as_millis()).unwrap_or(60_000);
        crate::timer_registry::set(ASK_TIMER, ms, false, move || {
            post(UiMsg::Mods(ModsMsgBox(ModsMsg::AskExpired { seq })));
        });
    }

    fn close_plugin_ask_view(&mut self) {
        if let Some(app) = self.app() {
            app.set_pask_open(false);
        }
        self.refresh();
        self.focus_terminal();
    }

    /// The person answered a fill: 0 Deny, 1 Allow, 2 Deny for this session. Only a press in Flare's own dialog
    /// calls this (Allow takes the pointer only).
    pub(super) fn answer_fill(&mut self, choice: i32) {
        let is_fill = self
            .plugin_ask
            .as_ref()
            .is_some_and(|a| matches!(a.what, AskWhat::Fill { .. }) && a.running.is_none());
        if !is_fill {
            return;
        }
        let Some(ask) = self.plugin_ask.take() else {
            return;
        };
        self.close_plugin_ask_view();
        match choice {
            1 => self.run_fill(ask),
            2 => {
                if let Some(m) = self.mods.as_ref() {
                    m.hub().asks().deny_for_session(&ask.plugin);
                }
                log::info!(
                    "{}",
                    fill::log_line(&ask.plugin, ask.host(), "denied for this session")
                );
                let _ = ask
                    .reply
                    .send(Ok(json!({"declined": true, "for_session": true})));
            }
            _ => {
                log::info!("{}", fill::log_line(&ask.plugin, ask.host(), "denied"));
                let _ = ask.reply.send(Ok(json!({"declined": true})));
            }
        }
    }

    /// Allow: the page, still on the site the person approved, gets the values.
    fn run_fill(&mut self, mut ask: PluginAsk) {
        let AskWhat::Fill {
            tab,
            site,
            values,
            via_call,
        } = &ask.what
        else {
            return;
        };
        if !self.plugin_live(&ask.plugin, ask.generation) {
            let _ = ask.reply.send(Err(CtlError::new(
                code::UNAUTHORIZED,
                "the plugin was turned off",
            )));
            return;
        }
        // A tab reached through an agent's call is filled only while that call still waits: a late Allow fills nothing.
        let call_ended = *via_call
            && !self
                .mods
                .as_ref()
                .is_some_and(|m| m.hub().has_call_from(&ask.plugin, tab));
        if call_ended {
            log::info!(
                "{}",
                fill::log_line(&ask.plugin, &site.host, "refused, the agent's call ended")
            );
            let _ = ask.reply.send(Err(CtlError::new(
                code::FORBIDDEN,
                "the agent's call ended before the fill was allowed",
            )));
            return;
        }
        let same = self
            .fill_page_url(tab, true)
            .ok()
            .and_then(|u| fill::page_site(&u))
            .is_some_and(|now| now == *site);
        if !same {
            log::info!(
                "{}",
                fill::log_line(&ask.plugin, &site.host, "refused, the page moved")
            );
            let _ = ask.reply.send(Err(CtlError::new(
                code::INTERRUPTED,
                "the page moved to another address before it was filled",
            )));
            return;
        }
        let script = fill::fill_script(values, site);
        match self.web.start_fill(tab, script.expose()) {
            Some(id) => {
                ask.running = Some(id);
                let seq = ask.seq;
                self.plugin_ask = Some(ask);
                self.arm_ask_timer(seq, FILL_RUN_WAIT);
            }
            None => {
                let _ = ask.reply.send(Err(CtlError::new(
                    code::NOT_FOUND,
                    "the tab's web page is not loaded",
                )));
            }
        }
    }

    /// The page answered a fill's script.
    pub(super) fn fill_done(&mut self, id: u64, result: Result<String, String>) {
        if !self
            .plugin_ask
            .as_ref()
            .is_some_and(|a| a.running == Some(id))
        {
            return;
        }
        let Some(ask) = self.plugin_ask.take() else {
            return;
        };
        // What the page said is read for the kinds of field only; the browser's own error is not passed on.
        let answer = match result {
            Ok(text) => fill::read_fill_answer(&text),
            Err(_) => Err(CtlError::new(
                code::UNAVAILABLE,
                "the page could not be filled",
            )),
        };
        let outcome = match &answer {
            Ok(f) => filled_words(&f.filled, &f.missing),
            Err(e) => format!("not filled ({})", e.code),
        };
        log::info!("{}", fill::log_line(&ask.plugin, ask.host(), &outcome));
        let _ = ask
            .reply
            .send(answer.map(|f| json!({"filled": f.filled, "missing": f.missing})));
    }

    /// The person answered a secret: OK with the text, or Cancel.
    pub(super) fn answer_secret(&mut self, ok: bool, text: SharedString) {
        let text = Secret::new(text.to_string());
        let is_secret = self
            .plugin_ask
            .as_ref()
            .is_some_and(|a| matches!(a.what, AskWhat::Secret));
        if !is_secret {
            return;
        }
        let Some(ask) = self.plugin_ask.take() else {
            return;
        };
        self.close_plugin_ask_view();
        let answer = if !ok {
            log::info!("secret.ask: plugin {}: cancelled", ask.plugin);
            Ok(json!({"cancelled": true}))
        } else if !self.plugin_live(&ask.plugin, ask.generation) {
            Err(CtlError::new(
                code::UNAUTHORIZED,
                "the plugin was turned off",
            ))
        } else {
            log::info!("secret.ask: plugin {}: answered", ask.plugin);
            Ok(json!({"text": text.expose()}))
        };
        let _ = ask.reply.send(answer);
    }

    /// The question's time ran out: a fill not answered counts as Deny, a secret as Cancel; a fill whose script never
    /// answered is a timeout.
    pub(super) fn plugin_ask_expired(&mut self, seq: u64) {
        if !self.plugin_ask.as_ref().is_some_and(|a| a.seq == seq) {
            return;
        }
        let Some(ask) = self.plugin_ask.take() else {
            return;
        };
        let answer = if ask.running.is_some() {
            Err(CtlError::new(
                code::TIMEOUT,
                "the page did not answer the fill in time",
            ))
        } else {
            self.close_plugin_ask_view();
            match ask.what {
                AskWhat::Fill { .. } => Ok(json!({"declined": true, "timed_out": true})),
                AskWhat::Secret => Ok(json!({"cancelled": true, "timed_out": true})),
            }
        };
        log::info!(
            "plugin {}: its question was not answered in time",
            ask.plugin
        );
        let _ = ask.reply.send(answer);
    }

    /// The plugin was turned off: its question goes, unanswered.
    pub(super) fn plugin_ask_off(&mut self, id: &str) {
        if !self.plugin_ask.as_ref().is_some_and(|a| a.plugin == id) {
            return;
        }
        let Some(ask) = self.plugin_ask.take() else {
            return;
        };
        if ask.running.is_none() {
            self.close_plugin_ask_view();
        }
        let _ = ask.reply.send(Err(CtlError::new(
            code::UNAUTHORIZED,
            "the plugin was turned off",
        )));
    }

    /// What the e2e reads of the plugin's question: whether one is up, its kind and plugin, whether its fill runs.
    pub fn e2e_plugin_ask(&self) -> Value {
        match &self.plugin_ask {
            None => json!({"asking": false}),
            Some(a) => json!({
                "asking": a.running.is_none(),
                "running": a.running.is_some(),
                "plugin": a.plugin,
                "kind": match a.what { AskWhat::Fill { .. } => "fill", AskWhat::Secret => "secret" },
                "host": a.host(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::filled_words;

    #[test]
    fn the_log_words_name_kinds_of_field_only() {
        assert_eq!(
            filled_words(&["username", "password"], &[]),
            "filled username, password"
        );
        assert_eq!(
            filled_words(&[], &["totp"]),
            "no field filled; no field for totp"
        );
    }
}

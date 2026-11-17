//! A plugin's question in Flare's own dialog (the `fill` permission, 2026-10-28), centred in the plugin's own look
//! (2026-11-08): a login to fill into a tab's web page, approved with Fill or Deny, and a
//! secret the plugin asks for, typed into a masked field. One question at a time; it waits at most [`fill::ASK_WAIT`]
//! and then counts as a no. A plugin the person set to "Always allow" on its details sheet has its fill typed with
//! no question; every other check (the site, the tab, a waiting call, the page that moved) is made just the same.
//!
//! The call's thread checked the plugin, the params, the tab and the plugin's asks; here the window checks the page's
//! site against the login's addresses (again after Allow, since the page may have moved), and only Allow's pointer
//! press sends the values to the page, through `ExecuteScript` with the values as JSON data
//! (`flare_mods::fill::fill_script`). A secret goes back to the plugin that asked and nowhere else. The one log line
//! of each has the plugin, the page's host and the outcome: never a value, a login's name or the typed text.
//!
//! A secret asked with `verify` (rule 1274, 2026-11-16) keeps its dialog open: the text goes to the plugin with the
//! ask's id and the dialog shows its checking state until the plugin's `secret.result` says right (a green check, then
//! the dialog closes after [`fill::RESULT_HOLD`]), wrong (the field comes back in place with the red line, and that call
//! waits for the next try), or done (the plugin's plain end line, closed after [`fill::END_HOLD`]). A check with no word
//! in [`fill::VERIFY_WAIT`] puts the field back with Flare's own red line; the text sent after that waits for the
//! plugin's next `secret.result`. Cancel while checking closes the dialog; the plugin's next `secret.result` for that
//! ask answers `{cancelled: true}`.

use std::sync::Arc;
use std::sync::mpsc::Sender;
use std::time::Duration;

use flare_ctl::{CtlError, code};
use flare_mods::fill::{self, Secret, Site, Values, Verdict};
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
    /// A secret to type; `verify` when the plugin checks it with the dialog still open (rule 1274).
    Secret { verify: Option<Verify> },
}

/// Where a checked secret's dialog is (rule 1274).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stage {
    /// The field, waiting for the person.
    Input,
    /// What was sent is being checked by the plugin: the field read-only, the button's spinner.
    Checking,
    /// A result (a green check, or the plugin's end line), closing by itself.
    Shown,
}

impl Stage {
    /// Its word in the e2e probe's answer.
    fn word(self) -> &'static str {
        match self {
            Stage::Input => "input",
            Stage::Checking => "checking",
            Stage::Shown => "shown",
        }
    }
}

/// A `secret.ask` with `verify`: its id, where its dialog is, and the text the person sent while no call of the
/// plugin waited (after a check ran out of time), kept for the plugin's next `secret.result`.
pub(crate) struct Verify {
    id: String,
    stage: Stage,
    held: Option<Secret>,
    /// The ask's own title, the heading of an end with none of its own.
    title: String,
}

/// The plugin's question on the screen (or, once Allow is pressed, its fill running in the page).
pub(crate) struct PluginAsk {
    pub(crate) plugin: String,
    generation: u64,
    what: AskWhat,
    /// The call that waits for the answer. A checked secret has one only while the plugin waits: its `secret.ask`
    /// until the first text is sent, then each wrong answer's `secret.result` until the next.
    reply: Option<Reply>,
    seq: u64,
    /// Numbers the stages of a checked secret, so the timer of an earlier stage ends nothing.
    round: u64,
    /// The fill's script runs (Allow was pressed): its id. The dialog is down by then.
    running: Option<u64>,
}

impl PluginAsk {
    fn host(&self) -> &str {
        match &self.what {
            AskWhat::Fill { site, .. } => &site.host,
            AskWhat::Secret { .. } => "",
        }
    }

    /// Answers the waiting call, if one waits. Gives back whether one did.
    fn answer(&mut self, r: Result<Value, CtlError>) -> bool {
        match self.reply.take() {
            Some(tx) => {
                let _ = tx.send(r);
                true
            }
            None => false,
        }
    }

    fn verify(&self) -> Option<&Verify> {
        match &self.what {
            AskWhat::Secret { verify } => verify.as_ref(),
            AskWhat::Fill { .. } => None,
        }
    }

    fn verify_mut(&mut self) -> Option<&mut Verify> {
        match &mut self.what {
            AskWhat::Secret { verify } => verify.as_mut(),
            AskWhat::Fill { .. } => None,
        }
    }
}

/// The answer that hands the person's text to the plugin; a checked secret's carries the ask's id.
fn text_answer(text: &Secret, id: Option<&str>) -> Value {
    match id {
        Some(id) => json!({"text": text.expose(), "ask": id}),
        None => json!({"text": text.expose()}),
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
                    reply: Some(reply),
                    seq: 0,
                    round: 0,
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
                title,
                label,
                sub,
                button,
                error,
                verify,
                checking,
                generation,
            } => {
                if !self.plugin_live(&plugin, generation) {
                    let _ = reply.send(Err(CtlError::new(
                        code::UNAUTHORIZED,
                        "the plugin was turned off",
                    )));
                    return;
                }
                let mut view =
                    crate::plugin_ask::secret_view(&title, &label, &sub, &button, &error);
                if verify {
                    view.checking = crate::plugin_ask::checking_line(&checking);
                }
                let verify = verify.then(|| Verify {
                    // Set to the ask's number as it goes up.
                    id: String::new(),
                    stage: Stage::Input,
                    held: None,
                    title: view.title.clone(),
                });
                let ask = PluginAsk {
                    plugin,
                    generation,
                    what: AskWhat::Secret { verify },
                    reply: Some(reply),
                    seq: 0,
                    round: 0,
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
        let verify = match ask.verify_mut() {
            Some(vf) => {
                vf.id = format!("s{seq}");
                true
            }
            None => false,
        };
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
            app.set_pask_error(v.error.as_str().into());
            app.set_pask_label(v.label.as_str().into());
            app.set_pask_button(v.button.as_str().into());
            app.set_pask_site(v.site.as_str().into());
            app.set_pask_asker(v.asker.as_str().into());
            app.set_pask_letter(v.letter.as_str().into());
            app.set_pask_look(crate::plugin_look::look_of(brand.as_ref()));
            app.set_pask_stage(0);
            app.set_pask_verify(verify);
            app.set_pask_checking(v.checking.as_str().into());
            app.set_pask_result_title(SharedString::new());
            app.set_pask_result_line(SharedString::new());
            // The question has the keys: a page that holds them gives them back (Escape must reach the question).
            self.web.release_focus(crate::window::hwnd_of(&app));
            app.set_pask_open(true);
        }
        self.refresh();
        self.arm_ask_timer(seq, 0, fill::ASK_WAIT);
    }

    fn arm_ask_timer(&self, seq: u64, round: u64, after: Duration) {
        let post = Arc::clone(&self.deps.post);
        let ms = u64::try_from(after.as_millis()).unwrap_or(60_000);
        crate::timer_registry::set(ASK_TIMER, ms, false, move || {
            post(UiMsg::Mods(ModsMsgBox(ModsMsg::AskExpired { seq, round })));
        });
    }

    /// A checked secret moves to `stage` (the dialog's 0 the field, 1 checking, 2 right, 3 an end) and its timer is
    /// set again for the new stage.
    fn set_verify_stage(&mut self, stage: Stage, shown: i32, after: Duration) {
        let Some(ask) = self.plugin_ask.as_mut() else {
            return;
        };
        if let Some(v) = ask.verify_mut() {
            v.stage = stage;
        }
        ask.round += 1;
        let (seq, round) = (ask.seq, ask.round);
        if let Some(app) = self.app() {
            app.set_pask_stage(shown);
        }
        self.refresh();
        self.arm_ask_timer(seq, round, after);
    }

    /// A checked secret ends with no call of the plugin waiting (a cancel, or a timeout): its next `secret.result`
    /// for this ask gets `answer`.
    fn end_verify_unheard(&mut self, mut ask: PluginAsk, answer: Value) {
        let id = ask.verify().map(|v| v.id.clone());
        if !ask.answer(Ok(answer.clone()))
            && let Some(id) = id
        {
            self.plugin_ask_ended = Some((ask.plugin.clone(), id, answer));
        }
    }

    fn close_plugin_ask_view(&mut self) {
        if let Some(app) = self.app() {
            app.set_pask_open(false);
        }
        self.refresh();
        self.focus_terminal();
    }

    /// The person answered a fill: 1 Allow, anything else Deny. Only a press in Flare's own dialog calls this (Allow
    /// takes the pointer only).
    pub(super) fn answer_fill(&mut self, choice: i32) {
        let is_fill = self
            .plugin_ask
            .as_ref()
            .is_some_and(|a| matches!(a.what, AskWhat::Fill { .. }) && a.running.is_none());
        if !is_fill {
            return;
        }
        let Some(mut ask) = self.plugin_ask.take() else {
            return;
        };
        self.close_plugin_ask_view();
        if choice == 1 {
            self.run_fill(ask);
        } else {
            log::info!("{}", fill::log_line(&ask.plugin, ask.host(), "denied"));
            ask.answer(Ok(json!({"declined": true})));
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
            ask.answer(Err(CtlError::new(
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
            ask.answer(Err(CtlError::new(
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
            ask.answer(Err(CtlError::new(
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
                self.arm_ask_timer(seq, 0, FILL_RUN_WAIT);
            }
            None => {
                ask.answer(Err(CtlError::new(
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
        let Some(mut ask) = self.plugin_ask.take() else {
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
        ask.answer(answer.map(|f| json!({"filled": f.filled, "missing": f.missing})));
    }

    /// The person answered a secret: OK with the text, or Cancel.
    pub(super) fn answer_secret(&mut self, ok: bool, text: SharedString) {
        let text = Secret::new(text.to_string());
        let stage = match self.plugin_ask.as_ref().map(|a| &a.what) {
            Some(AskWhat::Secret { verify }) => verify.as_ref().map(|v| v.stage),
            _ => return,
        };
        if let Some(stage) = stage {
            self.answer_checked_secret(stage, ok, text);
            return;
        }
        let Some(mut ask) = self.plugin_ask.take() else {
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
            Ok(text_answer(&text, None))
        };
        ask.answer(answer);
    }

    /// The person answered a checked secret (rule 1274) at `stage`: Cancel closes the dialog at any stage but a
    /// result; the button sends the text once, from the field, and the dialog stays up checking it.
    fn answer_checked_secret(&mut self, stage: Stage, ok: bool, text: Secret) {
        match (stage, ok) {
            // A result closes by itself; while checking the button is off.
            (Stage::Shown, _) | (Stage::Checking, true) => {}
            (_, false) => {
                let Some(ask) = self.plugin_ask.take() else {
                    return;
                };
                crate::timer_registry::clear(ASK_TIMER);
                self.close_plugin_ask_view();
                log::info!("secret.ask: plugin {}: cancelled", ask.plugin);
                self.end_verify_unheard(ask, json!({"cancelled": true}));
            }
            // An empty field sends nothing: the plugin would read it as a cancel and never answer, leaving the box
            // checking until it timed out.
            (Stage::Input, true) if text.expose().is_empty() => {}
            (Stage::Input, true) => {
                let live = self
                    .plugin_ask
                    .as_ref()
                    .is_some_and(|a| self.plugin_live(&a.plugin, a.generation));
                if !live {
                    if let Some(mut ask) = self.plugin_ask.take() {
                        self.close_plugin_ask_view();
                        ask.answer(Err(CtlError::new(
                            code::UNAUTHORIZED,
                            "the plugin was turned off",
                        )));
                    }
                    return;
                }
                self.set_verify_stage(Stage::Checking, 1, fill::VERIFY_WAIT);
                let Some(ask) = self.plugin_ask.as_mut() else {
                    return;
                };
                let id = ask.verify().map(|v| v.id.clone());
                if ask.answer(Ok(text_answer(&text, id.as_deref()))) {
                    log::info!("secret.ask: plugin {}: answered, checking", ask.plugin);
                } else if let Some(v) = ask.verify_mut() {
                    // No call waits (the last check ran out of time): the text waits for the plugin's next word.
                    v.held = Some(text);
                }
            }
        }
    }

    /// `secret.result`: the plugin's word on the text of its checked secret `id` (rule 1274). Right shows the green
    /// check and closes the dialog after [`fill::RESULT_HOLD`]; done shows the plugin's end line and closes after
    /// [`fill::END_HOLD`]; both answer `{shown: true}` at once. Wrong brings the field back in place with the red line
    /// and the call waits for the person's next try (`{text, ask}`, or `{cancelled: true}`); a text the person already
    /// sent after a check ran out of time answers it at once.
    pub(super) fn secret_result(
        &mut self,
        plugin: &str,
        generation: u64,
        id: &str,
        verdict: Verdict,
        reply: Reply,
    ) {
        let open = self.plugin_ask.as_ref().is_some_and(|a| {
            a.plugin == plugin
                && a.verify()
                    .is_some_and(|v| v.id == id && v.stage != Stage::Shown)
        });
        if !open {
            let answer = match self.plugin_ask_ended.take() {
                Some((p, i, answer)) if p == plugin && i == id => Ok(answer),
                other => {
                    self.plugin_ask_ended = other;
                    Err(CtlError::new(
                        code::NOT_FOUND,
                        "no question of this plugin with that id is open",
                    ))
                }
            };
            let _ = reply.send(answer);
            return;
        }
        if !self.plugin_live(plugin, generation) {
            let _ = reply.send(Err(CtlError::new(
                code::UNAUTHORIZED,
                "the plugin was turned off",
            )));
            return;
        }
        if self.plugin_ask.as_ref().is_some_and(|a| a.reply.is_some()) {
            let _ = reply.send(Err(CtlError::new(
                code::BUSY,
                "a call already waits for the person's answer to this question",
            )));
            return;
        }
        match verdict {
            Verdict::Right { title, line } => {
                let (title, line) = crate::plugin_ask::result_view(
                    &title,
                    &line,
                    crate::plugin_ask::DEFAULT_RIGHT_TITLE,
                );
                self.show_verify_result(2, &title, &line, fill::RESULT_HOLD);
                log::info!("secret.ask: plugin {plugin}: checked, right");
                let _ = reply.send(Ok(json!({"shown": true})));
            }
            Verdict::End { title, line } => {
                let fallback = self
                    .plugin_ask
                    .as_ref()
                    .and_then(PluginAsk::verify)
                    .map(|v| v.title.clone())
                    .unwrap_or_default();
                let (title, line) = crate::plugin_ask::result_view(&title, &line, &fallback);
                self.show_verify_result(3, &title, &line, fill::END_HOLD);
                log::info!("secret.ask: plugin {plugin}: checked, ended");
                let _ = reply.send(Ok(json!({"shown": true})));
            }
            Verdict::Wrong { error } => {
                log::info!("secret.ask: plugin {plugin}: checked, wrong");
                let Some(ask) = self.plugin_ask.as_mut() else {
                    return;
                };
                let held = ask.verify_mut().and_then(|v| v.held.take());
                if let Some(text) = held {
                    // The person sent the next try while the plugin was late: it is checked now.
                    let id = ask.verify().map(|v| v.id.clone());
                    let _ = reply.send(Ok(text_answer(&text, id.as_deref())));
                    self.set_verify_stage(Stage::Checking, 1, fill::VERIFY_WAIT);
                    return;
                }
                ask.reply = Some(reply);
                let line = crate::plugin_ask::wrong_line(&error);
                if let Some(app) = self.app() {
                    app.set_pask_error(line.as_str().into());
                }
                self.set_verify_stage(Stage::Input, 0, fill::ASK_WAIT);
            }
        }
    }

    /// A checked secret's result: `shown` 2 the green check, 3 the plugin's end line; the dialog closes after `hold`.
    fn show_verify_result(&mut self, shown: i32, title: &str, line: &str, hold: Duration) {
        if let Some(ask) = self.plugin_ask.as_mut()
            && let Some(v) = ask.verify_mut()
        {
            v.held = None;
        }
        if let Some(app) = self.app() {
            app.set_pask_result_title(title.into());
            app.set_pask_result_line(line.into());
        }
        self.set_verify_stage(Stage::Shown, shown, hold);
    }

    /// The question's time ran out: a fill not answered counts as Deny, a secret as Cancel; a fill whose script never
    /// answered is a timeout. A checked secret (rule 1274): a check with no word goes back to the field with Flare's
    /// red line, and a result closes.
    pub(super) fn plugin_ask_expired(&mut self, seq: u64, round: u64) {
        if !self
            .plugin_ask
            .as_ref()
            .is_some_and(|a| a.seq == seq && a.round == round)
        {
            return;
        }
        match self
            .plugin_ask
            .as_ref()
            .and_then(PluginAsk::verify)
            .map(|v| v.stage)
        {
            Some(Stage::Checking) => {
                log::info!(
                    "secret.ask: the plugin did not say in time whether the answer was right"
                );
                if let Some(v) = self.plugin_ask.as_mut().and_then(PluginAsk::verify_mut) {
                    v.held = None;
                }
                if let Some(app) = self.app() {
                    app.set_pask_error(crate::plugin_ask::TOOK_TOO_LONG.into());
                }
                self.set_verify_stage(Stage::Input, 0, fill::ASK_WAIT);
                return;
            }
            Some(Stage::Shown) => {
                self.plugin_ask = None;
                self.close_plugin_ask_view();
                return;
            }
            Some(Stage::Input) => {
                let Some(ask) = self.plugin_ask.take() else {
                    return;
                };
                self.close_plugin_ask_view();
                log::info!(
                    "plugin {}: its question was not answered in time",
                    ask.plugin
                );
                self.end_verify_unheard(ask, json!({"cancelled": true, "timed_out": true}));
                return;
            }
            None => {}
        }
        let Some(mut ask) = self.plugin_ask.take() else {
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
                AskWhat::Secret { .. } => Ok(json!({"cancelled": true, "timed_out": true})),
            }
        };
        log::info!(
            "plugin {}: its question was not answered in time",
            ask.plugin
        );
        ask.answer(answer);
    }

    /// The plugin was turned off: its question goes, unanswered.
    pub(super) fn plugin_ask_off(&mut self, id: &str) {
        if self
            .plugin_ask_ended
            .as_ref()
            .is_some_and(|(p, _, _)| p == id)
        {
            self.plugin_ask_ended = None;
        }
        if !self.plugin_ask.as_ref().is_some_and(|a| a.plugin == id) {
            return;
        }
        let Some(mut ask) = self.plugin_ask.take() else {
            return;
        };
        if ask.running.is_none() {
            crate::timer_registry::clear(ASK_TIMER);
            self.close_plugin_ask_view();
        }
        ask.answer(Err(CtlError::new(
            code::UNAUTHORIZED,
            "the plugin was turned off",
        )));
    }

    /// The open question's timer: its number and its stage's round (the tests play the timer).
    #[cfg(test)]
    pub(crate) fn plugin_ask_timer(&self) -> Option<(u64, u64)> {
        self.plugin_ask.as_ref().map(|a| (a.seq, a.round))
    }

    /// What the e2e reads of the plugin's question: whether one is up, its kind and plugin, whether its fill runs,
    /// and for a checked secret its stage (`input`, `checking`, `shown`) and whether a call of the plugin waits.
    pub fn e2e_plugin_ask(&self) -> Value {
        match &self.plugin_ask {
            None => json!({"asking": false}),
            Some(a) => {
                let mut v = json!({
                    "asking": a.running.is_none(),
                    "running": a.running.is_some(),
                    "plugin": a.plugin,
                    "kind": match a.what { AskWhat::Fill { .. } => "fill", AskWhat::Secret { .. } => "secret" },
                    "host": a.host(),
                    "verify": a.verify().is_some(),
                });
                if let Some(vf) = a.verify() {
                    v["stage"] = json!(vf.stage.word());
                    v["waiting"] = json!(a.reply.is_some());
                }
                v
            }
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

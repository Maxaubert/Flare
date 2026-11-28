//! The tab menu's "Claude sessions" rows in the window (rules 1380 to 1383): the rows, the picks, the busy question
//! in the dialog's turn, and the runner stepped on every drained batch of terminal events and on the timer. The
//! decisions are `crate::claude_sessions`'s; here the facts are read and its acts are typed.

use flare_agent::kinds::DetectedAgent;
use flare_tabs::tabs::TabKind;
use flare_term::handle::SessionKey;

use super::AppCtl;
use crate::claude_sessions::{
    Act, Mode, Outcome, Pick, Press, Runner, TabFacts, ask_words, busy_among, end_targets,
    menu_rows, restart_targets,
};
use crate::menus_ui::MenuRowData;

/// The busy question on the dialog: the tabs it is about (the idle ones too) and what they get.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ClaudeAsk {
    pub(crate) mode: Mode,
    pub(crate) tabs: Vec<String>,
}

impl AppCtl {
    /// What the rows know of one tab, read now.
    pub(crate) fn claude_facts(&self, id: &str) -> TabFacts {
        let local_tab = self
            .model
            .state
            .tabs
            .iter()
            .any(|t| t.id == id && t.kind == TabKind::Shell && t.remote.is_none());
        let handle = self.handle_of(id).filter(|h| h.shell_id().is_some());
        let (Some(h), true) = (
            handle,
            local_tab && !self.model.is_hidden(id) && !self.hub.is_remote(id),
        ) else {
            return TabFacts {
                id: id.to_string(),
                ..TabFacts::default()
            };
        };
        let here = self.hub.agent_here(id);
        let claude = here && self.hub.agent_kind(id) == Some(DetectedAgent::Claude);
        let armed = self.hub.armed_agent(id).is_some();
        let question = self.hub.question_up(id)
            || flare_agent::question::looks_like_question(&h.question_rows());
        TabFacts {
            id: id.to_string(),
            local: true,
            polled: self.hub.polled(id),
            claude,
            other_agent: !claude && (here || armed),
            busy: claude && (self.hub.working(id) || question),
            draft: claude
                && flare_agent::question::prompt_draft(
                    &h.typed_tail(flare_agent::indicator::SCREEN_TAIL_ROWS),
                ) == Some(true),
            at_prompt: self.hub.at_prompt(id),
            shell: h.shell_id(),
            session: self.hub.last_claude_session(id).map(str::to_string),
            prompts: self.hub.prompts_seen(id),
        }
    }

    /// Every tab's facts, in strip order.
    pub(crate) fn claude_all_facts(&self) -> Vec<TabFacts> {
        self.model
            .state
            .tabs
            .iter()
            .map(|t| self.claude_facts(&t.id))
            .collect()
    }

    /// The rows for `tab`'s menu (rule 1380): this tab's own (a shell tab only) and the "Claude sessions" flyout.
    pub(super) fn claude_menu_rows(&self, tab: &str, rows: &mut Vec<(MenuRowData, String)>) {
        let all = self.claude_all_facts();
        let this = all.iter().find(|t| t.id == tab && t.local);
        rows.extend(menu_rows(this, &all, rows.len()));
    }

    /// A pick in a tab's menu that is one of the "Claude sessions" rows: handled (true).
    pub(super) fn claude_menu_picked(&mut self, tab: &str, path: &str) -> bool {
        let Some(pick) = Pick::of_path(path) else {
            return false;
        };
        let all = self.claude_all_facts();
        match pick {
            Pick::EndTab => {
                let ids: Vec<String> = all
                    .iter()
                    .filter(|t| t.id == tab && t.can_end())
                    .map(|t| t.id.clone())
                    .collect();
                self.claude_end(ids, Mode::End);
            }
            Pick::StartTab => {
                let one: Vec<TabFacts> = all.into_iter().filter(|t| t.id == tab).collect();
                let acts = Runner::start(&one);
                self.claude_acts(acts);
            }
            Pick::EndAll => self.claude_end(end_targets(&all), Mode::End),
            Pick::StartAll => {
                let acts = Runner::start(&all);
                self.claude_acts(acts);
            }
            Pick::RestartAll => self.claude_end(restart_targets(&all), Mode::Restart),
        }
        self.refresh();
        true
    }

    /// Ends (or restarts) `ids`: at once when none is busy, else after the busy question (rule 1382). Nothing for no
    /// tab, and nothing while another question has the dialog.
    fn claude_end(&mut self, ids: Vec<String>, mode: Mode) {
        if ids.is_empty() {
            return;
        }
        let facts: Vec<TabFacts> = ids.iter().map(|id| self.claude_facts(id)).collect();
        if busy_among(&ids, &facts).is_empty() {
            self.claude_run_end(&ids, mode, false);
            return;
        }
        if self.ask.is_some()
            || self.claude_ask.is_some()
            || self.downloads.current().is_some()
            || self.app_asks.current().is_some()
        {
            return;
        }
        self.claude_ask = Some(ClaudeAsk { mode, tabs: ids });
        self.show_claude_ask();
    }

    fn claude_run_end(&mut self, ids: &[String], mode: Mode, interrupt: bool) {
        let facts: Vec<TabFacts> = ids.iter().map(|id| self.claude_facts(id)).collect();
        let now = self.now();
        let acts = self.claude_run.end(&facts, mode, interrupt, now);
        self.claude_acts(acts);
        self.claude_note_done();
        self.kick();
    }

    /// The busy question (rule 1382): "N sessions are working", Cancel, Wait (the default), End now.
    fn show_claude_ask(&mut self) {
        let Some(a) = self.claude_ask.clone() else {
            return;
        };
        let facts: Vec<TabFacts> = a.tabs.iter().map(|id| self.claude_facts(id)).collect();
        let busy = busy_among(&a.tabs, &facts).len().max(1);
        let w = ask_words(a.mode, busy);
        let Some(app) = self.app() else { return };
        self.clear_tip();
        self.close_menu_quietly();
        self.close_overflow_quietly();
        app.set_dialog_title(w.title.as_str().into());
        app.set_dialog_body(slint::StyledText::from_plain_text(w.body));
        app.set_dialog_primary(w.primary.into());
        app.set_dialog_alt_label(w.alt.into());
        app.set_dialog_danger(false);
        app.set_dialog_default_cancel(false);
        app.set_dialog_by_key(false);
        app.set_dialog_question(app.get_dialog_question().wrapping_add(1));
        self.web.release_focus(crate::window::hwnd_of(&app));
        app.set_dialog_open(true);
        self.refresh();
    }

    /// The busy question is on the dialog.
    pub(super) fn claude_ask_up(&self) -> bool {
        self.claude_ask.is_some()
    }

    /// The person answered the busy question: None is Cancel (nothing happens, the idle tabs included), `Some(false)`
    /// is Wait, `Some(true)` is End now (Escape first). A question waiting behind it is asked next.
    pub(super) fn answer_claude_ask(&mut self, choice: Option<bool>) {
        let Some(a) = self.claude_ask.take() else {
            return;
        };
        if let Some(app) = self.app() {
            app.set_dialog_alt_label("".into());
            app.set_dialog_open(false);
        }
        if let Some(interrupt) = choice {
            self.claude_run_end(&a.tabs, a.mode, interrupt);
        }
        self.show_download_ask();
        self.refresh();
        self.focus_terminal();
    }

    /// A close question took the dialog: the busy question goes, as a Cancel.
    pub(super) fn drop_claude_ask(&mut self) {
        if self.claude_ask.take().is_some()
            && let Some(app) = self.app()
        {
            app.set_dialog_alt_label("".into());
        }
    }

    /// Moves the ends and restarts on with the tabs as they are now.
    pub(super) fn claude_step(&mut self) {
        if !self.claude_run.active() {
            return;
        }
        let now = self.now();
        let mut run = std::mem::take(&mut self.claude_run);
        let acts = run.step(now, &|id| Some(self.claude_facts(id)));
        self.claude_run = run;
        self.claude_acts(acts);
        self.claude_note_done();
    }

    /// When the runner is due on its own.
    pub(super) fn claude_next_due(&self) -> Option<u64> {
        self.claude_run.next_due()
    }

    /// The tabs the runner let go are written to the log (a kept or given-up tab is the person's to look at).
    fn claude_note_done(&mut self) {
        for (tab, why) in self.claude_run.take_done() {
            let word = match why {
                Outcome::Done => "done",
                Outcome::KeptDraft => "kept-draft",
                Outcome::GaveUp => "gave-up",
                Outcome::Gone => "gone",
            };
            crate::diag_ctl::crumb(
                "claude-session-end",
                &[
                    ("tab", serde_json::Value::from(tab)),
                    ("outcome", serde_json::Value::from(word)),
                ],
            );
        }
    }

    /// Carries out the runner's acts: each key typed through the session intent, never as free text.
    fn claude_acts(&mut self, acts: Vec<Act>) {
        for act in acts {
            match act {
                Act::Press { tab, key } => {
                    if let Some(h) = self.handle_of(&tab) {
                        h.session_key(match key {
                            Press::Exit => SessionKey::Exit,
                            Press::Enter => SessionKey::Enter,
                            Press::Interrupt => SessionKey::Interrupt,
                        });
                    }
                }
                Act::Start { tab, shell, resume } => {
                    if let Some(h) = self.handle_of(&tab) {
                        h.session_key(SessionKey::Start {
                            shell: &shell,
                            resume: resume.as_deref(),
                        });
                    }
                }
                Act::LookSoon => self.hub.look_soon(),
            }
        }
    }

    /// The tabs the runner is at work on (the tests read it).
    #[cfg(test)]
    pub(crate) fn claude_running(&self) -> Vec<String> {
        self.claude_run.tabs()
    }
}

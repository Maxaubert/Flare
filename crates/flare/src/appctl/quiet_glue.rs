//! The quiet context in the window (rule 1268): the prompt hook of the tab's Claude asks `panel.state` with
//! `"context": true` (`CtlOp::PanelContext`), and the answer gains `context`, what the person marked in the side panel
//! right now. Nothing is typed and nothing is kept for later: a selection is handed on while it is selected, the drawn
//! arrow while the page keeps it (until a look sees it, the page moves on or five minutes pass).
//!
//! - **A file's selection** (a text file, or a markdown file's Code view) is known here at once.
//! - **A page's selection** is read from the page with the selection script, only on a plain web page or a markdown
//!   view's page (which runs no script). A mini-app's or a mod's page is never read: its own script could fake what
//!   "the person selected". The call waits for it at most [`PAGE_READ_MS`], then is answered without it. A web page's
//!   script can still set its own selection, so the hook words it as page content, never as Flare's words.
//! - **The drawn arrow** is where it is kept (`WebCtl::kept_arrow_place`); what it points at is not handed on.
//!
//! **Once per prompt.** The context is handed on only for a prompt the person sent: a plain Enter they pressed in the
//! terminal of the tab in front, with an agent there ([`AppCtl::quiet_prompt_seen`]). Each such Enter lets one context
//! call through, for that tab while it is in front; any other call (a repeat, a tool of the agent's own, a tab in the
//! back) gets the plain panel state. Flare sees no pointer or key event inside a web page (the browser takes them in a
//! window of its own), so it cannot tell whether a page's selection followed the person's own input.

use std::collections::{BTreeMap, HashMap};
use std::sync::mpsc::Sender;

use flare_ctl::CtlError;
use serde_json::{Value, json};

use super::AppCtl;
use crate::ctl_app::CtlOp;
use crate::panel_ctl::SelectionIn;
use crate::quiet_context::{self, Place};
use crate::web_ctl::KeptArrowPlace;

type Reply = Sender<Result<Value, CtlError>>;

/// How long a page has to say what is selected before the hook is answered without it (milliseconds). The hook waits
/// one second for its answer.
pub(crate) const PAGE_READ_MS: u64 = 400;

/// One hook call waiting for a page's selection: its answer so far.
pub(crate) struct QuietRead {
    reply: Reply,
    answer: Value,
}

/// The most prompts a tab keeps unused (Enters pressed while Claude was busy, queued as prompts).
const PROMPTS_MAX: u8 = 4;

/// The quiet context's state: the hook calls waiting for a page's selection, by the read's id, and per tab the
/// prompts the person sent that no context call has used yet.
#[derive(Default)]
pub(crate) struct QuietReads {
    waiting: BTreeMap<u64, QuietRead>,
    prompts: HashMap<String, u8>,
}

/// The place of a kept arrow, as the context names it.
fn arrow_place(place: &KeptArrowPlace) -> Place<'_> {
    match place {
        KeptArrowPlace::File(name) => Place::File { name, lines: None },
        KeptArrowPlace::Page { title, url } => Place::Page { title, url },
        KeptArrowPlace::Other => Place::Other,
    }
}

impl AppCtl {
    /// The person pressed a plain Enter in the terminal of `tab` (in front, with an agent there): one more prompt may
    /// get the quiet context.
    pub(crate) fn quiet_prompt_seen(&mut self, tab: &str) {
        let n = self.quiet_reads.prompts.entry(tab.to_string()).or_default();
        *n = n.saturating_add(1).min(PROMPTS_MAX);
    }

    /// Whether this context call may have the context: it uses one prompt the person sent in `tab`, and `tab` is in
    /// front. Every call uses one, so a call for a tab in the back still costs it.
    fn quiet_prompt_take(&mut self, tab: &str) -> bool {
        let had = match self.quiet_reads.prompts.get_mut(tab) {
            Some(n) if *n > 0 => {
                *n -= 1;
                if *n == 0 {
                    self.quiet_reads.prompts.remove(tab);
                }
                true
            }
            _ => false,
        };
        had && self.front_id().as_deref() == Some(tab)
    }

    /// The answer to a `panel.state` asked with `context` that may not have it: the plain panel state.
    fn plain_panel_state(&mut self, tab: &str, cwd: Option<String>) -> Result<Value, CtlError> {
        self.ctl_op(CtlOp::PanelState {
            tab: tab.to_string(),
            cwd,
        })
    }

    /// `panel.state` with `context` answered at once (no page is read): the context only for a prompt the person sent.
    pub(super) fn panel_context_once(
        &mut self,
        tab: &str,
        cwd: Option<String>,
    ) -> Result<Value, CtlError> {
        if self.quiet_prompt_take(tab) {
            self.panel_context_now(tab, cwd)
        } else {
            self.plain_panel_state(tab, cwd)
        }
    }

    /// `panel.state` with `context`, from what is known without asking a page: the kept arrow, and the selection of a
    /// file in the panel. A closed panel has no context.
    pub(super) fn panel_context_now(
        &mut self,
        tab: &str,
        cwd: Option<String>,
    ) -> Result<Value, CtlError> {
        let mut v = self.ctl_op(CtlOp::PanelState {
            tab: tab.to_string(),
            cwd,
        })?;
        if v.get("open").and_then(Value::as_bool) != Some(true) {
            return Ok(v);
        }
        let mut context = json!({});
        if let Some(place) = self.web.kept_arrow_place(tab) {
            context["mark"] = quiet_context::arrow(arrow_place(&place));
        }
        if let Some((place, lines, text)) = self.panels.selection_now(tab)
            && let Some(sel) = quiet_context::selection(
                &text,
                match &place {
                    SelectionIn::File(name) => Place::File {
                        name,
                        lines: Some(&lines),
                    },
                    SelectionIn::Text(title) => Place::Text {
                        title,
                        lines: Some(&lines),
                    },
                },
            )
        {
            context["selection"] = sel;
        }
        v["context"] = context;
        Ok(v)
    }

    /// Whether the tab's panel shows a page whose selection may be read: a plain web page, or a markdown view's page
    /// (it runs no script). Never a mini-app's or a mod's page.
    fn panel_page_shown(&self, tab: &str) -> bool {
        self.web.has_tab(tab)
            && (self.panels.is_md_page(tab)
                || (self.web.is_plain_page(tab)
                    && self
                        .panels
                        .snap(tab)
                        .is_some_and(|s| s.open && s.kind == "web")))
    }

    /// The hook's `panel.state` with `context`: answered at once, or once the page in the panel says what is selected
    /// (at most [`PAGE_READ_MS`] later). Only a call for a prompt the person sent has the context; any other gets the
    /// plain state.
    pub(super) fn ctl_panel_context(&mut self, tab: &str, cwd: Option<String>, reply: Reply) {
        if !self.quiet_prompt_take(tab) {
            let _ = reply.send(self.plain_panel_state(tab, cwd));
            return;
        }
        let answer = match self.panel_context_now(tab, cwd) {
            Ok(v) => v,
            Err(e) => {
                let _ = reply.send(Err(e));
                return;
            }
        };
        let open = answer.get("open").and_then(Value::as_bool) == Some(true);
        let has_selection = answer.pointer("/context/selection").is_some();
        let read = (open && !has_selection && self.panel_page_shown(tab))
            .then(|| self.web.ask_selection(tab))
            .flatten();
        let Some(id) = read else {
            let _ = reply.send(Ok(answer));
            return;
        };
        self.quiet_reads
            .waiting
            .insert(id, QuietRead { reply, answer });
        let weak = self.me.clone();
        crate::timer_registry::set("quiet.read", PAGE_READ_MS, false, move || {
            if let Some(rc) = weak.upgrade()
                && let Ok(mut c) = rc.try_borrow_mut()
            {
                c.quiet_reads_expire();
            }
        });
    }

    /// The page took too long: every waiting call is answered with what it has.
    pub(crate) fn quiet_reads_expire(&mut self) {
        for (_, r) in std::mem::take(&mut self.quiet_reads.waiting) {
            let _ = r.reply.send(Ok(r.answer));
        }
    }

    /// A page answered a selection read (`WebOut::AskSelection`): the waiting call gets the selection, when there is
    /// one that prints, named by the markdown file a view was made from, or the plain page that answered. Any other
    /// answer (a page Flare made) and a failed read answer without it.
    pub(super) fn quiet_selection_read(
        &mut self,
        id: u64,
        file: Option<&str>,
        page: Option<&crate::ask_agent::PageRef>,
        result: Result<String, String>,
    ) {
        let Some(mut r) = self.quiet_reads.waiting.remove(&id) else {
            return;
        };
        if self.quiet_reads.waiting.is_empty() {
            crate::timer_registry::clear("quiet.read");
        }
        let place = match (file, page) {
            (Some(name), _) => Place::File { name, lines: None },
            (None, Some(p)) => Place::Page {
                title: &p.title,
                url: &p.address,
            },
            (None, None) => {
                let _ = r.reply.send(Ok(r.answer));
                return;
            }
        };
        if let Ok(text) = result
            && let Some(sel) = quiet_context::selection(&text, place)
        {
            r.answer["context"]["selection"] = sel;
        }
        let _ = r.reply.send(Ok(r.answer));
    }

    /// How many hook calls wait for a page's selection (the tests).
    #[cfg(test)]
    pub(crate) fn quiet_reads_waiting(&self) -> usize {
        self.quiet_reads.waiting.len()
    }
}

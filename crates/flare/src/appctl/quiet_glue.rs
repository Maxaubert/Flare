//! The quiet context in the window (rule 1268): the prompt hook of the tab's Claude asks `panel.state` with
//! `"context": true` (`CtlOp::PanelContext`), and the answer gains `context`, what the person marked in the side panel
//! right now. Nothing is typed and nothing is kept for later: a selection is handed on while it is selected, the drawn
//! arrow while the page keeps it (until a look sees it, the page moves on or five minutes pass).
//!
//! - **A file's selection** (a text file, or a markdown file's Code view) is known here at once.
//! - **A page's selection** (a web page, a mini-app or a markdown view's page) is read from the page with the
//!   selection script; the call waits for it at most [`PAGE_READ_MS`], then is answered without it.
//! - **The drawn arrow** is where it is kept (`WebCtl::kept_arrow_place`); what it points at is not handed on.

use std::collections::BTreeMap;
use std::sync::mpsc::Sender;

use flare_ctl::CtlError;
use serde_json::{Value, json};

use super::AppCtl;
use crate::ctl_app::CtlOp;
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

/// The hook calls waiting for a page's selection, by the read's id.
#[derive(Default)]
pub(crate) struct QuietReads(BTreeMap<u64, QuietRead>);

/// The place of a kept arrow, as the context names it.
fn arrow_place(place: &KeptArrowPlace) -> Place<'_> {
    match place {
        KeptArrowPlace::File(name) => Place::File { name, lines: None },
        KeptArrowPlace::Page { title, url } => Place::Page { title, url },
        KeptArrowPlace::Other => Place::Other,
    }
}

impl AppCtl {
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
        if let Some((name, lines, text)) = self.panels.selection_now(tab)
            && let Some(sel) = quiet_context::selection(
                &text,
                Place::File {
                    name: &name,
                    lines: Some(&lines),
                },
            )
        {
            context["selection"] = sel;
        }
        v["context"] = context;
        Ok(v)
    }

    /// Whether the tab's panel shows a page whose selection can be read: a web page, a mini-app, a plugin's page or a
    /// markdown view's page.
    fn panel_page_shown(&self, tab: &str) -> bool {
        self.web.has_tab(tab)
            && (self.panels.is_md_page(tab)
                || self.panels.snap(tab).is_some_and(|s| {
                    s.open && (s.kind == "web" || s.kind == "app" || s.kind.starts_with("mod:"))
                }))
    }

    /// The hook's `panel.state` with `context`: answered at once, or once the page in the panel says what is selected
    /// (at most [`PAGE_READ_MS`] later).
    pub(super) fn ctl_panel_context(&mut self, tab: &str, cwd: Option<String>, reply: Reply) {
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
        self.quiet_reads.0.insert(id, QuietRead { reply, answer });
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
        for (_, r) in std::mem::take(&mut self.quiet_reads.0) {
            let _ = r.reply.send(Ok(r.answer));
        }
    }

    /// A page answered a selection read (`WebOut::AskSelection`): the waiting call gets the selection, when there is
    /// one that prints, named by the markdown file a view was made from, or the page that answered, or as a page
    /// Flare made. A failed read answers without it.
    pub(super) fn quiet_selection_read(
        &mut self,
        id: u64,
        file: Option<&str>,
        page: Option<&crate::ask_agent::PageRef>,
        result: Result<String, String>,
    ) {
        let Some(mut r) = self.quiet_reads.0.remove(&id) else {
            return;
        };
        if self.quiet_reads.0.is_empty() {
            crate::timer_registry::clear("quiet.read");
        }
        let place = match (file, page) {
            (Some(name), _) => Place::File { name, lines: None },
            (None, Some(p)) => Place::Page {
                title: &p.title,
                url: &p.address,
            },
            (None, None) => Place::Other,
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
        self.quiet_reads.0.len()
    }
}

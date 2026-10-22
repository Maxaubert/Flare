//! The web panel inside the window controller (Surfaces stage 3): the address bar's buttons, the
//! browser's answers, the keys the page hands back, the two channel methods that wait for a page to
//! load, and the numbers the e2e reads.
//!
//! The browser itself is `WebCtl`; this file is the part that needs `AppCtl`'s private state (the
//! panels, the front tab, the chords, the channel's events).

use std::sync::mpsc::Sender;
use std::time::Instant;

use flare_ctl::{CtlError, code};
use flare_surfaces::PanelKind;
use flare_web::Nav;
use serde_json::{Value, json};

use super::{AppCtl, SharedString};
use crate::ctl_app::NavTo;
use crate::web_ctl::{Settled, WebFactory, WebMsg, WebOut, key_event_of};

type Reply = Sender<Result<Value, CtlError>>;

impl AppCtl {
    /// Flare's own window is the foreground window of Windows (the keyboard may be in the page's child
    /// window, which is still this window being in use).
    pub(super) fn window_is_foreground(&self) -> bool {
        let Some(hwnd) = self.app().and_then(|a| crate::window::hwnd_of(&a)) else {
            return false;
        };
        // SAFETY: GetForegroundWindow takes no arguments and reads the desktop's state.
        let fg = unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
        fg as isize == hwnd
    }

    /// How the browser is made (startup; the tests hand a stand-in).
    pub fn set_web_factory(&mut self, f: WebFactory) {
        self.web.set_factory(f);
    }

    /// The browser's controllers now (0 while no web panel is open).
    pub fn web_controllers(&self) -> usize {
        self.web.controllers()
    }

    /// The download question of the first waiting download is on the dialog (unless the close question
    /// has it: that one is answered first and this is asked after).
    pub(super) fn show_download_ask(&mut self) {
        if self.ask.is_some() {
            return;
        }
        let Some(app) = self.app() else { return };
        match self.downloads.current() {
            Some(a) => {
                let v = crate::download_ask::download_view(a, &crate::chrome_apply::text_ink());
                let body = slint::StyledText::from_markdown(&v.body)
                    .unwrap_or_else(|_| slint::StyledText::from_plain_text(&v.body));
                self.clear_tip();
                self.close_menu_quietly();
                app.set_dialog_title(v.title.into());
                app.set_dialog_body(body);
                app.set_dialog_primary(v.primary.into());
                app.set_dialog_danger(false);
                app.set_dialog_open(true);
            }
            None => app.set_dialog_open(false),
        }
        self.refresh();
    }

    /// The person answered the download question.
    pub(super) fn answer_download(&mut self, save: bool) {
        let Some(a) = self.downloads.answered() else {
            return;
        };
        self.web.answer_download(a.id, save);
        if self.downloads.current().is_some() {
            self.show_download_ask();
        } else {
            if let Some(app) = self.app() {
                app.set_dialog_open(false);
            }
            self.refresh();
            self.focus_terminal();
        }
    }

    /// A download whose page is gone is not asked about (the browser cancelled it when it closed).
    pub(super) fn prune_downloads(&mut self) {
        if self.downloads.is_empty() {
            return;
        }
        let head = self.downloads.current().map(|a| a.id);
        self.downloads.retain(|a| self.web.has_tab(&a.tab));
        if self.downloads.current().map(|a| a.id) != head && self.ask.is_none() {
            self.show_download_ask();
        }
    }

    /// The page moved or resized in the window, or an overlay opened or closed: the browser window
    /// follows (the same sync a refresh runs).
    pub(super) fn web_moved(&mut self) {
        let Some(app) = self.app() else { return };
        let front = self.model.active_shell().map(|t| t.id.clone());
        let ctx = self.key_context();
        self.web.sync(&app, &self.panels, front.as_deref(), ctx);
        self.prune_downloads();
    }

    /// The button of the missing-runtime note: Microsoft's page, in the browser the user has.
    pub(super) fn web_download(&mut self) {
        (self.deps.open_url)(crate::web_ctl::RUNTIME_URL);
    }

    /// A button of the browser bar.
    pub(super) fn web_nav(&mut self, nav: Nav) {
        let Some(id) = self.front_id() else { return };
        if self.panels.web_target(&id).is_some()
            && !self.panels.is_app(&id)
            && self.web.can_go(&id, nav)
        {
            self.web.nav(&id, nav);
        }
    }

    /// Enter in the address field.
    pub(super) fn web_submit(&mut self, text: &SharedString) {
        let Some(id) = self.front_id() else { return };
        if self.panels.web_target(&id).is_none() || self.panels.is_app(&id) {
            return;
        }
        match self.web.submit(&id, text) {
            Ok(url) => {
                self.panels.set_web_url(&id, &url);
                // As in a browser, the keys go to the page once an address is sent to it.
                if self.web.focus_page(&id) {
                    self.panel_focus_seen = false;
                }
                self.refresh();
                self.panel_changed(&id);
            }
            Err(e) => {
                self.web.notice(&id, &e.to_string());
                self.refresh();
            }
        }
    }

    /// What the browser told the window.
    pub(super) fn apply_web(&mut self, outs: Vec<WebOut>) {
        for out in outs {
            match out {
                WebOut::Settle { reply, tab, result } => {
                    let answer = result.map(|s| self.web_answer(&tab, &s));
                    let _ = reply.send(answer);
                }
                WebOut::Address { tab, url } => {
                    if self.panels.set_web_url(&tab, &url) {
                        self.panel_changed(&tab);
                    }
                }
                WebOut::Title { tab, title } => {
                    if self.panels.set_web_title(&tab, &title) {
                        self.panel_changed(&tab);
                    }
                }
                WebOut::Key(k) => {
                    if k.vk == 0x1B {
                        // The second Escape: the keys go back to the terminal.
                        self.panel_escape();
                    } else if let Some(ev) = key_event_of(&k) {
                        let _ = self.on_key(&ev);
                    }
                }
                // The page is part of the window: its taking the keyboard is not the window losing it.
                WebOut::PageFocus(true) => self.on_focus(true),
                WebOut::PageFocus(false) => {
                    if !self.window_is_foreground() {
                        self.on_focus_lost();
                    }
                }
                WebOut::Changed => self.refresh(),
                o @ (WebOut::ModMessage { .. }
                | WebOut::ModPage { .. }
                | WebOut::ModLost { .. }) => {
                    self.on_mod_web(o);
                }
                WebOut::Download {
                    tab,
                    id,
                    file,
                    host,
                } => {
                    let ask = crate::download_ask::DownloadAsk {
                        id,
                        tab,
                        file,
                        host,
                    };
                    if self.downloads.push(ask) {
                        self.show_download_ask();
                    }
                }
            }
        }
    }

    /// `{panel, url, title, loaded, status}`: what a call that waited for a page answers.
    fn web_answer(&self, tab: &str, s: &Settled) -> Value {
        let mut v = json!({
            "url": s.url,
            "title": s.title,
            "loaded": s.loaded,
        });
        if !s.status.is_empty() {
            v["status"] = json!(s.status);
        }
        if let Some(snap) = self.panels.snap(tab) {
            v["panel"] = Self::panel_info(&snap);
        }
        v
    }

    /// `browser.open` and `browser.navigate`: the answer waits for the page to finish loading (or
    /// fail), so it is sent from the browser's message, not from here. A call for a tab that is behind
    /// has no browser to wait for and answers at once with what is stored.
    pub(super) fn ctl_browser(
        &mut self,
        op: crate::ctl_app::CtlOp,
        reply: Reply,
        deadline: Instant,
    ) {
        // The address the call sends the page to (none for back, forward and reload): the load that
        // answers it is the one for this address.
        let wanted = match &op {
            crate::ctl_app::CtlOp::BrowserOpen { url, .. } => url.clone(),
            crate::ctl_app::CtlOp::BrowserNavigate {
                to: NavTo::Url(url),
                ..
            } => url.clone(),
            _ => String::new(),
        };
        let r = match op {
            crate::ctl_app::CtlOp::BrowserOpen { tab, url } => self.browser_open(&tab, &url),
            crate::ctl_app::CtlOp::BrowserNavigate { tab, to } => self.browser_navigate(&tab, to),
            _ => Err(CtlError::new(code::INTERNAL, "not a browser call")),
        };
        match r {
            Err(e) => {
                let _ = reply.send(Err(e));
            }
            Ok((tab, true)) => self.web.wait_for_load(&tab, reply, deadline, &wanted),
            Ok((tab, false)) => {
                let s = self.web.snapshot(&tab);
                let s = Settled {
                    url: self.panels.web_target(&tab).unwrap_or(s.url),
                    ..s
                };
                let answer = self.web_answer(&tab, &s);
                let _ = reply.send(Ok(answer));
            }
        }
    }

    /// `browser.look` and the calls that use the page (stage 4). The agent's own tab only (the dispatcher
    /// names it from the token); the answer is sent by the browser's messages, or by the person taking
    /// over. The ribbon goes up with the call.
    pub(super) fn ctl_browser_agent(&mut self, op: crate::ctl_app::CtlOp, reply: Reply) {
        let crate::ctl_app::CtlOp::BrowserAgent { tab, op } = op else {
            let _ = reply.send(Err(CtlError::new(
                code::INTERNAL,
                "not an agent browser call",
            )));
            return;
        };
        if !self.is_shell_tab(&tab) {
            let _ = reply.send(Err(CtlError::new(code::NO_TAB, "that tab is closed")));
            return;
        }
        // A mod's page is the mod's own: the browser tools do not look at it or click in it.
        if self.panels.mod_of(&tab).is_some() {
            let _ = reply.send(Err(CtlError::new(
                code::UNAVAILABLE,
                "the panel in this tab belongs to a mod; the browser tools drive a web page (open_page opens one)",
            )));
            return;
        }
        let Some(url) = self.panels.web_target(&tab) else {
            let _ = reply.send(Err(CtlError::new(
                code::UNAVAILABLE,
                "no web page is open in this tab; open_page (browser.open) opens one",
            )));
            return;
        };
        // A tab that is behind has no browser yet; the agent's call makes one, hidden.
        let app = self.panels.app_spec(&tab).cloned();
        self.web.ensure_hidden(&tab, &url, app);
        let label = crate::web_ctl::agent_label(self.agent_named(&tab));
        self.web.start_agent(&tab, op, &label, reply);
        self.web_moved();
    }

    /// `view.html` and `view.diagram` (stage 5): the page is already written; the tab's panel shows it as
    /// a mini-app (the folder is the panel's from here on). The answer is sent when the page has loaded,
    /// and for a diagram when it has drawn (or said why not), by the browser's message.
    pub(super) fn ctl_show_app(
        &mut self,
        op: crate::ctl_app::CtlOp,
        reply: Reply,
        deadline: Instant,
    ) {
        let crate::ctl_app::CtlOp::ShowApp {
            tab,
            spec,
            title,
            diagram,
        } = op
        else {
            let _ = reply.send(Err(CtlError::new(code::INTERNAL, "not a mini-app call")));
            return;
        };
        if !self.is_shell_tab(&tab) {
            self.panels.remove_folder(spec);
            let _ = reply.send(Err(CtlError::new(code::NO_TAB, "that tab is closed")));
            return;
        }
        let url = spec.url.clone();
        if !self.panels.show_app(&tab, spec.clone(), &title) {
            let _ = reply.send(Err(CtlError::new(
                code::INTERNAL,
                "the mini-app could not be shown",
            )));
            return;
        }
        self.refresh();
        // A tab that is behind (or whose slot has no place yet) gets its page made hidden, so the page
        // loads and a diagram draws whether or not anyone is looking.
        self.web.ensure_hidden(&tab, &url, Some(spec));
        self.panel_changed(&tab);
        if let Some((why, missing)) = self.web.failure(&tab) {
            let msg = if missing {
                format!(
                    "the WebView2 Runtime is not installed ({})",
                    crate::web_ctl::RUNTIME_URL
                )
            } else {
                format!("the web view could not be made: {why}")
            };
            let _ = reply.send(Err(CtlError::new(code::UNAVAILABLE, msg)));
            return;
        }
        self.web
            .wait_for_load_then(&tab, reply, deadline, &url, diagram);
        self.web_moved();
    }

    /// The apps' folder (startup): a folder is removed from here only.
    pub fn set_apps_root(&mut self, root: std::path::PathBuf) {
        self.panels.set_apps_root(root);
    }

    /// The person took over the front page (the e2e's stand-in for their click or key).
    pub fn web_user_input(&mut self) {
        let Some(app) = self.app() else { return };
        let outs = self.web.on_msg(&app, WebMsg::UserInput);
        self.apply_web(outs);
        self.web_moved();
    }

    /// Shows the page in the tab's panel. The bool: a browser is up for the tab, so the call waits.
    pub(super) fn browser_open(
        &mut self,
        tab: &str,
        url: &str,
    ) -> Result<(String, bool), CtlError> {
        if !self.is_shell_tab(tab) {
            return Err(CtlError::new(code::NO_TAB, "that tab is closed"));
        }
        // A tab that showed a mini-app gets a fresh view for the page (the app's view is its own kind).
        let had = self.web.has_tab(tab)
            && self.panels.web_target(tab).is_some()
            && !self.panels.is_app(tab);
        if !self.panels.show(tab, PanelKind::Web, url) {
            return Err(CtlError::new(
                code::BAD_PARAMS,
                "that address cannot be opened",
            ));
        }
        self.refresh();
        if had {
            // The tab already had a page: send it to the new address.
            self.web.go_to(tab, url);
        }
        self.panel_changed(tab);
        if let Some((why, missing)) = self.web.failure(tab) {
            let msg = if missing {
                format!(
                    "the WebView2 Runtime is not installed ({})",
                    crate::web_ctl::RUNTIME_URL
                )
            } else {
                format!("the web view could not be made: {why}")
            };
            return Err(CtlError::new(code::UNAVAILABLE, msg));
        }
        Ok((tab.to_string(), self.web.has_tab(tab)))
    }

    fn browser_navigate(&mut self, tab: &str, to: NavTo) -> Result<(String, bool), CtlError> {
        if !self.is_shell_tab(tab) {
            return Err(CtlError::new(code::NO_TAB, "that tab is closed"));
        }
        if self.panels.web_target(tab).is_none() {
            return Err(CtlError::new(
                code::UNAVAILABLE,
                "no web page is open in this tab; browser.open opens one",
            ));
        }
        let up = self.web.has_tab(tab);
        match to {
            NavTo::Url(url) => {
                if self.panels.is_app(tab) {
                    return Err(CtlError::new(
                        code::UNAVAILABLE,
                        "a mini-app is one page and cannot go anywhere; open_page replaces it",
                    ));
                }
                if !self.panels.set_web_url(tab, &url) {
                    return Err(CtlError::new(
                        code::BAD_PARAMS,
                        "that address cannot be opened",
                    ));
                }
                if up {
                    self.web.go_to(tab, &url);
                }
                self.refresh();
                self.panel_changed(tab);
            }
            NavTo::Back | NavTo::Forward | NavTo::Reload => {
                let nav = match to {
                    NavTo::Back => Nav::Back,
                    NavTo::Forward => Nav::Forward,
                    _ => Nav::Reload,
                };
                if !up {
                    return Err(CtlError::new(
                        code::UNAVAILABLE,
                        "the page is not loaded yet; its tab is behind",
                    ));
                }
                if !self.web.can_go(tab, nav) {
                    return Err(CtlError::new(
                        code::UNAVAILABLE,
                        if nav == Nav::Back {
                            "there is no earlier page"
                        } else {
                            "there is no later page"
                        },
                    ));
                }
                self.web.nav(tab, nav);
            }
        }
        if let Some((why, _)) = self.web.failure(tab) {
            return Err(CtlError::new(code::UNAVAILABLE, why));
        }
        Ok((tab.to_string(), up))
    }

    // ---- the e2e's reads ---------------------------------------------------------------------------

    /// Sets the browser zoom of the front tab's page (the e2e's seam).
    pub fn web_zoom(&mut self, factor: f64) -> bool {
        let Some(id) = self.front_id() else {
            return false;
        };
        let up = self.web.has_tab(&id);
        if up {
            self.web.set_zoom(&id, factor);
        }
        up
    }

    /// Starts a script in the front tab's page (the e2e's seam; the answer comes from `web_script_result`).
    pub fn web_script(&mut self, js: &str) -> Option<u64> {
        let id = self.front_id()?;
        self.web
            .has_tab(&id)
            .then(|| self.web.start_script(&id, js))
    }

    /// The answer of the latest script started (None while it runs).
    pub fn web_script_latest(&self) -> Option<Result<String, String>> {
        self.web.script_latest()
    }

    /// What the web panel is, for the `web.state` probe.
    pub fn web_probe(&self) -> Value {
        let front = self.front_id();
        let tab = front.as_deref().filter(|t| self.web.has_tab(t));
        let probe = tab.and_then(|t| self.web.probe(t));
        let views: Vec<Value> = self
            .web
            .tab_ids()
            .iter()
            .map(|t| {
                let p = self.web.probe(t);
                json!({
                    "tab": t,
                    "visible": p.as_ref().map(|p| p.visible),
                    "url": self.web.tab_url(t),
                })
            })
            .collect();
        json!({
            "views": views,
            "controllers": self.web.controllers(),
            "backend": self.web.has_backend(),
            "front": front,
            "has_tab": tab.is_some(),
            "phase": self.web.phase_name(),
            "shown_tab": self.web.shown_tab(),
            "has_still": self.web.has_still(),
            "page_focus": self.web.has_focus(),
            "agent": front.as_deref().map(|t| json!({
                "op": self.web.agent_op(t),
                "refs": self.web.agent_refs(t),
                "ribbon": self.web.ribbon_text(t),
                "keys_heard": self.web.agent_keys_heard(),
            })),
            "url": tab.and_then(|t| self.web.tab_url(t)),
            "controller": probe.map(|p| json!({
                "bounds": {"x": p.bounds.x, "y": p.bounds.y, "w": p.bounds.w, "h": p.bounds.h},
                "visible": p.visible,
                "url": p.url,
                "title": p.title,
                "browser_pid": p.browser_pid,
                "profile": p.profile,
                "private": p.private,
                "children": p.child_windows.iter().map(|(c, r)| json!({
                    "class": c,
                    "x": r.x, "y": r.y, "w": r.w, "h": r.h,
                })).collect::<Vec<_>>(),
            })),
        })
    }
}

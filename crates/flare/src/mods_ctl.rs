//! Mods inside the window (Surfaces stage 8): the state the UI thread keeps about the pages mods are showing,
//! and what happens to a message a mod's page sends.
//!
//! The rules live elsewhere and are tested there: the manifest, the token and the tool table in `flare_mods`,
//! the grant in `flare_ctl`, the page's address and walls in `flare_surfaces::modview` and `flare_web`.
//! This file is the part that needs the window's thread:
//!
//! - **The bridge.** `window.flare.call(method, params)` arrives as a web message. It is checked against the
//!   mod's grant HERE (the page never holds a token: the host looks the mod's live grant up, so a mod that was
//!   turned off is refused at once), then run through the same dispatcher a pipe connection uses, on a worker,
//!   with the tab the mod's panel sits in. `window.flare.respond` answers a tool call through the hub.
//! - **Which tab.** A mod acts on ONE tab: the tab whose panel shows its page. No call names a tab.
//! - **The switches.** Turning a mod on or off is applied in memory at once (a token minted or killed) and saved
//!   on a worker.
//!
//! Nothing here blocks the UI thread: scans, saves and calls run on workers and come back as `UiMsg::Mods`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use flare_ctl::{CallContext, Dispatcher, Method, Namespace, code};
use flare_mods::bridge::{self, PageMsg};
use flare_mods::hub::{Change, Hub};
use flare_settings::page::mods::{ModInfo, ModsInfo, Problem};
use serde_json::Value;

use crate::appctl::{Post, UiMsg};

/// Calls from one mod's page running at once.
pub const MAX_PAGE_CALLS: usize = 16;

/// What a worker or the Settings page says to the mods controller (carried by `UiMsg::Mods`).
pub enum ModsMsg {
    /// A scan finished (a worker): what changed.
    Scanned(Change),
    /// A switch was written to `mods.json` (or could not be).
    Persisted {
        id: String,
        result: Result<(), String>,
    },
    /// A call the mod's page made has its answer: `json` is the message for the page.
    Reply {
        tab: String,
        mod_id: String,
        json: String,
    },
    /// Settings: scan again (the Mods page opened).
    Rescan,
    /// Settings: the switch of a mod.
    SetEnabled { id: String, on: bool },
    /// Settings: show the mod's panel in the most recent shell tab.
    ShowPanel { id: String },
}

/// A `ModsMsg` that can sit in a `UiMsg` (which is `Debug`): it prints its kind only, never a call's data.
pub struct ModsMsgBox(pub ModsMsg);

impl std::fmt::Debug for ModsMsgBox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self.0 {
            ModsMsg::Scanned(_) => "ModsMsg::Scanned",
            ModsMsg::Persisted { .. } => "ModsMsg::Persisted",
            ModsMsg::Reply { .. } => "ModsMsg::Reply",
            ModsMsg::Rescan => "ModsMsg::Rescan",
            ModsMsg::SetEnabled { .. } => "ModsMsg::SetEnabled",
            ModsMsg::ShowPanel { .. } => "ModsMsg::ShowPanel",
        })
    }
}

/// A mod's page in a tab.
#[derive(Debug, Default)]
struct ModTab {
    mod_id: String,
    /// The page has loaded and can be spoken to.
    ready: bool,
    /// Messages for the page that came before it was ready, in order.
    queue: Vec<String>,
}

/// The mods controller (UI thread).
pub struct ModCtl {
    hub: Arc<Hub>,
    post: Post,
    dispatcher: Option<Arc<dyn Dispatcher>>,
    tabs: HashMap<String, ModTab>,
    /// Calls a page made that a worker is still running, by mod.
    inflight: HashMap<String, usize>,
    /// A scan is on a worker; another was asked for meanwhile.
    scanning: Arc<AtomicBool>,
    rescan: bool,
    /// The last switch that could not be saved, for the Settings page.
    pub save_error: Arc<Mutex<Option<String>>>,
    /// Scans finished (the tests and the probe read it).
    pub scans: u64,
}

impl ModCtl {
    pub fn new(hub: Arc<Hub>, post: Post) -> Self {
        Self {
            hub,
            post,
            dispatcher: None,
            tabs: HashMap::new(),
            inflight: HashMap::new(),
            scanning: Arc::new(AtomicBool::new(false)),
            rescan: false,
            save_error: Arc::new(Mutex::new(None)),
            scans: 0,
        }
    }

    pub fn hub(&self) -> &Arc<Hub> {
        &self.hub
    }

    /// Where a page's calls run (the control channel's dispatcher). Without one every call of a page answers
    /// `unavailable`.
    pub fn set_dispatcher(&mut self, d: Arc<dyn Dispatcher>) {
        self.dispatcher = Some(d);
    }

    /// Reads the mods folder on a worker. One scan at a time; one asked for meanwhile runs after it.
    pub fn start_scan(&mut self) {
        if self.scanning.swap(true, Ordering::SeqCst) {
            self.rescan = true;
            return;
        }
        let (hub, post, flag) = (
            Arc::clone(&self.hub),
            Arc::clone(&self.post),
            Arc::clone(&self.scanning),
        );
        let job = move || {
            // The folder is made here (a worker) so the Open mods folder button has somewhere to go.
            let _ = std::fs::create_dir_all(&hub.config().dir); // sync-ok: the scan worker
            let change = hub.scan();
            flag.store(false, Ordering::SeqCst);
            post(UiMsg::Mods(ModsMsgBox(ModsMsg::Scanned(change))));
        };
        if let Err(e) = std::thread::Builder::new()
            .name("flare-mods-scan".into())
            .spawn(job)
        {
            self.scanning.store(false, Ordering::SeqCst);
            log::warn!("could not start the mods scan: {e}");
        }
    }

    /// A scan has finished. True when another was asked for meanwhile and has been started.
    pub fn scan_done(&mut self) -> bool {
        self.scans += 1;
        if std::mem::take(&mut self.rescan) {
            self.start_scan();
            return true;
        }
        false
    }

    /// Writes a switch on a worker (the file gets the mod's state as it is when the write runs).
    pub fn persist(&self, id: &str) {
        let (hub, post) = (Arc::clone(&self.hub), Arc::clone(&self.post));
        let id = id.to_string();
        let job = {
            let id = id.clone();
            move || {
                let result = hub.persist(&id);
                post(UiMsg::Mods(ModsMsgBox(ModsMsg::Persisted { id, result })));
            }
        };
        if let Err(e) = std::thread::Builder::new()
            .name("flare-mods-save".into())
            .spawn(job)
        {
            log::warn!("could not start the mods save: {e}");
        }
    }

    // ---- the pages ------------------------------------------------------------------------------------------------

    /// The tab shows this mod's page now. A page the tab already had for the same mod keeps its state.
    pub fn bind(&mut self, tab: &str, mod_id: &str) {
        let same = self.tabs.get(tab).is_some_and(|t| t.mod_id == mod_id);
        if !same {
            self.tabs.insert(
                tab.to_string(),
                ModTab {
                    mod_id: mod_id.to_string(),
                    ..ModTab::default()
                },
            );
        }
    }

    /// The tab no longer shows a mod's page. Gives the mod it was.
    pub fn unbind(&mut self, tab: &str) -> Option<String> {
        self.tabs.remove(tab).map(|t| t.mod_id)
    }

    pub fn bound(&self) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = self
            .tabs
            .iter()
            .map(|(t, m)| (t.clone(), m.mod_id.clone()))
            .collect();
        v.sort();
        v
    }

    pub fn mod_of(&self, tab: &str) -> Option<&str> {
        self.tabs.get(tab).map(|t| t.mod_id.as_str())
    }

    pub fn is_ready(&self, tab: &str) -> bool {
        self.tabs.get(tab).is_some_and(|t| t.ready)
    }

    /// The page loaded (`true`) or began to load (`false`). Going ready gives back what was queued for it.
    pub fn set_ready(&mut self, tab: &str, ready: bool) -> Vec<String> {
        match self.tabs.get_mut(tab) {
            Some(t) => {
                t.ready = ready;
                if ready {
                    std::mem::take(&mut t.queue)
                } else {
                    Vec::new()
                }
            }
            None => Vec::new(),
        }
    }

    /// A message for the page: Some when it can be sent now, None when it was queued until the page is ready (or
    /// there is no such page).
    pub fn queue_or_send(&mut self, tab: &str, msg: String) -> Option<String> {
        let t = self.tabs.get_mut(tab)?;
        if t.ready {
            Some(msg)
        } else {
            t.queue.push(msg);
            None
        }
    }

    /// The tabs showing the mod's page.
    pub fn tabs_of(&self, mod_id: &str) -> Vec<String> {
        let mut v: Vec<String> = self
            .tabs
            .iter()
            .filter(|(_, t)| t.mod_id == mod_id)
            .map(|(tab, _)| tab.clone())
            .collect();
        v.sort();
        v
    }

    /// A call a page made has been answered.
    pub fn call_done(&mut self, mod_id: &str) {
        if let Some(n) = self.inflight.get_mut(mod_id) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                self.inflight.remove(mod_id);
            }
        }
    }

    pub fn inflight(&self, mod_id: &str) -> usize {
        self.inflight.get(mod_id).copied().unwrap_or(0)
    }

    /// A message the page of `tab` posted. Returns the message to post back to the page at once (an
    /// answer, or the reason for a refusal); a call that is allowed runs on a worker and answers later.
    /// A message from a tab that shows no mod's page is ignored.
    pub fn on_page_message(&mut self, tab: &str, text: &str) -> Option<String> {
        let mod_id = self.tabs.get(tab)?.mod_id.clone();
        let msg = match bridge::parse_page_message(text) {
            Ok(m) => m,
            Err(why) => {
                log::debug!("a mod's page sent something that is not a message: {why}");
                return None;
            }
        };
        match msg {
            PageMsg::Respond { call_id, outcome } => {
                // Answering needs the `tools` permission, like `tools.respond` over the pipe.
                let allowed = self
                    .hub
                    .grant_of_mod(&mod_id)
                    .is_some_and(|g| g.check(Method::ToolsRespond).is_ok());
                if allowed {
                    // A call that is gone (answered, timed out, another mod's) is not the page's problem.
                    if let Err(e) = self.hub.respond(&mod_id, &call_id, outcome) {
                        log::debug!("a mod answered a call that is not open: {}", e.message);
                    }
                }
                None
            }
            PageMsg::Call { id, method, params } => {
                self.page_call(tab, &mod_id, id, &method, params)
            }
        }
    }

    fn page_call(
        &mut self,
        tab: &str,
        mod_id: &str,
        id: u64,
        method: &str,
        params: Value,
    ) -> Option<String> {
        let refuse = |c: &str, m: &str| Some(bridge::reply_err(id, c, m));
        // The grant is the mod's live one: a mod that was turned off has none.
        let Some(grant) = self.hub.grant_of_mod(mod_id) else {
            return refuse(code::UNAUTHORIZED, "this mod is turned off");
        };
        let Some(m) = Method::parse(method) else {
            return refuse(code::UNKNOWN_METHOD, "that method is not in the protocol");
        };
        match m {
            Method::Hello => {
                return refuse(
                    code::BAD_PARAMS,
                    "hello is not needed: the page is already known",
                );
            }
            Method::ToolsRespond => {
                return refuse(
                    code::BAD_PARAMS,
                    "use window.flare.respond to answer a tool call",
                );
            }
            _ => {}
        }
        if let Err(e) = grant.check(m) {
            return refuse(&e.code, &e.message);
        }
        // A mod's own browser panel needs a second slot beside its page (recorded in decisions.md): until it
        // is built, a mod that holds `browser` is allowed the methods but they answer `unavailable`.
        if m.namespace() == Namespace::Browser {
            return refuse(
                code::UNAVAILABLE,
                "a mod's own browser panel is not built yet",
            );
        }
        let Some(d) = self.dispatcher.clone() else {
            return refuse(code::UNAVAILABLE, "the control channel is not running");
        };
        if self.inflight(mod_id) >= MAX_PAGE_CALLS {
            return refuse(code::RATE_LIMITED, "too many calls at once");
        }
        *self.inflight.entry(mod_id.to_string()).or_insert(0) += 1;
        let (post, tab_name, mod_name) =
            (Arc::clone(&self.post), tab.to_string(), mod_id.to_string());
        let job = move || {
            let ctx = CallContext {
                conn: 0,
                client: format!("mod:{mod_name}"),
                client_version: String::new(),
                grant,
                tab: Some(tab_name.clone()),
            };
            let json = match d.call(&ctx, m, params) {
                Ok(v) => bridge::reply_ok(id, &v),
                Err(e) => bridge::reply_err(id, &e.code, &e.message),
            };
            post(UiMsg::Mods(ModsMsgBox(ModsMsg::Reply {
                tab: tab_name,
                mod_id: mod_name,
                json,
            })));
        };
        if std::thread::Builder::new()
            .name("flare-mod-call".into())
            .spawn(job)
            .is_err()
        {
            self.call_done(mod_id);
            return refuse(code::UNAVAILABLE, "the call could not start");
        }
        None
    }

    /// Fails everything waiting on a mod: its calls from the agent get `unavailable` with `why`.
    pub fn fail_calls(&self, mod_id: &str, why: &str) {
        self.hub.fail_calls(mod_id, why);
    }
}

/// What the Mods page shows, from the hub.
pub fn info_of(hub: &Hub, save_error: &Mutex<Option<String>>) -> ModsInfo {
    let s = hub.snapshot();
    ModsInfo {
        scanned: s.scanned,
        dir: hub.config().dir.to_string_lossy().into_owned(),
        mods: s
            .mods
            .into_iter()
            .map(|m| ModInfo {
                has_panel: m.panel_title.is_some(),
                permissions: m
                    .permissions
                    .iter()
                    .map(|p| flare_mods::hub::permission_words(*p).to_string())
                    .collect(),
                id: m.id,
                name: m.name,
                version: m.version,
                description: m.description,
                enabled: m.enabled,
            })
            .collect(),
        problems: s
            .errors
            .into_iter()
            .map(|e| Problem {
                folder: e.folder,
                reason: e.reason,
            })
            .collect(),
        notes: s.notes,
        save_error: save_error.lock().unwrap_or_else(|e| e.into_inner()).clone(),
    }
}

/// Settings' door to the mods (the Mods page): what to show and what the person did. Every action goes to the
/// window as a message, so the same code runs whether it was a click or a probe.
pub struct ModsSettings {
    pub hub: Arc<Hub>,
    pub post: Post,
    pub save_error: Arc<Mutex<Option<String>>>,
}

impl crate::settings_ctl::ModsPort for ModsSettings {
    fn info(&self) -> ModsInfo {
        info_of(&self.hub, &self.save_error)
    }

    fn page_shown(&self) {
        (self.post)(UiMsg::Mods(ModsMsgBox(ModsMsg::Rescan)));
    }

    fn set_enabled(&self, id: &str, on: bool) {
        (self.post)(UiMsg::Mods(ModsMsgBox(ModsMsg::SetEnabled {
            id: id.to_string(),
            on,
        })));
    }

    fn show_panel(&self, id: &str) {
        (self.post)(UiMsg::Mods(ModsMsgBox(ModsMsg::ShowPanel {
            id: id.to_string(),
        })));
    }

    fn dir(&self) -> std::path::PathBuf {
        self.hub.config().dir.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flare_ctl::{CtlError, Grant};
    use flare_mods::hub::Config;
    use std::sync::mpsc;
    use std::time::Duration;

    fn mod_json(perms: &str) -> String {
        let tools = if perms.contains("tools") {
            r#","tools":[{"name":"append_note","description":"d","input_schema":{"type":"object"}}]"#
        } else {
            ""
        };
        format!(
            r#"{{"id":"notes","name":"Notes","version":"1.0.0","protocol":1,"description":"d","permissions":{perms},"panel":{{"entry":"index.html","title":"Notes"}}{tools}}}"#
        )
    }

    type Recorded = Arc<Mutex<Vec<(Method, Value, Option<String>, String)>>>;

    struct Rig {
        _dir: tempfile::TempDir,
        ctl: ModCtl,
        rx: mpsc::Receiver<String>,
        calls: Recorded,
    }

    /// A dispatcher that records its calls and answers `{"ok":true}`.
    struct Rec {
        calls: Recorded,
    }

    impl Dispatcher for Rec {
        fn resolve_token(&self, _: &str) -> Option<Grant> {
            None
        }
        fn resolve_tab_token(&self, _: &str) -> Option<String> {
            None
        }
        fn call(&self, ctx: &CallContext, m: Method, p: Value) -> Result<Value, CtlError> {
            self.calls
                .lock()
                .unwrap()
                .push((m, p, ctx.tab.clone(), ctx.grant.kind().to_string()));
            Ok(serde_json::json!({"ok": true}))
        }
    }

    fn rig(perms: &str) -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("mods");
        let f = root.join("notes");
        std::fs::create_dir_all(&f).unwrap(); // sync-ok: test code only
        std::fs::write(f.join("mod.json"), mod_json(perms)).unwrap(); // sync-ok: test code only
        std::fs::write(f.join("index.html"), "<p>x</p>").unwrap(); // sync-ok: test code only
        let hub = Arc::new(Hub::new(Config {
            dir: root,
            state_file: dir.path().join("mods.json"),
        }));
        hub.scan();
        hub.set_enabled("notes", true).unwrap();
        let (tx, rx) = mpsc::channel::<String>();
        let tx = Mutex::new(tx);
        let post: Post = Arc::new(move |m: UiMsg| {
            if let UiMsg::Mods(ModsMsgBox(ModsMsg::Reply { json, .. })) = m {
                let _ = tx.lock().unwrap().send(json);
            }
        });
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut ctl = ModCtl::new(hub, post);
        ctl.set_dispatcher(Arc::new(Rec {
            calls: Arc::clone(&calls),
        }));
        ctl.bind("t1", "notes");
        Rig {
            _dir: dir,
            ctl,
            rx,
            calls,
        }
    }

    fn call(id: u64, method: &str, params: &str) -> String {
        format!(r#"{{"t":"call","id":{id},"method":"{method}","params":{params}}}"#)
    }

    fn reply_of(r: &Rig) -> Value {
        serde_json::from_str(&r.rx.recv_timeout(Duration::from_secs(5)).unwrap()).unwrap()
    }

    #[test]
    fn a_mod_without_browser_is_refused_every_browser_method_before_anything_runs() {
        let mut r = rig(r#"["panel","tools"]"#);
        for (i, m) in [
            "browser.open",
            "browser.look",
            "browser.eval",
            "browser.click",
        ]
        .iter()
        .enumerate()
        {
            let out = r
                .ctl
                .on_page_message("t1", &call(i as u64 + 1, m, r#"{"url":"https://a.test/"}"#))
                .expect("refused at once");
            let v: Value = serde_json::from_str(&out).unwrap();
            assert_eq!(v["t"], "reply");
            assert_eq!(v["id"], i as u64 + 1);
            assert_eq!(v["error"]["code"], "forbidden", "{m}");
        }
        assert!(
            r.calls.lock().unwrap().is_empty(),
            "the dispatcher saw nothing"
        );
        assert_eq!(r.ctl.inflight("notes"), 0);
    }

    #[test]
    fn a_mod_with_browser_is_allowed_the_method_but_its_own_panel_is_not_built() {
        let mut r = rig(r#"["panel","browser"]"#);
        let out = r
            .ctl
            .on_page_message(
                "t1",
                &call(1, "browser.open", r#"{"url":"https://a.test/"}"#),
            )
            .unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"]["code"], "unavailable");
        assert!(r.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn an_allowed_call_runs_on_the_dispatcher_with_the_mods_grant_and_its_own_tab() {
        let mut r = rig(r#"["panel","view","tools"]"#);
        assert_eq!(
            r.ctl
                .on_page_message("t1", &call(7, "view.text", r#"{"text":"hi"}"#)),
            None,
            "answered later"
        );
        assert_eq!(r.ctl.inflight("notes"), 1);
        let v = reply_of(&r);
        assert_eq!(
            v,
            serde_json::json!({"t":"reply","id":7,"result":{"ok":true}})
        );
        let calls = r.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, Method::ViewText);
        assert_eq!(calls[0].1, serde_json::json!({"text":"hi"}));
        assert_eq!(
            calls[0].2.as_deref(),
            Some("t1"),
            "the tab the page sits in"
        );
        assert_eq!(calls[0].3, "mod");
    }

    #[test]
    fn a_permission_the_mod_does_not_hold_is_forbidden_and_hello_and_respond_are_not_calls() {
        let mut r = rig(r#"["panel","tools"]"#);
        for (m, code) in [
            ("view.text", "forbidden"),
            ("convo.state", "forbidden"),
            ("mods.call", "forbidden"),
            ("settings.set", "forbidden"),
            ("mods.enable", "forbidden"),
            ("shell.input", "forbidden"),
            ("tools.respond", "bad_params"),
            ("hello", "bad_params"),
            ("nope.nothing", "unknown_method"),
        ] {
            let out = r.ctl.on_page_message("t1", &call(1, m, "{}")).unwrap();
            let v: Value = serde_json::from_str(&out).unwrap();
            assert_eq!(v["error"]["code"], code, "{m}");
        }
        assert!(r.calls.lock().unwrap().is_empty());
        // The words say which grant and method.
        let out = r
            .ctl
            .on_page_message("t1", &call(1, "view.text", "{}"))
            .unwrap();
        assert!(out.contains("view.text"), "{out}");
    }

    #[test]
    fn a_mod_that_is_turned_off_is_refused_at_once_even_with_its_page_open() {
        let mut r = rig(r#"["panel","view"]"#);
        r.ctl.hub().set_enabled("notes", false).unwrap();
        let out = r
            .ctl
            .on_page_message("t1", &call(1, "view.text", "{}"))
            .unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"]["code"], "unauthorized");
        assert!(r.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn a_page_makes_only_so_many_calls_at_once() {
        let mut r = rig(r#"["panel"]"#);
        r.ctl.inflight.insert("notes".into(), MAX_PAGE_CALLS);
        let out = r
            .ctl
            .on_page_message("t1", &call(1, "tabs.list", "{}"))
            .unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"]["code"], "rate_limited");
        r.ctl.call_done("notes");
        assert_eq!(r.ctl.inflight("notes"), MAX_PAGE_CALLS - 1);
    }

    #[test]
    fn without_a_dispatcher_a_call_is_unavailable() {
        let mut r = rig(r#"["panel"]"#);
        r.ctl.dispatcher = None;
        let out = r
            .ctl
            .on_page_message("t1", &call(1, "tabs.list", "{}"))
            .unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"]["code"], "unavailable");
    }

    #[test]
    fn respond_answers_the_mods_own_call_only_and_needs_the_tools_permission() {
        let mut r = rig(r#"["panel","tools"]"#);
        let c = r
            .ctl
            .hub()
            .begin_call("mod_notes_append_note", serde_json::json!({"text": "a"}))
            .unwrap();
        let text = format!(
            r#"{{"t":"respond","call_id":"{}","result":{{"count":1}}}}"#,
            c.id
        );
        assert_eq!(r.ctl.on_page_message("t1", &text), None);
        let v = r
            .ctl
            .hub()
            .wait(c, Duration::from_secs(1), &|| false)
            .unwrap();
        assert_eq!(v, serde_json::json!({"count": 1}));

        // Without `tools` the answer is ignored, and the call stays open.
        let mut r = rig(r#"["panel"]"#);
        // (the tool table is empty without the permission, so open a call through the hub's own door)
        assert!(
            r.ctl
                .hub()
                .begin_call("mod_notes_append_note", serde_json::json!({}))
                .is_err()
        );
        let _ = r
            .ctl
            .on_page_message("t1", r#"{"t":"respond","call_id":"c1","result":1}"#);
    }

    #[test]
    fn a_message_from_a_tab_with_no_mod_page_or_garbage_is_ignored() {
        let mut r = rig(r#"["panel"]"#);
        assert_eq!(
            r.ctl.on_page_message("other", &call(1, "tabs.list", "{}")),
            None
        );
        assert_eq!(r.ctl.on_page_message("t1", "not json"), None);
        assert_eq!(r.ctl.on_page_message("t1", r#"{"t":"nope"}"#), None);
        assert!(r.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn messages_for_a_page_wait_until_it_is_ready_and_come_out_in_order() {
        let mut r = rig(r#"["panel"]"#);
        assert_eq!(r.ctl.queue_or_send("t1", "a".into()), None);
        assert_eq!(r.ctl.queue_or_send("t1", "b".into()), None);
        assert_eq!(
            r.ctl.set_ready("t1", true),
            vec!["a".to_string(), "b".to_string()]
        );
        assert_eq!(r.ctl.queue_or_send("t1", "c".into()), Some("c".into()));
        assert!(r.ctl.set_ready("t1", false).is_empty());
        assert_eq!(r.ctl.queue_or_send("t1", "d".into()), None);
        assert_eq!(r.ctl.queue_or_send("nowhere", "x".into()), None);
    }

    #[test]
    fn binding_the_same_mod_again_keeps_the_page_state_and_another_mod_starts_over() {
        let mut r = rig(r#"["panel"]"#);
        r.ctl.set_ready("t1", true);
        r.ctl.bind("t1", "notes");
        assert!(r.ctl.is_ready("t1"));
        r.ctl.bind("t1", "other");
        assert!(!r.ctl.is_ready("t1"));
        assert_eq!(r.ctl.mod_of("t1"), Some("other"));
        assert_eq!(r.ctl.unbind("t1").as_deref(), Some("other"));
        assert_eq!(r.ctl.mod_of("t1"), None);
        assert!(r.ctl.bound().is_empty());
    }

    #[test]
    fn the_debug_form_of_a_message_prints_its_kind_only() {
        let m = ModsMsgBox(ModsMsg::Reply {
            tab: "t".into(),
            mod_id: "notes".into(),
            json: "SECRET".into(),
        });
        assert_eq!(format!("{m:?}"), "ModsMsg::Reply");
    }

    #[test]
    fn the_page_info_lists_the_permissions_in_plain_words_and_no_token() {
        let r = rig(r#"["panel","tools"]"#);
        let info = info_of(r.ctl.hub(), &r.ctl.save_error);
        assert_eq!(info.mods.len(), 1);
        assert_eq!(
            info.mods[0].permissions,
            vec![
                "Show its panel in a tab".to_string(),
                "Answer the agent's tool calls".to_string()
            ]
        );
        assert!(info.mods[0].enabled && info.mods[0].has_panel);
        assert!(!format!("{info:?}").contains("token"));
    }

    #[test]
    fn a_scan_runs_on_a_worker_and_a_second_ask_runs_after_it() {
        let dir = tempfile::tempdir().unwrap();
        let hub = Arc::new(Hub::new(Config {
            dir: dir.path().join("mods"),
            state_file: dir.path().join("mods.json"),
        }));
        let (tx, rx) = mpsc::channel::<String>();
        let tx = Mutex::new(tx);
        let post: Post = Arc::new(move |m: UiMsg| {
            if let UiMsg::Mods(ModsMsgBox(ModsMsg::Scanned(_))) = m {
                let _ = tx.lock().unwrap().send("scanned".into());
            }
        });
        let mut ctl = ModCtl::new(hub, post);
        ctl.start_scan();
        ctl.start_scan(); // asked while the first may still run
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), "scanned");
        // The folder was made so the Open mods folder button has somewhere to go.
        assert!(dir.path().join("mods").is_dir());
        // Whether or not the second ask was folded into the first, finishing the scan starts at most one more.
        let again = ctl.scan_done();
        if again {
            assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), "scanned");
        }
        assert_eq!(ctl.scans, 1);
    }
}

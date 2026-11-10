//! The web host tests' browser process (`tests/cef_macos.rs`): it runs inside a dev bundle, makes the headless CEF
//! application, drives the real [`WebHost`] through the [`WebBackend`] trait the window uses, and writes a JSON
//! report (`FLARE_WEB_PROBE_OUT`). The scenario comes from `FLARE_WEB_PROBE_SCENARIO` (environment, not arguments:
//! CEF owns the command line).
//!
//! - `panel`: the web panel. Navigation and the address bar's events, history, same-document moves, refusals,
//!   new windows into the same view, failed loads, script dialogs, permissions, downloads with the held yes or
//!   no, zoom, the picture, pointer, wheel, keys, the key filter, the input method, focus, a still, DevTools calls,
//!   a `<select>` list, a crashed page.
//! - `walls`: a sealed mini-app (the spike's exfiltration page, watched by the in-process `observers`, against a
//!   web panel control), a networked one, a mod's page and its bridge, a plugin's bar, the profiles.
//! - `agent`: the real `agent_job` drives a web panel through `WebBackend::cdp` (`agent.rs`): look, click, type,
//!   press, scroll, the frames, wait, navigate and state, eval and its runaway, a download, whose keys the window's
//!   filter is asked about.
//!
//! Every report carries the window server's count of this process's on-screen windows at the start and the end,
//! the activation policy and whether the app ever became active.

mod agent;
mod observers;
mod pages;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use flare_cef::macos::{app, screen};
use flare_surfaces::app::{AppSpec, app_host, app_url};
use flare_surfaces::modview::{mod_spec, plugin_spec};
use flare_surfaces::web::PhysRect;
use objc2::MainThreadMarker;
use serde_json::{Value, json};

use crate::backend::{
    ImeInput, KeyInput, Mods, Nav, Pointer, PointerAct, PointerButton, Slot, ViewFrame, WebBackend,
    WebEvent,
};
use crate::host::{HostConfig, WebHost, installed_runtime};

/// The view's rectangle: 983 by 700 CSS pixels at 2x.
const BOUNDS: PhysRect = PhysRect {
    x: 0,
    y: 0,
    w: 1966,
    h: 1400,
};

struct Report {
    v: Value,
    out: Option<PathBuf>,
}

impl Report {
    /// Written after every check, so a run that hangs still says how far it came.
    fn check(&mut self, id: &str, ok: bool, what: &str, evidence: Value) {
        if let Some(a) = self.v["checks"].as_array_mut() {
            a.push(json!({"id": id, "ok": ok, "what": what, "evidence": evidence}));
        }
        self.write();
    }

    fn write(&self) {
        if let Some(p) = &self.out {
            let _ = std::fs::write(p, serde_json::to_string_pretty(&self.v).unwrap_or_default());
        }
    }

    fn set(&mut self, k: &str, v: Value) {
        self.v[k] = v;
    }
}

/// The host and every event it reported, handled the way the window handles them (`Runtime` and `Pump` go back to
/// the host).
struct H {
    host: WebHost,
    queue: Arc<Mutex<Vec<WebEvent>>>,
    seen: Vec<WebEvent>,
    script_seq: u64,
}

fn ev_text(e: &WebEvent) -> String {
    match e {
        WebEvent::Loading { tab, url, .. } => format!("Loading {tab} {url}"),
        WebEvent::Loaded {
            tab,
            url,
            ok,
            status,
            ..
        } => format!("Loaded {tab} {url} ok={ok} status={status}"),
        WebEvent::Url { tab, url } => format!("Url {tab} {url}"),
        WebEvent::SameDocument { tab, url } => format!("SameDocument {tab} {url}"),
        WebEvent::Title { tab, title } => format!("Title {tab} {title}"),
        WebEvent::History {
            tab,
            can_back,
            can_forward,
        } => format!("History {tab} {can_back} {can_forward}"),
        WebEvent::Refused { tab, url } => format!("Refused {tab} {url}"),
        WebEvent::Download {
            tab, file, host, ..
        } => format!("Download {tab} {file} {host}"),
        WebEvent::Failed { tab, why, .. } => format!("Failed {tab} {why}"),
        WebEvent::Gone { tab, why } => format!("Gone {tab} {why}"),
        WebEvent::Focus { tab, has } => format!("Focus {tab} {has}"),
        WebEvent::ModMessage { tab, host, json } => format!("ModMessage {tab} {host} {json}"),
        WebEvent::View { slot, event } => format!("View {:?} {}", slot.kind, ev_text(event)),
        other => other.kind().to_string(),
    }
}

impl H {
    fn new(data: &Path, virtual_hosts: Vec<(String, PathBuf)>) -> H {
        let queue: Arc<Mutex<Vec<WebEvent>>> = Arc::default();
        let q = Arc::clone(&queue);
        let host = WebHost::new(HostConfig {
            parent: 0,
            data_dir: data.to_path_buf(),
            sink: Arc::new(move |e| q.lock().unwrap_or_else(|e| e.into_inner()).push(e)),
            runtime: Arc::new(installed_runtime),
            virtual_hosts,
            without_webrtc_script: false,
            limits: None,
        });
        H {
            host,
            queue,
            seen: Vec::new(),
            script_seq: 0,
        }
    }

    fn turn(&mut self) {
        self.host.turn_headless();
        loop {
            let batch: Vec<WebEvent> =
                std::mem::take(&mut *self.queue.lock().unwrap_or_else(|e| e.into_inner()));
            if batch.is_empty() {
                break;
            }
            for e in batch {
                match e {
                    WebEvent::Runtime(r) => self.host.runtime_checked(r),
                    WebEvent::Pump => self.host.pump(),
                    other => self.seen.push(other),
                }
            }
        }
    }

    fn wait(&mut self, ms: u64) {
        let end = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < end {
            self.turn();
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Turns until an event from `from` on matches, or `ms` pass; the match.
    fn until(&mut self, from: usize, ms: u64, f: impl Fn(&WebEvent) -> bool) -> Option<WebEvent> {
        let end = Instant::now() + Duration::from_millis(ms);
        loop {
            self.turn();
            if let Some(e) = self.seen.iter().skip(from).find(|e| f(e)) {
                return Some(e.clone());
            }
            if Instant::now() > end {
                return None;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn mark(&self) -> usize {
        self.seen.len()
    }

    fn since(&self, from: usize) -> Vec<String> {
        self.seen.iter().skip(from).map(ev_text).collect()
    }

    /// `run_script` and its answer (the value's JSON).
    fn script(&mut self, tab: &str, js: &str) -> Result<String, String> {
        self.script_seq += 1;
        let id = self.script_seq;
        let from = self.mark();
        self.host.run_script(tab, id, js);
        match self.until(
            from,
            10_000,
            |e| matches!(e, WebEvent::Script { id: i, .. } if *i == id),
        ) {
            Some(WebEvent::Script { result, .. }) => result,
            _ => Err("no answer".into()),
        }
    }

    /// A DevTools `Runtime.evaluate` that awaits a promise, through `WebBackend::cdp`; the value.
    fn eval(&mut self, tab: &str, expr: &str) -> Result<Value, String> {
        self.script_seq += 1;
        let id = 1_000_000 + self.script_seq;
        let from = self.mark();
        let params = json!({"expression": expr, "awaitPromise": true, "returnByValue": true});
        self.host
            .cdp(tab, id, "Runtime.evaluate", &params.to_string());
        match self.until(
            from,
            15_000,
            |e| matches!(e, WebEvent::Cdp { id: i, .. } if *i == id),
        ) {
            Some(WebEvent::Cdp { result, .. }) => {
                let v: Value = serde_json::from_str(&result?).map_err(|e| e.to_string())?;
                Ok(v["result"]["value"].clone())
            }
            _ => Err("no answer".into()),
        }
    }

    fn opened(&mut self, from: usize, tab: &str, title: &str) -> bool {
        self.until(
            from,
            20_000,
            |e| matches!(e, WebEvent::Title { tab: t, title: n } if t == tab && n == title),
        )
        .is_some()
            && self
                .until(
                    from,
                    10_000,
                    |e| matches!(e, WebEvent::Loaded { tab: t, ok: true, .. } if t == tab),
                )
                .is_some()
    }

    fn click(&mut self, slot: &Slot, x: f32, y: f32) {
        for act in [
            PointerAct::Move,
            PointerAct::Down(PointerButton::Left, 1),
            PointerAct::Up(PointerButton::Left, 1),
        ] {
            self.host.pointer(
                slot,
                Pointer {
                    x,
                    y,
                    act,
                    mods: Mods::default(),
                },
            );
            self.wait(30);
        }
    }

    fn type_text(&mut self, slot: &Slot, vk: u32, mac: u16, text: &str) {
        for down in [true, false] {
            self.host.key(
                slot,
                KeyInput {
                    down,
                    repeat: false,
                    vk,
                    mac_code: mac,
                    text: text.to_string(),
                    mods: Mods::default(),
                },
            );
        }
        self.wait(30);
    }
}

/// The names in a folder (hidden ones too), sorted; none when it is not there.
fn names_in(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

fn png_size(png: &[u8]) -> Option<(u32, u32)> {
    if png.len() < 24 || &png[..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let w = u32::from_be_bytes(png[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(png[20..24].try_into().ok()?);
    Some((w, h))
}

fn panel(rep: &mut Report, data: &Path) -> Result<(), String> {
    let srv = pages::start()?;
    let downloads = data.join("Downloads");
    std::fs::create_dir_all(&downloads).map_err(|e| e.to_string())?;
    let mut h = H::new(data, Vec::new());
    h.host.set_downloads_dir(downloads.clone());
    h.host.set_scale(2.0);
    let tab = "t1";
    let slot = Slot::panel(tab);
    let main = srv.url("/main");

    // ---- opening and the address bar's events
    let from = h.mark();
    h.host.open(tab, &main, BOUNDS, true);
    let ready = h
        .until(
            from,
            20_000,
            |e| matches!(e, WebEvent::Ready { tab: t } if t == tab),
        )
        .is_some();
    let loaded = h.opened(from, tab, "Main");
    let ev = h.since(from);
    let has = |p: &str| ev.iter().any(|e| e.starts_with(p));
    rep.check(
        "panel.open",
        ready
            && loaded
            && has(&format!("Loading t1 {main}"))
            && has(&format!("Url t1 {main}"))
            && has(&format!("Loaded t1 {main} ok=true status=0"))
            && h.host.controllers() == 1,
        "open: Ready, then Loading, Url, Title and Loaded (status 0) for the address, as WebView2 reports them",
        json!({"events": ev}),
    );

    // ---- the picture: CEF's first paint comes some time after Loaded (on a CI runner's software compositor well
    // past the 300 ms a Mac with a GPU needs), so the check waits for a frame of the panel's size, as the engine's own
    // paint probe does, and says how long it took.
    let px_of = |f: &ViewFrame| {
        let i = ((100 * f.width + 240) * 4) as usize;
        f.bgra.get(i..i + 4).map(|p| (p[2], p[1], p[0], p[3]))
    };
    let t = Instant::now();
    let mut frame = None;
    while t.elapsed() < Duration::from_secs(10) {
        h.wait(50);
        frame = h.host.frame(&slot);
        if frame.as_ref().is_some_and(|f| {
            f.width == 1966 && f.height == 1400 && px_of(f) == Some((0x33, 0x66, 0xcc, 0xff))
        }) {
            break;
        }
    }
    let frame_ms = t.elapsed().as_millis();
    let px = frame.as_ref().and_then(px_of);
    rep.check(
        "panel.frame",
        frame
            .as_ref()
            .is_some_and(|f| f.width == 1966 && f.height == 1400)
            && px == Some((0x33, 0x66, 0xcc, 0xff)),
        "the page is a picture of the panel's device pixels, its colours where the page put them",
        json!({"size": frame.as_ref().map(|f| (f.width, f.height)), "pixel": format!("{px:?}"),
               "serial": frame.as_ref().map(|f| f.serial), "ms": frame_ms}),
    );
    let from = h.mark();
    h.host.capture_still(tab, 7);
    let still = h.until(from, 3_000, |e| {
        matches!(e, WebEvent::Still { token: 7, .. })
    });
    let size = match &still {
        Some(WebEvent::Still { png: Some(p), .. }) => png_size(p),
        _ => None,
    };
    rep.check(
        "panel.still",
        size == Some((1966, 1400)),
        "a still is a PNG of the page at the panel's size, answered with the caller's token",
        json!({"png_size": size}),
    );

    // ---- history
    let from = h.mark();
    h.host.navigate(tab, &srv.url("/page2"));
    let p2 = h.opened(from, tab, "Page two");
    let hist = h
        .until(from, 5_000, |e| {
            matches!(e, WebEvent::History { can_back: true, .. })
        })
        .is_some();
    let from_b = h.mark();
    h.host.go(tab, Nav::Back);
    // Each move is let finish (its load done, the history it leaves reported) before the next: a Forward sent
    // while Back's load is still under way is not a move forward (on the CI runner it reloaded the page Back was
    // loading).
    let back = h
        .until(
            from_b,
            10_000,
            |e| matches!(e, WebEvent::Url { url, .. } if url.ends_with("/main")),
        )
        .is_some()
        && h.until(from_b, 10_000, |e| {
            matches!(
                e,
                WebEvent::History {
                    can_forward: true,
                    ..
                }
            )
        })
        .is_some();
    let from_f = h.mark();
    h.host.go(tab, Nav::Forward);
    let fwd = h
        .until(
            from_f,
            10_000,
            |e| matches!(e, WebEvent::Url { url, .. } if url.ends_with("/page2")),
        )
        .is_some()
        && h.until(
            from_f,
            10_000,
            |e| matches!(e, WebEvent::Loaded { url, ok: true, .. } if url.ends_with("/page2")),
        )
        .is_some();
    let from_r = h.mark();
    h.host.go(tab, Nav::Reload);
    let reload = h
        .until(
            from_r,
            10_000,
            |e| matches!(e, WebEvent::Loading { url, .. } if url.ends_with("/page2")),
        )
        .is_some();
    h.host.go(tab, Nav::Stop);
    rep.check(
        "panel.history",
        p2 && hist && back && fwd && reload,
        "navigate, then back, forward and reload move the page and report History (can go back) and the address",
        json!({"page2": p2, "history": hist, "back": back, "forward": fwd, "reload": reload, "events": h.since(from)}),
    );
    let from = h.mark();
    h.host.navigate(tab, &main);
    h.opened(from, tab, "Main");

    // ---- same-document moves
    let from = h.mark();
    let _ = h.script(tab, "location.hash = 'frag'; 1");
    let frag = h
        .until(
            from,
            5_000,
            |e| matches!(e, WebEvent::SameDocument { url, .. } if url.ends_with("#frag")),
        )
        .is_some();
    let _ = h.script(tab, "history.pushState({}, '', '/pushed'); 1");
    let pushed = h
        .until(
            from,
            5_000,
            |e| matches!(e, WebEvent::SameDocument { url, .. } if url.ends_with("/pushed")),
        )
        .is_some();
    let ev = h.since(from);
    rep.check(
        "panel.same-document",
        frag && pushed && !ev.iter().any(|e| e.starts_with("Loading")),
        "a fragment and history.pushState are SameDocument moves, with no navigation",
        json!({"events": ev}),
    );

    // ---- refusals, a new window into the same view, a failed load
    let from = h.mark();
    let _ = h.script(tab, "location.href = 'mailto:someone@example.com'; 1");
    let mailto = h
        .until(
            from,
            5_000,
            |e| matches!(e, WebEvent::Refused { url, .. } if url.starts_with("mailto:")),
        )
        .is_some();
    let _ = h.script(tab, "location.href = 'file:///etc/hosts'; 1");
    let file = h
        .until(
            from,
            5_000,
            |e| matches!(e, WebEvent::Refused { url, .. } if url.starts_with("file:")),
        )
        .is_some();
    let os = h.host.refused(&slot).map(|r| r.os_launches);
    let ev = h.since(from);
    // Chromium itself stops a web page's file: navigation before it is a navigation; either way it never loads.
    let file_kept_out = file || !ev.iter().any(|e| e.contains(" file:"));
    let no_end_for_refused = !ev.iter().any(|e| e.starts_with("Loaded t1 mailto:"));
    rep.check(
        "panel.external-schemes",
        mailto && file_kept_out && no_end_for_refused,
        "mailto: is refused (WebEvent::Refused, with no load end of its own) and file: never loads; no program is started",
        json!({"events": h.since(from), "os_launch_requests": os}),
    );
    let from = h.mark();
    // A link with target=_blank, clicked (a popup needs the person's gesture).
    h.click(&slot, 240.0, 490.0);
    let same = h.opened(from, tab, "Page two");
    rep.check(
        "panel.new-window-same-view",
        same && h.host.controllers() == 1 && screen::windows_of_self() == 0,
        "window.open loads in the same view: no second view, no window",
        json!({"events": h.since(from), "controllers": h.host.controllers()}),
    );
    let closed_port = std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .unwrap_or(9);
    let from = h.mark();
    h.host
        .navigate(tab, &format!("http://127.0.0.1:{closed_port}/"));
    let failed = h.until(from, 10_000, |e| {
        matches!(e, WebEvent::Loaded { ok: false, .. })
    });
    rep.check(
        "panel.failed-load",
        matches!(&failed, Some(WebEvent::Loaded { status, .. }) if status == "12"),
        "a refused connection ends the load not ok, with WebView2's CANNOT_CONNECT (12), so the failure notice reads it",
        json!({"event": failed.as_ref().map(ev_text)}),
    );
    let from = h.mark();
    h.host.navigate(tab, &main);
    h.opened(from, tab, "Main");

    // ---- script dialogs and permissions, never shown
    let dialogs = h.script(tab, "[alert('a'), confirm('b'), prompt('c')]");
    rep.check(
        "panel.dialogs",
        dialogs.as_deref() == Ok("[null,false,null]"),
        "alert returns, confirm is false, prompt is null: answered by the host, no dialog",
        json!({"result": format!("{dialogs:?}"), "refused": format!("{:?}", h.host.refused(&slot))}),
    );
    let perms = h.eval(
        tab,
        "(async () => { const o = {};\
         try { await navigator.mediaDevices.getUserMedia({video: true, audio: true}); o.media = 'GOT'; } catch (e) { o.media = e.name; }\
         o.geo = await new Promise(r => { navigator.geolocation.getCurrentPosition(() => r('GOT'), e => r('error ' + e.code)); setTimeout(() => r('no answer'), 4000); });\
         o.notif = await Notification.requestPermission();\
         try { await navigator.clipboard.readText(); o.clip = 'READ'; } catch (e) { o.clip = e.name; }\
         return o; })()",
    );
    let p = perms.clone().unwrap_or(Value::Null);
    rep.check(
        "panel.permissions",
        (p["media"] == "NotAllowedError" || p["media"] == "NotFoundError")
            && p["geo"] == "error 1"
            && p["notif"] == "denied"
            && p["clip"] == "NotAllowedError"
            && screen::windows_of_self() == 0,
        "camera and mic (CEF's media stream is off: no device is offered), location, notifications and clipboard read are denied, with no prompt",
        json!({"result": p, "error": perms.err(), "refused": format!("{:?}", h.host.refused(&slot))}),
    );

    // ---- downloads: held for a yes, nothing saved on a no
    let from = h.mark();
    h.host.navigate(tab, &srv.url("/report.txt"));
    let d1 = h.until(from, 10_000, |e| matches!(e, WebEvent::Download { .. }));
    let saved = downloads.join("report.txt");
    let held_empty = !saved.exists();
    if let Some(WebEvent::Download { id, .. }) = &d1 {
        h.host.answer_download(*id, false);
    }
    h.wait(1_500);
    let no_left_nothing = !saved.exists();
    let from2 = h.mark();
    h.host.navigate(tab, &srv.url("/report.txt"));
    let d2 = h.until(from2, 10_000, |e| matches!(e, WebEvent::Download { .. }));
    if let Some(WebEvent::Download { id, .. }) = &d2 {
        h.host.answer_download(*id, true);
    }
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(10)
        && std::fs::read_to_string(&saved).ok().as_deref() != Some("the report's text")
    {
        h.wait(50);
    }
    let text = std::fs::read_to_string(&saved).unwrap_or_default();
    // A saved file carries Gatekeeper's quarantine mark (Chromium's own, as Safari's), so a downloaded app or
    // script is checked by the system before it first runs.
    let quarantine = Command::new("/usr/bin/xattr")
        .args(["-p", "com.apple.quarantine"])
        .arg(&saved)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    rep.check(
        "panel.download-quarantine",
        quarantine.as_deref().is_some_and(|q| !q.is_empty()),
        "a file saved after the yes carries com.apple.quarantine",
        json!({"quarantine": quarantine}),
    );
    rep.check(
        "panel.downloads",
        matches!(&d1, Some(WebEvent::Download { file, host, .. }) if file == "report.txt" && host == "127.0.0.1")
            && held_empty
            && no_left_nothing
            && d2.is_some()
            && text == "the report's text",
        "a download waits for the person: nothing is saved while it is held or after a no; a yes saves it in Downloads",
        json!({"first": d1.as_ref().map(ev_text), "second": d2.as_ref().map(ev_text), "held_empty": held_empty,
               "after_no": no_left_nothing, "saved": text}),
    );

    // ---- a held download's bytes wait in Flare's staging folder, never in the person's Downloads
    let staging = crate::walls::download_staging(data);
    let mine = |name: &str| {
        name.contains("flare")
            || name.contains("held.bin")
            || name.ends_with(".crdownload")
            || name.starts_with("Unconfirmed")
    };
    let home_dl = std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Downloads"));
    let before = home_dl.as_deref().map(names_in).unwrap_or_default();
    let from = h.mark();
    h.host.navigate(tab, &srv.url("/held.bin"));
    let held = h.until(from, 10_000, |e| matches!(e, WebEvent::Download { .. }));
    // Chromium writes the first part while the yes is awaited.
    h.wait(1_500);
    let staged = names_in(&staging);
    let new_home: Vec<String> = home_dl
        .as_deref()
        .map(names_in)
        .unwrap_or_default()
        .into_iter()
        .filter(|n| !before.contains(n) && mine(n))
        .collect();
    if let Some(WebEvent::Download { id, .. }) = &held {
        h.host.answer_download(*id, false);
    }
    h.wait(1_000);
    let after_no = names_in(&staging);
    let staging_pref = h
        .host
        .profile_preference(&slot, flare_cef::macos::context::DOWNLOAD_DIR_PREF);
    rep.check(
        "panel.download-staging",
        held.is_some()
            && home_dl.is_some()
            && new_home.is_empty()
            && !staged.is_empty()
            && after_no.is_empty()
            && staging_pref == staging.to_string_lossy(),
        "while a download is held, its bytes are in Flare's staging folder (the profile's download folder), nothing new is in ~/Downloads, and a no leaves nothing",
        json!({"held": held.as_ref().map(ev_text), "staged": staged, "new_in_home_downloads": new_home,
               "staging_after_no": after_no, "download_dir_pref": staging_pref}),
    );

    // ---- no native picker: the colour panel and the print panel cannot come on screen
    let guarded = flare_cef::macos::panels::installed();
    let printing = h
        .host
        .profile_preference(&slot, flare_cef::macos::context::PRINTING_PREF);
    let mut picker = json!({"guarded": guarded, "printing_pref": printing});
    let mut pickers_ok = false;
    // Only once a panel cannot appear is one asked for (the screen rule).
    if guarded && printing == "false" {
        let (c0, p0) = flare_cef::macos::panels::blocked();
        let from = h.mark();
        h.host.navigate(tab, &srv.url("/pickers"));
        let opened = h.opened(from, tab, "Pickers");
        h.wait(200);
        h.host.focus(tab);
        h.click(&slot, 240.0, 120.0);
        h.wait(1_000);
        let windows_after_click = screen::windows_of_self();
        let picked = h.eval(tab, "window.__picked");
        let printed = h.eval(tab, "window.print(); window.__printed");
        h.wait(1_000);
        let (c1, p1) = flare_cef::macos::panels::blocked();
        let windows = screen::windows_of_self();
        pickers_ok = opened
            && picked.as_ref().is_ok_and(|v| *v == json!(1))
            && windows_after_click == 0
            && windows == 0
            && printed.is_ok();
        picker = json!({"guarded": guarded, "printing_pref": printing, "opened": opened,
                        "colour_panels_kept_off": c1 - c0, "print_runs_cancelled": p1 - p0,
                        "trusted_clicks_on_the_input": format!("{picked:?}"),
                        "print_answer": format!("{printed:?}"), "windows_after_click": windows_after_click,
                        "windows": windows});
    }
    rep.check(
        "panel.no-native-pickers",
        pickers_ok,
        "a colour input's click and window.print() put no panel on the screen: printing is off in the profile and the colour, print and page layout panels cannot be ordered in",
        picker,
    );
    let from = h.mark();
    h.host.navigate(tab, &main);
    h.opened(from, tab, "Main");

    // ---- zoom
    h.host.set_zoom(tab, 1.5);
    h.wait(300);
    let level = h.host.zoom_level(&slot).unwrap_or(0.0);
    let dpr = h.script(tab, "window.devicePixelRatio");
    h.host.set_zoom(tab, 1.0);
    h.wait(300);
    rep.check(
        "panel.zoom",
        (level - flare_cef::status::zoom_level(1.5)).abs() < 0.01 && dpr.as_deref() == Ok("3"),
        "the browser's zoom of the page (Cmd+wheel's): 150 % is Chromium's level 2.22, the page's devicePixelRatio 3 at 2x",
        json!({"level": level, "dpr": format!("{dpr:?}")}),
    );

    // ---- pointer, wheel, keys, the key filter, the input method, focus
    // The click on the new-window link already gave the page the keyboard: take it back first.
    h.host.blur(tab);
    h.wait(50);
    let from = h.mark();
    h.host.focus(tab);
    let focus = h
        .until(from, 3_000, |e| {
            matches!(e, WebEvent::Focus { has: true, .. })
        })
        .is_some();
    h.click(&slot, 240.0, 100.0);
    let clicks = h.script(tab, "window.__clicks");
    let trusted = h.script(
        tab,
        "window.__log.filter(function (l) { return l.indexOf('click') === 0; }).join(',')",
    );
    rep.check(
        "panel.pointer",
        clicks.as_deref() == Ok("1") && trusted.as_deref() == Ok("\"click:true\""),
        "a click the window forwards is one trusted click where it was made (device pixels to CSS pixels)",
        json!({"clicks": format!("{clicks:?}"), "log": format!("{trusted:?}"), "focus_event": focus}),
    );
    h.click(&slot, 340.0, 270.0);
    h.type_text(&slot, 0x41, 0x00, "a");
    h.type_text(&slot, 0, 0, "é");
    h.host.ime(
        &slot,
        ImeInput::Preedit {
            text: "ni".into(),
            cursor: 2,
        },
    );
    h.wait(50);
    h.host.ime(&slot, ImeInput::Commit("你好".into()));
    h.wait(100);
    let value = h.script(tab, "document.getElementById('t1').value");
    // Backspace as the window forwards it: the key alone, no text (Slint's key names, `web_live::key_of`).
    h.host.key(
        &slot,
        KeyInput {
            down: true,
            vk: 0x08,
            mac_code: 0x33,
            ..Default::default()
        },
    );
    h.wait(50);
    let after_down = h.script(tab, "document.getElementById('t1').value");
    h.host.key(
        &slot,
        KeyInput {
            down: false,
            vk: 0x08,
            mac_code: 0x33,
            ..Default::default()
        },
    );
    h.wait(50);
    let after_back = h.script(tab, "document.getElementById('t1').value");
    rep.check(
        "panel.keys-and-ime",
        value.as_deref() == Ok("\"aé你好\"")
            && after_down.as_deref() == Ok("\"aé你\"")
            && after_back.as_deref() == Ok("\"aé你\""),
        "keys type into the focused field (ASCII and an accent), the input method's text goes in when committed, Backspace deletes",
        json!({"value": format!("{value:?}"), "after_backspace_down": format!("{after_down:?}"),
               "after_backspace_up": format!("{after_back:?}")}),
    );
    let keys_before = h.script(tab, "window.__keys").unwrap_or_default();
    h.host.set_key_filter(Rc::new(|k| k.ctrl && k.vk == 0x54));
    let from = h.mark();
    h.host.key(
        &slot,
        KeyInput {
            down: true,
            repeat: false,
            vk: 0x54,
            mac_code: 0x11,
            text: "t".into(),
            mods: Mods {
                cmd: true,
                ..Default::default()
            },
        },
    );
    let taken = h
        .until(
            from,
            2_000,
            |e| matches!(e, WebEvent::Key { key, .. } if key.vk == 0x54 && key.ctrl),
        )
        .is_some();
    h.wait(100);
    let keys_after = h.script(tab, "window.__keys").unwrap_or_default();
    rep.check(
        "panel.key-filter",
        taken && keys_before == keys_after,
        "a chord the app takes (Cmd+T, the filter's ctrl on a Mac) goes to the window and never to the page",
        json!({"taken": taken, "page_keydowns_before": keys_before, "after": keys_after}),
    );
    h.host.pointer(
        &slot,
        Pointer {
            x: 800.0,
            y: 800.0,
            act: PointerAct::Wheel { dx: 0.0, dy: 480.0 },
            mods: Mods::default(),
        },
    );
    h.wait(400);
    let scrolled = h.script(tab, "window.scrollY");
    rep.check(
        "panel.wheel",
        scrolled
            .as_deref()
            .ok()
            .and_then(|s| s.parse::<f64>().ok())
            .is_some_and(|y| y > 0.0),
        "a wheel turned toward the person scrolls the page down",
        json!({"scrollY": format!("{scrolled:?}")}),
    );
    let _ = h.script(tab, "window.scrollTo(0, 0); 1");
    h.wait(100);
    let popups_before = h.host.popup_paints(&slot).unwrap_or(0);
    h.click(&slot, 240.0, 372.0);
    h.wait(500);
    let popups = h.host.popup_paints(&slot).unwrap_or(0);
    rep.check(
        "panel.select-list",
        popups > popups_before && screen::windows_of_self() == 0,
        "a <select>'s list is a painted layer of the picture, not a native menu",
        json!({"popup_paints": popups}),
    );
    h.host.pointer(
        &slot,
        Pointer {
            x: 10.0,
            y: 10.0,
            act: PointerAct::Leave,
            mods: Mods::default(),
        },
    );
    let from = h.mark();
    h.host.blur(tab);
    let blurred = h
        .until(from, 2_000, |e| {
            matches!(e, WebEvent::Focus { has: false, .. })
        })
        .is_some();
    rep.check(
        "panel.focus",
        focus && blurred,
        "the page takes the keyboard (Focus true) and gives it back (Focus false)",
        json!({"focus": focus, "blur": blurred}),
    );

    // ---- DevTools, a crashed page, closing
    let two = h.eval(tab, "1 + 1");
    rep.check(
        "panel.cdp",
        two == Ok(json!(2)),
        "a DevTools call through the backend answers with Chromium's result",
        json!({"result": format!("{two:?}")}),
    );
    let from = h.mark();
    h.host.cdp(tab, 999_999, "Page.crash", "{}");
    let gone = h.until(from, 10_000, |e| matches!(e, WebEvent::Gone { .. }));
    rep.check(
        "panel.gone",
        gone.is_some(),
        "a page whose process ends is reported Gone",
        json!({"event": gone.as_ref().map(ev_text)}),
    );
    h.host.close(tab);
    h.wait(300);
    let refused = h.host.refused(&slot);
    rep.check(
        "panel.closed",
        h.host.controllers() == 0 && refused.is_none(),
        "close frees the view",
        json!({"controllers": h.host.controllers()}),
    );
    rep.set(
        "events",
        json!(h.seen.iter().map(ev_text).collect::<Vec<_>>()),
    );
    rep.set("requests", json!(srv.requests()));
    h.host.close_all();
    Ok(())
}

/// The dual-stack server's log as one line per connection: the peer and the request line, or `TLS` for a
/// ClientHello (an https or wss attempt, whose binary text spreads over several log lines).
fn hit_summary(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .filter_map(|l| {
            let (ts, rest) = l.split_once(' ')?;
            let (secs, ms) = ts.split_once('.')?;
            if secs.is_empty() || !secs.bytes().all(|b| b.is_ascii_digit()) || ms.len() != 3 {
                return None;
            }
            let (peer, req) = rest.split_once(' ')?;
            Some(if let Some(get) = req.strip_prefix("GET ") {
                format!("{peer} GET {}", get.split_whitespace().next().unwrap_or(""))
            } else if req.starts_with('\u{16}') {
                format!("{peer} TLS")
            } else {
                format!("{peer} other")
            })
        })
        .collect()
}

struct Observers {
    dir: PathBuf,
}

impl Observers {
    fn start(dir: &Path) -> Result<Observers, String> {
        let _ = std::fs::remove_dir_all(dir);
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        observers::start(dir).map_err(|e| format!("the network observers did not start: {e}"))?;
        Ok(Observers {
            dir: dir.to_path_buf(),
        })
    }

    fn lines(&self, name: &str) -> Vec<String> {
        std::fs::read_to_string(self.dir.join(format!("{name}.log")))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

fn walls(rep: &mut Report, data: &Path) -> Result<(), String> {
    let srv = pages::start()?;
    let obs = Observers::start(&data.join("netlogs"))?;
    // The observers must be listening, or an empty log proves nothing: a connection of the probe's own is seen.
    let seen_before = obs.lines("tcp").len();
    let _ = std::net::TcpStream::connect(("127.0.0.1", pages::OBS_TCP))
        .and_then(|mut s| std::io::Write::write_all(&mut s, b"probe-self-test"));
    let t = Instant::now();
    while obs.lines("tcp").len() == seen_before && t.elapsed() < Duration::from_secs(3) {
        std::thread::sleep(Duration::from_millis(50));
    }
    let tcp_up = obs.lines("tcp").len() > seen_before;
    rep.check(
        "walls.observers",
        tcp_up,
        "the network observers listen: a connection of the probe's own is logged",
        json!({"tcp": obs.lines("tcp")}),
    );
    let pages_dir = data.join("pages");
    std::fs::create_dir_all(&pages_dir).map_err(|e| e.to_string())?;
    let mut h = H::new(
        &data.join("profile"),
        vec![("flare-test.local".to_string(), pages_dir.clone())],
    );
    h.host.set_scale(2.0);

    // ---- control: the web panel has no request wall, so the exfiltration page's requests leave
    let tcp0 = obs.lines("tcp").len();
    let from = h.mark();
    h.host.open("ctl", &srv.url("/exfil"), BOUNDS, true);
    let ctl_open = h.opened(from, "ctl", "Exfil");
    h.wait(4_000);
    let ctl_hits = srv.hits("advx-");
    let ctl_tcp = obs.lines("tcp").len() - tcp0;
    rep.check(
        "walls.control",
        ctl_open && ctl_hits.len() >= 10,
        "control: the same page in the web panel (no wall) reaches the server on most of its channels, so the observers see what leaves",
        json!({"opened": ctl_open, "hits": ctl_hits, "tcp_accepts": ctl_tcp}),
    );
    h.host.close("ctl");
    h.wait(300);

    // ---- a sealed mini-app: its folder, nothing of the network
    let app_dir = data.join("app1");
    std::fs::create_dir_all(&app_dir).map_err(|e| e.to_string())?;
    let tok = format!("s{}", std::process::id());
    std::fs::write(app_dir.join("index.html"), pages::exfil(srv.port, &tok))
        .map_err(|e| e.to_string())?;
    let host1 = app_host(1);
    let spec = AppSpec {
        url: app_url(&host1),
        host: host1,
        folder: app_dir.clone(),
        allow_network: false,
        module: None,
    };
    let l0 = srv.requests().len();
    let t0 = obs.lines("tcp").len();
    let from = h.mark();
    h.host.open_app("app", spec, BOUNDS, true);
    let app_open = h.opened(from, "app", "Exfil");
    h.wait(5_000);
    let page = h.script("app", "JSON.stringify(window.__r)");
    let hits: Vec<String> = srv.requests()[l0..]
        .iter()
        .filter(|p| p.contains("advx-"))
        .cloned()
        .collect();
    let tcp: Vec<String> = obs.lines("tcp")[t0..].to_vec();
    let mdns: Vec<String> = obs
        .lines("mdns")
        .into_iter()
        .filter(|l| l.contains(&tok))
        .collect();
    let app_slot = Slot::panel("app");
    let refused = h.host.refused(&app_slot);
    let page_v: Value = page
        .as_deref()
        .ok()
        .and_then(|s| serde_json::from_str::<String>(s).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null);
    rep.check(
        "walls.mini-app-sealed",
        app_open
            && hits.is_empty()
            && tcp.is_empty()
            && mdns.is_empty()
            && page_v["rtc"] == "undefined"
            && page_v["fetch"] == "blocked"
            && refused.as_ref().is_some_and(|r| r.requests > 0),
        "a no-network mini-app is served from its folder and nothing leaves: preconnect, dns-prefetch, .local names, ping, preload, fonts, media, object, svg, srcset, srcdoc, form, import(), a worker, fetch, a beacon, a websocket; no peer connection API",
        json!({"opened": app_open, "server_hits": hits, "tcp_accepts": tcp, "mdns": mdns, "page": page_v,
               "refused": format!("{refused:?}")}),
    );
    let proxy = h.host.profile_preference(&app_slot, "proxy");
    let rtc = h
        .host
        .profile_preference(&app_slot, flare_cef::macos::context::WEBRTC_PREF);
    rep.check(
        "walls.mini-app-profile",
        proxy.contains("mode=fixed_servers")
            && proxy.contains("server=http://127.0.0.1:9")
            && proxy.contains("bypass_list=<-loopback>")
            && rtc == "disable_non_proxied_udp",
        "behind the wall, the profile's own: every request to a proxy that is not there, loopback included, and no UDP outside a proxy for WebRTC",
        json!({"proxy": proxy, "webrtc": rtc}),
    );
    let from = h.mark();
    let _ = h.script("app", "location.href = 'https://example.com/'; 1");
    let nav_refused = h
        .until(
            from,
            5_000,
            |e| matches!(e, WebEvent::Refused { tab, .. } if tab == "app"),
        )
        .is_some();
    let _ = h.script("app", "window.open('https://example.com/'); 1");
    h.wait(500);
    let ev = h.since(from);
    rep.check(
        "walls.mini-app-stays",
        nav_refused
            && !ev
                .iter()
                .any(|e| e.starts_with("Loading app https://example.com"))
            && h.host.controllers() == 1,
        "a mini-app cannot leave its page or open a window",
        json!({"events": ev}),
    );
    h.host.close("app");
    h.wait(300);

    // ---- two mini-apps open at once: a worker of one, shared or dedicated, gets its own files and none of the other's
    let host_a = app_host(5);
    let host_b = app_host(6);
    let a_dir = data.join("swa");
    let b_dir = data.join("swb");
    for d in [&a_dir, &b_dir] {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    std::fs::write(
        a_dir.join("index.html"),
        "<!doctype html><title>App A</title>A",
    )
    .map_err(|e| e.to_string())?;
    std::fs::write(a_dir.join("secret.txt"), "TOPSECRET-A").map_err(|e| e.to_string())?;
    std::fs::write(a_dir.join("leak.js"), "self.__leak = 'A-SCRIPT-SECRET';")
        .map_err(|e| e.to_string())?;
    std::fs::write(b_dir.join("own.txt"), "OWN-B").map_err(|e| e.to_string())?;
    std::fs::write(b_dir.join("worker.js"), pages::cross_app_worker(&host_a))
        .map_err(|e| e.to_string())?;
    std::fs::write(b_dir.join("index.html"), pages::CROSS_APP_PAGE).map_err(|e| e.to_string())?;
    let mut cross_open = true;
    for (tab, host, dir, title) in [
        ("swa", &host_a, &a_dir, "App A"),
        ("swb", &host_b, &b_dir, "App B"),
    ] {
        let from = h.mark();
        h.host.open_app(
            tab,
            AppSpec {
                url: app_url(host),
                host: host.clone(),
                folder: dir.clone(),
                allow_network: false,
                module: None,
            },
            BOUNDS,
            true,
        );
        cross_open &= h.opened(from, tab, title);
    }
    let mut cross = Value::Null;
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(10) {
        cross = h
            .eval("swb", "JSON.stringify(window.__w || {})")
            .ok()
            .and_then(|v| v.as_str().and_then(|s| serde_json::from_str(s).ok()))
            .unwrap_or(Value::Null);
        if cross["shared"].is_object() && cross["dedicated"].is_object() {
            break;
        }
        h.wait(200);
    }
    // Refused is a 403 of Flare's wall (or a failed fetch), never the other app's bytes, not even opaque ones.
    let walled = |w: &Value| {
        let other = w["other"].as_str().unwrap_or("");
        w["own"] == "basic 200 OWN-B"
            && !other.contains("TOPSECRET")
            && (other.contains(" 403") || other.starts_with("ERR"))
            && w["import"].as_str().is_some_and(|i| i.starts_with("ERR"))
    };
    let a_profile = h
        .host
        .probe_view(&Slot::panel("swa"))
        .map(|p| (p.profile, p.private));
    rep.check(
        "walls.mini-app-shared-worker",
        cross_open && walled(&cross["shared"]) && walled(&cross["dedicated"])
            && a_profile == Some((flare_surfaces::app::PROFILE_NAME.to_string(), true)),
        "with two mini-apps open, a shared worker and a dedicated worker of the second get its own file and neither the first app's file (no-cors fetch) nor its script (importScripts): each mini-app has its own private profile and wall",
        json!({"opened": cross_open, "workers": cross, "profile": format!("{a_profile:?}")}),
    );
    h.host.close("swa");
    h.host.close("swb");
    h.wait(300);

    // ---- a mini-app with the network: through Flare's filtering proxy, and never to this machine
    let net_dir = data.join("app2");
    std::fs::create_dir_all(&net_dir).map_err(|e| e.to_string())?;
    std::fs::write(
        net_dir.join("index.html"),
        format!(
            "<!doctype html><title>Net</title><script>window.__r={{}};\
             fetch('http://127.0.0.1:{p}/advx-net-http').then(()=>__r.http='LEAK',()=>__r.http='blocked');\
             fetch('https://127.0.0.1:{p}/advx-net-https').then(()=>__r.https='LEAK',()=>__r.https='blocked');</script>",
            p = srv.port
        ),
    )
    .map_err(|e| e.to_string())?;
    let host2 = app_host(2);
    let net = AppSpec {
        url: app_url(&host2),
        host: host2,
        folder: net_dir,
        allow_network: true,
        module: None,
    };
    let from = h.mark();
    h.host.open_app("net", net, BOUNDS, true);
    let net_open = h.opened(from, "net", "Net");
    h.wait(3_000);
    let net_slot = Slot::panel("net");
    let net_proxy = h.host.profile_preference(&net_slot, "proxy");
    let netport = flare_surfaces::netproxy::port().unwrap_or(0);
    let r = h.script("net", "JSON.stringify(window.__r)");
    let net_hits = srv.hits("advx-net");
    rep.check(
        "walls.mini-app-network",
        net_open
            && net_proxy.contains(&format!("server=http://127.0.0.1:{netport}"))
            && net_hits.is_empty(),
        "a mini-app granted https goes through Flare's filtering proxy, and neither http nor this machine is reached",
        json!({"opened": net_open, "proxy": net_proxy, "netproxy_port": netport, "page": format!("{r:?}"), "hits": net_hits}),
    );
    h.host.close("net");
    h.wait(300);

    // ---- every spelling of this machine and its network, from a view behind Flare's proxy (the spike's proxy
    // page): a networked mini-app, a plugin with the network, a mod, against the web panel as the control
    let (lan, local_name) = pages::lan_and_name();
    let mut lit = Vec::new();
    let mut lit_ok = true;
    for (tab, kind) in [
        ("lit-ctl", "web panel (control)"),
        ("lit-app", "mini-app with the network"),
        ("lit-plug", "plugin with the network"),
        ("lit-mod", "mod"),
    ] {
        let dir = data.join(tab);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        std::fs::write(
            dir.join("index.html"),
            pages::literals(&lan, &local_name, tab),
        )
        .map_err(|e| e.to_string())?;
        let dir = std::fs::canonicalize(&dir).map_err(|e| e.to_string())?;
        let h0 = obs.lines("http").len();
        let t0 = obs.lines("tcp").len();
        let m0 = obs.lines("mdns").len();
        let from = h.mark();
        match tab {
            "lit-ctl" => h.host.open(tab, &srv.url("/literals"), BOUNDS, true),
            "lit-app" => {
                let host = app_host(4);
                h.host.open_app(
                    tab,
                    AppSpec {
                        url: app_url(&host),
                        host,
                        folder: dir.clone(),
                        allow_network: true,
                        module: None,
                    },
                    BOUNDS,
                    true,
                );
            }
            "lit-plug" => h.host.open_app(
                tab,
                plugin_spec("litnet", &dir, "index.html", true),
                BOUNDS,
                true,
            ),
            _ => h
                .host
                .open_app(tab, mod_spec("lit", &dir, "index.html"), BOUNDS, true),
        }
        let opened = h.opened(from, tab, "Literals");
        h.wait(6_000);
        let page = h.script(tab, "JSON.stringify(window.__r)");
        let direct = hit_summary(&obs.lines("http")[h0..]);
        let tcp = obs.lines("tcp")[t0..].to_vec();
        let mdns: Vec<String> = obs.lines("mdns")[m0..]
            .iter()
            .filter(|l| l.contains(&format!("advdns-{tab}")))
            .cloned()
            .collect();
        let proxy = h.host.profile_preference(&Slot::panel(tab), "proxy");
        let control = tab == "lit-ctl";
        let ok = if control {
            opened && direct.len() >= 5
        } else {
            opened
                && direct.is_empty()
                && tcp.is_empty()
                && mdns.is_empty()
                && proxy.contains("bypass_list=<-loopback>")
        };
        lit_ok &= ok;
        lit.push(json!({"view": kind, "ok": ok, "opened": opened, "direct_hits": direct, "tcp_accepts": tcp, "mdns": mdns,
                        "proxy": proxy, "page": format!("{page:?}")}));
        h.host.close(tab);
        h.wait(300);
    }
    rep.check(
        "walls.netproxy-literals",
        lit_ok,
        "behind Flare's proxy (a networked mini-app, a plugin with the network, a mod) no spelling of this machine or \
         its network reaches the observers' server: loopback in five IPv4 spellings, 0.0.0.0, localhost and its forms, \
         IPv6 loopback and the mapped address, the LAN address, .local names (no mDNS query either), names that \
         resolve to loopback, over http, https, ws and wss; the same page in the web panel (no proxy) does reach it",
        json!({"lan": lan, "local_name": local_name, "views": lit}),
    );

    // ---- a mod's page: its files, its bridge, its walls
    let mod_dir = data.join("mod");
    std::fs::create_dir_all(&mod_dir).map_err(|e| e.to_string())?;
    std::fs::write(mod_dir.join("index.html"), pages::MOD_PAGE).map_err(|e| e.to_string())?;
    std::fs::write(data.join("secret.txt"), "secret").map_err(|e| e.to_string())?;
    let mod_dir = std::fs::canonicalize(&mod_dir).map_err(|e| e.to_string())?;
    let spec = mod_spec("probe", &mod_dir, "index.html");
    let mod_host = spec.host.clone();
    let from = h.mark();
    h.host.open_app("mod", spec, BOUNDS, true);
    let mod_open = h.opened(from, "mod", "Mod page");
    let msg = h.until(
        from,
        5_000,
        |e| matches!(e, WebEvent::ModMessage { json, .. } if json.contains("view.text")),
    );
    h.host
        .post_message("mod", r#"{"t":"reply","id":1,"result":5}"#);
    h.wait(300);
    let got = h.script("mod", "JSON.stringify(window.__got)");
    let bridge = h.script(
        "mod",
        "typeof window.flare + ',' + typeof window.__flarePost + ',' + Object.isFrozen(window.chrome.webview)",
    );
    let outside = h.eval(
        "mod",
        &format!(
            "Promise.all([fetch('/../../secret.txt').then(r => r.status, () => 'blocked'),\
             fetch('http://127.0.0.1:{}/advx-frommod').then(() => 'LEAK', () => 'blocked')])",
            srv.port
        ),
    );
    rep.check(
        "walls.mod-bridge",
        mod_open
            && matches!(&msg, Some(WebEvent::ModMessage { host, .. }) if *host == mod_host)
            && got
                .as_deref()
                .is_ok_and(|g| g.contains("reply") && g.contains("5"))
            && bridge.as_deref() == Ok("\"object,undefined,true\""),
        "a mod's page talks over window.chrome.webview (the bridge unchanged): its message reaches the window with its host, the window's reaches the page, the binding itself is hidden",
        json!({"opened": mod_open, "message": msg.as_ref().map(ev_text), "got": format!("{got:?}"), "bridge": format!("{bridge:?}")}),
    );
    rep.check(
        "walls.mod-files",
        outside
            .as_ref()
            .is_ok_and(|v| v[0] != 200 && v[1] == "blocked")
            && srv.hits("advx-frommod").is_empty(),
        "a mod is served only its own folder and reaches nothing else (its policy stops the fetch; the wall would answer it)",
        json!({"result": format!("{outside:?}")}),
    );
    let mod_profile = h
        .host
        .probe_view(&Slot::panel("mod"))
        .map(|p| (p.profile, p.private));
    let kept = data.join("profile/mods/flare-mod-probe").exists();
    rep.check(
        "walls.profiles",
        mod_profile == Some(("flare-mod-probe".to_string(), false))
            && kept
            && !data.join("profile/mods/flare-app").exists(),
        "a mod keeps its own profile on disk; a mini-app's is private and leaves nothing",
        json!({"mod": format!("{mod_profile:?}"), "mod_folder": kept}),
    );

    // ---- a plugin's bar: sealed, no download
    let bar_dir = data.join("bar");
    std::fs::create_dir_all(&bar_dir).map_err(|e| e.to_string())?;
    std::fs::write(
        bar_dir.join("index.html"),
        "<!doctype html><title>Bar</title><a id=d download href=\"data.txt\">d</a>",
    )
    .map_err(|e| e.to_string())?;
    std::fs::write(bar_dir.join("data.txt"), "x").map_err(|e| e.to_string())?;
    let bar_dir = std::fs::canonicalize(&bar_dir).map_err(|e| e.to_string())?;
    let bar = Slot::bar("mod");
    let from = h.mark();
    h.host.open_view(
        &bar,
        plugin_spec("bar", &bar_dir, "index.html", false),
        PhysRect {
            w: 600,
            h: 80,
            ..BOUNDS
        },
        true,
    );
    let bar_ready = h
        .until(
            from,
            20_000,
            |e| matches!(e, WebEvent::View { event, .. } if matches!(**event, WebEvent::Ready { .. })),
        )
        .is_some();
    h.wait(1_000);
    // A script for a bar goes through DevTools of the slot: the window never runs one there, the test does.
    h.host
        .inspect_exec(&bar, "document.getElementById('d').click(); alert('x');");
    h.wait(1_500);
    let ev = h.since(from);
    let bar_refused = h.host.refused(&bar);
    rep.check(
        "walls.plugin-bar",
        bar_ready
            && !ev.iter().any(|e| e.contains("Download"))
            && bar_refused
                .as_ref()
                .is_some_and(|r| r.downloads >= 1 && r.js_dialogs >= 1),
        "a plugin's bar is sealed: its download is refused before it starts, its alert answered, nothing asked",
        json!({"events": ev, "refused": format!("{bar_refused:?}")}),
    );
    // A bar takes only a plugin's page.
    let from = h.mark();
    let h3 = app_host(3);
    h.host.open_view(
        &Slot::bar("x"),
        AppSpec {
            url: app_url(&h3),
            host: h3,
            folder: data.to_path_buf(),
            allow_network: false,
            module: None,
        },
        BOUNDS,
        true,
    );
    h.wait(100);
    let ev = h.since(from);
    rep.check(
        "walls.bar-takes-plugins-only",
        ev.iter().any(|e| e.contains("Failed")),
        "a bar slot refuses a mini-app",
        json!({"events": ev}),
    );
    rep.set(
        "events",
        json!(h.seen.iter().map(ev_text).collect::<Vec<_>>()),
    );
    h.host.close_all();
    Ok(())
}

/// The processes of this probe's bundle (the probe and every helper CEF started): (pid, command line).
fn bundle_processes() -> Vec<(String, String)> {
    let exe = std::env::current_exe().unwrap_or_default();
    let app = exe
        .ancestors()
        .find(|p| p.extension().is_some_and(|e| e == "app"))
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let out = Command::new("/bin/ps")
        .args(["-axww", "-o", "pid=,command="])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    out.lines()
        .filter(|l| !app.is_empty() && l.contains(&app))
        .filter_map(|l| {
            let l = l.trim_start();
            let (pid, cmd) = l.split_once(' ')?;
            Some((pid.to_string(), cmd.to_string()))
        })
        .collect()
}

/// The internet sockets of `pids` (lsof's lines), and those that are not a loopback connection of a test: any
/// listener, any UDP socket, any connection to an address that is not this machine's loopback.
fn sockets_of(pids: &[String]) -> (Vec<String>, Vec<String>) {
    if pids.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let out = Command::new("/usr/sbin/lsof")
        .args(["-nP", "-a", "-p", &pids.join(","), "-i"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    let lines: Vec<String> = out.lines().skip(1).map(str::to_string).collect();
    let loopback = |a: &str| a.starts_with("127.") || a.starts_with("[::1]");
    let outside = lines
        .iter()
        .filter(|l| {
            let udp = l.split_whitespace().nth(7) == Some("UDP");
            let listen = l.contains("(LISTEN)");
            let remote_out = l
                .split_once("->")
                .is_some_and(|(_, r)| !loopback(r.split_whitespace().next().unwrap_or("")));
            udp || listen || remote_out
        })
        .cloned()
        .collect();
    (lines, outside)
}

/// Every URL in a Chromium network log (`--log-net-log`), once each.
fn net_log_urls(path: &Path) -> Result<Vec<String>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("no network log: {e}"))?;
    // A log the engine did not close has no final `]}`: what is there is still read, event by event.
    let v: Value = serde_json::from_str(&text)
        .or_else(|_| serde_json::from_str(&format!("{}]}}", text.trim_end().trim_end_matches(','))))
        .map_err(|e| format!("the network log does not parse: {e}"))?;
    let mut urls = std::collections::BTreeSet::new();
    for e in v["events"].as_array().into_iter().flatten() {
        for k in ["url", "original_url"] {
            if let Some(u) = e["params"][k].as_str() {
                urls.insert(u.to_string());
            }
        }
    }
    Ok(urls.into_iter().collect())
}

/// `quiet`: the engine with nothing but a sealed mini-app, started with switches on its own command line that CEF
/// must not take (`tests/cef_macos.rs` passes `--remote-debugging-port`, `--no-sandbox`, `--proxy-server`,
/// `--disable-web-security`). Nothing listens, no helper got them, and in 10 s no process of the bundle opens a
/// socket to anything but loopback, nor asks for any address but the app's own (Chromium's network log).
fn quiet(rep: &mut Report, data: &Path) -> Result<(), String> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let port: Option<u16> = argv
        .iter()
        .find_map(|a| a.strip_prefix("--remote-debugging-port="))
        .and_then(|p| p.parse().ok());
    let net_log = data.join("net-log.json");
    std::fs::create_dir_all(data).map_err(|e| e.to_string())?;
    let mut h = H::new(&data.join("profile"), Vec::new());
    h.host.set_net_log(net_log.clone());
    h.host.set_scale(2.0);
    let dir = data.join("quiet-app");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(
        dir.join("index.html"),
        "<!doctype html><title>Quiet</title>quiet",
    )
    .map_err(|e| e.to_string())?;
    let host = app_host(7);
    let from = h.mark();
    h.host.open_app(
        "q",
        AppSpec {
            url: app_url(&host),
            host: host.clone(),
            folder: dir,
            allow_network: false,
            module: None,
        },
        BOUNDS,
        true,
    );
    let opened = h.opened(from, "q", "Quiet");
    let mut all = std::collections::BTreeSet::new();
    let mut outside = std::collections::BTreeSet::new();
    let mut procs = Vec::new();
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(10) {
        procs = bundle_processes();
        let pids: Vec<String> = procs.iter().map(|(p, _)| p.clone()).collect();
        let (lines, out) = sockets_of(&pids);
        all.extend(lines);
        outside.extend(out);
        h.wait(400);
    }
    let devtools = port.map(|p| std::net::TcpStream::connect(("127.0.0.1", p)).is_ok());
    let port_file = walk_names(data)
        .into_iter()
        .filter(|n| n.contains("DevToolsActivePort"))
        .collect::<Vec<_>>();
    let leaked: Vec<String> = procs
        .iter()
        .map(|(_, c)| c.clone())
        .filter(|c| {
            [
                "remote-debugging-port",
                "--no-sandbox",
                "proxy-server",
                "disable-web-security",
            ]
            .iter()
            .any(|s| c.contains(s))
                && c.contains("--type=")
        })
        .collect();
    let helpers = procs.iter().filter(|(_, c)| c.contains("--type=")).count();
    rep.check(
        "quiet.own-switches-only",
        opened
            && port.is_some()
            && devtools == Some(false)
            && port_file.is_empty()
            && leaked.is_empty()
            && helpers > 0,
        "a switch on Flare's command line is not CEF's: --remote-debugging-port opens no port and leaves no DevToolsActivePort, and no helper is started with it, --no-sandbox, --proxy-server or --disable-web-security",
        json!({"argv": argv, "devtools_port_answered": devtools, "port_files": port_file, "helpers": helpers,
               "helpers_with_switches": leaked}),
    );
    let features = procs
        .iter()
        .find(|(_, c)| c.contains("--type=renderer"))
        .and_then(|(_, c)| {
            c.split_whitespace()
                .find(|a| a.starts_with("--disable-features="))
        })
        .map(str::to_string);
    h.host.close_all();
    let urls = net_log_urls(&net_log);
    let own = format!("https://{host}/");
    let asked: Vec<String> = urls
        .as_ref()
        .map(|u| {
            u.iter()
                // The component updater's checks go to the dead address (rule 263), refused on loopback.
                .filter(|u| {
                    !u.starts_with(&own)
                        && !u.starts_with("about:")
                        && !u.starts_with("data:")
                        && !u.starts_with("http://127.0.0.1:9/")
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    rep.check(
        "quiet.no-outside-calls",
        opened && outside.is_empty() && urls.as_ref().is_ok_and(|u| !u.is_empty()) && asked.is_empty(),
        "with only a sealed mini-app open, no process of the engine listens, holds a UDP socket or connects past loopback in 10 s, and the network log has no address but the app's own (no component updater, network time, AI mode or other Google service)",
        json!({"outside_sockets": outside, "sockets": all, "urls_other_than_the_apps": asked,
               "urls": format!("{urls:?}"), "renderer_disable_features": features}),
    );
    Ok(())
}

/// Every path under a folder, as text.
fn walk_names(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() && !p.is_symlink() {
                stack.push(p.clone());
            }
            out.push(p.to_string_lossy().into_owned());
        }
    }
    out
}

/// The probe's `main`: the exit code.
pub fn main() -> i32 {
    let scenario = std::env::var("FLARE_WEB_PROBE_SCENARIO").unwrap_or_default();
    let out = std::env::var_os("FLARE_WEB_PROBE_OUT").map(PathBuf::from);
    let mut rep = Report {
        v: json!({"scenario": scenario, "checks": []}),
        out,
    };
    rep.set("windows_at_start", json!(screen::windows_of_self()));
    let write = |rep: &Report| rep.write();
    let Some(mtm) = MainThreadMarker::new() else {
        rep.set("fatal", json!("not on the main thread"));
        write(&rep);
        return 2;
    };
    // The headless application first, as the real app makes its own before winit: the host adopts it.
    let nsapp = match app::install(mtm) {
        Ok(a) => a,
        Err(e) => {
            rep.set("fatal", json!(e));
            write(&rep);
            return 2;
        }
    };
    let data = std::env::temp_dir().join(format!("flare-web-probe-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data);
    let t = Instant::now();
    let r = match scenario.as_str() {
        "panel" => panel(&mut rep, &data),
        "walls" => walls(&mut rep, &data),
        "agent" => agent::run(&mut rep, &data),
        "quiet" => quiet(&mut rep, &data),
        other => Err(format!("no scenario {other:?}")),
    };
    if let Err(e) = r {
        rep.set("fatal", json!(e));
    }
    rep.set("scenario_ms", json!(t.elapsed().as_millis() as u64));
    rep.set("cef_app_protocol", json!(app::conforms(&nsapp)));
    rep.set(
        "cef_app_protocol_parts",
        json!(
            app::conformance(&nsapp)
                .into_iter()
                .collect::<BTreeMap<_, _>>()
        ),
    );
    rep.set(
        "activation_policy",
        json!(format!("{:?}", nsapp.activationPolicy())),
    );
    rep.set("active_at_end", json!(nsapp.isActive()));
    rep.set("windows_at_end", json!(screen::windows_of_self()));
    rep.set("shutdown", json!(true));
    write(&rep);
    let _ = std::fs::remove_dir_all(&data);
    0
}

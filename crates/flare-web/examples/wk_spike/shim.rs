//! The DevTools Protocol shim: the 11 CDP methods `flare_surfaces::agent_job` asks for, answered by
//! WKWebView. The real `Job` state machine (the same code Windows runs) drives it, so the answers have
//! to be in Chromium's shapes. Node numbers are `frame * SPAN + node`, frame 0 the main one.

use std::collections::VecDeque;
use std::time::Instant;

use flare_surfaces::agent_job::{Call, Ids, Job, JobError, Op, Out, Page, RefMap};
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSApplication, NSEventModifierFlags, NSEventType};
use objc2_web_kit::{WKContentWorld, WKFrameInfo, WKWebView};
use serde_json::{Value, json};

use crate::mac;

pub const SPAN: i64 = 1_000_000;

struct Frame {
    info: Option<Retained<WKFrameInfo>>,
    parent: usize,
    iframe: i64,
    origin_label: String,
}

pub struct Shim<'a> {
    app: &'a NSApplication,
    pub wv: &'a WKWebView,
    world: Retained<WKContentWorld>,
    page: Retained<WKContentWorld>,
    frames: Vec<Frame>,
    pub methods: Vec<String>,
    pub notes: Vec<String>,
    pub last_shot: Option<(Vec<u8>, usize, usize)>,
}

impl<'a> Shim<'a> {
    pub fn new(app: &'a NSApplication, wv: &'a WKWebView) -> Self {
        let mtm = MainThreadMarker::from(app);
        Shim {
            app,
            wv,
            world: mac::agent_world(mtm),
            page: unsafe { WKContentWorld::pageWorld(mtm) },
            frames: vec![Frame {
                info: None,
                parent: 0,
                iframe: 0,
                origin_label: "main".into(),
            }],
            methods: Vec::new(),
            notes: Vec::new(),
            last_shot: None,
        }
    }

    fn agent(&self, f: usize, js: &str) -> Result<Value, String> {
        let fr = self.frames.get(f).ok_or("no such frame")?;
        mac::eval(
            self.app,
            self.wv,
            fr.info.as_deref(),
            &self.world,
            js,
            4_000,
        )
    }

    /// A helper that answers JSON text, parsed.
    fn agent_json(&self, f: usize, js: &str) -> Result<Value, String> {
        match self.agent(f, js)? {
            Value::String(s) => serde_json::from_str(&s).map_err(|e| e.to_string()),
            v => Ok(v),
        }
    }

    /// The WKFrameInfo of the frame whose origin label is `label` (from the last look).
    pub fn frame_info(&self, label: &str) -> Option<Retained<WKFrameInfo>> {
        self.frames
            .iter()
            .find(|f| f.origin_label == label)
            .and_then(|f| f.info.clone())
    }

    pub fn frame_labels(&self) -> Vec<String> {
        self.frames.iter().map(|f| f.origin_label.clone()).collect()
    }

    pub fn call(&mut self, method: &str, p: &Value) -> Result<String, String> {
        self.methods.push(method.to_string());
        let v = match method {
            "DOM.enable" => Ok(json!({})),
            "Page.getLayoutMetrics" => self.agent_json(0, "__flareAgent.metrics()"),
            "Accessibility.getFullAXTree" => self.ax_tree(),
            "Runtime.evaluate" => self.evaluate(p),
            "Page.captureScreenshot" => self.screenshot(p),
            "DOM.getContentQuads" => self.quads(p["backendNodeId"].as_i64().unwrap_or(-1)),
            "DOM.scrollIntoViewIfNeeded" => {
                self.scroll_into(p["backendNodeId"].as_i64().unwrap_or(-1))
            }
            "DOM.focus" => self.focus(p["backendNodeId"].as_i64().unwrap_or(-1)),
            "Input.dispatchMouseEvent" => self.mouse(p),
            "Input.dispatchKeyEvent" => self.key(p),
            "Input.insertText" => {
                let before = self.seen();
                mac::insert_text(self.wv, p["text"].as_str().unwrap_or(""));
                self.settle(before, 500);
                Ok(json!({}))
            }
            other => Err(format!("{other} is not in the shim")),
        }?;
        Ok(v.to_string())
    }

    /// Trusted input events seen so far, over every frame the last look found.
    fn seen(&self) -> i64 {
        (0..self.frames.len())
            .map(|f| {
                self.agent(f, "__flareAgent.seen()")
                    .ok()
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0)
            })
            .sum()
    }

    /// Waits (at most `ms`) until the page has handled what was just sent. WebKit queues input for the
    /// content process and answers nothing; CDP answers once the renderer has handled the event.
    fn settle(&self, before: i64, ms: u64) -> bool {
        let app = self.app;
        mac::wait_until(app, ms, || self.seen() > before)
    }

    /// The last keydown any frame saw (and whether its page cancelled it); each frame's record is cleared.
    fn last_key(&self) -> Value {
        let mut found = Value::Null;
        for f in 0..self.frames.len() {
            let k = self
                .agent_json(f, "__flareAgent.lastKey()")
                .unwrap_or(Value::Null);
            if found.is_null() && !k.is_null() {
                found = k;
            }
        }
        found
    }

    fn split(&self, b: i64) -> Result<(usize, i64), String> {
        if b < 0 {
            return Err("No node with given id found".into());
        }
        let f = usize::try_from(b / SPAN).map_err(|e| e.to_string())?;
        if f >= self.frames.len() {
            return Err("No node with given id found".into());
        }
        Ok((f, b % SPAN))
    }

    /// Where frame `f`'s (0, 0) is in the main frame's viewport, asked afresh (a parent may have scrolled).
    fn origin(&self, f: usize) -> Result<(f64, f64), String> {
        if f == 0 {
            return Ok((0.0, 0.0));
        }
        let fr = &self.frames[f];
        let (px, py) = self.origin(fr.parent)?;
        let o = self.agent_json(
            fr.parent,
            &format!("__flareAgent.frameOrigin({})", fr.iframe),
        )?;
        let x = o[0].as_f64().ok_or("frame gone")?;
        let y = o[1].as_f64().ok_or("frame gone")?;
        Ok((px + x, py + y))
    }

    fn ax_tree(&mut self) -> Result<Value, String> {
        self.frames.truncate(1);
        let nonce = format!(
            "n{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let mut all: Vec<Value> = Vec::new();
        let mut used: Vec<String> = Vec::new();
        // (frame, the iframe node's id in its parent's numbering)
        let mut queue: VecDeque<(usize, Option<String>)> = VecDeque::from([(0, None)]);
        while let Some((f, attach)) = queue.pop_front() {
            let snap = match self.agent_json(f, "__flareAgent.snapshot()") {
                Ok(s) => s,
                Err(e) if f == 0 => return Err(e),
                Err(e) => {
                    self.notes.push(format!("frame {f} could not be read: {e}"));
                    continue;
                }
            };
            let off = i64::try_from(f).unwrap_or(0) * SPAN;
            let nodes = snap["nodes"].as_array().cloned().unwrap_or_default();
            let root_id = nodes
                .first()
                .and_then(|n| n["nodeId"].as_str())
                .map(|id| format!("{f}:{id}"));
            for mut n in nodes {
                let id = n["nodeId"].as_str().unwrap_or("").to_string();
                n["nodeId"] = json!(format!("{f}:{id}"));
                let kids: Vec<Value> = n["childIds"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|k| k.as_str().map(|k| json!(format!("{f}:{k}"))))
                    .collect();
                n["childIds"] = Value::Array(kids);
                if let Some(b) = n["backendDOMNodeId"].as_i64() {
                    n["backendDOMNodeId"] = json!(off + b);
                }
                all.push(n);
            }
            if let (Some(parent_node), Some(root)) = (attach, root_id)
                && let Some(p) = all.iter_mut().find(|n| n["nodeId"] == json!(parent_node))
                && let Some(k) = p["childIds"].as_array_mut()
            {
                k.push(json!(root));
            }
            for fr in snap["frames"].as_array().cloned().unwrap_or_default() {
                let Some(local) = fr["id"].as_i64() else {
                    continue;
                };
                let key = format!("{f}:{local}");
                let before = mac::st(|s| s.probes.len());
                let _ = self.agent(
                    f,
                    &format!("__flareAgent.probe({local}, '{nonce}', '{key}')"),
                );
                let app = self.app;
                let found = mac::wait_until(app, 1_500, || {
                    mac::st(|s| {
                        s.probes[before..]
                            .iter()
                            .any(|(n, k, _)| *n == nonce && *k == key)
                    })
                });
                if !found {
                    // A frame the helper's user script did not run in (a sandbox without allow-scripts:
                    // WebKit runs no user scripts and no listeners there) still takes the agent's world
                    // through evaluateJavaScript with the WKFrameInfo the navigation delegate saw. The
                    // probe cannot reach it, so it is paired with this iframe by viewport size, in
                    // document order.
                    if let Some(info) = self.unscripted_for(f, local, &nonce, &mut used) {
                        let o = unsafe { info.securityOrigin() };
                        let label = format!(
                            "{}://{}:{} (unscripted)",
                            unsafe { o.protocol() },
                            unsafe { o.host() },
                            unsafe { o.port() }
                        );
                        self.frames.push(Frame {
                            info: Some(info),
                            parent: f,
                            iframe: local,
                            origin_label: label,
                        });
                        queue.push_back((self.frames.len() - 1, Some(key)));
                        continue;
                    }
                }
                if !found {
                    self.notes
                        .push(format!("iframe {key} did not answer the probe"));
                    continue;
                }
                let info = mac::st(|s| {
                    s.probes
                        .iter()
                        .rev()
                        .find(|(n, k, _)| *n == nonce && *k == key)
                        .map(|(_, _, i)| i.clone())
                });
                let label = info
                    .as_ref()
                    .map(|i| {
                        let o = unsafe { i.securityOrigin() };
                        format!(
                            "{}://{}:{}",
                            unsafe { o.protocol() },
                            unsafe { o.host() },
                            unsafe { o.port() }
                        )
                    })
                    .unwrap_or_default();
                self.frames.push(Frame {
                    info,
                    parent: f,
                    iframe: local,
                    origin_label: label,
                });
                queue.push_back((self.frames.len() - 1, Some(key)));
            }
        }
        Ok(json!({ "nodes": all }))
    }

    /// The unscripted frame (see `ax_tree`) that fits iframe `local` of frame `f`: the helper is put in
    /// where it is missing (tagged, so a frame the delegate saw twice is one frame), then the first frame
    /// that did not take this look's probe, is not yet used in this look and has the iframe's size.
    fn unscripted_for(
        &self,
        f: usize,
        local: i64,
        nonce: &str,
        used: &mut Vec<String>,
    ) -> Option<Retained<WKFrameInfo>> {
        let size = self
            .agent_json(f, &format!("__flareAgent.frameSize({local})"))
            .ok()?;
        let me = self.wv as *const WKWebView;
        let infos: Vec<Retained<WKFrameInfo>> = mac::st(|s| {
            s.sub_frames
                .iter()
                .filter(|(_, i)| {
                    unsafe { i.webView() }.is_some_and(|w| std::ptr::eq(Retained::as_ptr(&w), me))
                })
                .map(|(_, i)| i.clone())
                .collect()
        });
        for i in infos {
            let ev = |js: &str| mac::eval(self.app, self.wv, Some(&i), &self.world, js, 1_000);
            // The helper is put in where it is missing; a frame that took this look's probe is a
            // scripted one, already paired.
            if ev("typeof __flareAgent") == Ok(Value::String("undefined".into()))
                && ev(crate::tests::AGENT_JS).is_err()
            {
                continue;
            }
            if ev("__flareAgent.probed()") == Ok(Value::String(nonce.into())) {
                continue;
            }
            let tag = match ev(
                "window.__flareTag || (window.__flareTag = 'u' + Math.random().toString(36).slice(2))",
            ) {
                Ok(Value::String(t)) => t,
                _ => continue,
            };
            if used.contains(&tag) {
                continue;
            }
            let inner = ev("JSON.stringify([innerWidth, innerHeight])")
                .ok()
                .and_then(|v| {
                    v.as_str()
                        .and_then(|s| serde_json::from_str::<Value>(s).ok())
                });
            if inner.is_some_and(|v| v == size) {
                used.push(tag);
                return Some(i);
            }
        }
        None
    }

    fn quads(&self, b: i64) -> Result<Value, String> {
        let (f, local) = self.split(b)?;
        let q = self.agent_json(f, &format!("__flareAgent.quads({local})"))?;
        let Some(list) = q.as_array() else {
            return Err("Could not compute content quads.".into());
        };
        if list.is_empty() {
            return Err("Could not compute content quads.".into());
        }
        let (ox, oy) = self.origin(f)?;
        let out: Vec<Value> = list
            .iter()
            .map(|quad| {
                let n: Vec<f64> = quad
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_f64)
                    .collect();
                json!(
                    n.iter()
                        .enumerate()
                        .map(|(i, v)| if i % 2 == 0 { v + ox } else { v + oy })
                        .collect::<Vec<f64>>()
                )
            })
            .collect();
        Ok(json!({ "quads": out }))
    }

    fn scroll_into(&self, b: i64) -> Result<Value, String> {
        let (f, local) = self.split(b)?;
        // Outer frames first: the iframe into view, then the element inside it.
        let mut chain = vec![];
        let mut c = f;
        while c != 0 {
            chain.push((self.frames[c].parent, self.frames[c].iframe));
            c = self.frames[c].parent;
        }
        for (pf, el) in chain.into_iter().rev() {
            let _ = self.agent(pf, &format!("__flareAgent.scrollInto({el})"));
        }
        match self.agent(f, &format!("__flareAgent.scrollInto({local})"))? {
            Value::String(s) if s == "ok" => Ok(json!({})),
            v => Err(format!("Node is detached from document ({v})")),
        }
    }

    fn focus(&self, b: i64) -> Result<Value, String> {
        let (f, local) = self.split(b)?;
        match self.agent(f, &format!("__flareAgent.focus({local})"))? {
            Value::String(s) if s == "ok" => Ok(json!({})),
            v => Err(v.as_str().unwrap_or("Element is not focusable").to_string()),
        }
    }

    /// `Runtime.evaluate`, main frame; the value comes back through WebKit's own conversion
    /// (returnByValue). No string is ever `eval`ed, so a page's CSP without 'unsafe-eval' does not apply.
    ///
    /// - A fixed question of `agent_job` (the look's `devicePixelRatio`, a wait's check: the calls without
    ///   `awaitPromise`) runs in the agent's own content world, where the page's getters cannot trap it.
    /// - `browser_eval` (`awaitPromise`) runs in the page's world, as on Windows. An expression goes through
    ///   `callAsyncJavaScript` as `return await (expr)`; a statement script (it does not parse as an
    ///   expression, checked in the agent's world without running it) is split there into its statements
    ///   and its last expression: the statements run as a script through `evaluateJavaScript` (globals as
    ///   CDP makes them), then the expression is awaited through `callAsyncJavaScript`. A script that
    ///   does not end in an expression runs whole through `evaluateJavaScript` (no promise to await).
    fn evaluate(&self, p: &Value) -> Result<Value, String> {
        let expr = p["expression"].as_str().unwrap_or("");
        let ms = p["timeout"].as_u64().unwrap_or(10_000);
        let answer = if !p["awaitPromise"].as_bool().unwrap_or(false) {
            mac::eval(self.app, self.wv, None, &self.world, expr, ms)
        } else {
            let src = serde_json::to_string(expr).unwrap_or_default();
            let kind = self.agent(0, &format!("__flareAgent.parsesAsExpression({src})"));
            let split = || {
                self.agent_json(0, &format!("__flareAgent.splitLast({src})"))
                    .ok()
                    .filter(|v| v.is_object())
            };
            if kind == Ok(Value::Bool(true)) {
                let body = format!("return await (\n{expr}\n);");
                mac::call_async(self.app, self.wv, None, &self.page, &body, ms)
            } else if let Some(parts) = split() {
                // The statements as a script, then the last expression awaited (both CSP-safe).
                let head = parts["head"].as_str().unwrap_or("");
                let tail = parts["tail"].as_str().unwrap_or("");
                match mac::eval(
                    self.app,
                    self.wv,
                    None,
                    &self.page,
                    &format!("{head}\n;void 0"),
                    ms,
                ) {
                    Ok(_) => {
                        let body = format!("return await (\n{tail}\n);");
                        mac::call_async(self.app, self.wv, None, &self.page, &body, ms)
                    }
                    Err(e) => Err(e),
                }
            } else {
                mac::eval(self.app, self.wv, None, &self.page, expr, ms)
            }
        };
        match answer {
            Ok(v) => Ok(json!({"result": {"type": type_of(&v), "value": v}})),
            Err(e) if e.starts_with("no answer") => Err(e),
            Err(e) => Ok(json!({
                "result": {"type": "object", "subtype": "error"},
                "exceptionDetails": {"text": "Uncaught", "exception": {"description": e}}
            })),
        }
    }

    fn screenshot(&mut self, p: &Value) -> Result<Value, String> {
        let clip = &p["clip"];
        let m = self.agent_json(0, "__flareAgent.metrics()")?;
        let px = m["cssVisualViewport"]["pageX"].as_f64().unwrap_or(0.0);
        let py = m["cssVisualViewport"]["pageY"].as_f64().unwrap_or(0.0);
        let zoom = unsafe { self.wv.pageZoom() };
        let x = (clip["x"].as_f64().unwrap_or(px) - px) * zoom;
        let y = (clip["y"].as_f64().unwrap_or(py) - py) * zoom;
        let w = clip["width"].as_f64().unwrap_or(0.0) * zoom;
        let h = clip["height"].as_f64().unwrap_or(0.0) * zoom;
        let scale = clip["scale"].as_f64().unwrap_or(1.0);
        let r = mac::rect(x, y, w, h);
        let (png, pw, ph) = mac::snapshot_png(self.app, self.wv, Some(r), Some(w * scale))?;
        self.notes.push(format!(
            "screenshot {pw}x{ph} px for {w}x{h} pt at scale {scale}"
        ));
        let data = b64(&png);
        self.last_shot = Some((png, pw, ph));
        Ok(json!({ "data": data }))
    }

    fn mouse(&mut self, p: &Value) -> Result<Value, String> {
        let x = p["x"].as_f64().unwrap_or(0.0);
        let y = p["y"].as_f64().unwrap_or(0.0);
        let flags = mods(p["modifiers"].as_u64().unwrap_or(0));
        let button = p["button"].as_str().unwrap_or("none");
        let clicks = p["clickCount"].as_i64().unwrap_or(1) as isize;
        let kind = match (p["type"].as_str().unwrap_or(""), button) {
            ("mouseMoved", _) => NSEventType::MouseMoved,
            ("mousePressed", "left") => NSEventType::LeftMouseDown,
            ("mouseReleased", "left") => NSEventType::LeftMouseUp,
            ("mousePressed", "middle") => NSEventType::OtherMouseDown,
            ("mouseReleased", "middle") => NSEventType::OtherMouseUp,
            ("mousePressed" | "mouseReleased", "right") => {
                // A right click opens WebKit's context menu, an on-screen NSMenu: not in a headless run.
                return Err("right click not delivered in the headless spike".into());
            }
            ("mouseWheel", _) => {
                let ev = mac::wheel_event(
                    self.wv,
                    x,
                    y,
                    p["deltaX"].as_f64().unwrap_or(0.0),
                    p["deltaY"].as_f64().unwrap_or(0.0),
                );
                let before = self.seen();
                self.wv.scrollWheel(&ev);
                self.settle(before, 800);
                return Ok(json!({}));
            }
            (t, b) => return Err(format!("mouse {t} {b} not mapped")),
        };
        let ev = mac::mouse_event(self.wv, kind, x, y, clicks, flags);
        let before = self.seen();
        mac::deliver_mouse(self.wv, &ev);
        self.settle(
            before,
            if kind == NSEventType::MouseMoved {
                150
            } else {
                800
            },
        );
        Ok(json!({}))
    }

    fn key(&mut self, p: &Value) -> Result<Value, String> {
        let t = p["type"].as_str().unwrap_or("");
        let vk = p["windowsVirtualKeyCode"].as_u64().unwrap_or(0) as u32;
        let m = p["modifiers"].as_u64().unwrap_or(0);
        let key = p["key"].as_str().unwrap_or("");
        let (code, special) = mac_key(vk);
        let mut flags = mods(m);
        if special.is_some() && !matches!(vk, 8 | 9 | 13 | 27 | 32) {
            flags |= NSEventModifierFlags::Function;
        }
        let unmod: String = match special {
            Some(c) => c.to_string(),
            None => key.to_lowercase(),
        };
        let chars: String = if let Some(t) = p["text"].as_str() {
            t.to_string()
        } else if m & 2 != 0 && unmod.len() == 1 && unmod.as_bytes()[0].is_ascii_lowercase() {
            // Control+letter types the control character, as AppKit's own events carry it.
            char::from(unmod.as_bytes()[0] - b'a' + 1).to_string()
        } else {
            unmod.clone()
        };
        let down = t != "keyUp";
        let ev = mac::key_event(self.wv, down, &chars, &unmod, code, flags);
        let before = self.seen();
        mac::deliver_key(self.wv, &ev);
        self.settle(before, 800);
        // AppKit runs Command chords through the menu bar, not the page: a chord the page did not cancel
        // becomes the responder action itself (what Chromium's `commands` field does on a Mac).
        if down && m & 4 != 0 {
            let last = self.last_key();
            if last["prevented"] != true {
                let wv = self.wv;
                let done = match unmod.as_str() {
                    "a" => {
                        let _: () = unsafe {
                            objc2::msg_send![wv, selectAll: std::ptr::null::<objc2::runtime::AnyObject>()]
                        };
                        "selectAll:"
                    }
                    "c" => {
                        let _: () = unsafe {
                            objc2::msg_send![wv, copy: std::ptr::null::<objc2::runtime::AnyObject>()]
                        };
                        "copy:"
                    }
                    "x" => {
                        let _: () = unsafe {
                            objc2::msg_send![wv, cut: std::ptr::null::<objc2::runtime::AnyObject>()]
                        };
                        "cut:"
                    }
                    "v" => {
                        let _: () = unsafe {
                            objc2::msg_send![wv, paste: std::ptr::null::<objc2::runtime::AnyObject>()]
                        };
                        "paste:"
                    }
                    "z" => {
                        let _: () = unsafe {
                            objc2::msg_send![wv, undo: std::ptr::null::<objc2::runtime::AnyObject>()]
                        };
                        "undo:"
                    }
                    _ => "",
                };
                if !done.is_empty() {
                    self.notes.push(format!("Command+{unmod} performed {done}"));
                    mac::spin(self.app, 50);
                }
            }
        }
        Ok(json!({}))
    }
}

fn type_of(v: &Value) -> &'static str {
    match v {
        Value::Null => "undefined",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        _ => "object",
    }
}

fn mods(m: u64) -> NSEventModifierFlags {
    let mut f = NSEventModifierFlags::empty();
    if m & 1 != 0 {
        f |= NSEventModifierFlags::Option;
    }
    if m & 2 != 0 {
        f |= NSEventModifierFlags::Control;
    }
    if m & 4 != 0 {
        f |= NSEventModifierFlags::Command;
    }
    if m & 8 != 0 {
        f |= NSEventModifierFlags::Shift;
    }
    f
}

/// A Windows virtual-key code (what `agent_web::key_events` sends) to a Mac key code and, for keys that
/// type no character, the character AppKit puts in the event.
pub fn mac_key(vk: u32) -> (u16, Option<char>) {
    const LETTERS: [u16; 26] = [
        0, 11, 8, 2, 14, 3, 5, 4, 34, 38, 40, 37, 46, 45, 31, 35, 12, 15, 1, 17, 32, 9, 13, 7, 16,
        6,
    ];
    const DIGITS: [u16; 10] = [29, 18, 19, 20, 21, 23, 22, 26, 28, 25];
    const FKEYS: [u16; 12] = [122, 120, 99, 118, 96, 97, 98, 100, 101, 109, 103, 111];
    match vk {
        65..=90 => (LETTERS[(vk - 65) as usize], None),
        48..=57 => (DIGITS[(vk - 48) as usize], None),
        112..=123 => (
            FKEYS[(vk - 112) as usize],
            char::from_u32(0xF704 + vk - 112),
        ),
        8 => (51, Some('\u{7f}')),
        9 => (48, Some('\t')),
        13 => (36, Some('\r')),
        27 => (53, Some('\u{1b}')),
        32 => (49, Some(' ')),
        33 => (116, Some('\u{f72c}')),
        34 => (121, Some('\u{f72d}')),
        35 => (119, Some('\u{f72b}')),
        36 => (115, Some('\u{f729}')),
        37 => (123, Some('\u{f702}')),
        38 => (126, Some('\u{f700}')),
        39 => (124, Some('\u{f703}')),
        40 => (125, Some('\u{f701}')),
        45 => (114, Some('\u{f727}')),
        46 => (117, Some('\u{f728}')),
        186 => (41, None),
        187 => (24, None),
        188 => (43, None),
        189 => (27, None),
        190 => (47, None),
        191 => (44, None),
        192 => (50, None),
        219 => (33, None),
        220 => (42, None),
        221 => (30, None),
        222 => (39, None),
        _ => (0xFFFF, None),
    }
}

pub fn b64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// Runs one agent call through the real `Job` and the shim, as the window would: calls go out in order,
/// answers come back, the clock ticks, the cursor's glide is zero.
pub fn run_job(
    shim: &mut Shim<'_>,
    op: Op,
    refs: &RefMap,
) -> (Result<Value, JobError>, Option<RefMap>) {
    let t0 = Instant::now();
    let now = |t0: Instant| u64::try_from(t0.elapsed().as_millis()).unwrap_or(u64::MAX);
    let mut counter = 1u64;
    let (job, out) = Job::start(op, refs, now(t0), &mut Ids(&mut counter));
    let mut pending: VecDeque<Call> = VecDeque::new();
    let mut job = match (job, out) {
        (_, Out::Done(r)) => return (r, None),
        (Some(j), Out::Calls(c)) => {
            pending.extend(c);
            j
        }
        (Some(j), Out::Idle) => j,
        (None, _) => return (Err(Job::interrupted()), None),
    };
    job.set_glide_ms(0);
    loop {
        if let Some(p) = job.take_point() {
            shim.notes.push(format!("pointer lands at {p:?}"));
            job.glide_for(0, now(t0));
        }
        if let Some(c) = pending.pop_front() {
            let reply = shim.call(&c.method, &c.params);
            let url = mac::url_of(shim.wv);
            let title = mac::title_of(shim.wv);
            let out = job.on_reply(
                c.id,
                reply,
                Page {
                    url: &url,
                    title: &title,
                },
                now(t0),
                &mut Ids(&mut counter),
            );
            match out {
                Out::Calls(c) => pending.extend(c),
                Out::Done(r) => return (r, job.take_refs()),
                Out::Idle => {}
            }
            continue;
        }
        mac::spin(shim_app(shim), 10);
        match job.on_tick(now(t0), &mut Ids(&mut counter)) {
            Out::Calls(c) => pending.extend(c),
            Out::Done(r) => return (r, job.take_refs()),
            Out::Idle => {}
        }
    }
}

fn shim_app<'a>(s: &Shim<'a>) -> &'a NSApplication {
    s.app
}

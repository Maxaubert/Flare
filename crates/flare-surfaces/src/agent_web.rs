//! What an agent sees and does in the web panel (Surfaces stage 4), as pure functions: the page
//! snapshot trimmed out of the browser's accessibility tree, element rectangles and click points, the
//! screenshot's scale, the keys, and the fixed checker of `browser.wait`. No COM, no Slint, no clock.
//!
//! Everything a page says is untrusted text: it is cleaned here (control and invisible characters
//! gone, whitespace collapsed, every field capped) before it can reach an agent, and the agent's
//! tool layer cleans it once more. Nothing in this file builds a script from page or agent text
//! except [`wait_expression`], which only ever puts one JSON string literal into a fixed template.

use serde_json::{Value, json};

/// The most elements one look reports.
pub const MAX_ELEMENTS: usize = 600;
/// The longest name or value of one element, in characters.
pub const TEXT_MAX: usize = 160;
/// The longest link address kept, in characters.
pub const HREF_MAX: usize = 200;
/// The widest screenshot sent to an agent, in pixels.
pub const SHOT_MAX_W: u32 = 1600;
/// The most text one `browser.type` inserts.
pub const TYPE_MAX: usize = 20_000;
/// The longest text or selector a wait looks for.
pub const WAIT_TEXT_MAX: usize = 500;
/// The longest a wait lasts (the caller's `timeout_ms` is cut to it).
pub const WAIT_MAX_MS: u64 = 30_000;
/// A wait with no `timeout_ms`.
pub const WAIT_DEFAULT_MS: u64 = 10_000;
/// The longest script `browser.eval` takes, in characters.
pub const EVAL_MAX: usize = 20_000;
/// The longest answer of `browser.eval`, in characters of its JSON.
pub const EVAL_RESULT_MAX: usize = 100_000;
/// The largest screenshot kept (decoded bytes); a bigger one is left out of the answer.
pub const SHOT_BYTES_MAX: usize = 8 * 1024 * 1024;

// ---- cleaning ---------------------------------------------------------------------------------

/// Page text made safe to hand on: one line, no control or invisible characters (the tag characters
/// and variation selectors among them), runs of white space collapsed, at most `max` characters (a cut
/// ends in `...`). The one copy lives in `flare_ctl::text`, shared with the agent tools.
pub fn clean_text(s: &str, max: usize) -> String {
    flare_ctl::text::clean_text(s, max)
}

fn invisible(c: char) -> bool {
    flare_ctl::text::invisible(c)
}

// ---- the snapshot -----------------------------------------------------------------------------

/// One thing worth showing an agent, before its place on the screen is known.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    /// The browser's number for the node; a ref maps to it.
    pub backend_id: i64,
    pub role: String,
    pub name: String,
    pub value: String,
    pub states: Vec<String>,
    /// A control a person can act on (as opposed to text).
    pub interactive: bool,
    /// A heading's level.
    pub level: Option<u32>,
    /// A link's address.
    pub href: Option<String>,
}

const INTERACTIVE: [&str; 24] = [
    "button",
    "link",
    "textbox",
    "searchbox",
    "combobox",
    "checkbox",
    "radio",
    "switch",
    "menuitem",
    "menuitemcheckbox",
    "menuitemradio",
    "tab",
    "option",
    "slider",
    "spinbutton",
    "listbox",
    "treeitem",
    "popupbutton",
    "comboboxselect",
    "comboboxmenubutton",
    "disclosuretriangle",
    "colorwell",
    "textfield",
    "togglebutton",
];

/// Roles kept for what they say, with a name: they rank with text when the list must be cut.
const NAMED: [&str; 8] = [
    "heading",
    "image",
    "img",
    "alert",
    "status",
    "dialog",
    "alertdialog",
    "statictext",
];

fn prop<'a>(node: &'a Value, name: &str) -> Option<&'a Value> {
    node.get("properties")?
        .as_array()?
        .iter()
        .find(|p| p.get("name").and_then(Value::as_str) == Some(name))
        .and_then(|p| p.get("value"))
        .and_then(|v| v.get("value"))
}

fn prop_true(node: &Value, name: &str) -> bool {
    match prop(node, name) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s == "true",
        _ => false,
    }
}

fn scalar_text(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}

fn states_of(node: &Value, role: &str) -> Vec<String> {
    let mut v = Vec::new();
    for s in ["disabled", "required", "readonly", "invalid", "modal"] {
        if prop_true(node, s) {
            v.push(s.to_string());
        }
    }
    if prop_true(node, "focused") {
        v.push("focused".to_string());
    }
    if prop_true(node, "selected") {
        v.push("selected".to_string());
    }
    let checkable = matches!(
        role,
        "checkbox" | "radio" | "switch" | "menuitemcheckbox" | "menuitemradio" | "togglebutton"
    );
    match prop(node, "checked") {
        Some(Value::Bool(true)) => v.push("checked".to_string()),
        Some(Value::String(s)) if s == "true" => v.push("checked".to_string()),
        Some(Value::String(s)) if s == "mixed" => v.push("mixed".to_string()),
        Some(_) if checkable => v.push("unchecked".to_string()),
        None if checkable => v.push("unchecked".to_string()),
        _ => {}
    }
    match prop(node, "pressed") {
        Some(Value::Bool(true)) => v.push("pressed".to_string()),
        Some(Value::String(s)) if s == "true" => v.push("pressed".to_string()),
        Some(Value::String(s)) if s == "mixed" => v.push("mixed".to_string()),
        _ => {}
    }
    match prop(node, "expanded") {
        Some(Value::Bool(true)) => v.push("expanded".to_string()),
        Some(Value::Bool(false)) => v.push("collapsed".to_string()),
        _ => {}
    }
    v
}

/// The trimmed list out of `Accessibility.getFullAXTree`'s answer: interactive controls and the text
/// worth reading, in document order, at most [`MAX_ELEMENTS`]. A page with more keeps every control
/// first and fills what is left with its text; the flag says it was cut. Text inside a named control
/// or heading (the words of a link) is not repeated.
pub fn candidates(ax: &Value) -> (Vec<Candidate>, bool) {
    let Some(nodes) = ax.get("nodes").and_then(Value::as_array) else {
        return (Vec::new(), false);
    };
    let index: std::collections::HashMap<&str, usize> = nodes
        .iter()
        .enumerate()
        .filter_map(|(i, n)| Some((n.get("nodeId")?.as_str()?, i)))
        .collect();
    // Roots: nodes that are nobody's child (normally just the first).
    let mut has_parent = vec![false; nodes.len()];
    for n in nodes {
        for c in n
            .get("childIds")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(&i) = c.as_str().and_then(|c| index.get(c)) {
                has_parent[i] = true;
            }
        }
    }
    let mut visited = vec![false; nodes.len()];
    let mut out: Vec<Candidate> = Vec::new();
    // Depth-first, document order. The flag: a named control or heading is above, so its words are
    // not repeated as text.
    // Seeds: the roots, then every node in order (reached only when a cycle or an orphan left it unseen).
    let seeds: Vec<usize> = (0..nodes.len())
        .filter(|i| !has_parent[*i])
        .chain(0..nodes.len())
        .collect();
    let mut stack: Vec<(usize, bool)> = seeds.into_iter().rev().map(|i| (i, false)).collect();
    // A cycle or a shared child in a hostile tree is visited once; a total bound stops a runaway.
    let mut budget = 200_000usize;
    while let Some((i, covered)) = stack.pop() {
        if visited[i] || budget == 0 {
            continue;
        }
        visited[i] = true;
        budget -= 1;
        let node = &nodes[i];
        let ignored = node
            .get("ignored")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let mut child_covered = covered;
        if !ignored && let Some(c) = candidate_of(node, covered) {
            child_covered =
                covered || ((c.interactive || c.role == "heading") && !c.name.is_empty());
            // The same words twice in a row are one element.
            let dup = !c.interactive
                && out
                    .last()
                    .is_some_and(|l| !l.interactive && l.role == c.role && l.name == c.name);
            if !dup {
                out.push(c);
            }
        }
        if let Some(kids) = node.get("childIds").and_then(Value::as_array) {
            for k in kids.iter().rev() {
                if let Some(&j) = k.as_str().and_then(|k| index.get(k)) {
                    stack.push((j, child_covered));
                }
            }
        }
    }
    cap(out)
}

fn candidate_of(node: &Value, covered: bool) -> Option<Candidate> {
    let backend_id = node.get("backendDOMNodeId")?.as_i64()?;
    let role_raw = node.get("role")?.get("value")?.as_str()?;
    let role = role_raw.to_ascii_lowercase();
    let interactive = INTERACTIVE.contains(&role.as_str());
    let named = NAMED.contains(&role.as_str());
    if !interactive && !named {
        return None;
    }
    let name = clean_text(
        &scalar_text(node.get("name").and_then(|n| n.get("value"))),
        TEXT_MAX,
    );
    if !interactive && name.is_empty() {
        return None;
    }
    if role == "statictext" && covered {
        return None;
    }
    let value = if interactive {
        clean_text(
            &scalar_text(node.get("value").and_then(|n| n.get("value"))),
            TEXT_MAX,
        )
    } else {
        String::new()
    };
    let level = prop(node, "level")
        .and_then(Value::as_u64)
        .and_then(|l| u32::try_from(l).ok());
    let href = (role == "link")
        .then(|| scalar_text(prop(node, "url")))
        .map(|u| clean_text(&u, HREF_MAX))
        .filter(|u| !u.is_empty());
    // A role is shown the way the page's author would say it: "statictext" is just text.
    let shown = match role.as_str() {
        "statictext" => "text".to_string(),
        "img" => "image".to_string(),
        "popupbutton" | "comboboxselect" | "comboboxmenubutton" => "combobox".to_string(),
        "textfield" => "textbox".to_string(),
        other => other.to_string(),
    };
    Some(Candidate {
        backend_id,
        states: states_of(node, &role),
        role: shown,
        name,
        value,
        interactive,
        level,
        href,
    })
}

fn cap(all: Vec<Candidate>) -> (Vec<Candidate>, bool) {
    if all.len() <= MAX_ELEMENTS {
        return (all, false);
    }
    let mut keep = vec![false; all.len()];
    let mut left = MAX_ELEMENTS;
    for (i, c) in all.iter().enumerate() {
        if c.interactive && left > 0 {
            keep[i] = true;
            left -= 1;
        }
    }
    for (i, c) in all.iter().enumerate() {
        if !c.interactive && left > 0 {
            keep[i] = true;
            left -= 1;
        }
    }
    let kept = all
        .into_iter()
        .zip(keep)
        .filter_map(|(c, k)| k.then_some(c))
        .collect();
    (kept, true)
}

/// What a person's arrow points at, read the way a look reads the page: the browser's node, its role and its name.
#[derive(Clone, Debug, PartialEq)]
pub struct Pointed {
    pub backend_id: i64,
    pub role: String,
    pub name: String,
}

/// What `Accessibility.getPartialAXTree` (with its relatives) says is at the node `backend` that
/// `DOM.getNodeForLocation` found under an arrow's head: the nearest of it and its ancestors that a look would list
/// (a control, or a heading, an image or text with words), so an arrow on a button's label names the button. With
/// none, the node itself with its role (and no name), or None when the tree does not have it.
pub fn pointed_at(ax: &Value, backend: i64) -> Option<Pointed> {
    let nodes = ax.get("nodes").and_then(Value::as_array)?;
    let by_id: std::collections::HashMap<&str, &Value> = nodes
        .iter()
        .filter_map(|n| Some((n.get("nodeId")?.as_str()?, n)))
        .collect();
    // A parent from `parentId`, or from whoever lists the node as a child.
    let mut parent: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    for n in nodes {
        let Some(id) = n.get("nodeId").and_then(Value::as_str) else {
            continue;
        };
        for c in n
            .get("childIds")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            parent.entry(c).or_insert(id);
        }
    }
    for n in nodes {
        if let (Some(id), Some(p)) = (
            n.get("nodeId").and_then(Value::as_str),
            n.get("parentId").and_then(Value::as_str),
        ) {
            parent.insert(id, p);
        }
    }
    let start = nodes
        .iter()
        .find(|n| n.get("backendDOMNodeId").and_then(Value::as_i64) == Some(backend))?;
    let mut at = start;
    // A bound: a hostile tree with a loop in it is walked a few dozen steps at most.
    for _ in 0..64 {
        let ignored = at.get("ignored").and_then(Value::as_bool).unwrap_or(false);
        if !ignored && let Some(c) = candidate_of(at, false) {
            return Some(Pointed {
                backend_id: c.backend_id,
                role: c.role,
                name: c.name,
            });
        }
        let Some(next) = at
            .get("nodeId")
            .and_then(Value::as_str)
            .and_then(|id| parent.get(id))
            .and_then(|p| by_id.get(p))
        else {
            break;
        };
        at = next;
    }
    let role = start
        .get("role")
        .and_then(|r| r.get("value"))
        .and_then(Value::as_str)
        .map(|r| clean_text(&r.to_ascii_lowercase(), 24))
        .filter(|r| !r.is_empty() && r != "none" && r != "generic" && r != "ignored")
        .unwrap_or_else(|| "element".to_string());
    Some(Pointed {
        backend_id: backend,
        role,
        name: String::new(),
    })
}

// ---- places on the page -----------------------------------------------------------------------

/// The page's visible area in CSS pixels, and where it sits in the document.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub w: f64,
    pub h: f64,
    pub page_x: f64,
    pub page_y: f64,
}

/// Reads `Page.getLayoutMetrics`: the visual viewport in CSS pixels (the older device-pixel fields
/// are used only when the CSS ones are missing).
pub fn viewport_of(metrics: &Value) -> Option<Viewport> {
    let v = metrics
        .get("cssVisualViewport")
        .or_else(|| metrics.get("cssLayoutViewport"))
        .or_else(|| metrics.get("visualViewport"))
        .or_else(|| metrics.get("layoutViewport"))?;
    let num = |a: &str, b: &str| {
        v.get(a)
            .and_then(Value::as_f64)
            .or_else(|| v.get(b).and_then(Value::as_f64))
    };
    let w = num("clientWidth", "width")?;
    let h = num("clientHeight", "height")?;
    if !(w >= 1.0 && h >= 1.0 && w.is_finite() && h.is_finite()) {
        return None;
    }
    Some(Viewport {
        w,
        h,
        page_x: num("pageX", "x").unwrap_or(0.0),
        page_y: num("pageY", "y").unwrap_or(0.0),
    })
}

/// Rectangles `[x, y, w, h]` and centres of `DOM.getContentQuads`'s answer.
fn quad_rects(quads: &Value) -> Vec<[f64; 4]> {
    quads
        .get("quads")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|q| {
            let q = q.as_array()?;
            if q.len() != 8 {
                return None;
            }
            let n: Vec<f64> = q.iter().filter_map(Value::as_f64).collect();
            if n.len() != 8 || n.iter().any(|v| !v.is_finite()) {
                return None;
            }
            let xs = [n[0], n[2], n[4], n[6]];
            let ys = [n[1], n[3], n[5], n[7]];
            let min = |a: &[f64; 4]| a.iter().copied().fold(f64::INFINITY, f64::min);
            let max = |a: &[f64; 4]| a.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            Some([min(&xs), min(&ys), max(&xs) - min(&xs), max(&ys) - min(&ys)])
        })
        .collect()
}

/// The union of an element's boxes: `[x, y, w, h]` in CSS pixels from the viewport's top left. None
/// when it has no box (not rendered) or an empty one.
pub fn element_rect(quads: &Value) -> Option<[f64; 4]> {
    let rs = quad_rects(quads);
    let first = rs.first()?;
    let (mut x0, mut y0, mut x1, mut y1) =
        (first[0], first[1], first[0] + first[2], first[1] + first[3]);
    for r in &rs[1..] {
        x0 = x0.min(r[0]);
        y0 = y0.min(r[1]);
        x1 = x1.max(r[0] + r[2]);
        y1 = y1.max(r[1] + r[3]);
    }
    (x1 - x0 > 0.0 && y1 - y0 > 0.0).then_some([x0, y0, x1 - x0, y1 - y0])
}

/// Where to press on an element: the middle of the first of its boxes that shows in the viewport (a
/// link that wraps lines has several), clamped to what shows. None when no box shows.
pub fn click_point(quads: &Value, vp: &Viewport) -> Option<(f64, f64)> {
    for r in quad_rects(quads) {
        let x0 = r[0].max(0.0);
        let y0 = r[1].max(0.0);
        let x1 = (r[0] + r[2]).min(vp.w);
        let y1 = (r[1] + r[3]).min(vp.h);
        if x1 - x0 >= 1.0 && y1 - y0 >= 1.0 {
            return Some(((x0 + x1) / 2.0, (y0 + y1) / 2.0));
        }
    }
    None
}

// ---- the screenshot ---------------------------------------------------------------------------

/// The page's device pixel ratio from `window.devicePixelRatio` (device pixels per CSS pixel: 2.25 on a
/// 225 percent display), kept to a sane range; 1 when the browser did not say.
pub fn clean_dpr(v: Option<f64>) -> f64 {
    v.filter(|d| d.is_finite() && *d > 0.0)
        .map_or(1.0, |d| d.clamp(0.5, 8.0))
}

/// How a screenshot is scaled so it is no wider than [`SHOT_MAX_W`] pixels. The browser draws at the
/// device's resolution (`dpr` pixels per CSS pixel), which is the sharpest picture there is, so a
/// page that fits is taken as it is (1.0) and a wider one is scaled down to fit.
pub fn shot_scale(css_w: f64, dpr: f64) -> f64 {
    let device_w = css_w * dpr;
    if device_w <= f64::from(SHOT_MAX_W) {
        1.0
    } else {
        f64::from(SHOT_MAX_W) / device_w
    }
}

/// The `Page.captureScreenshot` parameters of the visible area, scaled by [`shot_scale`].
pub fn shot_params(vp: &Viewport, dpr: f64) -> Value {
    json!({
        "format": "png",
        "fromSurface": true,
        "captureBeyondViewport": false,
        "clip": {
            "x": vp.page_x,
            "y": vp.page_y,
            "width": vp.w,
            "height": vp.h,
            "scale": shot_scale(vp.w, dpr),
        }
    })
}

/// A picture pixel to a CSS pixel (what `browser.click`'s x and y are).
pub fn image_to_css(px: f64, scale: f64) -> f64 {
    px / scale.max(f64::MIN_POSITIVE)
}

/// A CSS pixel to a picture pixel.
pub fn css_to_image(css: f64, scale: f64) -> f64 {
    css * scale
}

fn b64_val(c: u8) -> Option<u32> {
    match c {
        b'A'..=b'Z' => Some(u32::from(c - b'A')),
        b'a'..=b'z' => Some(u32::from(c - b'a') + 26),
        b'0'..=b'9' => Some(u32::from(c - b'0') + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// The width and height of a PNG given as base64, read from its header alone.
pub fn png_size_b64(data: &str) -> Option<(u32, u32)> {
    let head = data.as_bytes().get(..32)?;
    let mut bytes = Vec::with_capacity(24);
    for chunk in head.chunks(4) {
        let mut acc = 0u32;
        for &c in chunk {
            acc = (acc << 6) | b64_val(c)?;
        }
        bytes.extend_from_slice(&acc.to_be_bytes()[1..]);
    }
    const SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    if bytes.len() < 24 || bytes[..8] != SIG || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    (w > 0 && h > 0).then_some((w, h))
}

// ---- keys -------------------------------------------------------------------------------------

/// A key press as the browser's input events take it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeySpec {
    pub key: String,
    pub code: String,
    /// The Windows virtual-key code.
    pub vk: u32,
    /// The character the key types, when it types one (no control or alt chord does).
    pub text: Option<String>,
    /// Alt 1, Control 2, Meta 4, Shift 8.
    pub modifiers: u32,
}

const MOD_ALT: u32 = 1;
const MOD_CTRL: u32 = 2;
const MOD_META: u32 = 4;
const MOD_SHIFT: u32 = 8;

fn modifier_of(name: &str) -> Option<u32> {
    match name.to_ascii_lowercase().as_str() {
        "control" | "ctrl" => Some(MOD_CTRL),
        "alt" | "option" => Some(MOD_ALT),
        "shift" => Some(MOD_SHIFT),
        "meta" | "cmd" | "command" | "win" | "windows" | "super" => Some(MOD_META),
        _ => None,
    }
}

/// The names `browser.press` knows, for its error message and the tool's description.
pub const KEY_NAMES: &str = "Enter, Tab, Escape, Backspace, Delete, Insert, Space, ArrowUp, ArrowDown, ArrowLeft, ArrowRight, PageUp, PageDown, Home, End, F1 to F12, or one character (a letter, a digit or punctuation); with modifiers: Control+a, Shift+Tab, Alt+ArrowLeft";

fn named_key(name: &str) -> Option<(&'static str, &'static str, u32, Option<&'static str>)> {
    // (key, code, vk, text)
    Some(match name.to_ascii_lowercase().as_str() {
        "enter" | "return" => ("Enter", "Enter", 13, Some("\r")),
        "tab" => ("Tab", "Tab", 9, Some("\t")),
        "escape" | "esc" => ("Escape", "Escape", 27, None),
        "backspace" => ("Backspace", "Backspace", 8, None),
        "delete" | "del" => ("Delete", "Delete", 46, None),
        "insert" => ("Insert", "Insert", 45, None),
        "space" | "spacebar" => (" ", "Space", 32, Some(" ")),
        "arrowup" | "up" => ("ArrowUp", "ArrowUp", 38, None),
        "arrowdown" | "down" => ("ArrowDown", "ArrowDown", 40, None),
        "arrowleft" | "left" => ("ArrowLeft", "ArrowLeft", 37, None),
        "arrowright" | "right" => ("ArrowRight", "ArrowRight", 39, None),
        "pageup" => ("PageUp", "PageUp", 33, None),
        "pagedown" => ("PageDown", "PageDown", 34, None),
        "home" => ("Home", "Home", 36, None),
        "end" => ("End", "End", 35, None),
        _ => return None,
    })
}

const FKEYS: [(&str, &str); 12] = [
    ("F1", "F1"),
    ("F2", "F2"),
    ("F3", "F3"),
    ("F4", "F4"),
    ("F5", "F5"),
    ("F6", "F6"),
    ("F7", "F7"),
    ("F8", "F8"),
    ("F9", "F9"),
    ("F10", "F10"),
    ("F11", "F11"),
    ("F12", "F12"),
];

fn punctuation(c: char) -> Option<(&'static str, u32)> {
    Some(match c {
        '-' => ("Minus", 189),
        '=' => ("Equal", 187),
        ',' => ("Comma", 188),
        '.' => ("Period", 190),
        '/' => ("Slash", 191),
        ';' => ("Semicolon", 186),
        '\'' => ("Quote", 222),
        '[' => ("BracketLeft", 219),
        ']' => ("BracketRight", 221),
        '\\' => ("Backslash", 220),
        '`' => ("Backquote", 192),
        '+' => ("Equal", 187),
        _ => return None,
    })
}

/// Reads `Enter`, `Control+a`, `Shift+Tab`, `Alt+ArrowLeft`, `F5`, `a` into a [`KeySpec`].
pub fn parse_key(spec: &str) -> Result<KeySpec, String> {
    let spec = spec.trim();
    if spec.is_empty() || spec.chars().count() > 64 {
        return Err("key is required: a key name such as Enter or Control+a".to_string());
    }
    // "Control++" presses the plus key: a trailing plus is the key, not a separator.
    let (mods_part, key_part) = if spec == "+" {
        ("", "+")
    } else if let Some(head) = spec.strip_suffix('+') {
        (head.trim_end_matches('+'), "+")
    } else if let Some((h, k)) = spec.rsplit_once('+') {
        (h, k)
    } else {
        ("", spec)
    };
    let mut modifiers = 0u32;
    for m in mods_part.split('+').filter(|m| !m.is_empty()) {
        modifiers |= modifier_of(m)
            .ok_or_else(|| format!("{m} is not a modifier (Control, Alt, Shift, Meta)"))?;
    }
    let typing = modifiers & (MOD_CTRL | MOD_ALT | MOD_META) == 0;
    if let Some((key, code, vk, text)) = named_key(key_part) {
        return Ok(KeySpec {
            key: key.to_string(),
            code: code.to_string(),
            vk,
            text: (typing && text.is_some()).then(|| text.unwrap_or_default().to_string()),
            modifiers,
        });
    }
    if let Some(n) = key_part
        .strip_prefix(['F', 'f'])
        .and_then(|n| n.parse::<usize>().ok())
        && (1..=12).contains(&n)
    {
        let (key, code) = FKEYS[n - 1];
        return Ok(KeySpec {
            key: key.to_string(),
            code: code.to_string(),
            vk: 111 + u32::try_from(n).unwrap_or(1),
            text: None,
            modifiers,
        });
    }
    let mut chars = key_part.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else {
        return Err(format!("{key_part} is not a key I know; use {KEY_NAMES}"));
    };
    let shift = modifiers & MOD_SHIFT != 0;
    let (key_c, code, vk) = if c.is_ascii_alphabetic() {
        let up = c.to_ascii_uppercase();
        let shown = if shift { up } else { c.to_ascii_lowercase() };
        (shown, format!("Key{up}"), u32::from(up))
    } else if c.is_ascii_digit() {
        (c, format!("Digit{c}"), u32::from(c))
    } else if let Some((code, vk)) = punctuation(c) {
        (c, code.to_string(), vk)
    } else if c.is_control() || invisible(c) {
        return Err(format!("{key_part} is not a key I know; use {KEY_NAMES}"));
    } else {
        (c, String::new(), 0)
    };
    Ok(KeySpec {
        key: key_c.to_string(),
        code,
        vk,
        text: typing.then(|| key_c.to_string()),
        modifiers,
    })
}

/// The `Input.dispatchKeyEvent` calls of a press: key down (with its character when it types one),
/// key up.
pub fn key_events(k: &KeySpec) -> Vec<Value> {
    let mut down = json!({
        "type": if k.text.is_some() { "keyDown" } else { "rawKeyDown" },
        "modifiers": k.modifiers,
        "key": k.key,
        "windowsVirtualKeyCode": k.vk,
        "nativeVirtualKeyCode": k.vk,
    });
    if !k.code.is_empty() {
        down["code"] = json!(k.code);
    }
    if let Some(t) = &k.text {
        down["text"] = json!(t);
        down["unmodifiedText"] = json!(t);
    }
    let mut up = json!({
        "type": "keyUp",
        "modifiers": k.modifiers,
        "key": k.key,
        "windowsVirtualKeyCode": k.vk,
        "nativeVirtualKeyCode": k.vk,
    });
    if !k.code.is_empty() {
        up["code"] = json!(k.code);
    }
    vec![down, up]
}

// ---- mouse ------------------------------------------------------------------------------------

/// Which mouse button a click uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
    Middle,
}

impl Button {
    pub fn parse(s: &str) -> Option<Button> {
        match s.to_ascii_lowercase().as_str() {
            "left" => Some(Button::Left),
            "right" => Some(Button::Right),
            "middle" => Some(Button::Middle),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Button::Left => "left",
            Button::Right => "right",
            Button::Middle => "middle",
        }
    }

    fn mask(self) -> u32 {
        match self {
            Button::Left => 1,
            Button::Right => 2,
            Button::Middle => 4,
        }
    }
}

/// The `Input.dispatchMouseEvent` calls of a click at a point: move, press, release (twice, with the
/// click count rising, for a double click).
pub fn click_events(x: f64, y: f64, button: Button, double: bool) -> Vec<Value> {
    let mut v = vec![json!({"type": "mouseMoved", "x": x, "y": y, "button": "none", "buttons": 0})];
    for count in 1..=(if double { 2 } else { 1 }) {
        v.push(json!({
            "type": "mousePressed", "x": x, "y": y, "button": button.name(),
            "buttons": button.mask(), "clickCount": count
        }));
        v.push(json!({
            "type": "mouseReleased", "x": x, "y": y, "button": button.name(),
            "buttons": 0, "clickCount": count
        }));
    }
    v
}

/// The wheel event of a scroll at a point (deltas in CSS pixels, down and right positive).
pub fn wheel_event(x: f64, y: f64, dx: f64, dy: f64) -> Value {
    json!({"type": "mouseWheel", "x": x, "y": y, "deltaX": dx, "deltaY": dy})
}

/// The check a click by ref makes before it presses, run on the element itself (`this`) with the click's point as
/// its two numbers. It answers `{ok: true}` when the click lands, `{covered: true, tag, name}` when something else
/// is on top, and null when it cannot say.
///
/// It takes the deepest element at the point, through open shadow roots. The click lands when that element is the
/// target, is inside it, is a label of it, or wraps it (a closed shadow host, or a wrapper when the target takes no
/// pointer events); `html` and `body` wrap everything, so they say nothing and count as something else. Null: an
/// element in a frame (a look does not list those, and the point is the top page's), or a point the page does not
/// place on the target (the coordinates do not agree).
pub const HIT_TEST_FN: &str = "function(x,y){var n=this,t=n.nodeType===1?n:n.parentElement||(n.parentNode&&n.parentNode.host);if(!t)return null;var w=t.ownerDocument.defaultView,r,i,hit,rs,inb=false;function deep(d){var h=d.elementFromPoint(x,y),s,e;while(h&&(s=h.shadowRoot)&&(e=s.elementFromPoint(x,y))&&e!==h)h=e;return h;}function up(e){return e.assignedSlot||e.parentNode||e.host||null;}function within(a,b){for(;a;a=up(a))if(a===b)return true;return false;}function lab(h){for(var e=h;e;e=up(e))if(e.localName==='label')return e.control===t;return false;}function cover(h){var k=h.localName||'';return{covered:true,tag:k,name:k==='html'||k==='body'?'':String(h.getAttribute('aria-label')||h.innerText||h.textContent||h.id||'').slice(0,200)};}if(!w||w!==w.parent)return null;rs=t.getClientRects();for(i=0;i<rs.length;i++){r=rs[i];if(x>=r.left-2&&x<=r.right+2&&y>=r.top-2&&y<=r.bottom+2)inb=true;}if(!inb)return null;hit=deep(t.ownerDocument);if(!hit)return null;if(within(hit,t)||lab(hit))return{ok:true};if(within(t,hit)&&hit.localName!=='html'&&hit.localName!=='body')return{ok:true};return cover(hit);}";

/// The longest piece of a covering element's text that is reported.
const COVER_NAME_MAX: usize = 60;

/// The `Runtime.callFunctionOn` parameters of [`HIT_TEST_FN`] on a resolved element. The script is fixed: the only
/// things that vary are the object and the two numbers.
pub fn hit_test_params(object_id: &str, x: f64, y: f64) -> Value {
    json!({
        "objectId": object_id,
        "functionDeclaration": HIT_TEST_FN,
        "arguments": [{"value": x}, {"value": y}],
        "returnByValue": true,
        "silent": true,
    })
}

/// What covers a click's target, out of the check's answer: the element's tag and a piece of its text (`div:
/// Accept all`), cleaned like every other text of the page. None when the click lands or the page could not say,
/// and an answer that is not one of [`HIT_TEST_FN`]'s is a page that could not say.
pub fn covered_by(answer: &Value) -> Option<String> {
    if answer.get("exceptionDetails").is_some() {
        return None;
    }
    let v = answer.pointer("/result/value")?;
    if v.get("covered").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    let tag: String = v
        .get("tag")
        .and_then(Value::as_str)
        .unwrap_or("")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .take(24)
        .collect();
    let tag = if tag.is_empty() {
        "element".to_string()
    } else {
        tag
    };
    let name = clean_text(
        v.get("name").and_then(Value::as_str).unwrap_or(""),
        COVER_NAME_MAX,
    );
    Some(if name.is_empty() {
        tag
    } else {
        format!("{tag}: {name}")
    })
}

// ---- wait -------------------------------------------------------------------------------------

/// What `browser.wait` waits for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WaitFor {
    /// Visible text anywhere on the page (case does not matter).
    Text(String),
    /// A CSS selector that matches an element.
    Selector(String),
    /// A pause.
    Ms(u64),
}

/// A JSON string literal that is also a safe JavaScript string literal: the line and paragraph
/// separators are written as escapes too.
fn js_literal(s: &str) -> String {
    serde_json::to_string(s)
        .unwrap_or_else(|_| "\"\"".to_string())
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

/// The fixed template with the one place text goes (`@`), kept apart so a test can prove the rest.
pub const WAIT_TEXT_TEMPLATE: &str = "(function(t){var b=document.body;return !!b&&(b.innerText||'').toLowerCase().indexOf(t.toLowerCase())>=0;})(@)";
pub const WAIT_SELECTOR_TEMPLATE: &str =
    "(function(s){try{return document.querySelector(s)!==null;}catch(e){return false;}})(@)";

/// The script of one poll of a text or selector wait: a fixed template around ONE JSON string literal.
/// There is no way for the text to end the literal or add code. None for a pause.
pub fn wait_expression(w: &WaitFor) -> Option<String> {
    match w {
        WaitFor::Text(t) => Some(WAIT_TEXT_TEMPLATE.replace('@', &js_literal(t))),
        WaitFor::Selector(s) => Some(WAIT_SELECTOR_TEMPLATE.replace('@', &js_literal(s))),
        WaitFor::Ms(_) => None,
    }
}

/// The `timeout_ms` of a wait, cut to [`WAIT_MAX_MS`] and defaulted.
pub fn wait_timeout(ms: Option<u64>) -> u64 {
    ms.unwrap_or(WAIT_DEFAULT_MS).min(WAIT_MAX_MS)
}

#[cfg(test)]
mod tests {
    use super::*;

    // A recorded-shape answer of Accessibility.getFullAXTree (synthetic: a small login page).
    fn ax_fixture() -> Value {
        json!({"nodes": [
            {"nodeId": "1", "ignored": false, "role": {"type": "role", "value": "RootWebArea"},
             "name": {"type": "computedString", "value": "Sign in"}, "childIds": ["2", "3", "4", "8", "9", "10"], "backendDOMNodeId": 1},
            {"nodeId": "2", "ignored": false, "role": {"value": "heading"}, "name": {"value": "Sign in"},
             "properties": [{"name": "level", "value": {"type": "integer", "value": 1}}],
             "childIds": ["20"], "backendDOMNodeId": 12},
            {"nodeId": "20", "ignored": false, "role": {"value": "StaticText"}, "name": {"value": "Sign in"},
             "childIds": ["21"], "backendDOMNodeId": 13},
            {"nodeId": "21", "ignored": false, "role": {"value": "InlineTextBox"}, "name": {"value": "Sign in"},
             "backendDOMNodeId": 13},
            {"nodeId": "3", "ignored": false, "role": {"value": "textbox"}, "name": {"value": "Email"},
             "value": {"type": "string", "value": "me@example.test"},
             "properties": [{"name": "focused", "value": {"type": "boolean", "value": true}},
                            {"name": "required", "value": {"type": "boolean", "value": true}}],
             "backendDOMNodeId": 14},
            {"nodeId": "4", "ignored": true, "role": {"value": "none"}, "childIds": ["5", "6"], "backendDOMNodeId": 15},
            {"nodeId": "5", "ignored": false, "role": {"value": "checkbox"}, "name": {"value": "Remember me"},
             "properties": [{"name": "checked", "value": {"type": "tristate", "value": "false"}}],
             "backendDOMNodeId": 16},
            {"nodeId": "6", "ignored": false, "role": {"value": "link"}, "name": {"value": "Forgot\n  password?\u{202E}"},
             "properties": [{"name": "url", "value": {"type": "string", "value": "https://flare-test.local/forgot"}}],
             "childIds": ["7"], "backendDOMNodeId": 17},
            {"nodeId": "7", "ignored": false, "role": {"value": "StaticText"}, "name": {"value": "Forgot password?"},
             "backendDOMNodeId": 18},
            {"nodeId": "8", "ignored": false, "role": {"value": "button"}, "name": {"value": "Go"},
             "properties": [{"name": "disabled", "value": {"type": "boolean", "value": true}}],
             "backendDOMNodeId": 19},
            {"nodeId": "9", "ignored": false, "role": {"value": "StaticText"}, "name": {"value": "Welcome back"}, "backendDOMNodeId": 21},
            {"nodeId": "10", "ignored": false, "role": {"value": "generic"}, "name": {"value": ""}, "backendDOMNodeId": 22}
        ]})
    }

    #[test]
    fn the_snapshot_keeps_controls_and_text_and_drops_the_rest() {
        let (c, cut) = candidates(&ax_fixture());
        assert!(!cut);
        let seen: Vec<(&str, &str)> = c
            .iter()
            .map(|c| (c.role.as_str(), c.name.as_str()))
            .collect();
        assert_eq!(
            seen,
            [
                ("heading", "Sign in"),
                ("textbox", "Email"),
                ("checkbox", "Remember me"),
                ("link", "Forgot password?"),
                ("button", "Go"),
                ("text", "Welcome back"),
            ],
            "the text inside the heading and the link is not repeated, the generic box and the root are gone"
        );
        let email = &c[1];
        assert_eq!(email.value, "me@example.test");
        assert_eq!(email.states, ["required", "focused"]);
        assert_eq!(email.backend_id, 14);
        assert_eq!(c[0].level, Some(1));
        assert_eq!(c[2].states, ["unchecked"]);
        assert_eq!(
            c[3].href.as_deref(),
            Some("https://flare-test.local/forgot")
        );
        assert_eq!(c[4].states, ["disabled"]);
        assert!(c[5].backend_id == 21 && !c[5].interactive);
    }

    #[test]
    fn a_node_below_an_ignored_one_is_still_found() {
        let (c, _) = candidates(&ax_fixture());
        assert!(c.iter().any(|c| c.name == "Remember me"));
    }

    #[test]
    fn page_text_is_cleaned_before_it_goes_anywhere() {
        // The right-to-left override in the link's name is gone, the newline and runs of spaces are one space.
        let (c, _) = candidates(&ax_fixture());
        assert_eq!(c[3].name, "Forgot password?");
        assert_eq!(clean_text("a\u{200B}b\tc\n\nd   e", 100), "ab c d e");
        assert_eq!(clean_text("  lead and trail  ", 100), "lead and trail");
        assert_eq!(clean_text("abcdefghij", 5), "abcde...");
        assert_eq!(clean_text("ab cd", 4), "ab c...");
        assert_eq!(clean_text("\u{0}\u{7}\u{1b}[31mred", 100), "[31mred");
        assert_eq!(clean_text("", 5), "");
        // Unicode tag characters and variation selectors hide words from a person and not from a model.
        assert_eq!(
            clean_text(
                "ok\u{E0069}\u{E0067}\u{E006E}\u{FE0F}\u{E0100}\u{180E}\u{3164}!",
                100
            ),
            "ok!"
        );
    }

    #[test]
    fn a_long_name_is_cut_to_the_cap() {
        let long = "x".repeat(1000);
        let ax = json!({"nodes": [{"nodeId": "1", "role": {"value": "button"}, "name": {"value": long}, "backendDOMNodeId": 5}]});
        let (c, _) = candidates(&ax);
        assert_eq!(c[0].name.chars().count(), TEXT_MAX + 3);
        assert!(c[0].name.ends_with("..."));
    }

    #[test]
    fn a_big_page_keeps_every_control_first_and_says_it_was_cut() {
        let mut nodes = vec![
            json!({"nodeId": "r", "role": {"value": "RootWebArea"}, "childIds": [], "backendDOMNodeId": 1}),
        ];
        let mut kids = Vec::new();
        // 500 paragraphs of text, then 150 buttons.
        for i in 0..500 {
            let id = format!("t{i}");
            nodes.push(json!({"nodeId": id, "role": {"value": "StaticText"}, "name": {"value": format!("line {i}")}, "backendDOMNodeId": 1000 + i}));
            kids.push(id);
        }
        for i in 0..150 {
            let id = format!("b{i}");
            nodes.push(json!({"nodeId": id, "role": {"value": "button"}, "name": {"value": format!("b{i}")}, "backendDOMNodeId": 5000 + i}));
            kids.push(id);
        }
        nodes[0]["childIds"] = json!(kids);
        let (c, cut) = candidates(&json!({ "nodes": nodes }));
        assert!(cut);
        assert_eq!(c.len(), MAX_ELEMENTS);
        assert_eq!(
            c.iter().filter(|c| c.interactive).count(),
            150,
            "every control survives the cut"
        );
        assert_eq!(
            c.iter().filter(|c| !c.interactive).count(),
            450,
            "the rest is the first of the text"
        );
        assert_eq!(c[0].name, "line 0");
        assert_eq!(
            c.last().unwrap().name,
            "b149",
            "and the order is the page's"
        );
    }

    #[test]
    fn a_hostile_tree_cannot_loop_or_blow_up() {
        // A cycle, a shared child, a missing child, junk shapes.
        let ax = json!({"nodes": [
            {"nodeId": "1", "role": {"value": "button"}, "name": {"value": "a"}, "childIds": ["2", "1", "404"], "backendDOMNodeId": 1},
            {"nodeId": "2", "role": {"value": "button"}, "name": {"value": "b"}, "childIds": ["1", "2"], "backendDOMNodeId": 2},
            {"nodeId": "3", "role": 7, "name": [], "backendDOMNodeId": "x"},
            {"nodeId": "4"},
            7, null
        ]});
        let (c, _) = candidates(&ax);
        assert_eq!(c.len(), 2);
        assert_eq!(candidates(&json!({})).0.len(), 0);
        assert_eq!(candidates(&json!({"nodes": 5})).0.len(), 0);
    }

    #[test]
    fn identical_text_in_a_row_is_one_element() {
        let ax = json!({"nodes": [
            {"nodeId": "1", "role": {"value": "RootWebArea"}, "childIds": ["2", "3", "4"], "backendDOMNodeId": 1},
            {"nodeId": "2", "role": {"value": "StaticText"}, "name": {"value": "Hi"}, "backendDOMNodeId": 2},
            {"nodeId": "3", "role": {"value": "StaticText"}, "name": {"value": "Hi"}, "backendDOMNodeId": 3},
            {"nodeId": "4", "role": {"value": "StaticText"}, "name": {"value": "There"}, "backendDOMNodeId": 4}
        ]});
        assert_eq!(candidates(&ax).0.len(), 2);
    }

    fn quads(list: &[[f64; 8]]) -> Value {
        json!({ "quads": list })
    }

    #[test]
    fn an_elements_rect_is_the_union_of_its_boxes() {
        let q = quads(&[
            [10.0, 10.0, 60.0, 10.0, 60.0, 30.0, 10.0, 30.0],
            [5.0, 40.0, 20.0, 40.0, 20.0, 60.0, 5.0, 60.0],
        ]);
        assert_eq!(element_rect(&q), Some([5.0, 10.0, 55.0, 50.0]));
        assert_eq!(element_rect(&quads(&[])), None);
        assert_eq!(
            element_rect(&quads(&[[1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0]])),
            None,
            "a box with no area is not there"
        );
        assert_eq!(element_rect(&json!({"quads": [[1, 2, 3]]})), None);
    }

    #[test]
    fn a_click_lands_in_the_middle_of_the_first_box_that_shows() {
        let vp = Viewport {
            w: 800.0,
            h: 600.0,
            page_x: 0.0,
            page_y: 0.0,
        };
        // A link wrapping two lines: the first box is above the viewport, the second shows.
        let q = quads(&[
            [10.0, -30.0, 90.0, -30.0, 90.0, -10.0, 10.0, -10.0],
            [10.0, 20.0, 50.0, 20.0, 50.0, 40.0, 10.0, 40.0],
        ]);
        assert_eq!(click_point(&q, &vp), Some((30.0, 30.0)));
        // One box half out of the viewport: the middle of what shows.
        let q = quads(&[[700.0, 100.0, 900.0, 100.0, 900.0, 140.0, 700.0, 140.0]]);
        assert_eq!(click_point(&q, &vp), Some((750.0, 120.0)));
        // Nothing shows.
        let q = quads(&[[10.0, 700.0, 50.0, 700.0, 50.0, 720.0, 10.0, 720.0]]);
        assert_eq!(click_point(&q, &vp), None);
    }

    #[test]
    fn the_screenshot_is_never_wider_than_1600_device_pixels() {
        // At 100 percent a 1000 px page is taken as it is; at 225 percent it is 2250 device pixels
        // wide, so it is scaled to fit; a narrow page keeps the display's full resolution.
        assert_eq!(shot_scale(1000.0, 1.0), 1.0);
        assert_eq!(shot_scale(1600.0, 1.0), 1.0);
        assert!((shot_scale(3200.0, 1.0) - 0.5).abs() < 1e-12);
        assert!((shot_scale(1000.0, 2.25) - 1600.0 / 2250.0).abs() < 1e-12);
        assert_eq!(shot_scale(400.0, 2.25), 1.0);
        assert_eq!(shot_scale(710.0, 2.25), 1.0);
        for (css, dpr) in [
            (400.0, 2.25),
            (1000.0, 2.25),
            (3000.0, 1.0),
            (5000.0, 3.0),
            (100.0, 1.0),
        ] {
            assert!(
                css * dpr * shot_scale(css, dpr) <= 1600.0 + 1e-9,
                "{css} x {dpr}"
            );
        }
        assert_eq!(clean_dpr(Some(2.25)), 2.25);
        assert_eq!(clean_dpr(None), 1.0);
        assert_eq!(clean_dpr(Some(0.0)), 1.0);
        assert_eq!(clean_dpr(Some(f64::NAN)), 1.0);
        assert_eq!(clean_dpr(Some(100.0)), 8.0);
        assert_eq!(clean_dpr(Some(0.1)), 0.5);
    }

    #[test]
    fn the_screenshot_clip_is_the_visible_area_in_document_space() {
        let vp = Viewport {
            w: 2000.0,
            h: 1000.0,
            page_x: 0.0,
            page_y: 340.0,
        };
        let p = shot_params(&vp, 1.0);
        assert_eq!(p["clip"]["scale"], json!(0.8));
        assert_eq!(
            p["clip"]["y"],
            json!(340.0),
            "the clip is in document space, so the scroll offset is added"
        );
        assert_eq!(p["captureBeyondViewport"], json!(false));
        assert_eq!(p["format"], json!("png"));
        // The agent's picture pixels and the page's css pixels convert both ways.
        assert!((image_to_css(400.0, 0.8) - 500.0).abs() < 1e-9);
        assert!((css_to_image(500.0, 0.8) - 400.0).abs() < 1e-9);
        assert!((image_to_css(css_to_image(123.0, 0.37), 0.37) - 123.0).abs() < 1e-9);
    }

    #[test]
    fn layout_metrics_give_the_visible_area_in_css_pixels() {
        let m = json!({
            "layoutViewport": {"pageX": 0, "pageY": 0, "clientWidth": 2250, "clientHeight": 1440},
            "cssLayoutViewport": {"pageX": 0, "pageY": 0, "clientWidth": 1000, "clientHeight": 640},
            "cssVisualViewport": {"offsetX": 0, "offsetY": 0, "pageX": 0, "pageY": 120.5, "clientWidth": 1000, "clientHeight": 640, "scale": 1, "zoom": 1}
        });
        let v = viewport_of(&m).unwrap();
        assert_eq!((v.w, v.h, v.page_x, v.page_y), (1000.0, 640.0, 0.0, 120.5));
        assert!(viewport_of(&json!({})).is_none());
        assert!(
            viewport_of(&json!({"cssVisualViewport": {"clientWidth": 0, "clientHeight": 5}}))
                .is_none()
        );
    }

    fn png_b64(w: u32, h: u32) -> String {
        let mut b = vec![
            0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13, b'I', b'H', b'D', b'R',
        ];
        b.extend_from_slice(&w.to_be_bytes());
        b.extend_from_slice(&h.to_be_bytes());
        b.extend_from_slice(&[8, 6, 0, 0, 0]);
        const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut s = String::new();
        for c in b.chunks(3) {
            let n = (u32::from(c[0]) << 16)
                | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
                | u32::from(*c.get(2).unwrap_or(&0));
            for i in 0..4 {
                s.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
            }
        }
        s
    }

    #[test]
    fn a_pngs_size_is_read_from_its_header() {
        assert_eq!(png_size_b64(&png_b64(1600, 900)), Some((1600, 900)));
        assert_eq!(png_size_b64(&png_b64(1, 1)), Some((1, 1)));
        assert_eq!(png_size_b64("not a png at all, not a png at all"), None);
        assert_eq!(png_size_b64("short"), None);
        assert_eq!(png_size_b64(&png_b64(0, 5)), None);
    }

    #[test]
    fn keys_read_by_name_with_modifiers() {
        let k = parse_key("Enter").unwrap();
        assert_eq!(
            (k.key.as_str(), k.vk, k.text.as_deref(), k.modifiers),
            ("Enter", 13, Some("\r"), 0)
        );
        let k = parse_key("Control+a").unwrap();
        assert_eq!(
            (k.key.as_str(), k.code.as_str(), k.vk, k.modifiers),
            ("a", "KeyA", 65, 2)
        );
        assert_eq!(k.text, None, "a control chord types nothing");
        let k = parse_key("Shift+Tab").unwrap();
        assert_eq!((k.vk, k.modifiers), (9, 8));
        assert_eq!(k.text.as_deref(), Some("\t"));
        let k = parse_key("ctrl+shift+P").unwrap();
        assert_eq!((k.key.as_str(), k.modifiers), ("P", 10));
        let k = parse_key("Alt+ArrowLeft").unwrap();
        assert_eq!((k.vk, k.modifiers), (37, 1));
        let k = parse_key("PageDown").unwrap();
        assert_eq!(k.vk, 34);
        let k = parse_key("f5").unwrap();
        assert_eq!((k.key.as_str(), k.vk), ("F5", 116));
        let k = parse_key("7").unwrap();
        assert_eq!(
            (k.code.as_str(), k.vk, k.text.as_deref()),
            ("Digit7", 55, Some("7"))
        );
        let k = parse_key("Control++").unwrap();
        assert_eq!((k.key.as_str(), k.modifiers), ("+", 2));
        let k = parse_key("+").unwrap();
        assert_eq!(k.text.as_deref(), Some("+"));
        let k = parse_key("Space").unwrap();
        assert_eq!((k.key.as_str(), k.text.as_deref()), (" ", Some(" ")));
        let k = parse_key("é").unwrap();
        assert_eq!(
            (k.code.as_str(), k.vk, k.text.as_deref()),
            ("", 0, Some("é"))
        );
        for named in [
            "Escape",
            "Backspace",
            "Delete",
            "Home",
            "End",
            "PageUp",
            "ArrowUp",
            "ArrowDown",
            "ArrowRight",
            "Insert",
        ] {
            assert!(parse_key(named).is_ok(), "{named}");
        }
    }

    #[test]
    fn bad_keys_are_refused_with_the_list() {
        for bad in [
            "",
            "   ",
            "Hyper+a",
            "Banana",
            "F13",
            "Control+Banana",
            "\u{7}",
        ] {
            let e = parse_key(bad).unwrap_err();
            assert!(!e.is_empty(), "{bad:?}");
        }
        assert!(parse_key("Banana").unwrap_err().contains("ArrowUp"));
        assert!(parse_key(&"a".repeat(100)).is_err());
    }

    #[test]
    fn a_key_press_is_a_down_and_an_up() {
        let ev = key_events(&parse_key("Enter").unwrap());
        assert_eq!(ev.len(), 2);
        assert_eq!(ev[0]["type"], json!("keyDown"));
        assert_eq!(ev[0]["text"], json!("\r"));
        assert_eq!(ev[0]["windowsVirtualKeyCode"], json!(13));
        assert_eq!(ev[1]["type"], json!("keyUp"));
        let ev = key_events(&parse_key("Control+a").unwrap());
        assert_eq!(ev[0]["type"], json!("rawKeyDown"));
        assert_eq!(ev[0]["modifiers"], json!(2));
        assert!(ev[0].get("text").is_none());
        let ev = key_events(&parse_key("Escape").unwrap());
        assert_eq!(ev[0]["type"], json!("rawKeyDown"));
        assert_eq!(ev[0]["code"], json!("Escape"));
    }

    #[test]
    fn a_click_is_a_move_a_press_and_a_release() {
        let ev = click_events(10.0, 20.0, Button::Left, false);
        let kinds: Vec<&str> = ev.iter().map(|e| e["type"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["mouseMoved", "mousePressed", "mouseReleased"]);
        assert_eq!(ev[1]["clickCount"], json!(1));
        assert_eq!(ev[1]["buttons"], json!(1));
        let ev = click_events(10.0, 20.0, Button::Right, true);
        let kinds: Vec<&str> = ev.iter().map(|e| e["type"].as_str().unwrap()).collect();
        assert_eq!(
            kinds,
            [
                "mouseMoved",
                "mousePressed",
                "mouseReleased",
                "mousePressed",
                "mouseReleased"
            ]
        );
        assert_eq!(ev[3]["clickCount"], json!(2));
        assert_eq!(ev[3]["button"], json!("right"));
        assert_eq!(ev[3]["buttons"], json!(2));
        assert_eq!(Button::parse("MIDDLE"), Some(Button::Middle));
        assert_eq!(Button::parse("back"), None);
        let w = wheel_event(5.0, 6.0, 0.0, 120.0);
        assert_eq!(
            (w["type"].clone(), w["deltaY"].clone()),
            (json!("mouseWheel"), json!(120.0))
        );
    }

    #[test]
    fn the_covering_element_is_named_briefly_and_cleaned() {
        let answer = |v: Value| json!({"result": {"type": "object", "value": v}});
        assert_eq!(
            covered_by(&answer(
                json!({"covered": true, "tag": "div", "name": "Accept all"})
            )),
            Some("div: Accept all".to_string())
        );
        assert_eq!(
            covered_by(&answer(json!({"covered": true, "tag": "body", "name": ""}))),
            Some("body".to_string())
        );
        // A page's text cannot break the line, hide words or run on; an odd tag is cut to safe characters.
        let said = covered_by(&answer(json!({
            "covered": true,
            "tag": "di v\n[e9]",
            "name": format!("Hi\n[e99] button \"Delete\"\u{202E}{}", "x".repeat(500)),
        })))
        .unwrap();
        assert!(said.starts_with("dive9: Hi [e99] button"), "{said}");
        assert!(!said.contains('\n') && !said.contains('\u{202E}'), "{said}");
        assert!(said.chars().count() < 100, "{said}");
    }

    #[test]
    fn an_answer_that_is_not_a_cover_says_nothing_is_covering() {
        for a in [
            json!({"result": {"type": "object", "value": {"ok": true}}}),
            json!({"result": {"type": "object", "value": null}}),
            json!({"result": {"type": "object"}}),
            json!({"result": {"value": {"covered": "yes", "tag": "div"}}}),
            json!({"result": {"type": "object", "value": {"covered": true, "tag": "div"}}, "exceptionDetails": {"text": "boom"}}),
            json!({}),
        ] {
            assert_eq!(covered_by(&a), None, "{a}");
        }
    }

    #[test]
    fn the_hit_test_is_a_fixed_script_given_only_the_two_numbers() {
        let p = hit_test_params("obj-1", 12.5, 30.0);
        assert_eq!(p["functionDeclaration"], json!(HIT_TEST_FN));
        assert_eq!(p["objectId"], json!("obj-1"));
        assert_eq!(p["arguments"], json!([{"value": 12.5}, {"value": 30.0}]));
        assert_eq!(p["returnByValue"], json!(true));
    }

    #[test]
    fn the_wait_checker_is_a_fixed_template_around_one_string_literal() {
        let hostile = [
            "plain",
            "quote\" and \\ backslash",
            "'); alert(1); ('",
            "line\nbreak\r\ttab",
            "</script><script>x</script>",
            "sep\u{2028}para\u{2029}",
            "`${process}`",
            "*/ evil /*",
            "\u{0}nul",
            "emoji \u{1F600} and é",
            "",
        ];
        for t in hostile {
            for (w, template) in [
                (WaitFor::Text(t.to_string()), WAIT_TEXT_TEMPLATE),
                (WaitFor::Selector(t.to_string()), WAIT_SELECTOR_TEMPLATE),
            ] {
                let expr = wait_expression(&w).unwrap();
                let lit = js_literal(t);
                // The literal parses back to exactly the text: it cannot have ended early.
                let back: String = serde_json::from_str(
                    &lit.replace("\\u2028", "\u{2028}")
                        .replace("\\u2029", "\u{2029}"),
                )
                .unwrap();
                assert_eq!(back, t);
                // Everything but the literal is the fixed template, byte for byte.
                assert_eq!(expr.replacen(&lit, "@", 1), template, "{t:?}");
                // The literal holds no raw newline or separator that could end a line of script.
                assert!(
                    !lit.contains('\n')
                        && !lit.contains('\r')
                        && !lit.contains('\u{2028}')
                        && !lit.contains('\u{2029}')
                );
            }
        }
        assert_eq!(wait_expression(&WaitFor::Ms(5)), None);
    }

    #[test]
    fn a_wait_lasts_at_most_thirty_seconds() {
        assert_eq!(wait_timeout(None), WAIT_DEFAULT_MS);
        assert_eq!(wait_timeout(Some(500)), 500);
        assert_eq!(wait_timeout(Some(600_000)), WAIT_MAX_MS);
        assert_eq!(wait_timeout(Some(0)), 0);
    }

    #[test]
    fn an_arrow_on_a_buttons_label_names_the_button_and_one_on_nothing_named_says_its_role() {
        let tree = json!({"nodes": [
            {"nodeId": "1", "role": {"value": "RootWebArea"}, "childIds": ["2"], "backendDOMNodeId": 1},
            {"nodeId": "2", "role": {"value": "button"}, "name": {"value": "Copy key"}, "childIds": ["3"], "backendDOMNodeId": 20},
            {"nodeId": "3", "role": {"value": "generic"}, "ignored": true, "parentId": "2", "childIds": ["4"], "backendDOMNodeId": 21},
            {"nodeId": "4", "role": {"value": "StaticText"}, "name": {"value": ""}, "parentId": "3", "backendDOMNodeId": 22}
        ]});
        assert_eq!(
            pointed_at(&tree, 22),
            Some(Pointed {
                backend_id: 20,
                role: "button".into(),
                name: "Copy key".into()
            })
        );
        assert_eq!(pointed_at(&tree, 20).unwrap().backend_id, 20);
        // A plain box with nothing a look would list: its own role, no name.
        let tree = json!({"nodes": [
            {"nodeId": "9", "role": {"value": "region"}, "backendDOMNodeId": 90}
        ]});
        assert_eq!(
            pointed_at(&tree, 90),
            Some(Pointed {
                backend_id: 90,
                role: "region".into(),
                name: String::new()
            })
        );
        let tree = json!({"nodes": [{"nodeId": "9", "role": {"value": "generic"}, "backendDOMNodeId": 90}]});
        assert_eq!(pointed_at(&tree, 90).unwrap().role, "element");
        assert_eq!(pointed_at(&tree, 91), None);
        assert_eq!(pointed_at(&json!({}), 1), None);
    }

    #[test]
    fn a_pointed_name_is_cleaned_and_a_tree_that_loops_ends() {
        let tree = json!({"nodes": [
            {"nodeId": "1", "role": {"value": "generic"}, "parentId": "2", "backendDOMNodeId": 1},
            {"nodeId": "2", "role": {"value": "generic"}, "parentId": "1", "backendDOMNodeId": 2},
        ]});
        assert_eq!(pointed_at(&tree, 1).unwrap().backend_id, 1);
        let tree = json!({"nodes": [
            {"nodeId": "1", "role": {"value": "link"}, "name": {"value": "Pay\u{202e}\u{1b}[2J now"}, "backendDOMNodeId": 5}
        ]});
        let p = pointed_at(&tree, 5).unwrap();
        assert!(
            p.name.chars().all(|c| !c.is_control() && c != '\u{202e}'),
            "{:?}",
            p.name
        );
    }
}

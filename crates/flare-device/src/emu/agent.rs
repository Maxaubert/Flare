//! The agent helper's pure parts (Android plan PR A3, rule 1369): `flare-agent.jar`, built from
//! `tools/android-agent/src` by `tools/android-agent/build.py` and checked in under `crates/flare-device/assets` with its
//! hashes. The emu host pushes it into an emulator (never a real phone in v1), starts it with `app_process` as the
//! shell user, and talks to it through `adb forward tcp:0 localabstract:flare-agent`.
//!
//! Here: the jar's expected hash, the device-side command, the JSON lines both ways, the tree the helper reads turned
//! into the tree model (`tree::UiNode`, frames in dp), and the `uiautomator dump` fallback read the same way.

use serde::Deserialize;
use serde_json::Value;

use crate::geom::Size;
use crate::tree::{AndroidNode, Rect, UiNode, android_role};

/// The helper's wire protocol (`Agent.PROTOCOL` in the Java source).
pub const PROTOCOL: u64 = 1;
/// The abstract socket the helper listens on (`Agent.SOCKET`). One helper per device: a second Flare reuses it.
pub const SOCKET: &str = "flare-agent";
/// The helper's process name in the device (`app_process --nice-name`), so an old one can be found and stopped.
pub const PROCESS: &str = "flare-agent";
/// The helper's main class.
const MAIN_CLASS: &str = "dev.flare.agent.Agent";
/// Where `uiautomator dump` writes in the fallback.
pub const DUMP_PATH: &str = "/data/local/tmp/flare-ui.xml";

/// `flare-agent.jar.sha256`: `jar <hex>` and `source <hex>` lines, written by the build script.
const SUMS: &str = include_str!("../../assets/flare-agent.jar.sha256");

fn sum(key: &str) -> &'static str {
    SUMS.lines()
        .find_map(|l| l.strip_prefix(key)?.strip_prefix(' '))
        .map(str::trim)
        .unwrap_or("")
}

/// The SHA-256 of the jar this build of Flare takes (hex). A jar with another hash is never pushed.
pub fn jar_sha256() -> &'static str {
    sum("jar")
}

/// The hash of the sources the checked-in jar came from (see [`source_sha256`]).
pub fn recorded_source_sha256() -> &'static str {
    sum("source")
}

/// Where the jar goes in the device: named by its hash, so two builds never overwrite each other's.
pub fn device_jar(sha256: &str) -> String {
    format!(
        "/data/local/tmp/flare-agent-{}.jar",
        sha256.get(..12).unwrap_or(sha256)
    )
}

/// The device-side command that starts the helper (run through `adb shell`). The path is Flare's own, made by
/// [`device_jar`] from a hex hash, so nothing else reaches the shell.
pub fn start_command(device_jar: &str) -> String {
    format!(
        "CLASSPATH={device_jar} exec app_process /system/bin --nice-name={PROCESS} {MAIN_CLASS} {SOCKET}"
    )
}

/// The device-side command that removes the jars of other builds, keeping `keep` (a path from [`device_jar`]).
pub fn sweep_command(keep: &str) -> String {
    format!(
        "for f in /data/local/tmp/flare-agent-*.jar; do [ \"$f\" = \"{keep}\" ] || rm -f \"$f\"; done; true"
    )
}

/// Rule 1369: the helper goes only to an emulator, never to a real phone in v1. An emulator's adb serial is
/// `emulator-<console port>` (the emu host takes it from the emulator's own discovery file).
pub fn serial_ok(serial: &str) -> bool {
    serial
        .strip_prefix("emulator-")
        .is_some_and(|p| !p.is_empty() && p.len() <= 5 && p.bytes().all(|b| b.is_ascii_digit()))
}

/// The port `adb forward tcp:0 ...` printed.
pub fn forward_port(out: &str) -> Option<u16> {
    out.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())?
        .parse()
        .ok()
        .filter(|p| *p > 0)
}

/// The helper's first stdout line, `ready <protocol> <jar sha256> <socket>`: its protocol, or why it did not start.
pub fn ready_line(line: &str) -> Result<u64, String> {
    let mut w = line.split_whitespace();
    match w.next() {
        Some("ready") => w
            .next()
            .and_then(|p| p.parse().ok())
            .ok_or_else(|| format!("the agent helper said {line:?}")),
        Some("taken") => Err("another agent helper holds the socket".to_string()),
        Some("no_ui") => Err(format!(
            "the agent helper got no UI automation: {}",
            w.collect::<Vec<_>>().join(" ")
        )),
        _ => Err(format!(
            "the agent helper said {:?}",
            line.chars().take(200).collect::<String>()
        )),
    }
}

/// A request line to the helper (one JSON object, no newline).
pub fn request(id: u64, op: &str, mut extra: Value) -> String {
    let mut m = serde_json::Map::new();
    m.insert("id".to_string(), Value::from(id));
    m.insert("op".to_string(), Value::from(op));
    if let Some(o) = extra.as_object_mut() {
        m.append(o);
    }
    Value::Object(m).to_string()
}

/// A line from the helper.
#[derive(Clone, Debug, PartialEq)]
pub enum Line {
    /// An answer: its result, or a [`crate::job::Fail`] code and why.
    Answer {
        id: u64,
        result: Result<Value, (String, String)>,
    },
    /// The count of window changes the helper has seen (sent at most ten times a second).
    Windows { count: u64 },
}

/// One line from the helper read back; None for anything else (logged by the caller).
pub fn read_line(line: &str) -> Option<Line> {
    let v: Value = serde_json::from_str(line.trim_end()).ok()?;
    if v.get("event").and_then(Value::as_str) == Some("windows") {
        return Some(Line::Windows {
            count: v.get("count")?.as_u64()?,
        });
    }
    let id = v.get("id")?.as_u64()?;
    let result = match v.get("ok") {
        Some(ok) => Ok(ok.clone()),
        None => Err((
            v.get("code")
                .and_then(Value::as_str)
                .unwrap_or("unavailable")
                .to_string(),
            v.get("err")
                .and_then(Value::as_str)
                .unwrap_or("the agent helper failed")
                .to_string(),
        )),
    };
    Some(Line::Answer { id, result })
}

/// The helper's `hello` answer.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Hello {
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub protocol: u64,
    #[serde(default)]
    pub jar_sha256: String,
    #[serde(default)]
    pub api: u32,
    #[serde(default)]
    pub release: String,
}

impl Hello {
    /// Whether this is the helper this build pushes (so it can be reused, say by a second Flare).
    pub fn is_ours(&self) -> bool {
        self.agent == "flare-agent" && self.protocol == PROTOCOL && self.jar_sha256 == jar_sha256()
    }
}

/// One node as the helper (or the dump) reports it. Bounds are screen pixels in the current rotation.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Node {
    pub class: String,
    pub text: String,
    pub desc: String,
    pub hint: String,
    pub res_id: String,
    /// `[left, top, right, bottom]`.
    pub b: [i64; 4],
    pub clickable: bool,
    pub long_clickable: bool,
    pub scrollable: bool,
    pub editable: bool,
    pub checkable: bool,
    pub checked: bool,
    pub focused: bool,
    pub selected: bool,
    pub password: bool,
    pub showing_hint: bool,
    pub disabled: bool,
    pub ch: Vec<Node>,
}

/// One window in the helper's tree.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Window {
    #[serde(rename = "type")]
    pub kind: i64,
    pub layer: i64,
    pub active: bool,
    pub package: String,
    pub root: Node,
}

/// The helper's `tree` answer.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Tree {
    pub windows: Vec<Window>,
    pub front_app: String,
    pub nodes: u64,
    pub capped: bool,
    /// Time in the device, microseconds.
    pub us: u64,
}

/// The longest label made by joining a row's texts.
const MERGED_MAX: usize = 200;

/// The tree model's node for the screen: an `AXApplication` the size of the screen (in dp) holding each window's
/// root, every frame in dp (`px / scale`, rule 1367). Roles come from [`android_role`]; a field's label is its hint
/// (its text is the value, never a password's); a tappable row with no label of its own takes its texts' (as
/// TalkBack reads it), and those texts are not listed again.
pub fn to_ui_tree(t: &Tree, scale: f64, screen_dp: Size) -> UiNode {
    let scale = if scale > 0.0 { scale } else { 1.0 };
    UiNode {
        role: "AXApplication".to_string(),
        label: String::new(),
        value: String::new(),
        frame: Rect {
            x: 0.0,
            y: 0.0,
            w: screen_dp.w,
            h: screen_dp.h,
        },
        enabled: true,
        children: t
            .windows
            .iter()
            .map(|w| {
                let mut n = node(&w.root, scale);
                merge_labels(&mut n);
                n
            })
            .collect(),
    }
}

fn node(n: &Node, scale: f64) -> UiNode {
    let role = android_role(AndroidNode {
        class: &n.class,
        clickable: n.clickable || n.long_clickable,
        editable: n.editable,
        checkable: n.checkable,
        scrollable: n.scrollable,
    });
    let text = if n.password || n.showing_hint {
        ""
    } else {
        n.text.as_str()
    };
    let first = |xs: &[&str]| {
        xs.iter()
            .find(|s| !s.trim().is_empty())
            .map(|s| s.to_string())
            .unwrap_or_default()
    };
    let id_name = n.res_id.rsplit('/').next().unwrap_or_default();
    let (label, value) = if role == "AXTextField" || role == "AXSearchField" {
        (first(&[&n.hint, &n.desc, id_name]), text.to_string())
    } else if n.checkable {
        (
            first(&[text, &n.desc]),
            if n.checked { "on" } else { "off" }.to_string(),
        )
    } else {
        (first(&[text, &n.desc]), String::new())
    };
    let [l, t, r, b] = n.b;
    let px = |v: i64| v as f64 / scale;
    UiNode {
        role: role.to_string(),
        label,
        value,
        frame: Rect {
            x: px(l),
            y: px(t),
            w: px((r - l).max(0)),
            h: px((b - t).max(0)),
        },
        enabled: !n.disabled,
        children: n.ch.iter().map(|c| node(c, scale)).collect(),
    }
}

/// Roles that act by themselves: a row's merge stops at them.
fn acts(role: &str) -> bool {
    matches!(
        role,
        "AXButton"
            | "AXTextField"
            | "AXSearchField"
            | "AXSwitch"
            | "AXCheckBox"
            | "AXSlider"
            | "AXIncrementor"
            | "AXPopUpButton"
            | "AXTab"
    )
}

fn merge_labels(n: &mut UiNode) {
    if n.role == "AXButton" && n.label.trim().is_empty() {
        let mut texts = Vec::new();
        for c in &mut n.children {
            take_texts(c, &mut texts);
        }
        let mut label = texts.join(", ");
        if label.chars().count() > MERGED_MAX {
            label = label.chars().take(MERGED_MAX).collect();
        }
        n.label = label;
    }
    for c in &mut n.children {
        merge_labels(c);
    }
}

/// Moves the labels of a row's plain texts into `out` (they are said by the row now), stopping at anything that acts.
fn take_texts(n: &mut UiNode, out: &mut Vec<String>) {
    if acts(&n.role) {
        return;
    }
    let l = std::mem::take(&mut n.label);
    if !l.trim().is_empty() {
        out.push(l.trim().to_string());
    }
    for c in &mut n.children {
        take_texts(c, out);
    }
}

/// `uiautomator dump`'s XML (the slow fallback, about 2 s) as a [`Tree`] of one window. None when it does not read.
/// roxmltree's default options refuse a DOCTYPE, so no entity is expanded.
pub fn parse_dump(xml: &str) -> Option<Tree> {
    let start = xml.find("<?xml").or_else(|| xml.find("<hierarchy"))?;
    let end = xml.rfind("</hierarchy>")? + "</hierarchy>".len();
    let doc = roxmltree::Document::parse(xml.get(start..end)?).ok()?;
    let h = doc.root_element();
    if h.tag_name().name() != "hierarchy" {
        return None;
    }
    let mut count = 0u64;
    let windows: Vec<Window> = h
        .children()
        .filter(|c| c.has_tag_name("node"))
        .map(|c| {
            let root = dump_node(c, &mut count, 0);
            Window {
                kind: 1,
                layer: 0,
                active: true,
                package: c.attribute("package").unwrap_or_default().to_string(),
                root,
            }
        })
        .collect();
    let front_app = windows
        .first()
        .map(|w| w.package.clone())
        .unwrap_or_default();
    Some(Tree {
        windows,
        front_app,
        nodes: count,
        capped: false,
        us: 0,
    })
}

/// `[l,t][r,b]` in a dump's `bounds`.
pub fn dump_bounds(s: &str) -> Option<[i64; 4]> {
    let nums: Vec<i64> = s
        .split(['[', ']', ','])
        .filter(|p| !p.is_empty())
        .map(|p| p.trim().parse().ok())
        .collect::<Option<_>>()?;
    <[i64; 4]>::try_from(nums).ok()
}

/// The deepest a dump is read.
const DUMP_DEPTH: usize = 80;

fn dump_node(e: roxmltree::Node<'_, '_>, count: &mut u64, depth: usize) -> Node {
    *count += 1;
    let a = |k: &str| e.attribute(k).unwrap_or_default().to_string();
    let yes = |k: &str| e.attribute(k) == Some("true");
    let class = a("class");
    let short = class.rsplit('.').next().unwrap_or_default();
    Node {
        editable: short.ends_with("EditText") || short.ends_with("AutoCompleteTextView"),
        text: a("text"),
        desc: a("content-desc"),
        hint: a("hint"),
        res_id: a("resource-id"),
        b: dump_bounds(&a("bounds")).unwrap_or_default(),
        clickable: yes("clickable"),
        long_clickable: yes("long-clickable"),
        scrollable: yes("scrollable"),
        checkable: yes("checkable"),
        checked: yes("checked"),
        focused: yes("focused"),
        selected: yes("selected"),
        password: yes("password"),
        showing_hint: false,
        disabled: e.attribute("enabled") == Some("false"),
        ch: if depth < DUMP_DEPTH {
            e.children()
                .filter(|c| c.has_tag_name("node"))
                .map(|c| dump_node(c, count, depth + 1))
                .collect()
        } else {
            Vec::new()
        },
        class,
    }
}

/// The hash of the helper's sources as `build.py` takes it: for each file under `tools/android-agent/src`, and
/// `build.py` itself, in path order: its path (with `/`), a newline, its bytes without CR, a newline. CR is dropped
/// so a Windows checkout hashes like a Mac one.
#[cfg(test)]
fn source_sha256(agent_dir: &std::path::Path) -> String {
    use sha2::{Digest, Sha256};
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap().map(Result::unwrap) {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else {
                out.push(p);
            }
        }
    }
    let mut files = Vec::new();
    walk(&agent_dir.join("src"), &mut files);
    files.push(agent_dir.join("build.py"));
    let mut named: Vec<(String, std::path::PathBuf)> = files
        .into_iter()
        .map(|p| {
            let rel = p
                .strip_prefix(agent_dir)
                .unwrap()
                .components()
                .map(|c| c.as_os_str().to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join("/");
            (rel, p)
        })
        .collect();
    named.sort();
    let mut h = Sha256::new();
    for (rel, p) in named {
        h.update(rel.as_bytes());
        h.update(b"\n");
        let bytes: Vec<u8> = std::fs::read(&p)
            .unwrap()
            .into_iter()
            .filter(|b| *b != b'\r')
            .collect();
        h.update(&bytes);
        h.update(b"\n");
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::filter_tree;
    use std::path::Path;

    fn repo() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    /// Rule 1369: the checked-in jar is the one its sources build (run `python tools/android-agent/build.py` after
    /// changing them). CI needs no Java for this.
    #[test]
    fn the_checked_in_jar_is_built_from_its_sources() {
        use sha2::{Digest, Sha256};
        let jar = std::fs::read(repo().join("crates/flare-device/assets/flare-agent.jar")).unwrap();
        let hex: String = Sha256::digest(&jar)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(jar_sha256().len(), 64);
        assert_eq!(
            hex,
            jar_sha256(),
            "the jar is not the one flare-agent.jar.sha256 names: run python tools/android-agent/build.py"
        );
        assert_eq!(
            source_sha256(&repo().join("tools/android-agent")),
            recorded_source_sha256(),
            "the helper's sources changed since the jar was built: run python tools/android-agent/build.py"
        );
        // A dex jar: a zip holding classes.dex.
        assert!(jar.starts_with(b"PK"));
        assert!(jar.windows(11).any(|w| w == b"classes.dex"));
    }

    /// The Rust side and the Java side agree on the protocol, the socket and the process name.
    #[test]
    fn the_helpers_constants_match_its_source() {
        let java = std::fs::read_to_string(
            repo().join("tools/android-agent/src/dev/flare/agent/Agent.java"),
        )
        .unwrap();
        assert!(java.contains(&format!("static final int PROTOCOL = {PROTOCOL};")));
        assert!(java.contains(&format!("static final String SOCKET = \"{SOCKET}\";")));
        assert!(
            java.contains("Looper.prepareMainLooper()"),
            "S4: needed on API 37"
        );
        assert!(
            java.contains("getUid() != 2000 && c.getUid() != 0"),
            "only adb's forward may talk to it"
        );
    }

    /// Rule 1369: pushed only to an emulator, by a serial from its discovery file; the start command holds only
    /// Flare's own path.
    #[test]
    fn the_helper_goes_only_to_an_emulator() {
        for ok in ["emulator-5554", "emulator-5584", "emulator-65534"] {
            assert!(serial_ok(ok), "{ok}");
        }
        for bad in [
            "",
            "emulator-",
            "R58M12ABCDE",
            "192.168.1.5:5555",
            "emulator-5554;reboot",
            "emulator-5554 ",
            "emulator-123456",
            "localhost:5555",
        ] {
            assert!(!serial_ok(bad), "{bad}");
        }
        let jar = device_jar(jar_sha256());
        assert!(jar.starts_with("/data/local/tmp/flare-agent-"));
        assert_eq!(jar.len(), "/data/local/tmp/flare-agent-.jar".len() + 12);
        assert_eq!(
            start_command(&jar),
            format!(
                "CLASSPATH={jar} exec app_process /system/bin --nice-name=flare-agent dev.flare.agent.Agent flare-agent"
            )
        );
    }

    #[test]
    fn the_helpers_lines_read_back() {
        assert_eq!(forward_port("40123\n"), Some(40123));
        assert_eq!(forward_port("\n"), None);
        assert_eq!(forward_port("error: closed"), None);
        assert_eq!(
            ready_line("ready 1 3e92a2b3 flare-agent"),
            Ok(1),
            "protocol 1"
        );
        assert!(ready_line("taken flare-agent").is_err());
        assert!(
            ready_line("no_ui IllegalStateException: already registered")
                .unwrap_err()
                .contains("already registered")
        );
        assert!(ready_line("Aborted").is_err());
        assert_eq!(
            request(3, "set_text", serde_json::json!({"text": "bl\u{e5}"})),
            r#"{"id":3,"op":"set_text","text":"blå"}"#
        );
        assert_eq!(request(1, "tree", Value::Null), r#"{"id":1,"op":"tree"}"#);
        assert_eq!(
            read_line(r#"{"event":"windows","count":7}"#),
            Some(Line::Windows { count: 7 })
        );
        assert_eq!(
            read_line(r#"{"id":2,"ok":{"cursor":true}}"#),
            Some(Line::Answer {
                id: 2,
                result: Ok(serde_json::json!({"cursor": true}))
            })
        );
        assert_eq!(
            read_line(r#"{"id":4,"code":"not_found","err":"no text field has the focus"}"#),
            Some(Line::Answer {
                id: 4,
                result: Err((
                    "not_found".to_string(),
                    "no text field has the focus".to_string()
                ))
            })
        );
        assert_eq!(read_line("not json"), None);
        assert_eq!(read_line(r#"{"ok":1}"#), None, "no id");
        let h: Hello = serde_json::from_str(&format!(
            r#"{{"agent":"flare-agent","protocol":1,"jar_sha256":"{}","api":37,"release":"17","pid":1234}}"#,
            jar_sha256()
        ))
        .unwrap();
        assert!(h.is_ours());
        assert!(
            !Hello {
                jar_sha256: "0".repeat(64),
                ..h.clone()
            }
            .is_ours(),
            "another build's helper is stopped, not reused"
        );
    }

    /// A Settings-like screen at 420 dpi (scale 2.625) as the helper sends it.
    const SETTINGS: &str = r#"{"windows":[
      {"type":1,"layer":0,"active":true,"focused":true,"package":"com.android.settings","b":[0,0,1080,2400],
       "root":{"class":"android.widget.FrameLayout","b":[0,0,1080,2400],"ch":[
         {"class":"android.widget.TextView","text":"Settings","b":[42,200,500,300]},
         {"class":"android.widget.EditText","hint":"Search settings","text":"Search settings","showing_hint":true,
          "editable":true,"clickable":true,"focused":true,"b":[42,320,1038,446]},
         {"class":"android.widget.EditText","text":"hunter2","password":true,"editable":true,
          "res_id":"com.x:id/pin","b":[42,460,1038,560]},
         {"class":"androidx.recyclerview.widget.RecyclerView","scrollable":true,"b":[0,600,1080,2400],"ch":[
           {"class":"android.widget.LinearLayout","clickable":true,"b":[0,600,1080,789],"ch":[
             {"class":"android.widget.ImageView","b":[42,650,105,713]},
             {"class":"android.widget.TextView","text":"Network & internet","b":[147,630,700,690]},
             {"class":"android.widget.TextView","text":"Mobile, Wi-Fi, hotspot","b":[147,690,800,740]}
           ]},
           {"class":"android.widget.LinearLayout","clickable":true,"b":[0,789,1080,978],"ch":[
             {"class":"android.widget.TextView","text":"Dark theme","b":[147,820,700,880]},
             {"class":"android.widget.Switch","checkable":true,"checked":true,"b":[900,830,1038,900]}
           ]},
           {"class":"android.widget.Button","text":"OK","disabled":true,"b":[700,2200,1038,2326]}
         ]}
       ]}},
      {"type":3,"layer":1,"package":"com.android.systemui","b":[0,0,1080,118],
       "root":{"class":"android.widget.FrameLayout","b":[0,0,1080,118],"ch":[
         {"class":"android.widget.TextView","text":"12:30","b":[60,30,180,90]}]}}
    ],"front_app":"com.android.settings","nodes":14,"us":2100}"#;

    #[test]
    fn the_helpers_tree_becomes_the_tree_model_in_dp() {
        let t: Tree = serde_json::from_str(SETTINGS).unwrap();
        assert_eq!(t.front_app, "com.android.settings");
        assert_eq!(t.windows.len(), 2);
        let screen = Size::new(1080.0 / 2.625, 2400.0 / 2.625);
        let root = to_ui_tree(&t, 2.625, screen);
        assert_eq!(root.role, "AXApplication");
        assert_eq!(root.children.len(), 2);
        let l = filter_tree(&root, screen);
        let seen: Vec<(&str, &str, &str)> = l
            .refs
            .iter()
            .map(|r| (r.role.as_str(), r.label.as_str(), r.value.as_str()))
            .collect();
        assert_eq!(
            seen,
            vec![
                ("AXStaticText", "12:30", ""),
                ("AXStaticText", "Settings", ""),
                // The hint is the label; a field showing its hint has no value.
                ("AXTextField", "Search settings", ""),
                // A password's text is never listed; its id names it.
                ("AXTextField", "pin", ""),
                // A tappable row says its texts, which are not listed again.
                ("AXButton", "Network & internet, Mobile, Wi-Fi, hotspot", ""),
                ("AXButton", "Dark theme", ""),
                ("AXSwitch", "", "on"),
                ("AXButton", "OK", ""),
            ]
        );
        let ok = l.refs.iter().find(|r| r.label == "OK").unwrap();
        assert!(!ok.enabled);
        // Frames in dp: 700 px at 2.625 is 266.67 dp.
        assert!((ok.frame.x - 700.0 / 2.625).abs() < 1e-9);
        assert!((ok.frame.w - 338.0 / 2.625).abs() < 1e-9);
        let search = &l.refs[2];
        assert!((search.frame.y - 320.0 / 2.625).abs() < 1e-9);
    }

    /// The real thing: Settings on API 37 (a temporary emulator, 2026-11-22), from the helper and from
    /// `uiautomator dump`. Both list the same rows.
    #[test]
    fn a_real_settings_screen_reads_the_same_both_ways() {
        let screen = Size::new(1080.0 / 2.625, 2400.0 / 2.625);
        let t: Tree =
            serde_json::from_str(include_str!("fixtures/agent-tree-settings.json")).unwrap();
        assert_eq!(t.front_app, "com.android.settings");
        assert!(t.nodes > 100, "{}", t.nodes);
        let helper = filter_tree(&to_ui_tree(&t, 2.625, screen), screen);
        let d = parse_dump(include_str!("fixtures/uiautomator-dump-settings.xml")).unwrap();
        let dump = filter_tree(&to_ui_tree(&d, 2.625, screen), screen);
        for l in [&helper, &dump] {
            let labels: Vec<&str> = l.refs.iter().map(|r| r.label.as_str()).collect();
            assert!(
                labels.iter().any(|x| x.starts_with("Network & internet, ")),
                "{labels:?}"
            );
            assert!(
                labels.contains(&"Search Settings") || labels.iter().any(|x| x.contains("Search"))
            );
            // Every frame is on the screen, in dp.
            for r in &l.refs {
                assert!(
                    r.frame.x >= 0.0 && r.frame.x + r.frame.w <= screen.w + 0.5,
                    "{r:?}"
                );
                assert!(r.frame.w > 0.0 && r.frame.h > 0.0, "{r:?}");
            }
        }
        // The helper also sees the status bar's window; the dump only the app's.
        assert!(helper.refs.len() >= dump.refs.len());
    }

    const DUMP: &str = r#"<?xml version='1.0' encoding='UTF-8' standalone='yes' ?><hierarchy rotation="0"><node index="0" text="" resource-id="" class="android.widget.FrameLayout" package="com.android.settings" content-desc="" checkable="false" checked="false" clickable="false" enabled="true" focusable="false" focused="false" scrollable="false" long-clickable="false" password="false" selected="false" bounds="[0,0][1080,2400]"><node index="0" text="Network &amp; internet" resource-id="android:id/title" class="android.widget.TextView" package="com.android.settings" content-desc="" checkable="false" checked="false" clickable="false" enabled="true" focusable="false" focused="false" scrollable="false" long-clickable="false" password="false" selected="false" bounds="[147,630][700,690]" /><node index="1" text="" resource-id="com.x:id/q" class="android.widget.EditText" package="com.android.settings" content-desc="Search" checkable="false" checked="false" clickable="true" enabled="true" focusable="true" focused="true" scrollable="false" long-clickable="true" password="false" selected="false" bounds="[42,320][1038,446]" /><node index="2" text="" resource-id="" class="android.widget.ImageButton" package="com.android.settings" content-desc="Navigate up" checkable="false" checked="false" clickable="true" enabled="false" focusable="true" focused="false" scrollable="false" long-clickable="false" password="false" selected="false" bounds="[0,118][147,265]" /></node></hierarchy>UI hierchary dumped to: /dev/tty"#;

    #[test]
    fn a_uiautomator_dump_reads_as_the_same_tree() {
        let t = parse_dump(DUMP).expect("reads");
        assert_eq!(t.front_app, "com.android.settings");
        assert_eq!(t.nodes, 4);
        let screen = Size::new(1080.0 / 2.625, 2400.0 / 2.625);
        let l = filter_tree(&to_ui_tree(&t, 2.625, screen), screen);
        let seen: Vec<(&str, &str, bool)> = l
            .refs
            .iter()
            .map(|r| (r.role.as_str(), r.label.as_str(), r.enabled))
            .collect();
        assert_eq!(
            seen,
            vec![
                ("AXButton", "Navigate up", false),
                ("AXTextField", "Search", true),
                ("AXStaticText", "Network & internet", true),
            ]
        );
        assert_eq!(dump_bounds("[0,118][147,265]"), Some([0, 118, 147, 265]));
        assert_eq!(dump_bounds("[0,118][147]"), None);
        assert_eq!(dump_bounds("junk"), None);
        assert_eq!(parse_dump("ERROR: could not get idle state."), None);
        // A DOCTYPE (and so any entity) is refused.
        assert_eq!(
            parse_dump(
                r#"<?xml version="1.0"?><!DOCTYPE h [<!ENTITY x "y">]><hierarchy><node text="&x;"/></hierarchy>"#
            ),
            None
        );
    }
}

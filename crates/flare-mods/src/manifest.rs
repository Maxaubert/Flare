//! The manifest: the strict parse and validation of `mod.json` (protocol 1, `docs/surfaces/mods.md`, "The
//! manifest") and of `plugin.json` (protocol 2, the plugins design, sections 3 and 13).
//!
//! The file is read as one object. Unknown keys are refused at every level, so a typo (`permision`)
//! is an error and never a silently missing permission. Every refusal is a human-readable sentence that
//! names the field, for the Settings > Plugins row and for the author. Nothing here touches the disk:
//! the folder checks (the entry files, the helper, the folder name, links) are in `discover`.
//!
//! Protocol 2 keeps every protocol 1 rule and adds `author`, `homepage`, `background`, `bar`, `contributes`
//! and `helper`, and the plugin permissions. `mod.json` must say protocol 1 and `plugin.json` protocol 2, so a
//! protocol 1 mod reads exactly as it always did. The runtime of `background`, `bar`, `contributes` and
//! `helper` comes in later phases; their keys are checked now, so a manifest valid today stays valid.

use flare_ctl::Permission;
use flare_ctl::decor::{is_badge_icon, valid_key};
use serde_json::{Map, Value};

/// The protocol of `mod.json`.
pub const PROTOCOL_V1: u64 = 1;
/// The protocol of `plugin.json`.
pub const PROTOCOL_V2: u64 = 2;
/// The manifest file's name inside a protocol 1 mod folder.
pub const MANIFEST_FILE: &str = "mod.json";
/// The manifest file's name inside a protocol 2 plugin folder.
pub const PLUGIN_FILE: &str = "plugin.json";
/// The largest manifest read (bytes).
pub const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
pub const MIN_ID_LEN: usize = 3;
pub const MAX_ID_LEN: usize = 40;
pub const MAX_NAME_CHARS: usize = 80;
pub const MAX_TITLE_CHARS: usize = 80;
pub const MAX_DESCRIPTION_CHARS: usize = 500;
/// Tools one mod may declare.
pub const MAX_TOOLS: usize = 32;
pub const MAX_TOOL_NAME_LEN: usize = 40;
/// The serialised size of one tool's `input_schema`.
pub const MAX_SCHEMA_BYTES: usize = 8 * 1024;
/// The longest `author`, in characters (the rule of `name`).
pub const MAX_AUTHOR_CHARS: usize = MAX_NAME_CHARS;
/// The longest `homepage`, in characters.
pub const MAX_HOMEPAGE_CHARS: usize = 200;
/// The height of a plugin's bar under the terminal, in logical pixels.
pub const MIN_BAR_HEIGHT: u64 = 24;
pub const MAX_BAR_HEIGHT: u64 = 160;
/// Sidebar sections one plugin may declare.
pub const MAX_SIDEBAR: usize = 2;
/// Actions one plugin may declare.
pub const MAX_ACTIONS: usize = 8;
/// Named icons one plugin may declare, and the longest path of one (bytes).
pub const MAX_ICONS: usize = 32;
pub const MAX_ICON_PATH_BYTES: usize = 2 * 1024;
/// The most path commands one icon may have, and how far from 0 any of its numbers may be (the box is 24 units).
pub const MAX_ICON_COMMANDS: usize = 256;
pub const MAX_ICON_COORD: f64 = 1000.0;
/// Arguments of the helper program, and the longest one (characters).
pub const MAX_HELPER_ARGS: usize = 16;
pub const MAX_HELPER_ARG_CHARS: usize = 200;

/// The permissions a protocol 1 manifest may ask for (the list stays as it was). `eval` is the user's own switch
/// for `browser.eval` (S4), so a mod can never ask for it.
pub const ASKABLE_V1: [Permission; 5] = [
    Permission::Panel,
    Permission::View,
    Permission::Browser,
    Permission::Convo,
    Permission::Tools,
];

/// The permissions a protocol 2 manifest may ask for: the protocol 1 list and the plugin ones. Never `eval`.
pub const ASKABLE_V2: [Permission; 16] = [
    Permission::Panel,
    Permission::View,
    Permission::Browser,
    Permission::Convo,
    Permission::Tools,
    Permission::Tabs,
    Permission::Decorate,
    Permission::Sidebar,
    Permission::Actions,
    Permission::Bar,
    Permission::Sessions,
    Permission::Input,
    Permission::Network,
    Permission::Process,
    Permission::AgentLooks,
    Permission::Fill,
];

const TOP_KEYS_V1: [&str; 8] = [
    "id",
    "name",
    "version",
    "protocol",
    "description",
    "permissions",
    "panel",
    "tools",
];
/// The keys protocol 2 adds to the top level.
const TOP_KEYS_V2_ONLY: [&str; 6] = [
    "author",
    "homepage",
    "background",
    "bar",
    "contributes",
    "helper",
];
const TOP_KEYS_V2: [&str; 14] = [
    "id",
    "name",
    "version",
    "protocol",
    "description",
    "author",
    "homepage",
    "permissions",
    "background",
    "panel",
    "bar",
    "contributes",
    "helper",
    "tools",
];
const PANEL_KEYS: [&str; 2] = ["entry", "title"];
const TOOL_KEYS: [&str; 3] = ["name", "description", "input_schema"];
const BAR_KEYS: [&str; 2] = ["entry", "height"];
const CONTRIBUTES_KEYS: [&str; 3] = ["sidebar", "actions", "icons"];
const SIDEBAR_KEYS: [&str; 2] = ["key", "title"];
const ACTION_KEYS: [&str; 4] = ["key", "title", "icon", "where"];
const HELPER_KEYS: [&str; 2] = ["command", "args"];

/// Device names Windows reserves; a mod id cannot be a folder named one of them.
const RESERVED_IDS: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Which manifest file is read: it fixes the protocol, the keys and the permissions a manifest may use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `mod.json`, protocol 1.
    Mod,
    /// `plugin.json`, protocol 2.
    Plugin,
}

impl Kind {
    pub fn file(self) -> &'static str {
        match self {
            Kind::Mod => MANIFEST_FILE,
            Kind::Plugin => PLUGIN_FILE,
        }
    }

    pub fn protocol(self) -> u64 {
        match self {
            Kind::Mod => PROTOCOL_V1,
            Kind::Plugin => PROTOCOL_V2,
        }
    }

    /// The permissions this manifest may ask for.
    pub fn askable(self) -> &'static [Permission] {
        match self {
            Kind::Mod => &ASKABLE_V1,
            Kind::Plugin => &ASKABLE_V2,
        }
    }

    /// The word the refusals use for the thing the manifest describes.
    fn noun(self) -> &'static str {
        match self {
            Kind::Mod => "mod",
            Kind::Plugin => "plugin",
        }
    }

    fn top_keys(self) -> &'static [&'static str] {
        match self {
            Kind::Mod => &TOP_KEYS_V1,
            Kind::Plugin => &TOP_KEYS_V2,
        }
    }
}

/// A validated manifest.
#[derive(Clone, Debug, PartialEq)]
pub struct Manifest {
    /// 1 for `mod.json`, 2 for `plugin.json`.
    pub protocol: u64,
    pub id: String,
    pub name: String,
    /// A semver string, kept as written.
    pub version: String,
    pub description: String,
    /// In the order the manifest lists them, without duplicates.
    pub permissions: Vec<Permission>,
    pub panel: Option<Panel>,
    pub tools: Vec<ToolDef>,
    /// Protocol 2: who made it, shown in Settings.
    pub author: Option<String>,
    /// Protocol 2: an `https://` address, shown in Settings.
    pub homepage: Option<String>,
    /// Protocol 2: the page that runs while the plugin is on, in a hidden view.
    pub background: Option<String>,
    /// Protocol 2: the strip under a tab's terminal.
    pub bar: Option<Bar>,
    /// Protocol 2: sidebar sections, actions and named icons. Empty for a mod.
    pub contributes: Contributes,
    /// Protocol 2: a program inside the plugin folder Flare starts while the plugin is on.
    pub helper: Option<Helper>,
}

impl Manifest {
    /// A protocol 2 plugin (`plugin.json`), not a protocol 1 mod.
    pub fn is_plugin(&self) -> bool {
        self.protocol == PROTOCOL_V2
    }

    /// Which manifest file it came from.
    pub fn kind(&self) -> Kind {
        if self.is_plugin() {
            Kind::Plugin
        } else {
            Kind::Mod
        }
    }
}

/// How two `version` texts compare (`MAJOR.MINOR.PATCH`, then a pre-release below its release); build metadata does
/// not count. A text that does not read as a version compares equal, so it never blocks an install.
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    fn split(v: &str) -> Option<([u64; 3], Option<&str>)> {
        let core = v.split('+').next()?;
        let (nums, pre) = match core.split_once('-') {
            Some((n, p)) => (n, Some(p)),
            None => (core, None),
        };
        let mut out = [0u64; 3];
        let mut parts = nums.split('.');
        for slot in &mut out {
            *slot = parts.next()?.parse().ok()?;
        }
        parts.next().is_none().then_some((out, pre))
    }
    let (Some((na, pa)), Some((nb, pb))) = (split(a), split(b)) else {
        return Ordering::Equal;
    };
    na.cmp(&nb).then_with(|| match (pa, pb) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(x), Some(y)) => {
            let (mut xs, mut ys) = (x.split('.'), y.split('.'));
            loop {
                match (xs.next(), ys.next()) {
                    (None, None) => break Ordering::Equal,
                    (None, Some(_)) => break Ordering::Less,
                    (Some(_), None) => break Ordering::Greater,
                    (Some(p), Some(q)) => {
                        let o = match (p.parse::<u64>(), q.parse::<u64>()) {
                            (Ok(m), Ok(n)) => m.cmp(&n),
                            (Ok(_), Err(_)) => Ordering::Less,
                            (Err(_), Ok(_)) => Ordering::Greater,
                            (Err(_), Err(_)) => p.cmp(q),
                        };
                        if o != Ordering::Equal {
                            break o;
                        }
                    }
                }
            }
        }
    })
}

/// The mod's panel: an HTML file inside the mod folder, shown under `title`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Panel {
    /// A relative path with `/` or `\`, checked by `paths::check_relative`.
    pub entry: String,
    pub title: String,
}

/// A tool the agent may call; the mod answers it through the channel.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolDef {
    /// `a-z`, `0-9` and `_`, 1 to 40 characters.
    pub name: String,
    pub description: String,
    /// A JSON Schema object, at most 8 KB serialised.
    pub input_schema: Value,
}

/// The plugin's bar: an HTML file shown under a tab's terminal, `height` logical pixels tall.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bar {
    pub entry: String,
    /// 24 to 160.
    pub height: u32,
}

/// What a plugin adds to Flare's own surfaces, as data Flare draws.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Contributes {
    pub sidebar: Vec<SidebarDef>,
    pub actions: Vec<ActionDef>,
    /// In the order the manifest lists them.
    pub icons: Vec<IconDef>,
}

/// A sidebar section the plugin fills with `sidebar.set`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SidebarDef {
    /// A decor key: 1 to 24 of `a-z`, `0-9` and `-`.
    pub key: String,
    pub title: String,
}

/// A button or menu item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionDef {
    pub key: String,
    pub title: String,
    /// A built-in badge icon or one of the plugin's `contributes.icons`.
    pub icon: Option<String>,
    /// Where it shows (`where` in the manifest), at least one, each once.
    pub places: Vec<Place>,
}

/// Where an action shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// The tab's right-click menu.
    TabMenu,
    /// A small button on the tab while it is hovered (one per plugin).
    TabHover,
    /// A button left of the window buttons (one per plugin).
    TopBar,
    /// Inside the plugin's own bar (needs a `bar`).
    SessionBar,
}

impl Place {
    pub const ALL: [Place; 4] = [
        Place::TabMenu,
        Place::TabHover,
        Place::TopBar,
        Place::SessionBar,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Place::TabMenu => "tab-menu",
            Place::TabHover => "tab-hover",
            Place::TopBar => "top-bar",
            Place::SessionBar => "session-bar",
        }
    }

    pub fn parse(name: &str) -> Option<Place> {
        Place::ALL.into_iter().find(|p| p.name() == name)
    }
}

/// A named SVG path on the 24-unit box, for the plugin's badges and actions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IconDef {
    pub name: String,
    pub path: String,
}

/// The plugin's helper program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Helper {
    /// An `.exe` inside the plugin folder, checked by `paths::check_program`.
    pub command: String,
    pub args: Vec<String>,
}

/// Whether `id` is a legal mod id: 3 to 40 characters of `a-z`, `0-9` and `-`, not a Windows device name.
pub fn valid_id(id: &str) -> bool {
    id_problem(id).is_none()
}

fn id_problem(id: &str) -> Option<String> {
    let n = id.chars().count();
    if !(MIN_ID_LEN..=MAX_ID_LEN).contains(&n)
        || !id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Some(format!(
            "id must be {MIN_ID_LEN} to {MAX_ID_LEN} characters: lower-case a-z, digits 0-9 and '-'"
        ));
    }
    if RESERVED_IDS.contains(&id) {
        return Some(format!(
            "id \"{id}\" is a Windows device name and cannot be a folder"
        ));
    }
    None
}

/// Whether `name` is a legal tool name: 1 to 40 characters of `a-z`, `0-9` and `_`.
pub fn valid_tool_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_TOOL_NAME_LEN
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// Parses and validates the text of a `mod.json` (protocol 1). The reason is one human-readable sentence.
pub fn parse(text: &str) -> Result<Manifest, String> {
    parse_as(Kind::Mod, text)
}

/// Parses and validates the text of a `plugin.json` (protocol 2).
pub fn parse_plugin(text: &str) -> Result<Manifest, String> {
    parse_as(Kind::Plugin, text)
}

/// Parses and validates the text of the manifest file `kind` names.
pub fn parse_as(kind: Kind, text: &str) -> Result<Manifest, String> {
    parse_inner(kind, text).map_err(|e| match kind {
        // The shared field helpers name the protocol 1 file when a top-level key is missing.
        Kind::Plugin => match e.strip_prefix("mod.json is missing") {
            Some(rest) => format!("{PLUGIN_FILE} is missing{rest}"),
            None => e,
        },
        Kind::Mod => e,
    })
}

fn parse_inner(kind: Kind, text: &str) -> Result<Manifest, String> {
    let file = kind.file();
    if text.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(format!(
            "{file} is larger than {} KB",
            MAX_MANIFEST_BYTES / 1024
        ));
    }
    let value: Value =
        serde_json::from_str(text).map_err(|e| format!("{file} is not valid JSON: {e}"))?;
    let obj = value
        .as_object()
        .ok_or_else(|| format!("{file} must be a JSON object"))?;
    if kind == Kind::Mod
        && let Some(key) = obj.keys().find(|k| TOP_KEYS_V2_ONLY.contains(&k.as_str()))
    {
        return Err(format!(
            "\"{key}\" is a protocol 2 key; a plugin that uses it names its manifest {PLUGIN_FILE} and says protocol 2"
        ));
    }
    no_unknown(obj, kind.top_keys(), file)?;

    let id = text_of(obj, "id", "id")?;
    if let Some(problem) = id_problem(&id) {
        return Err(problem);
    }
    let name = clean_text(obj, "name", "name", MAX_NAME_CHARS)?;
    let version = text_of(obj, "version", "version")?;
    if !valid_semver(&version) {
        return Err(format!(
            "version \"{version}\" is not a semantic version such as 1.0.0"
        ));
    }
    protocol(kind, obj)?;
    let description = clean_text(obj, "description", "description", MAX_DESCRIPTION_CHARS)?;
    let permissions = permissions(kind, obj)?;
    let panel = panel(kind, obj, &permissions)?;
    let tools = tools(kind, obj)?;
    if !tools.is_empty() && !permissions.contains(&Permission::Tools) {
        return Err("tools need the \"tools\" permission; add it to permissions".into());
    }
    let (author, homepage, background, bar, contributes, helper) = match kind {
        Kind::Mod => (None, None, None, None, Contributes::default(), None),
        Kind::Plugin => (
            optional_clean(obj, "author", MAX_AUTHOR_CHARS)?,
            homepage(obj)?,
            background(obj)?,
            bar(obj, &permissions)?,
            contributes(obj, &permissions)?,
            helper(obj, &permissions)?,
        ),
    };
    // A tool is answered by the panel's page (or, for a plugin, its background page or its helper): with none of
    // them nothing could ever answer it.
    if !tools.is_empty() && panel.is_none() && background.is_none() && helper.is_none() {
        return Err(match kind {
            Kind::Mod => "tools need a panel: its page answers them; add a panel".into(),
            Kind::Plugin => {
                "tools need a panel, a background page or a helper to answer them; add one".into()
            }
        });
    }
    if contributes
        .actions
        .iter()
        .any(|a| a.places.contains(&Place::SessionBar))
        && bar.is_none()
    {
        return Err("an action placed in \"session-bar\" needs a bar; add a bar".into());
    }

    Ok(Manifest {
        protocol: kind.protocol(),
        id,
        name,
        version,
        description,
        permissions,
        panel,
        tools,
        author,
        homepage,
        background,
        bar,
        contributes,
        helper,
    })
}

fn protocol(kind: Kind, obj: &Map<String, Value>) -> Result<(), String> {
    let v = required(obj, "protocol", "protocol")?;
    let want = kind.protocol();
    match v.as_u64() {
        Some(n) if n == want => Ok(()),
        Some(PROTOCOL_V2) if kind == Kind::Mod => Err(format!(
            "protocol must be the number 1 in {MANIFEST_FILE}; a protocol 2 plugin names its manifest {PLUGIN_FILE}"
        )),
        Some(PROTOCOL_V1) if kind == Kind::Plugin => Err(format!(
            "protocol must be the number 2 in {PLUGIN_FILE}; a protocol 1 mod names its manifest {MANIFEST_FILE}"
        )),
        _ => Err(format!(
            "protocol must be the number {want}; this Flare reads protocol 1 in {MANIFEST_FILE} and protocol 2 in {PLUGIN_FILE}"
        )),
    }
}

fn permissions(kind: Kind, obj: &Map<String, Value>) -> Result<Vec<Permission>, String> {
    let list = required(obj, "permissions", "permissions")?
        .as_array()
        .ok_or_else(|| "permissions must be a list of names, such as [\"view\"]".to_string())?;
    let mut out: Vec<Permission> = Vec::new();
    for (i, v) in list.iter().enumerate() {
        let name = v
            .as_str()
            .ok_or_else(|| format!("permissions[{i}] must be a name, such as \"view\""))?;
        if name == "eval" {
            return Err(format!(
                "permissions[{i}]: \"eval\" is not a permission a {} can ask for; it is the user's own switch for browser.eval",
                kind.noun()
            ));
        }
        let p = Permission::parse(name)
            .filter(|p| kind.askable().contains(p))
            .ok_or_else(|| match kind {
                Kind::Mod if Permission::parse(name).is_some_and(|p| ASKABLE_V2.contains(&p)) => {
                    format!(
                        "permissions[{i}]: unknown permission \"{name}\" for protocol 1; a mod can ask for panel, view, browser, convo or tools, and \"{name}\" needs a protocol 2 {PLUGIN_FILE}"
                    )
                }
                Kind::Mod => format!(
                    "permissions[{i}]: unknown permission \"{name}\"; a mod can ask for panel, view, browser, convo or tools"
                ),
                Kind::Plugin => format!(
                    "permissions[{i}]: unknown permission \"{name}\"; a plugin can ask for {}",
                    names_list(&ASKABLE_V2)
                ),
            })?;
        if out.contains(&p) {
            return Err(format!("permissions[{i}]: \"{name}\" is listed twice"));
        }
        out.push(p);
    }
    Ok(out)
}

/// `a, b or c`.
fn names_list(perms: &[Permission]) -> String {
    let names: Vec<&str> = perms.iter().map(|p| p.name()).collect();
    match names.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} or {last}", rest.join(", ")),
        Some((last, _)) => (*last).to_string(),
        None => String::new(),
    }
}

fn panel(
    kind: Kind,
    obj: &Map<String, Value>,
    permissions: &[Permission],
) -> Result<Option<Panel>, String> {
    let Some(v) = obj.get("panel") else {
        if permissions.contains(&Permission::Panel) {
            return Err(
                "the \"panel\" permission needs a panel: add {\"entry\", \"title\"}".into(),
            );
        }
        return Ok(None);
    };
    let p = v
        .as_object()
        .ok_or_else(|| "panel must be an object with \"entry\" and \"title\"".to_string())?;
    no_unknown(p, &PANEL_KEYS, "panel")?;
    let entry = text_of(p, "entry", "panel.entry")?;
    crate::paths::check_relative(&entry).map_err(|e| format!("panel.entry {e}"))?;
    let title = clean_text(p, "title", "panel.title", MAX_TITLE_CHARS)?;
    if !permissions.contains(&Permission::Panel) {
        return Err(format!(
            "a {} with a panel needs the \"panel\" permission; add it to permissions",
            kind.noun()
        ));
    }
    Ok(Some(Panel { entry, title }))
}

fn tools(kind: Kind, obj: &Map<String, Value>) -> Result<Vec<ToolDef>, String> {
    let Some(v) = obj.get("tools") else {
        return Ok(Vec::new());
    };
    let list = v
        .as_array()
        .ok_or_else(|| "tools must be a list of tool objects".to_string())?;
    if list.len() > MAX_TOOLS {
        return Err(format!(
            "a {} can declare at most {MAX_TOOLS} tools",
            kind.noun()
        ));
    }
    let mut out: Vec<ToolDef> = Vec::new();
    for (i, t) in list.iter().enumerate() {
        let ctx = format!("tools[{i}]");
        let o = t
            .as_object()
            .ok_or_else(|| format!("{ctx} must be an object"))?;
        no_unknown(o, &TOOL_KEYS, &ctx)?;
        let name = text_of(o, "name", &format!("{ctx}.name"))?;
        if !valid_tool_name(&name) {
            return Err(format!(
                "{ctx}.name must be 1 to {MAX_TOOL_NAME_LEN} characters: a-z, 0-9 or _"
            ));
        }
        if out.iter().any(|t| t.name == name) {
            return Err(format!("{ctx}.name \"{name}\" is listed twice"));
        }
        let description = clean_text(
            o,
            "description",
            &format!("{ctx}.description"),
            MAX_DESCRIPTION_CHARS,
        )?;
        let schema = required(o, "input_schema", &format!("{ctx}.input_schema"))?;
        let schema_obj = schema
            .as_object()
            .ok_or_else(|| format!("{ctx}.input_schema must be a JSON Schema object"))?;
        // The MCP tool listing needs an object schema, so `type` is required, not just allowed.
        if schema_obj.get("type").and_then(Value::as_str) != Some("object") {
            return Err(format!("{ctx}.input_schema.type must be \"object\""));
        }
        let size = serde_json::to_string(schema)
            .map(|s| s.len())
            .unwrap_or(usize::MAX);
        if size > MAX_SCHEMA_BYTES {
            return Err(format!(
                "{ctx}.input_schema is {size} bytes; the limit is {MAX_SCHEMA_BYTES}"
            ));
        }
        out.push(ToolDef {
            name,
            description,
            input_schema: schema.clone(),
        });
    }
    Ok(out)
}

/// A text a person reads that may be left out: absent is `None`, present follows [`clean_text`].
fn optional_clean(
    obj: &Map<String, Value>,
    key: &str,
    max: usize,
) -> Result<Option<String>, String> {
    if !obj.contains_key(key) {
        return Ok(None);
    }
    clean_text(obj, key, key, max).map(Some)
}

/// `homepage`: an `https://` address with a host, no space, no user name, at most 200 characters.
fn homepage(obj: &Map<String, Value>) -> Result<Option<String>, String> {
    let Some(url) = optional_clean(obj, "homepage", MAX_HOMEPAGE_CHARS)? else {
        return Ok(None);
    };
    let Some(rest) = url.strip_prefix("https://") else {
        return Err("homepage must be an https:// address".into());
    };
    if url.chars().any(char::is_whitespace) {
        return Err("homepage must not contain a space".into());
    }
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if host.is_empty()
        || !host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':'))
    {
        return Err(
            "homepage must name a host after https://, such as https://example.com/".into(),
        );
    }
    Ok(Some(url))
}

/// `background`: the path of an `.html` page inside the plugin folder. It needs no permission by itself.
fn background(obj: &Map<String, Value>) -> Result<Option<String>, String> {
    if !obj.contains_key("background") {
        return Ok(None);
    }
    let entry = text_of(obj, "background", "background")?;
    crate::paths::check_relative(&entry).map_err(|e| format!("background {e}"))?;
    Ok(Some(entry))
}

/// `bar`: `{entry, height}`, and the `bar` permission goes with it.
fn bar(obj: &Map<String, Value>, permissions: &[Permission]) -> Result<Option<Bar>, String> {
    let Some(v) = obj.get("bar") else {
        if permissions.contains(&Permission::Bar) {
            return Err("the \"bar\" permission needs a bar: add {\"entry\", \"height\"}".into());
        }
        return Ok(None);
    };
    let b = v
        .as_object()
        .ok_or_else(|| "bar must be an object with \"entry\" and \"height\"".to_string())?;
    no_unknown(b, &BAR_KEYS, "bar")?;
    let entry = text_of(b, "entry", "bar.entry")?;
    crate::paths::check_relative(&entry).map_err(|e| format!("bar.entry {e}"))?;
    let height = required(b, "height", "bar.height")?
        .as_u64()
        .filter(|h| (MIN_BAR_HEIGHT..=MAX_BAR_HEIGHT).contains(h))
        .ok_or_else(|| {
            format!("bar.height must be a whole number from {MIN_BAR_HEIGHT} to {MAX_BAR_HEIGHT}")
        })?;
    if !permissions.contains(&Permission::Bar) {
        return Err(
            "a plugin with a bar needs the \"bar\" permission; add it to permissions".into(),
        );
    }
    Ok(Some(Bar {
        entry,
        height: height as u32,
    }))
}

/// `contributes`: `{sidebar?, actions?, icons?}`. The icons come first, so an action can name one.
fn contributes(
    obj: &Map<String, Value>,
    permissions: &[Permission],
) -> Result<Contributes, String> {
    let c = match obj.get("contributes") {
        None => Map::new(),
        Some(v) => v
            .as_object()
            .cloned()
            .ok_or_else(|| "contributes must be an object".to_string())?,
    };
    no_unknown(&c, &CONTRIBUTES_KEYS, "contributes")?;
    let icons = icons(&c)?;
    let sidebar = sidebar(&c)?;
    let actions = actions(&c, &icons)?;
    for (has_any, perm, key) in [
        (!sidebar.is_empty(), Permission::Sidebar, "sidebar"),
        (!actions.is_empty(), Permission::Actions, "actions"),
    ] {
        match (has_any, permissions.contains(&perm)) {
            (true, false) => {
                return Err(format!(
                    "contributes.{key} needs the \"{}\" permission; add it to permissions",
                    perm.name()
                ));
            }
            (false, true) => {
                return Err(format!(
                    "the \"{}\" permission needs at least one entry in contributes.{key}",
                    perm.name()
                ));
            }
            _ => {}
        }
    }
    Ok(Contributes {
        sidebar,
        actions,
        icons,
    })
}

/// A list under `contributes`, or none.
fn list_of<'a>(c: &'a Map<String, Value>, key: &str) -> Result<&'a [Value], String> {
    match c.get(key) {
        None => Ok(&[]),
        Some(v) => v
            .as_array()
            .map(Vec::as_slice)
            .ok_or_else(|| format!("contributes.{key} must be a list")),
    }
}

/// A key of a sidebar section or an action: the decor key rule.
fn key_of(o: &Map<String, Value>, ctx: &str) -> Result<String, String> {
    let key = text_of(o, "key", &format!("{ctx}.key"))?;
    if !valid_key(&key) {
        return Err(format!(
            "{ctx}.key must be 1 to {} characters: a-z, 0-9 or '-'",
            flare_ctl::decor::KEY_MAX
        ));
    }
    Ok(key)
}

fn sidebar(c: &Map<String, Value>) -> Result<Vec<SidebarDef>, String> {
    let list = list_of(c, "sidebar")?;
    if list.len() > MAX_SIDEBAR {
        return Err(format!(
            "a plugin can declare at most {MAX_SIDEBAR} sidebar sections"
        ));
    }
    let mut out: Vec<SidebarDef> = Vec::new();
    for (i, v) in list.iter().enumerate() {
        let ctx = format!("contributes.sidebar[{i}]");
        let o = v
            .as_object()
            .ok_or_else(|| format!("{ctx} must be an object"))?;
        no_unknown(o, &SIDEBAR_KEYS, &ctx)?;
        let key = key_of(o, &ctx)?;
        if out.iter().any(|s| s.key == key) {
            return Err(format!("{ctx}.key \"{key}\" is listed twice"));
        }
        let title = clean_text(o, "title", &format!("{ctx}.title"), MAX_TITLE_CHARS)?;
        out.push(SidebarDef { key, title });
    }
    Ok(out)
}

fn actions(c: &Map<String, Value>, icons: &[IconDef]) -> Result<Vec<ActionDef>, String> {
    let list = list_of(c, "actions")?;
    if list.len() > MAX_ACTIONS {
        return Err(format!(
            "a plugin can declare at most {MAX_ACTIONS} actions"
        ));
    }
    let mut out: Vec<ActionDef> = Vec::new();
    for (i, v) in list.iter().enumerate() {
        let ctx = format!("contributes.actions[{i}]");
        let o = v
            .as_object()
            .ok_or_else(|| format!("{ctx} must be an object"))?;
        no_unknown(o, &ACTION_KEYS, &ctx)?;
        let key = key_of(o, &ctx)?;
        if out.iter().any(|a| a.key == key) {
            return Err(format!("{ctx}.key \"{key}\" is listed twice"));
        }
        let title = clean_text(o, "title", &format!("{ctx}.title"), MAX_TITLE_CHARS)?;
        let icon = match o.get("icon") {
            None => None,
            Some(_) => {
                let icon = text_of(o, "icon", &format!("{ctx}.icon"))?;
                if !is_badge_icon(&icon) && !icons.iter().any(|d| d.name == icon) {
                    return Err(format!(
                        "{ctx}.icon \"{icon}\" is neither a built-in icon nor one in contributes.icons"
                    ));
                }
                Some(icon)
            }
        };
        let where_list = required(o, "where", &format!("{ctx}.where"))?
            .as_array()
            .ok_or_else(|| {
                format!("{ctx}.where must be a list of places, such as [\"tab-menu\"]")
            })?;
        if where_list.is_empty() {
            return Err(format!("{ctx}.where must name at least one place"));
        }
        let mut places: Vec<Place> = Vec::new();
        for (j, w) in where_list.iter().enumerate() {
            let place = w.as_str().and_then(Place::parse).ok_or_else(|| {
                format!(
                    "{ctx}.where[{j}] must be one of {}",
                    Place::ALL.map(Place::name).join(", ")
                )
            })?;
            if places.contains(&place) {
                return Err(format!(
                    "{ctx}.where[{j}]: \"{}\" is listed twice",
                    place.name()
                ));
            }
            places.push(place);
        }
        // One hover button and one top-bar button per plugin.
        for once in [Place::TabHover, Place::TopBar] {
            if places.contains(&once) && out.iter().any(|a| a.places.contains(&once)) {
                return Err(format!(
                    "{ctx}: a plugin can place only one action in \"{}\"",
                    once.name()
                ));
            }
        }
        out.push(ActionDef {
            key,
            title,
            icon,
            places,
        });
    }
    Ok(out)
}

fn icons(c: &Map<String, Value>) -> Result<Vec<IconDef>, String> {
    let Some(v) = c.get("icons") else {
        return Ok(Vec::new());
    };
    let map = v
        .as_object()
        .ok_or_else(|| "contributes.icons must be an object of names and SVG paths".to_string())?;
    if map.len() > MAX_ICONS {
        return Err(format!("a plugin can declare at most {MAX_ICONS} icons"));
    }
    let mut out = Vec::new();
    for (name, path) in map {
        let ctx = format!("contributes.icons.{name}");
        if !valid_key(name) {
            return Err(format!(
                "the icon name \"{name}\" must be 1 to {} characters: a-z, 0-9 or '-'",
                flare_ctl::decor::KEY_MAX
            ));
        }
        if is_badge_icon(name) {
            return Err(format!(
                "the icon name \"{name}\" is a built-in icon; give yours another name"
            ));
        }
        let path = path
            .as_str()
            .ok_or_else(|| format!("{ctx} must be an SVG path, such as \"M4 12l5 5L20 6\""))?;
        if let Some(why) = icon_path_problem(path) {
            return Err(format!("{ctx} {why}"));
        }
        out.push(IconDef {
            name: name.clone(),
            path: path.to_string(),
        });
    }
    Ok(out)
}

/// What is wrong with an icon's SVG path, if anything: it must start with a move, be at most 2 KB, and hold only
/// path commands, numbers, signs, dots, commas and spaces (nothing a renderer could read as markup or a URL).
fn icon_path_problem(path: &str) -> Option<String> {
    if path.len() > MAX_ICON_PATH_BYTES {
        return Some(format!(
            "is {} bytes; the limit is {MAX_ICON_PATH_BYTES}",
            path.len()
        ));
    }
    if !path.trim_start().starts_with(['M', 'm']) {
        return Some("must start with a move command (M or m)".into());
    }
    let allowed = |c: char| {
        "MmLlHhVvCcSsQqTtAaZz".contains(c)
            || c.is_ascii_digit()
            || matches!(
                c,
                '.' | '-' | '+' | ',' | 'e' | 'E' | ' ' | '\t' | '\n' | '\r'
            )
    };
    if let Some(c) = path.chars().find(|c| !allowed(*c)) {
        return Some(format!(
            "has {c:?}; only path commands, numbers, commas and spaces are allowed"
        ));
    }
    let commands = path
        .chars()
        .filter(|c| c.is_ascii_alphabetic() && !matches!(c, 'e' | 'E'))
        .count();
    if commands > MAX_ICON_COMMANDS {
        return Some(format!(
            "has {commands} commands; the limit is {MAX_ICON_COMMANDS}"
        ));
    }
    match path_numbers(path) {
        None => return Some("has a number that cannot be read".into()),
        Some(nums) => {
            if let Some(n) = nums.into_iter().find(|n| n.abs() > MAX_ICON_COORD) {
                return Some(format!(
                    "has the number {n}; every number is within -{MAX_ICON_COORD} to {MAX_ICON_COORD}"
                ));
            }
        }
    }
    None
}

/// The numbers of a path of the allowed characters, in order; None when one does not read as a number. A sign
/// starts a new number, and so does a second dot (`.5.5` is two numbers, as SVG reads it).
fn path_numbers(path: &str) -> Option<Vec<f64>> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut dot = false;
    let flush = |cur: &mut String, out: &mut Vec<f64>| -> Option<()> {
        if !cur.is_empty() {
            let n: f64 = cur.parse().ok()?;
            if !n.is_finite() {
                return None;
            }
            out.push(n);
            cur.clear();
        }
        Some(())
    };
    for c in path.chars() {
        let after_exp = cur.ends_with(['e', 'E']);
        match c {
            '0'..='9' => cur.push(c),
            'e' | 'E' if !cur.is_empty() => cur.push(c),
            '+' | '-' if after_exp => cur.push(c),
            '+' | '-' => {
                flush(&mut cur, &mut out)?;
                dot = false;
                cur.push(c);
            }
            '.' if dot || cur.contains(['e', 'E']) => {
                flush(&mut cur, &mut out)?;
                cur.push(c);
            }
            '.' => cur.push(c),
            _ => {
                flush(&mut cur, &mut out)?;
                dot = false;
            }
        }
        if c == '.' {
            dot = true;
        }
    }
    flush(&mut cur, &mut out)?;
    Some(out)
}

/// `helper`: `{command, args?}`, and the `process` permission goes with it.
fn helper(obj: &Map<String, Value>, permissions: &[Permission]) -> Result<Option<Helper>, String> {
    let Some(v) = obj.get("helper") else {
        if permissions.contains(&Permission::Process) {
            return Err(
                "the \"process\" permission needs a helper: add {\"command\": \"helper.exe\"}"
                    .into(),
            );
        }
        return Ok(None);
    };
    let h = v
        .as_object()
        .ok_or_else(|| "helper must be an object with \"command\"".to_string())?;
    no_unknown(h, &HELPER_KEYS, "helper")?;
    let command = text_of(h, "command", "helper.command")?;
    crate::paths::check_program(&command).map_err(|e| format!("helper.command {e}"))?;
    let args = match h.get("args") {
        None => Vec::new(),
        Some(a) => {
            let list = a
                .as_array()
                .ok_or_else(|| "helper.args must be a list of text".to_string())?;
            if list.len() > MAX_HELPER_ARGS {
                return Err(format!(
                    "helper.args can hold at most {MAX_HELPER_ARGS} arguments"
                ));
            }
            let mut out = Vec::new();
            for (i, v) in list.iter().enumerate() {
                let arg = v
                    .as_str()
                    .ok_or_else(|| format!("helper.args[{i}] must be text"))?;
                if arg.chars().count() > MAX_HELPER_ARG_CHARS {
                    return Err(format!(
                        "helper.args[{i}] is longer than {MAX_HELPER_ARG_CHARS} characters"
                    ));
                }
                if arg.chars().any(|c| c.is_control() || is_format_char(c)) {
                    return Err(format!(
                        "helper.args[{i}] must not contain a control or hidden formatting character"
                    ));
                }
                out.push(arg.to_string());
            }
            out
        }
    };
    if !permissions.contains(&Permission::Process) {
        return Err(
            "a plugin with a helper needs the \"process\" permission; add it to permissions".into(),
        );
    }
    Ok(Some(Helper { command, args }))
}

fn no_unknown(obj: &Map<String, Value>, allowed: &[&str], ctx: &str) -> Result<(), String> {
    for key in obj.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(format!(
                "unknown field \"{key}\" in {ctx} (allowed: {})",
                allowed.join(", ")
            ));
        }
    }
    Ok(())
}

/// The value of `key`, or the refusal. `shown` is the field as the author sees it: `id` at the top level,
/// `panel.entry` or `tools[0].input_schema` inside a block. A top-level miss reads `mod.json is missing "key"`.
fn required<'a>(obj: &'a Map<String, Value>, key: &str, shown: &str) -> Result<&'a Value, String> {
    obj.get(key).ok_or_else(|| {
        if shown == key {
            format!("mod.json is missing \"{key}\"")
        } else {
            format!("{shown} is missing")
        }
    })
}

/// A string field, exactly as written (no trimming; used for ids, versions and paths).
fn text_of(obj: &Map<String, Value>, key: &str, shown: &str) -> Result<String, String> {
    required(obj, key, shown)?
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| format!("{shown} must be text"))
}

/// A text field a person reads: not blank, at most `max` characters, no control characters, no hidden
/// formatting characters (a zero-width space, a bidi override), so the text shown is the text the agent reads.
fn clean_text(
    obj: &Map<String, Value>,
    key: &str,
    shown: &str,
    max: usize,
) -> Result<String, String> {
    clean_line(text_of(obj, key, shown)?, shown, max)
}

/// The rule of [`clean_text`] for a line that did not come from a manifest (a badge's tip, a progress label):
/// not blank, at most `max` characters, no control or hidden formatting character. `shown` names it in the error.
pub fn clean_line(s: String, shown: &str, max: usize) -> Result<String, String> {
    if s.trim().is_empty() {
        return Err(format!("{shown} must not be empty"));
    }
    if s.chars().count() > max {
        return Err(format!("{shown} is longer than {max} characters"));
    }
    if s.chars().any(char::is_control) {
        return Err(format!(
            "{shown} must not contain a control character or a line break"
        ));
    }
    if s.chars().any(is_format_char) {
        return Err(format!(
            "{shown} must not contain a hidden formatting character (a zero-width space or a bidi override)"
        ));
    }
    Ok(s)
}

/// The Unicode format characters (general category Cf) a person cannot see but a renderer or a model reads:
/// soft hyphen, the Arabic letter mark, the Mongolian vowel separator, the zero-width and bidi-control
/// characters, the word joiner, the invisible operators, the BOM and the interlinear annotation marks.
pub fn is_format_char(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{061C}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206F}'
            | '\u{FEFF}'
            | '\u{FFF9}'..='\u{FFFB}'
    )
}

/// A semantic version: `MAJOR.MINOR.PATCH`, numbers with no leading zero, then an optional `-pre` and an
/// optional `+build`, both made of dot-separated identifiers.
pub fn valid_semver(v: &str) -> bool {
    let (rest, build) = match v.split_once('+') {
        Some((r, b)) => (r, Some(b)),
        None => (v, None),
    };
    if let Some(b) = build
        && !dotted_idents(b, false)
    {
        return false;
    }
    let (core, pre) = match rest.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (rest, None),
    };
    if let Some(p) = pre
        && !dotted_idents(p, true)
    {
        return false;
    }
    let parts: Vec<&str> = core.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.bytes().all(|b| b.is_ascii_digit())
                && (p.len() == 1 || !p.starts_with('0'))
        })
}

/// Dot-separated identifiers of `[0-9A-Za-z-]`; a numeric pre-release identifier has no leading zero.
fn dotted_idents(s: &str, pre: bool) -> bool {
    !s.is_empty()
        && s.split('.').all(|id| {
            !id.is_empty()
                && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                && !(pre
                    && id.len() > 1
                    && id.starts_with('0')
                    && id.bytes().all(|b| b.is_ascii_digit()))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A complete, valid manifest; each test edits one part of it.
    fn good() -> Value {
        serde_json::json!({
            "id": "notes",
            "name": "Notes",
            "version": "1.0.0",
            "protocol": 1,
            "description": "A notes panel.",
            "permissions": ["panel", "tools"],
            "panel": { "entry": "index.html", "title": "Notes" },
            "tools": [{
                "name": "append_note",
                "description": "Adds a line.",
                "input_schema": { "type": "object", "properties": { "text": { "type": "string" } } }
            }]
        })
    }

    fn with(edit: impl FnOnce(&mut Value)) -> Result<Manifest, String> {
        let mut v = good();
        edit(&mut v);
        parse(&v.to_string())
    }

    fn err_of(edit: impl FnOnce(&mut Value)) -> String {
        with(edit).unwrap_err()
    }

    #[test]
    fn the_good_manifest_parses() {
        let m = parse(&good().to_string()).unwrap();
        assert_eq!(m.id, "notes");
        assert_eq!(m.permissions, vec![Permission::Panel, Permission::Tools]);
        assert_eq!(m.panel.as_ref().unwrap().entry, "index.html");
        assert_eq!(m.tools[0].name, "append_note");
    }

    #[test]
    fn a_mod_with_no_panel_and_no_permissions_is_valid() {
        let m = with(|v| {
            v.as_object_mut().unwrap().remove("panel");
            v.as_object_mut().unwrap().remove("tools");
            v["permissions"] = serde_json::json!([]);
        })
        .unwrap();
        assert!(m.panel.is_none() && m.tools.is_empty() && m.permissions.is_empty());
    }

    #[test]
    fn ids_are_three_to_forty_lower_case_characters() {
        assert!(err_of(|v| v["id"] = "ab".into()).contains("id must be 3 to 40"));
        assert!(err_of(|v| v["id"] = "a".repeat(41).into()).contains("id must be 3 to 40"));
        assert!(err_of(|v| v["id"] = "Notes".into()).contains("lower-case"));
        assert!(err_of(|v| v["id"] = "my_mod".into()).contains("lower-case"));
        assert!(err_of(|v| v["id"] = "my mod".into()).contains("lower-case"));
        assert!(err_of(|v| v["id"] = "ünï".into()).contains("lower-case"));
        assert!(with(|v| v["id"] = "a".repeat(40).into()).is_ok());
        assert!(with(|v| v["id"] = "abc".into()).is_ok());
        assert!(with(|v| v["id"] = "a-1-0".into()).is_ok());
    }

    #[test]
    fn windows_device_names_cannot_be_ids() {
        for id in ["con", "nul", "com1", "lpt9", "aux"] {
            assert!(
                err_of(|v| v["id"] = id.into()).contains("device name"),
                "{id}"
            );
        }
        assert!(with(|v| v["id"] = "conx".into()).is_ok());
    }

    #[test]
    fn the_protocol_must_be_one() {
        assert!(err_of(|v| v["protocol"] = 2.into()).contains("protocol must be the number 1"));
        assert!(err_of(|v| v["protocol"] = "1".into()).contains("protocol"));
        assert!(err_of(|v| v["protocol"] = 1.5_f64.into()).contains("protocol"));
        assert!(
            err_of(|v| {
                v.as_object_mut().unwrap().remove("protocol");
            })
            .contains("missing \"protocol\"")
        );
    }

    #[test]
    fn versions_are_semver() {
        for ok in [
            "0.0.1",
            "10.20.30",
            "1.0.0-alpha.1",
            "1.0.0+build.5",
            "2.0.0-rc.1+x",
        ] {
            assert!(with(|v| v["version"] = ok.into()).is_ok(), "{ok}");
        }
        for bad in [
            "1.0", "1", "v1.0.0", "01.0.0", "1.0.0-", "1.0.0-01", "1.0.0+", "1.0.0.0", "1.x.0", "",
        ] {
            assert!(
                err_of(|v| v["version"] = bad.into()).contains("semantic version"),
                "{bad}"
            );
        }
    }

    #[test]
    fn names_and_descriptions_are_text_a_person_reads() {
        assert!(err_of(|v| v["name"] = "   ".into()).contains("name must not be empty"));
        assert!(err_of(|v| v["name"] = "a".repeat(81).into()).contains("longer than 80"));
        assert!(err_of(|v| v["name"] = "two\nlines".into()).contains("control character"));
        assert!(err_of(|v| v["description"] = "".into()).contains("description must not be empty"));
        assert!(err_of(|v| v["description"] = "x".repeat(501).into()).contains("longer than 500"));
        assert!(err_of(|v| v["description"] = 5.into()).contains("must be text"));
    }

    #[test]
    fn permissions_are_the_five_askable_names_and_no_more() {
        let got = with(|v| {
            v["permissions"] = serde_json::json!(["panel", "view", "browser", "convo", "tools"])
        })
        .unwrap();
        assert_eq!(got.permissions.len(), 5);
        assert!(
            err_of(|v| v["permissions"] = serde_json::json!(["root"]))
                .contains("unknown permission \"root\"")
        );
        assert!(
            err_of(|v| v["permissions"] = serde_json::json!(["eval"]))
                .contains("user's own switch")
        );
        assert!(
            err_of(|v| v["permissions"] = serde_json::json!(["view", "view"]))
                .contains("listed twice")
        );
        assert!(err_of(|v| v["permissions"] = serde_json::json!([1])).contains("must be a name"));
        assert!(err_of(|v| v["permissions"] = "view".into()).contains("list of names"));
        // `decorate` is a real permission (the agent holds it) but not one a mod may ask for yet.
        assert!(Permission::parse("decorate").is_some());
        assert!(!ASKABLE_V1.contains(&Permission::Decorate));
        assert!(
            err_of(|v| v["permissions"] = serde_json::json!(["decorate"]))
                .contains("unknown permission \"decorate\"")
        );
        assert!(
            err_of(|v| {
                v.as_object_mut().unwrap().remove("permissions");
            })
            .contains("missing \"permissions\"")
        );
    }

    #[test]
    fn a_panel_and_the_panel_permission_go_together() {
        let no_perm = err_of(|v| v["permissions"] = serde_json::json!(["tools"]));
        assert!(
            no_perm.contains("needs the \"panel\" permission"),
            "{no_perm}"
        );
        let no_panel = err_of(|v| {
            v.as_object_mut().unwrap().remove("panel");
        });
        assert!(no_panel.contains("needs a panel"), "{no_panel}");
    }

    #[test]
    fn panel_fields_are_exact_and_the_entry_is_a_safe_html_path() {
        assert!(
            err_of(|v| v["panel"]["extra"] = 1.into()).contains("unknown field \"extra\" in panel")
        );
        assert!(err_of(|v| v["panel"]["entry"] = "../evil.html".into()).contains("panel.entry"));
        assert!(err_of(|v| v["panel"]["entry"] = "C:\\x.html".into()).contains("':'"));
        assert!(err_of(|v| v["panel"]["entry"] = "app.exe".into()).contains(".html"));
        assert!(err_of(|v| v["panel"] = "index.html".into()).contains("panel must be an object"));
        assert!(
            err_of(|v| {
                v["panel"].as_object_mut().unwrap().remove("title");
            })
            .contains("panel.title is missing")
        );
        assert!(
            err_of(|v| v["panel"]["title"] = "".into()).contains("panel.title must not be empty")
        );
    }

    #[test]
    fn unknown_top_level_keys_are_refused_with_the_allowed_list() {
        let e = err_of(|v| v["permision"] = serde_json::json!(["view"]));
        assert!(e.contains("unknown field \"permision\" in mod.json"), "{e}");
        assert!(e.contains("allowed:"), "{e}");
    }

    #[test]
    fn the_file_must_be_one_json_object_and_not_too_big() {
        assert!(parse("[]").unwrap_err().contains("JSON object"));
        assert!(parse("{").unwrap_err().contains("not valid JSON"));
        let big = format!("{{\"id\":\"{}\"}}", "a".repeat(70 * 1024));
        assert!(parse(&big).unwrap_err().contains("larger than 64 KB"));
    }

    #[test]
    fn tool_names_are_a_to_z_digits_and_underscore() {
        let forty = "a".repeat(40);
        let forty_one = "a".repeat(41);
        for ok in ["a", "append_note", "t2", "x_9", forty.as_str()] {
            assert!(valid_tool_name(ok), "{ok}");
        }
        for bad in ["", "Append", "a-b", "a b", "a.b", "ä", forty_one.as_str()] {
            assert!(!valid_tool_name(bad), "{bad}");
            let e = err_of(|v| v["tools"][0]["name"] = bad.into());
            assert!(e.contains("tools[0].name must be 1 to 40"), "{bad}: {e}");
        }
    }

    #[test]
    fn tool_names_are_unique_within_a_mod() {
        let t = serde_json::json!({ "name": "go", "description": "x", "input_schema": { "type": "object" } });
        let e = err_of(|v| v["tools"] = serde_json::json!([t.clone(), t.clone()]));
        assert!(e.contains("tools[1].name \"go\" is listed twice"), "{e}");
    }

    #[test]
    fn a_tool_needs_its_fields_and_no_extras() {
        assert!(
            err_of(|v| v["tools"][0]["extra"] = 1.into())
                .contains("unknown field \"extra\" in tools[0]")
        );
        assert!(
            err_of(|v| v["tools"][0]["input_schema"] = "x".into()).contains("JSON Schema object")
        );
        assert!(
            err_of(|v| v["tools"][0]["input_schema"] = serde_json::json!([]))
                .contains("JSON Schema object")
        );
        assert!(
            err_of(|v| v["tools"][0]["input_schema"]["type"] = "string".into())
                .contains("must be \"object\"")
        );
        assert!(
            err_of(|v| {
                v["tools"][0].as_object_mut().unwrap().remove("description");
            })
            .contains("tools[0].description is missing")
        );
        assert!(err_of(|v| v["tools"] = 5.into()).contains("list of tool objects"));
        assert!(err_of(|v| v["tools"][0] = 5.into()).contains("must be an object"));
    }

    /// Every nested required field names itself, with its place in the manifest, so the author can find it.
    #[test]
    fn a_missing_nested_field_is_named_with_its_place() {
        let e = err_of(|v| {
            v["panel"].as_object_mut().unwrap().remove("entry");
        });
        assert_eq!(e, "panel.entry is missing");
        let e = err_of(|v| {
            v["tools"][0].as_object_mut().unwrap().remove("name");
        });
        assert_eq!(e, "tools[0].name is missing");
        let e = err_of(|v| {
            v["tools"][0]
                .as_object_mut()
                .unwrap()
                .remove("input_schema");
        });
        assert_eq!(e, "tools[0].input_schema is missing");
        let e = err_of(|v| {
            v["tools"][0]["input_schema"] = serde_json::json!({ "type": "object" });
            v["tools"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::json!({ "name": "b", "description": "d" }));
        });
        assert_eq!(e, "tools[1].input_schema is missing");
        // Top level keeps the short form.
        let e = err_of(|v| {
            v.as_object_mut().unwrap().remove("name");
        });
        assert_eq!(e, "mod.json is missing \"name\"");
    }

    #[test]
    fn an_input_schema_without_type_object_is_refused() {
        let e = err_of(|v| v["tools"][0]["input_schema"] = serde_json::json!({}));
        assert!(
            e.contains("tools[0].input_schema.type must be \"object\""),
            "{e}"
        );
        let e = err_of(|v| {
            v["tools"][0]["input_schema"] = serde_json::json!({ "properties": {} });
        });
        assert!(e.contains("type must be \"object\""), "{e}");
    }

    #[test]
    fn hidden_formatting_characters_are_refused_in_every_text_field() {
        // A bidi override in a name, a zero-width space in a description, a BOM in a panel title.
        for (field, edit) in [
            ("name", "Note\u{202E}s"),
            ("description", "A\u{200B}notes panel."),
        ] {
            let e = err_of(|v| v[field] = edit.into());
            assert!(
                e.contains(field) && e.contains("hidden formatting"),
                "{field}: {e}"
            );
        }
        let e = err_of(|v| v["panel"]["title"] = "No\u{FEFF}tes".into());
        assert!(
            e.contains("panel.title") && e.contains("hidden formatting"),
            "{e}"
        );
        let e = err_of(|v| v["tools"][0]["description"] = "Adds\u{2066}a line".into());
        assert!(
            e.contains("tools[0].description") && e.contains("hidden formatting"),
            "{e}"
        );
        // Ordinary text with accents and a non-Latin script is fine.
        assert!(with(|v| v["name"] = "Notes ñandú 日本".into()).is_ok());
    }

    #[test]
    fn a_schema_over_eight_kilobytes_is_refused() {
        let big = "x".repeat(MAX_SCHEMA_BYTES);
        let e = err_of(|v| v["tools"][0]["input_schema"]["description"] = big.into());
        assert!(e.contains("limit is 8192"), "{e}");
    }

    #[test]
    fn a_mod_may_declare_at_most_thirty_two_tools() {
        let tools: Vec<Value> = (0..33)
            .map(|i| serde_json::json!({ "name": format!("t{i}"), "description": "d", "input_schema": {} }))
            .collect();
        let e = err_of(|v| v["tools"] = Value::Array(tools.clone()));
        assert!(e.contains("at most 32"), "{e}");
    }

    #[test]
    fn tools_need_a_panel_to_answer_them() {
        let e = err_of(|v| {
            v.as_object_mut().unwrap().remove("panel");
            v["permissions"] = serde_json::json!(["tools"]);
        });
        assert!(e.contains("tools need a panel"), "{e}");
    }

    #[test]
    fn tools_need_the_tools_permission() {
        let e = err_of(|v| v["permissions"] = serde_json::json!(["panel"]));
        assert!(e.contains("tools need the \"tools\" permission"), "{e}");
    }

    #[test]
    fn valid_id_matches_the_manifest_rule() {
        assert!(valid_id("notes"));
        assert!(!valid_id("no"));
        assert!(!valid_id("Notes"));
        assert!(!valid_id("con"));
    }

    #[test]
    fn semver_edges() {
        assert!(valid_semver("0.0.0"));
        assert!(valid_semver("1.0.0-0"));
        assert!(!valid_semver("1.0.0-0.01"));
        assert!(!valid_semver("1..0"));
        assert!(!valid_semver("1.0.0-a..b"));
    }

    // ---- protocol 2: plugin.json ----

    /// A complete, valid plugin manifest using every protocol 2 key; each test edits one part of it.
    fn good_plugin() -> Value {
        serde_json::json!({
            "id": "todo",
            "name": "Session to-dos",
            "version": "1.0.0",
            "protocol": 2,
            "description": "Things to check per session.",
            "author": "someone",
            "homepage": "https://github.com/someone/flare-todo",
            "permissions": ["panel", "tools", "decorate", "sidebar", "actions", "bar", "process"],
            "background": "background.html",
            "panel": { "entry": "panel.html", "title": "To-dos" },
            "bar": { "entry": "bar.html", "height": 48 },
            "contributes": {
                "sidebar": [{ "key": "todos", "title": "To-dos" }],
                "actions": [
                    { "key": "add", "title": "Add to-do", "icon": "plus", "where": ["tab-menu"] },
                    { "key": "done", "title": "Done", "icon": "check", "where": ["tab-hover", "session-bar"] }
                ],
                "icons": { "check": "M4 12l5 5L20 6" }
            },
            "helper": { "command": "bin/helper.exe", "args": ["--quiet"] },
            "tools": [{ "name": "add_todo", "description": "Adds one.", "input_schema": { "type": "object" } }]
        })
    }

    fn with_plugin(edit: impl FnOnce(&mut Value)) -> Result<Manifest, String> {
        let mut v = good_plugin();
        edit(&mut v);
        parse_plugin(&v.to_string())
    }

    fn plugin_err(edit: impl FnOnce(&mut Value)) -> String {
        with_plugin(edit).unwrap_err()
    }

    fn remove(v: &mut Value, key: &str) {
        v.as_object_mut().unwrap().remove(key);
    }

    #[test]
    fn the_good_plugin_parses_with_every_new_key() {
        let m = parse_plugin(&good_plugin().to_string()).unwrap();
        assert!(m.is_plugin());
        assert_eq!(m.protocol, 2);
        assert_eq!(m.author.as_deref(), Some("someone"));
        assert_eq!(
            m.homepage.as_deref(),
            Some("https://github.com/someone/flare-todo")
        );
        assert_eq!(m.background.as_deref(), Some("background.html"));
        assert_eq!(
            m.bar,
            Some(Bar {
                entry: "bar.html".into(),
                height: 48
            })
        );
        assert_eq!(m.contributes.sidebar[0].key, "todos");
        assert_eq!(m.contributes.actions.len(), 2);
        assert_eq!(m.contributes.actions[0].places, [Place::TabMenu]);
        assert_eq!(
            m.contributes.actions[1].places,
            [Place::TabHover, Place::SessionBar]
        );
        assert_eq!(m.contributes.icons[0].name, "check");
        assert_eq!(
            m.helper,
            Some(Helper {
                command: "bin/helper.exe".into(),
                args: vec!["--quiet".into()]
            })
        );
        assert!(m.permissions.contains(&Permission::Process));
    }

    #[test]
    fn a_minimal_plugin_needs_no_new_key() {
        let m = parse_plugin(
            r#"{"id":"tiny","name":"Tiny","version":"0.1.0","protocol":2,"description":"d","permissions":[]}"#,
        )
        .unwrap();
        assert!(m.is_plugin());
        assert_eq!(m.author, None);
        assert_eq!(m.contributes, Contributes::default());
    }

    #[test]
    fn a_protocol_1_mod_has_no_plugin_parts() {
        let m = parse(&good().to_string()).unwrap();
        assert!(!m.is_plugin());
        assert_eq!(m.protocol, 1);
        assert_eq!((m.author, m.homepage, m.background), (None, None, None));
        assert_eq!((m.bar, m.helper), (None, None));
        assert_eq!(m.contributes, Contributes::default());
    }

    #[test]
    fn each_file_must_say_its_own_protocol() {
        let e = err_of(|v| v["protocol"] = 2.into());
        assert!(e.contains("plugin.json"), "{e}");
        let e = plugin_err(|v| v["protocol"] = 1.into());
        assert!(
            e.contains("protocol must be the number 2") && e.contains("mod.json"),
            "{e}"
        );
        for bad in [Value::from(3), Value::from("2"), Value::from(2.5)] {
            let e = plugin_err(|v| v["protocol"] = bad.clone());
            assert!(e.contains("protocol must be the number 2"), "{bad}: {e}");
        }
        let e = plugin_err(|v| remove(v, "protocol"));
        assert_eq!(e, "plugin.json is missing \"protocol\"");
    }

    #[test]
    fn a_protocol_2_key_in_a_mod_json_is_refused_and_points_at_plugin_json() {
        for key in TOP_KEYS_V2_ONLY {
            let e = err_of(|v| v[key] = "x".into());
            assert!(
                e.contains(&format!("\"{key}\" is a protocol 2 key")) && e.contains("plugin.json"),
                "{key}: {e}"
            );
        }
    }

    #[test]
    fn unknown_keys_are_refused_in_a_plugin_at_every_level() {
        let e = plugin_err(|v| v["permision"] = serde_json::json!([]));
        assert!(
            e.contains("unknown field \"permision\" in plugin.json"),
            "{e}"
        );
        assert!(
            e.contains("contributes"),
            "the allowed list has the new keys: {e}"
        );
        for (edit, want) in [
            (
                Box::new(|v: &mut Value| v["bar"]["x"] = 1.into()) as Box<dyn Fn(&mut Value)>,
                "in bar",
            ),
            (
                Box::new(|v: &mut Value| v["contributes"]["x"] = 1.into()),
                "in contributes",
            ),
            (
                Box::new(|v: &mut Value| v["contributes"]["sidebar"][0]["x"] = 1.into()),
                "in contributes.sidebar[0]",
            ),
            (
                Box::new(|v: &mut Value| v["contributes"]["actions"][0]["x"] = 1.into()),
                "in contributes.actions[0]",
            ),
            (
                Box::new(|v: &mut Value| v["helper"]["x"] = 1.into()),
                "in helper",
            ),
        ] {
            let e = plugin_err(|v| edit(v));
            assert!(
                e.contains("unknown field \"x\"") && e.contains(want),
                "{want}: {e}"
            );
        }
    }

    #[test]
    fn plugin_permissions_are_the_askable_v2_list_and_never_eval() {
        let all: Vec<&str> = ASKABLE_V2.iter().map(|p| p.name()).collect();
        let m = parse_plugin(
            &serde_json::json!({
                "id": "wide", "name": "Wide", "version": "1.0.0", "protocol": 2, "description": "d",
                "permissions": all,
                "panel": {"entry": "p.html", "title": "P"},
                "bar": {"entry": "b.html", "height": 24},
                "contributes": {
                    "sidebar": [{"key": "s", "title": "S"}],
                    "actions": [{"key": "a", "title": "A", "where": ["tab-menu"]}]
                },
                "helper": {"command": "h.exe"}
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(m.permissions, ASKABLE_V2);
        assert!(!ASKABLE_V2.contains(&Permission::Eval));
        let e = plugin_err(|v| v["permissions"] = serde_json::json!(["eval"]));
        assert!(
            e.contains("user's own switch") && e.contains("plugin"),
            "{e}"
        );
        let e = plugin_err(|v| v["permissions"] = serde_json::json!(["root"]));
        assert!(
            e.contains("unknown permission \"root\"") && e.contains("process"),
            "{e}"
        );
        // Every protocol 1 name keeps its meaning in protocol 2.
        for p in ASKABLE_V1 {
            assert!(ASKABLE_V2.contains(&p), "{p:?}");
        }
    }

    /// `ASKABLE_V1` is the old list, so a protocol 1 mod can still ask only for what it always could, and each plugin
    /// permission it names is refused with a pointer to protocol 2.
    #[test]
    fn a_mod_cannot_ask_for_a_plugin_permission() {
        assert_eq!(ASKABLE_V1.len(), 5);
        for p in ASKABLE_V2.iter().filter(|p| !ASKABLE_V1.contains(p)) {
            let e = err_of(|v| v["permissions"] = serde_json::json!([p.name()]));
            assert!(
                e.contains(&format!("unknown permission \"{}\"", p.name()))
                    && e.contains("protocol 2"),
                "{}: {e}",
                p.name()
            );
        }
    }

    #[test]
    fn author_and_homepage_follow_the_text_rules_and_the_homepage_is_https() {
        assert!(plugin_err(|v| v["author"] = "".into()).contains("author must not be empty"));
        assert!(plugin_err(|v| v["author"] = "a".repeat(81).into()).contains("longer than 80"));
        assert!(plugin_err(|v| v["author"] = "Me\u{202E}".into()).contains("hidden formatting"));
        assert!(plugin_err(|v| v["author"] = 5.into()).contains("author must be text"));
        assert!(with_plugin(|v| remove(v, "author")).is_ok());
        assert!(with_plugin(|v| remove(v, "homepage")).is_ok());
        for ok in [
            "https://example.com",
            "https://example.com/",
            "https://a.b.example:8443/x?y=1#z",
        ] {
            assert!(with_plugin(|v| v["homepage"] = ok.into()).is_ok(), "{ok}");
        }
        for (bad, want) in [
            ("http://example.com/", "https://"),
            ("HTTPS://example.com/", "https://"),
            ("javascript:alert(1)", "https://"),
            ("example.com", "https://"),
            ("https://", "name a host"),
            ("https:///path", "name a host"),
            ("https://user@example.com/", "name a host"),
            ("https://exa mple.com/", "space"),
            ("https://example.com/\u{7}", "control"),
        ] {
            let e = plugin_err(|v| v["homepage"] = bad.into());
            assert!(e.contains("homepage") && e.contains(want), "{bad:?}: {e}");
        }
        let long = format!("https://example.com/{}", "a".repeat(200));
        assert!(plugin_err(|v| v["homepage"] = long.into()).contains("longer than 200"));
    }

    #[test]
    fn the_background_is_a_safe_html_path_and_needs_no_permission() {
        assert!(plugin_err(|v| v["background"] = "../x.html".into()).contains("background"));
        assert!(plugin_err(|v| v["background"] = "bg.js".into()).contains(".html"));
        assert!(plugin_err(|v| v["background"] = 1.into()).contains("background must be text"));
        let m = parse_plugin(
            r#"{"id":"bgonly","name":"B","version":"1.0.0","protocol":2,"description":"d","permissions":[],"background":"bg.html"}"#,
        )
        .unwrap();
        assert_eq!(m.background.as_deref(), Some("bg.html"));
    }

    #[test]
    fn a_bar_needs_its_permission_an_html_entry_and_a_height_from_24_to_160() {
        for h in [24, 160, 48] {
            assert!(
                with_plugin(|v| v["bar"]["height"] = h.into()).is_ok(),
                "{h}"
            );
        }
        for h in [
            Value::from(23),
            Value::from(161),
            Value::from(48.5),
            Value::from("48"),
            Value::from(-1),
        ] {
            let e = plugin_err(|v| v["bar"]["height"] = h.clone());
            assert!(
                e.contains("bar.height must be a whole number from 24 to 160"),
                "{h}: {e}"
            );
        }
        assert!(plugin_err(|v| v["bar"]["entry"] = "C:\\b.html".into()).contains("bar.entry"));
        assert_eq!(
            plugin_err(|v| {
                v["bar"].as_object_mut().unwrap().remove("height");
            }),
            "bar.height is missing"
        );
        assert!(plugin_err(|v| v["bar"] = "bar.html".into()).contains("bar must be an object"));
        // The permission and the key go together, both ways.
        let e = plugin_err(|v| {
            v["permissions"] = serde_json::json!([
                "panel", "tools", "decorate", "sidebar", "actions", "process"
            ]);
        });
        assert!(e.contains("needs the \"bar\" permission"), "{e}");
        let e = plugin_err(|v| {
            remove(v, "bar");
            v["contributes"]["actions"][1]["where"] = serde_json::json!(["tab-hover"]);
        });
        assert!(e.contains("the \"bar\" permission needs a bar"), "{e}");
    }

    #[test]
    fn a_session_bar_action_needs_a_bar() {
        let e = plugin_err(|v| {
            remove(v, "bar");
            v["permissions"] = serde_json::json!([
                "panel", "tools", "decorate", "sidebar", "actions", "process"
            ]);
        });
        assert!(e.contains("\"session-bar\" needs a bar"), "{e}");
    }

    #[test]
    fn sidebar_sections_are_at_most_two_with_unique_keys_and_need_their_permission() {
        let three = serde_json::json!([
            {"key": "a", "title": "A"}, {"key": "b", "title": "B"}, {"key": "c", "title": "C"}
        ]);
        assert!(
            plugin_err(|v| v["contributes"]["sidebar"] = three.clone())
                .contains("at most 2 sidebar")
        );
        let twice = serde_json::json!([{"key": "a", "title": "A"}, {"key": "a", "title": "B"}]);
        assert!(
            plugin_err(|v| v["contributes"]["sidebar"] = twice.clone())
                .contains("contributes.sidebar[1].key \"a\" is listed twice")
        );
        for bad in ["", "A", "a b", "a_b", &"a".repeat(25)] {
            let e = plugin_err(|v| v["contributes"]["sidebar"][0]["key"] = bad.into());
            assert!(
                e.contains("contributes.sidebar[0].key must be 1 to 24"),
                "{bad:?}: {e}"
            );
        }
        assert!(
            plugin_err(|v| v["contributes"]["sidebar"][0]["title"] = "".into())
                .contains("contributes.sidebar[0].title must not be empty")
        );
        assert!(plugin_err(|v| v["contributes"]["sidebar"] = 1.into()).contains("must be a list"));
        assert!(
            plugin_err(|v| v["contributes"]["sidebar"][0] = 1.into()).contains("must be an object")
        );
        let e = plugin_err(|v| {
            v["permissions"] =
                serde_json::json!(["panel", "tools", "decorate", "actions", "bar", "process"]);
        });
        assert!(
            e.contains("contributes.sidebar needs the \"sidebar\" permission"),
            "{e}"
        );
        let e = plugin_err(|v| v["contributes"]["sidebar"] = serde_json::json!([]));
        assert!(
            e.contains("the \"sidebar\" permission needs at least one entry"),
            "{e}"
        );
    }

    #[test]
    fn actions_are_at_most_eight_with_known_places_icons_and_one_hover_and_top_bar_each() {
        let nine: Vec<Value> = (0..9)
            .map(|i| serde_json::json!({"key": format!("a{i}"), "title": "A", "where": ["tab-menu"]}))
            .collect();
        assert!(
            plugin_err(|v| v["contributes"]["actions"] = Value::Array(nine.clone()))
                .contains("at most 8 actions")
        );
        let e =
            plugin_err(|v| v["contributes"]["actions"][0]["where"] = serde_json::json!(["sky"]));
        assert!(
            e.contains("where[0] must be one of tab-menu, tab-hover, top-bar, session-bar"),
            "{e}"
        );
        let e = plugin_err(|v| v["contributes"]["actions"][0]["where"] = serde_json::json!([]));
        assert!(e.contains("at least one place"), "{e}");
        let e = plugin_err(|v| {
            v["contributes"]["actions"][0]["where"] = serde_json::json!(["tab-menu", "tab-menu"])
        });
        assert!(e.contains("\"tab-menu\" is listed twice"), "{e}");
        let e = plugin_err(|v| v["contributes"]["actions"][0]["where"] = "tab-menu".into());
        assert!(e.contains("must be a list of places"), "{e}");
        let e = plugin_err(|v| {
            v["contributes"]["actions"][0]
                .as_object_mut()
                .unwrap()
                .remove("where");
        });
        assert_eq!(e, "contributes.actions[0].where is missing");
        // One hover button per plugin, one top-bar button per plugin.
        let e = plugin_err(|v| {
            v["contributes"]["actions"][0]["where"] = serde_json::json!(["tab-hover"])
        });
        assert!(e.contains("only one action in \"tab-hover\""), "{e}");
        let tops = serde_json::json!([
            {"key": "a", "title": "A", "where": ["top-bar"]},
            {"key": "b", "title": "B", "where": ["top-bar"]}
        ]);
        assert!(
            plugin_err(|v| v["contributes"]["actions"] = tops.clone())
                .contains("only one action in \"top-bar\"")
        );
        // An icon is a built-in or one of the plugin's own; there is no built-in check or dot.
        assert!(with_plugin(|v| v["contributes"]["actions"][0]["icon"] = "globe".into()).is_ok());
        assert!(
            with_plugin(|v| {
                v["contributes"]["actions"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("icon");
            })
            .is_ok()
        );
        let e = plugin_err(|v| v["contributes"]["actions"][0]["icon"] = "dot".into());
        assert!(
            e.contains("neither a built-in icon nor one in contributes.icons"),
            "{e}"
        );
        let e = plugin_err(|v| v["contributes"]["actions"][0]["key"] = "Add".into());
        assert!(
            e.contains("contributes.actions[0].key must be 1 to 24"),
            "{e}"
        );
        let e = plugin_err(|v| v["contributes"]["actions"][1]["key"] = "add".into());
        assert!(
            e.contains("contributes.actions[1].key \"add\" is listed twice"),
            "{e}"
        );
        let e = plugin_err(|v| {
            v["permissions"] =
                serde_json::json!(["panel", "tools", "decorate", "sidebar", "bar", "process"]);
        });
        assert!(
            e.contains("contributes.actions needs the \"actions\" permission"),
            "{e}"
        );
    }

    #[test]
    fn icons_are_at_most_thirty_two_named_path_strings_of_path_characters_only() {
        let many: Map<String, Value> = (0..33)
            .map(|i| (format!("i{i}"), Value::from("M0 0h1")))
            .collect();
        assert!(
            plugin_err(|v| v["contributes"]["icons"] = Value::Object(many.clone()))
                .contains("at most 32 icons")
        );
        for ok in [
            "M4 12l5 5L20 6",
            "m1,2 3,4z",
            "M1e-3 .5 -2+3",
            "M0 0C1 2 3 4 5 6S7 8 9 10Q1 1 2 2T3 3A1 1 0 0 1 4 4H5V6Z",
        ] {
            assert!(
                with_plugin(|v| v["contributes"]["icons"]["check"] = ok.into()).is_ok(),
                "{ok}"
            );
        }
        for (bad, want) in [
            ("L4 12", "move command"),
            ("", "move command"),
            ("M0 0<script>", "only path commands"),
            ("M0 0 url(#x)", "only path commands"),
            ("M0 0\"/><a", "only path commands"),
        ] {
            let e = plugin_err(|v| v["contributes"]["icons"]["check"] = bad.into());
            assert!(
                e.contains("contributes.icons.check") && e.contains(want),
                "{bad:?}: {e}"
            );
        }
        let big = format!("M{}", " 1".repeat(MAX_ICON_PATH_BYTES));
        assert!(
            plugin_err(|v| v["contributes"]["icons"]["check"] = big.into())
                .contains("the limit is 2048")
        );
        // At most 256 commands, every number within +-1000, and every number one that reads.
        let most = format!("M0 0{}", "l1 1".repeat(MAX_ICON_COMMANDS - 1));
        assert!(with_plugin(|v| v["contributes"]["icons"]["check"] = most.clone().into()).is_ok());
        let more = format!("{most}z");
        let e = plugin_err(|v| v["contributes"]["icons"]["check"] = more.into());
        assert!(e.contains("257 commands") && e.contains("256"), "{e}");
        for ok in ["M-1000 1000", "M1e3 0", "M.5.5-1.5-.5"] {
            assert!(
                with_plugin(|v| v["contributes"]["icons"]["check"] = ok.into()).is_ok(),
                "{ok}"
            );
        }
        for (bad, want) in [
            ("M1001 0", "1001"),
            ("M0 -2000", "-2000"),
            ("M1e4 0", "10000"),
            ("M0 0l1e400 0", "cannot be read"),
            ("M1e 0", "cannot be read"),
            ("M0 - 1", "cannot be read"),
        ] {
            let e = plugin_err(|v| v["contributes"]["icons"]["check"] = bad.into());
            assert!(e.contains(want), "{bad:?}: {e}");
        }
        assert!(
            plugin_err(|v| v["contributes"]["icons"]["check"] = 1.into())
                .contains("must be an SVG path")
        );
        let e = plugin_err(|v| v["contributes"]["icons"] = serde_json::json!({"Big": "M0 0"}));
        assert!(e.contains("the icon name \"Big\""), "{e}");
        let e = plugin_err(|v| {
            v["contributes"]["icons"] = serde_json::json!({"phone": "M0 0", "check": "M0 0"})
        });
        assert!(e.contains("\"phone\" is a built-in icon"), "{e}");
        assert!(
            plugin_err(|v| v["contributes"]["icons"] = serde_json::json!([]))
                .contains("contributes.icons must be an object")
        );
        assert!(
            plugin_err(|v| v["contributes"] = serde_json::json!([]))
                .contains("contributes must be an object")
        );
    }

    #[test]
    fn the_helper_is_an_exe_in_the_folder_with_a_few_short_args_and_needs_process() {
        assert!(
            with_plugin(|v| {
                v["helper"].as_object_mut().unwrap().remove("args");
            })
            .is_ok()
        );
        for (bad, want) in [
            ("helper.bat", ".exe"),
            ("../helper.exe", "'..'"),
            ("C:\\tools\\helper.exe", "':'"),
            ("\\\\server\\helper.exe", "absolute"),
        ] {
            let e = plugin_err(|v| v["helper"]["command"] = bad.into());
            assert!(
                e.contains("helper.command") && e.contains(want),
                "{bad:?}: {e}"
            );
        }
        let many: Vec<Value> = (0..17).map(|i| Value::from(format!("a{i}"))).collect();
        assert!(
            plugin_err(|v| v["helper"]["args"] = Value::Array(many.clone())).contains("at most 16")
        );
        assert!(
            plugin_err(|v| v["helper"]["args"] = serde_json::json!(["a".repeat(201)]))
                .contains("longer than 200")
        );
        assert!(
            plugin_err(|v| v["helper"]["args"] = serde_json::json!(["a\nb"])).contains("control")
        );
        assert!(
            plugin_err(|v| v["helper"]["args"] = serde_json::json!([1]))
                .contains("helper.args[0] must be text")
        );
        assert!(plugin_err(|v| v["helper"]["args"] = "x".into()).contains("list of text"));
        assert_eq!(
            plugin_err(|v| {
                v["helper"].as_object_mut().unwrap().remove("command");
            }),
            "helper.command is missing"
        );
        let e = plugin_err(|v| {
            v["permissions"] =
                serde_json::json!(["panel", "tools", "decorate", "sidebar", "actions", "bar"]);
        });
        assert!(e.contains("needs the \"process\" permission"), "{e}");
        let e = plugin_err(|v| remove(v, "helper"));
        assert!(
            e.contains("the \"process\" permission needs a helper"),
            "{e}"
        );
    }

    #[test]
    fn a_plugins_tools_may_be_answered_by_its_background_page_instead_of_a_panel() {
        let m = with_plugin(|v| {
            remove(v, "panel");
            v["permissions"] =
                serde_json::json!(["tools", "decorate", "sidebar", "actions", "bar", "process"]);
        })
        .unwrap();
        assert!(m.panel.is_none() && m.tools.len() == 1);
        // A helper alone answers them too (its connection hears the `tool` event).
        let m = with_plugin(|v| {
            remove(v, "panel");
            remove(v, "background");
            v["permissions"] =
                serde_json::json!(["tools", "decorate", "sidebar", "actions", "bar", "process"]);
        })
        .unwrap();
        assert!(m.panel.is_none() && m.background.is_none() && m.helper.is_some());
        let e = plugin_err(|v| {
            remove(v, "panel");
            remove(v, "background");
            remove(v, "helper");
            v["permissions"] =
                serde_json::json!(["tools", "decorate", "sidebar", "actions", "bar"]);
        });
        assert!(
            e.contains("tools need a panel, a background page or a helper"),
            "{e}"
        );
    }

    #[test]
    fn every_protocol_1_rule_holds_in_protocol_2() {
        assert!(plugin_err(|v| v["id"] = "ab".into()).contains("id must be 3 to 40"));
        assert!(plugin_err(|v| v["id"] = "con".into()).contains("device name"));
        assert!(plugin_err(|v| v["version"] = "1.0".into()).contains("semantic version"));
        assert!(plugin_err(|v| v["name"] = "two\nlines".into()).contains("control character"));
        assert!(plugin_err(|v| v["panel"]["entry"] = "../p.html".into()).contains("panel.entry"));
        assert!(plugin_err(|v| v["tools"][0]["name"] = "Bad".into()).contains("tools[0].name"));
        assert!(
            plugin_err(|v| v["permissions"] = serde_json::json!(["panel", "panel"]))
                .contains("listed twice")
        );
        assert_eq!(
            plugin_err(|v| remove(v, "name")),
            "plugin.json is missing \"name\""
        );
        assert!(
            parse_plugin("[]")
                .unwrap_err()
                .contains("plugin.json must be a JSON object")
        );
        assert!(
            parse_plugin("{")
                .unwrap_err()
                .contains("plugin.json is not valid JSON")
        );
        let big = format!("{{\"id\":\"{}\"}}", "a".repeat(70 * 1024));
        assert!(
            parse_plugin(&big)
                .unwrap_err()
                .contains("plugin.json is larger than 64 KB")
        );
    }

    #[test]
    fn every_place_name_round_trips() {
        for p in Place::ALL {
            assert_eq!(Place::parse(p.name()), Some(p));
        }
        assert_eq!(Place::parse("Tab-Menu"), None);
    }
}

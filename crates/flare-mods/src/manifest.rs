//! `mod.json`: the strict parse and validation (`docs/surfaces/mods.md`, "The manifest").
//!
//! The file is read as one object. Unknown keys are refused at every level, so a typo (`permision`)
//! is an error and never a silently missing permission. Every refusal is a human-readable sentence that
//! names the field, for the Settings > Mods row and for the mod author. Nothing here touches the disk:
//! the folder checks (the entry file, the folder name, links) are in `discover`.

use flare_ctl::Permission;
use serde_json::{Map, Value};

/// The only protocol this build reads.
pub const PROTOCOL: u64 = 1;
/// The manifest file's name inside a mod folder.
pub const MANIFEST_FILE: &str = "mod.json";
/// The largest `mod.json` read (bytes).
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

/// The permissions a manifest may ask for. `eval` is the user's own switch for `browser.eval` (S4), so a
/// mod can never ask for it.
pub const ASKABLE: [Permission; 5] = [
    Permission::Panel,
    Permission::View,
    Permission::Browser,
    Permission::Convo,
    Permission::Tools,
];

const TOP_KEYS: [&str; 8] = [
    "id",
    "name",
    "version",
    "protocol",
    "description",
    "permissions",
    "panel",
    "tools",
];
const PANEL_KEYS: [&str; 2] = ["entry", "title"];
const TOOL_KEYS: [&str; 3] = ["name", "description", "input_schema"];

/// Device names Windows reserves; a mod id cannot be a folder named one of them.
const RESERVED_IDS: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// A validated manifest.
#[derive(Clone, Debug, PartialEq)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    /// A semver string, kept as written.
    pub version: String,
    pub description: String,
    /// In the order the manifest lists them, without duplicates.
    pub permissions: Vec<Permission>,
    pub panel: Option<Panel>,
    pub tools: Vec<ToolDef>,
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

/// Parses and validates the text of a `mod.json`. The reason is one human-readable sentence.
pub fn parse(text: &str) -> Result<Manifest, String> {
    if text.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(format!(
            "mod.json is larger than {} KB",
            MAX_MANIFEST_BYTES / 1024
        ));
    }
    let value: Value =
        serde_json::from_str(text).map_err(|e| format!("mod.json is not valid JSON: {e}"))?;
    let obj = value
        .as_object()
        .ok_or_else(|| "mod.json must be a JSON object".to_string())?;
    no_unknown(obj, &TOP_KEYS, "mod.json")?;

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
    protocol(obj)?;
    let description = clean_text(obj, "description", "description", MAX_DESCRIPTION_CHARS)?;
    let permissions = permissions(obj)?;
    let panel = panel(obj, &permissions)?;
    let tools = tools(obj)?;
    if !tools.is_empty() && !permissions.contains(&Permission::Tools) {
        return Err("tools need the \"tools\" permission; add it to permissions".into());
    }
    // A tool is answered by the panel's page: with no panel nothing could ever answer it.
    if !tools.is_empty() && panel.is_none() {
        return Err("tools need a panel: its page answers them; add a panel".into());
    }

    Ok(Manifest {
        id,
        name,
        version,
        description,
        permissions,
        panel,
        tools,
    })
}

fn protocol(obj: &Map<String, Value>) -> Result<(), String> {
    let v = required(obj, "protocol", "protocol")?;
    match v.as_u64() {
        Some(PROTOCOL) => Ok(()),
        _ => Err(format!(
            "protocol must be the number {PROTOCOL}; this Flare reads protocol {PROTOCOL}"
        )),
    }
}

fn permissions(obj: &Map<String, Value>) -> Result<Vec<Permission>, String> {
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
                "permissions[{i}]: \"eval\" is not a permission a mod can ask for; it is the user's own switch for browser.eval"
            ));
        }
        let p = Permission::parse(name)
            .filter(|p| ASKABLE.contains(p))
            .ok_or_else(|| {
                format!(
                    "permissions[{i}]: unknown permission \"{name}\"; a mod can ask for panel, view, browser, convo or tools"
                )
            })?;
        if out.contains(&p) {
            return Err(format!("permissions[{i}]: \"{name}\" is listed twice"));
        }
        out.push(p);
    }
    Ok(out)
}

fn panel(obj: &Map<String, Value>, permissions: &[Permission]) -> Result<Option<Panel>, String> {
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
        return Err(
            "a mod with a panel needs the \"panel\" permission; add it to permissions".into(),
        );
    }
    Ok(Some(Panel { entry, title }))
}

fn tools(obj: &Map<String, Value>) -> Result<Vec<ToolDef>, String> {
    let Some(v) = obj.get("tools") else {
        return Ok(Vec::new());
    };
    let list = v
        .as_array()
        .ok_or_else(|| "tools must be a list of tool objects".to_string())?;
    if list.len() > MAX_TOOLS {
        return Err(format!("a mod can declare at most {MAX_TOOLS} tools"));
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
    let s = text_of(obj, key, shown)?;
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
fn is_format_char(c: char) -> bool {
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
}

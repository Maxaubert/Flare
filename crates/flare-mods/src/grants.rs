//! The person's own choices for a plugin's permissions (2026-11-08, `docs/surfaces/plugins.md` "The details sheet"),
//! and the tiles of the details sheet that shows and changes them.
//!
//! Turning a plugin on approves what its manifest asks for; the details sheet then lets the person hold back any of
//! it. A permission held back is left out of the plugin's live grant, so a call that needs it answers `forbidden`
//! exactly like one the plugin never asked for, and what it would open (a helper, a menu item, a tool) is not there.
//! The choices are kept with the switch in `mods.json` (`enabled`), survive updates and an off and on, and go with a
//! removal. They never widen anything: a permission the manifest does not ask for is never granted, and a new one
//! still turns the plugin off until it is approved.
//!
//! The sheet is data, the same for every plugin: one tile per permission the manifest asks for, in the words of the
//! catalog below, then the facts (its program, its homepage). A permission with levels (`process`, `input`, `fill`)
//! is a dropdown; every other permission is an on/off toggle.

use flare_ctl::Permission;

use crate::manifest::{Manifest, Place};

/// What the person chose for one plugin, beyond the switch. The default holds nothing back.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Choices {
    /// Permissions the plugin asks for that the person turned off, sorted by name, each once.
    pub withheld: Vec<Permission>,
    /// `fill` is "Always allow": a fill this plugin asks for is typed with no question.
    pub fill_always: bool,
}

impl Choices {
    /// Whether nothing is held back and nothing is allowed beyond the default.
    pub fn is_default(&self) -> bool {
        *self == Choices::default()
    }

    /// The permissions the plugin holds: what it asks for, less what the person held back.
    pub fn grants(&self, asked: &[Permission]) -> Vec<Permission> {
        asked
            .iter()
            .copied()
            .filter(|p| !self.withheld.contains(p))
            .collect()
    }

    /// Whether a fill of this plugin goes in with no question: `fill` is held and set to "Always allow".
    pub fn fills_without_asking(&self) -> bool {
        self.fill_always && !self.withheld.contains(&Permission::Fill)
    }
}

/// How a level is drawn: plain, in the plugin's brand colour (the usual choice), or amber (a choice that hands over
/// more than the usual: full control, a fill with no question). Amber is never drawn as neutral.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Plain,
    Brand,
    Warn,
}

/// One level of a dropdown permission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Level {
    pub id: &'static str,
    pub name: &'static str,
    pub tone: Tone,
}

/// The on and off values of a toggle tile.
pub const ON: &str = "on";
pub const OFF: &str = "off";

/// `process` and `input`: full control, or nothing.
pub const LEVELS_FULL: [Level; 2] = [
    Level {
        id: "full",
        name: "Full control",
        tone: Tone::Warn,
    },
    Level {
        id: OFF,
        name: "Off",
        tone: Tone::Plain,
    },
];

/// `fill`: Flare asks before each fill, fills with no question, or the plugin fills nothing (and asks no secret).
pub const LEVELS_FILL: [Level; 3] = [
    Level {
        id: "ask",
        name: "Ask each time",
        tone: Tone::Brand,
    },
    Level {
        id: "always",
        name: "Always allow",
        tone: Tone::Warn,
    },
    Level {
        id: OFF,
        name: "Off",
        tone: Tone::Plain,
    },
];

/// The levels of a permission that has them; None for an on/off one.
pub fn levels(p: Permission) -> Option<&'static [Level]> {
    match p {
        Permission::Process | Permission::Input => Some(&LEVELS_FULL),
        Permission::Fill => Some(&LEVELS_FILL),
        _ => None,
    }
}

/// The tile's name of a permission: short, a noun the person knows. `m` sharpens two of them (one tool, actions only
/// in the tab menu).
pub fn label(p: Permission, m: Option<&Manifest>) -> &'static str {
    match p {
        Permission::Panel => "Panel",
        Permission::View => "Shows files",
        Permission::Browser => "Web page",
        Permission::Convo => "Agent's changes",
        Permission::Tools => {
            if m.is_some_and(|m| m.tools.len() == 1) {
                "Agent tool"
            } else {
                "Agent tools"
            }
        }
        Permission::Eval => "Scripts in pages",
        Permission::Tabs => "All tabs",
        Permission::Decorate => "Tab badges",
        Permission::Sidebar => "Sidebar",
        Permission::Actions => {
            let only_menu = m.is_some_and(|m| {
                !m.contributes.actions.is_empty()
                    && m.contributes
                        .actions
                        .iter()
                        .all(|a| a.places == [Place::TabMenu])
            });
            if only_menu {
                "Tab menu"
            } else {
                "Buttons and menus"
            }
        }
        Permission::Bar => "Session bar",
        Permission::Sessions => "Reads sessions",
        Permission::Input => "Typing",
        Permission::Network => "Internet",
        Permission::Process => "Access",
        Permission::AgentLooks => "Agent icons",
        Permission::Fill => "Fills logins",
    }
}

/// The level a permission is at now: a toggle's `on` or `off`, a dropdown's level id.
pub fn level_of(c: &Choices, p: Permission) -> &'static str {
    let held_back = c.withheld.contains(&p);
    match p {
        Permission::Process | Permission::Input if !held_back => "full",
        Permission::Fill if !held_back => {
            if c.fill_always {
                "always"
            } else {
                "ask"
            }
        }
        _ if held_back => OFF,
        _ => ON,
    }
}

/// Sets permission `p` to `value` (`on`/`off` for a toggle, a level id for a dropdown). Err with a sentence for a
/// value the permission does not have.
pub fn choose(c: &mut Choices, p: Permission, value: &str) -> Result<(), String> {
    let known = match levels(p) {
        Some(list) => list.iter().any(|l| l.id == value),
        None => value == ON || value == OFF,
    };
    if !known || p == Permission::Eval {
        return Err(format!("\"{value}\" is not a choice for {}", p.name()));
    }
    let off = value == OFF;
    c.withheld.retain(|q| *q != p);
    if off {
        c.withheld.push(p);
        c.withheld.sort_by_key(|q| q.name());
    }
    if p == Permission::Fill && !off {
        c.fill_always = value == "always";
    }
    Ok(())
}

/// What a tile holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TileKind {
    /// An on/off permission.
    Toggle { on: bool },
    /// A permission with levels: the level now, and every level.
    Choice {
        value: &'static str,
        levels: &'static [Level],
    },
    /// A fact the person cannot change (the plugin's program, its homepage).
    Fact { value: String },
}

/// One tile of the details sheet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tile {
    /// The permission's name for a permission tile (what a change names), `fact:<name>` for a fact.
    pub key: String,
    pub label: String,
    pub kind: TileKind,
    /// What exactly it covers, from the manifest (its tools by name, its menu items, its panel), or empty.
    pub detail: String,
    /// The permission's words in Settings (`hub::permission_words`), for the screen reader and the tooltip.
    pub words: String,
    /// The tile's tooltip: its words, then the whole of what it covers (the detail line is cut and may be elided).
    pub tip: String,
}

impl Tile {
    /// The tone the tile's value is drawn in now.
    pub fn tone(&self) -> Tone {
        match &self.kind {
            TileKind::Choice { value, levels } => levels
                .iter()
                .find(|l| l.id == *value)
                .map_or(Tone::Plain, |l| l.tone),
            _ => Tone::Plain,
        }
    }
}

/// The longest detail line a tile carries (the rest is cut with an ellipsis).
const DETAIL_MAX: usize = 120;

fn cut(s: String) -> String {
    if s.chars().count() <= DETAIL_MAX {
        return s;
    }
    let head: String = s.chars().take(DETAIL_MAX - 3).collect();
    format!("{head}...")
}

/// The words and the whole of what a tile covers, for its tooltip.
fn tip_of(words: &str, full: &str) -> String {
    if full.is_empty() {
        words.to_string()
    } else {
        format!("{words}: {full}")
    }
}

/// What a permission covers in this manifest, as one line, uncut (the tile cuts it, its tooltip does not).
fn detail(p: Permission, m: &Manifest) -> String {
    let list = |items: Vec<String>| items.join(", ");
    match p {
        Permission::Tools => list(m.tools.iter().map(|t| t.name.clone()).collect()),
        Permission::Actions => list(
            m.contributes
                .actions
                .iter()
                .map(|a| a.title.clone())
                .collect(),
        ),
        Permission::Sidebar => list(
            m.contributes
                .sidebar
                .iter()
                .map(|s| s.title.clone())
                .collect(),
        ),
        Permission::Panel => m
            .panel
            .as_ref()
            .map(|p| p.title.clone())
            .unwrap_or_default(),
        Permission::Network => "Public https sites only".to_string(),
        _ => String::new(),
    }
}

/// The program's name as the sheet shows it: the file name of `helper.command` without `.exe`.
fn program_name(command: &str) -> String {
    let file = command.rsplit(['/', '\\']).next().unwrap_or(command);
    file.strip_suffix(".exe")
        .or_else(|| file.strip_suffix(".EXE"))
        .unwrap_or(file)
        .to_string()
}

/// The tiles of the details sheet: the permissions with levels first (they matter most), then the toggles in the
/// manifest's order, then the facts. A plugin that asks for nothing has only its facts.
pub fn tiles(m: &Manifest, c: &Choices) -> Vec<Tile> {
    let mut out = Vec::new();
    let tile = |p: Permission, kind: TileKind| {
        let full = detail(p, m);
        let words = crate::hub::permission_words(p);
        Tile {
            key: p.name().to_string(),
            label: label(p, Some(m)).to_string(),
            kind,
            tip: tip_of(words, &full),
            detail: cut(full),
            words: words.to_string(),
        }
    };
    for p in [Permission::Process, Permission::Input, Permission::Fill] {
        if let (true, Some(levels)) = (m.permissions.contains(&p), levels(p)) {
            out.push(tile(
                p,
                TileKind::Choice {
                    value: level_of(c, p),
                    levels,
                },
            ));
        }
    }
    for p in m
        .permissions
        .iter()
        .copied()
        .filter(|p| levels(*p).is_none())
    {
        out.push(tile(
            p,
            TileKind::Toggle {
                on: level_of(c, p) == ON,
            },
        ));
    }
    // The program this system starts (rule 1057); a plugin whose helper is for another system says it has none here.
    if let Some(h) = m.helper_here() {
        out.push(Tile {
            key: "fact:program".into(),
            label: "Program".into(),
            kind: TileKind::Fact {
                value: program_name(&h.command),
            },
            detail: String::new(),
            words: "The program Flare runs for it".into(),
            tip: tip_of("The program Flare runs for it", &h.command),
        });
    } else if m.has_any_helper() {
        out.push(Tile {
            key: "fact:program".into(),
            label: "Program".into(),
            kind: TileKind::Fact {
                value: "None on this system".into(),
            },
            detail: String::new(),
            words: "Its program is for another system: nothing runs here".into(),
            tip: "Its program is for another system: nothing runs here".into(),
        });
    }
    if let Some(url) = &m.homepage {
        let host = url
            .trim_start_matches("https://")
            .split(['/', '?', '#'])
            .next()
            .unwrap_or_default()
            .to_string();
        out.push(Tile {
            key: "fact:homepage".into(),
            label: "Homepage".into(),
            kind: TileKind::Fact { value: host },
            detail: String::new(),
            words: url.clone(),
            tip: url.clone(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugin(extra: serde_json::Value) -> Manifest {
        let mut v = serde_json::json!({
            "id": "vault",
            "name": "Vault",
            "version": "0.2.0",
            "protocol": 2,
            "description": "Fills logins.",
            "author": "someone",
            "homepage": "https://example.com/vault",
            "permissions": ["tools", "actions", "panel", "process", "fill"],
            "panel": {"entry": "panel.html", "title": "Vault"},
            "helper": {"command": "bin\\vault-helper.exe"},
            "helpers": {"macos": {"command": "bin/vault-helper"}},
            "contributes": {"actions": [{"key": "toggle", "title": "Lock", "where": ["tab-menu"]}]},
            "tools": [{"name": "fill_login", "description": "Fills.", "input_schema": {"type": "object"}}]
        });
        if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
            // A null takes the key out.
            for (k, x) in e {
                if x.is_null() {
                    o.remove(k);
                } else {
                    o.insert(k.clone(), x.clone());
                }
            }
        }
        crate::manifest::parse_plugin(&v.to_string()).unwrap()
    }

    #[test]
    fn levels_come_first_then_toggles_in_manifest_order_then_the_facts() {
        let t = tiles(&plugin(serde_json::json!({})), &Choices::default());
        let keys: Vec<&str> = t.iter().map(|t| t.key.as_str()).collect();
        assert_eq!(
            keys,
            [
                "process",
                "fill",
                "tools",
                "actions",
                "panel",
                "fact:program",
                "fact:homepage"
            ]
        );
        let labels: Vec<&str> = t.iter().map(|t| t.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Access",
                "Fills logins",
                "Agent tool",
                "Tab menu",
                "Panel",
                "Program",
                "Homepage"
            ]
        );
        assert_eq!(
            t[0].kind,
            TileKind::Choice {
                value: "full",
                levels: &LEVELS_FULL
            }
        );
        assert_eq!(t[0].tone(), Tone::Warn);
        assert_eq!(t[1].tone(), Tone::Brand);
        assert_eq!(t[2].detail, "fill_login");
        assert_eq!(t[3].detail, "Lock");
        assert_eq!(t[4].detail, "Vault");
        assert_eq!(
            t[5].kind,
            TileKind::Fact {
                value: "vault-helper".into()
            }
        );
        assert_eq!(
            t[6].kind,
            TileKind::Fact {
                value: "example.com".into()
            }
        );
        assert!(t.iter().all(|t| !t.words.is_empty()));
        assert_eq!(t[2].tip, "Answer the agent's tool calls: fill_login");
    }

    #[test]
    fn a_long_detail_is_cut_on_the_tile_and_whole_in_its_tooltip() {
        let tools: Vec<serde_json::Value> = (0..20)
            .map(|i| serde_json::json!({"name": format!("tool_number_{i}"), "description": "d", "input_schema": {"type": "object"}}))
            .collect();
        let t = tiles(
            &plugin(serde_json::json!({"tools": tools})),
            &Choices::default(),
        );
        let tools = t.iter().find(|t| t.key == "tools").unwrap();
        assert_eq!(tools.detail.chars().count(), DETAIL_MAX);
        assert!(tools.detail.ends_with("..."));
        assert!(tools.tip.ends_with("tool_number_19"), "{}", tools.tip);
    }

    /// Rule 1057: the sheet names the program this system starts, and says so when the plugin's program is for another.
    #[test]
    fn the_program_tile_is_this_systems_program_or_says_there_is_none() {
        let windows_only = plugin(serde_json::json!({"helpers": null}));
        let program = |m: &Manifest| {
            tiles(m, &Choices::default())
                .into_iter()
                .find(|t| t.key == "fact:program")
                .map(|t| t.kind)
        };
        let none = Some(TileKind::Fact {
            value: "None on this system".into(),
        });
        let named = Some(TileKind::Fact {
            value: "vault-helper".into(),
        });
        if cfg!(target_os = "macos") {
            assert_eq!(program(&windows_only), none);
        } else {
            assert_eq!(program(&windows_only), named);
        }
        assert_eq!(program(&plugin(serde_json::json!({}))), named);
    }

    #[test]
    fn every_askable_permission_gets_a_tile_with_its_own_label() {
        let mut seen = std::collections::HashSet::new();
        for p in crate::manifest::ASKABLE_V2 {
            assert!(seen.insert(label(p, None)), "{p:?}");
            let kind = levels(p).map_or("toggle", |_| "choice");
            assert!(!label(p, None).is_empty(), "{p:?} {kind}");
        }
        // A plugin with many permissions has a tile for each.
        let m = plugin(serde_json::json!({
            "permissions": ["tools", "actions", "panel", "process", "fill", "tabs", "decorate", "sessions", "network", "input", "view", "convo", "agent-looks"],
        }));
        let t = tiles(&m, &Choices::default());
        assert_eq!(t.len(), 13 + 2);
        assert_eq!(
            &t[..3].iter().map(|t| t.key.as_str()).collect::<Vec<_>>(),
            &["process", "input", "fill"]
        );
    }

    #[test]
    fn a_choice_holds_back_or_gives_back_and_fill_always_needs_fill_held() {
        let mut c = Choices::default();
        choose(&mut c, Permission::Process, OFF).unwrap();
        choose(&mut c, Permission::Tools, OFF).unwrap();
        assert_eq!(c.withheld, [Permission::Process, Permission::Tools]);
        assert_eq!(
            c.grants(&[Permission::Tools, Permission::Panel, Permission::Process]),
            [Permission::Panel]
        );
        choose(&mut c, Permission::Process, "full").unwrap();
        assert_eq!(c.withheld, [Permission::Tools]);
        choose(&mut c, Permission::Fill, "always").unwrap();
        assert!(c.fills_without_asking());
        assert_eq!(level_of(&c, Permission::Fill), "always");
        choose(&mut c, Permission::Fill, OFF).unwrap();
        assert!(!c.fills_without_asking());
        assert_eq!(level_of(&c, Permission::Fill), OFF);
        choose(&mut c, Permission::Fill, "ask").unwrap();
        assert!(!c.fill_always && !c.fills_without_asking());
        // A value a permission does not have is refused and changes nothing.
        let before = c.clone();
        assert!(choose(&mut c, Permission::Tools, "always").is_err());
        assert!(choose(&mut c, Permission::Process, ON).is_err());
        assert!(choose(&mut c, Permission::Eval, ON).is_err());
        assert_eq!(c, before);
    }

    #[test]
    fn a_held_back_toggle_and_level_show_as_off_and_amber_is_never_plain() {
        let m = plugin(serde_json::json!({}));
        let mut c = Choices::default();
        choose(&mut c, Permission::Panel, OFF).unwrap();
        choose(&mut c, Permission::Fill, "always").unwrap();
        let t = tiles(&m, &c);
        let panel = t.iter().find(|t| t.key == "panel").unwrap();
        assert_eq!(panel.kind, TileKind::Toggle { on: false });
        let fill = t.iter().find(|t| t.key == "fill").unwrap();
        assert_eq!(fill.tone(), Tone::Warn);
        for l in LEVELS_FULL.iter().chain(&LEVELS_FILL) {
            if l.id == "full" || l.id == "always" {
                assert_eq!(l.tone, Tone::Warn, "{}", l.name);
            }
        }
    }

    #[test]
    fn a_plugin_that_asks_for_nothing_has_only_its_facts_and_no_text_has_an_em_dash() {
        let m = crate::manifest::parse_plugin(
            r#"{"id":"tiny","name":"Tiny","version":"0.1.0","protocol":2,"description":"d","permissions":[]}"#,
        )
        .unwrap();
        assert!(tiles(&m, &Choices::default()).is_empty());
        for p in crate::manifest::ASKABLE_V2 {
            assert!(!label(p, None).contains('\u{2014}'));
        }
        for l in LEVELS_FULL.iter().chain(&LEVELS_FILL) {
            assert!(!l.name.contains('\u{2014}'));
        }
    }
}

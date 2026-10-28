//! The names the agent sees for mod tools, and the table that routes a call to its mod.
//!
//! A mod tool is named `mod_<id with - as _>_<tool>`, so `notes` with `append_note` is `mod_notes_append_note`.
//! The `mod_` prefix keeps every mod tool apart from the built-in agent tools (`show_*`, `browser_*`,
//! `open_page`, `close_panel`), and the same test pins it.
//!
//! The mapping is not reversible (`-` and `_` both become `_`, and an id may end where another begins),
//! so two different (id, tool) pairs can give one name. [`ToolTable::build`] finds every such clash and
//! leaves BOTH sides out of the table: an ambiguous name never routes to either mod. The clash is
//! reported (Settings > Mods shows it); nothing is guessed.

use crate::manifest::{Manifest, ToolDef};
use serde_json::Value;
use std::collections::BTreeMap;

/// The prefix every mod tool name starts with.
pub const PREFIX: &str = "mod_";

/// The longest name the agent may see. Claude Code shows a tool as `mcp__flare__<name>`, and the model API and
/// MCP clients take at most 64 characters in a tool name, so the name itself has 64 - 12 = 52.
pub const MAX_AGENT_NAME: usize = 64 - "mcp__flare__".len();

/// The agent-facing name of a mod's tool.
pub fn tool_name(id: &str, tool: &str) -> String {
    format!("{PREFIX}{}_{tool}", id.replace('-', "_"))
}

/// A mod tool whose agent-facing name is longer than [`MAX_AGENT_NAME`]. It is left out of the table, and
/// Settings > Mods says so (the same way as a clash).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TooLong {
    pub mod_id: String,
    pub tool: String,
    pub name: String,
}

/// One routed tool: the agent's name, the mod and its tool, and what the agent is told about it.
#[derive(Clone, Debug, PartialEq)]
pub struct Route {
    pub name: String,
    pub mod_id: String,
    pub tool: String,
    pub description: String,
    pub input_schema: Value,
}

/// Two or more (mod, tool) pairs that gave the same agent-facing name. Both are left out of the table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Collision {
    pub name: String,
    /// The clashing (mod id, tool name) pairs, sorted.
    pub claims: Vec<(String, String)>,
}

/// Every enabled mod's tools, by agent-facing name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolTable {
    routes: BTreeMap<String, Route>,
}

impl ToolTable {
    /// Builds the table from the manifests of the enabled mods. Returns the table, every clash and every tool
    /// whose name is too long for the agent (both left out of the table).
    pub fn build<'a>(
        mods: impl IntoIterator<Item = &'a Manifest>,
    ) -> (ToolTable, Vec<Collision>, Vec<TooLong>) {
        let mut by_name: BTreeMap<String, Vec<Route>> = BTreeMap::new();
        let mut too_long = Vec::new();
        for m in mods {
            for t in &m.tools {
                let route = route_of(&m.id, t);
                if route.name.len() > MAX_AGENT_NAME {
                    too_long.push(TooLong {
                        mod_id: m.id.clone(),
                        tool: t.name.clone(),
                        name: route.name,
                    });
                    continue;
                }
                by_name.entry(route.name.clone()).or_default().push(route);
            }
        }
        let mut routes = BTreeMap::new();
        let mut collisions = Vec::new();
        for (name, mut claims) in by_name {
            if claims.len() == 1 {
                let r = claims.remove(0);
                routes.insert(name, r);
            } else {
                let mut pairs: Vec<(String, String)> = claims
                    .iter()
                    .map(|r| (r.mod_id.clone(), r.tool.clone()))
                    .collect();
                pairs.sort();
                collisions.push(Collision {
                    name,
                    claims: pairs,
                });
            }
        }
        (ToolTable { routes }, collisions, too_long)
    }

    /// The route for an agent-facing name, if it is a live, unambiguous mod tool.
    pub fn route(&self, name: &str) -> Option<&Route> {
        self.routes.get(name)
    }

    /// Every routed tool, sorted by name.
    pub fn tools(&self) -> impl Iterator<Item = &Route> {
        self.routes.values()
    }

    pub fn len(&self) -> usize {
        self.routes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.routes.is_empty()
    }
}

fn route_of(mod_id: &str, t: &ToolDef) -> Route {
    Route {
        name: tool_name(mod_id, &t.name),
        mod_id: mod_id.to_string(),
        tool: t.name.clone(),
        description: t.description.clone(),
        input_schema: t.input_schema.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::parse;

    /// A manifest with the named tools (a panel to answer them, and the tools permission).
    fn mod_with_tools(id: &str, tools: &[&str]) -> Manifest {
        let list: Vec<Value> = tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "name": t,
                    "description": format!("the {t} tool"),
                    "input_schema": { "type": "object" }
                })
            })
            .collect();
        let text = serde_json::json!({
            "id": id,
            "name": "Test",
            "version": "1.0.0",
            "protocol": 1,
            "description": "test",
            "permissions": ["panel", "tools"],
            "panel": { "entry": "index.html", "title": "Test" },
            "tools": list,
        })
        .to_string();
        parse(&text).unwrap()
    }

    #[test]
    fn dashes_become_underscores_and_the_prefix_is_mod() {
        assert_eq!(tool_name("notes", "append_note"), "mod_notes_append_note");
        assert_eq!(tool_name("my-tool-kit", "go"), "mod_my_tool_kit_go");
    }

    #[test]
    fn every_mod_tool_name_is_clear_of_the_built_in_agent_tools() {
        let builtin = [
            "show_image",
            "show_file",
            "show_text",
            "show_diagram",
            "show_audio",
            "show_html",
            "open_page",
            "browser_look",
            "browser_click",
            "browser_type",
            "browser_press",
            "browser_scroll",
            "browser_navigate",
            "browser_wait",
            "browser_eval",
            "browser_state",
            "close_panel",
            "set_tab_badge",
            "clear_tab_badge",
            "set_tab_progress",
            "clear_tab_progress",
        ];
        for id in ["notes", "a-b-c", "zzz"] {
            let name = tool_name(id, "show_image");
            assert!(name.starts_with(PREFIX));
            assert!(!builtin.contains(&name.as_str()));
        }
    }

    #[test]
    fn a_tool_routes_to_its_mod_and_its_tool() {
        let m = mod_with_tools("notes", &["append_note"]);
        let (table, clashes, long) = ToolTable::build([&m]);
        assert!(clashes.is_empty() && long.is_empty());
        let r = table.route("mod_notes_append_note").unwrap();
        assert_eq!(r.mod_id, "notes");
        assert_eq!(r.tool, "append_note");
        assert_eq!(r.description, "the append_note tool");
        assert_eq!(table.len(), 1);
        assert!(table.route("mod_notes_nope").is_none());
    }

    #[test]
    fn only_enabled_mods_are_in_the_table() {
        let on = mod_with_tools("notes", &["append_note"]);
        let (table, _, _) = ToolTable::build(std::iter::empty::<&Manifest>());
        assert!(table.is_empty());
        let (table, _, _) = ToolTable::build([&on]);
        assert_eq!(table.tools().count(), 1);
    }

    #[test]
    fn a_name_reached_by_two_mods_is_a_collision_and_routes_to_neither() {
        // "ab-c" + "d_e" and "ab-c-d" + "e" both become mod_ab_c_d_e.
        let one = mod_with_tools("ab-c", &["d_e"]);
        let two = mod_with_tools("ab-c-d", &["e"]);
        let ok = mod_with_tools("other", &["fine"]);
        let (table, clashes, _) = ToolTable::build([&one, &two, &ok]);
        assert_eq!(clashes.len(), 1);
        assert_eq!(clashes[0].name, "mod_ab_c_d_e");
        assert_eq!(
            clashes[0].claims,
            vec![
                ("ab-c".to_string(), "d_e".to_string()),
                ("ab-c-d".to_string(), "e".to_string()),
            ]
        );
        assert!(table.route("mod_ab_c_d_e").is_none(), "neither side routes");
        assert!(
            table.route("mod_other_fine").is_some(),
            "the rest still does"
        );
    }

    #[test]
    fn the_same_mod_listed_twice_is_still_one_clash_not_a_panic() {
        let a = mod_with_tools("ab-c", &["d_e"]);
        let b = mod_with_tools("ab-c", &["d_e"]);
        let (table, clashes, _) = ToolTable::build([&a, &b]);
        assert_eq!(clashes.len(), 1);
        assert!(table.is_empty());
    }

    #[test]
    fn tools_come_out_sorted_by_name() {
        let m = mod_with_tools("notes", &["zeta", "alpha"]);
        let (table, _, _) = ToolTable::build([&m]);
        let names: Vec<&str> = table.tools().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["mod_notes_alpha", "mod_notes_zeta"]);
    }

    #[test]
    fn a_name_the_agent_cannot_take_is_left_out_and_reported() {
        // 40-character id and 40-character tool: "mod_" + 40 + "_" + 40 = 85 characters, over the 52 the agent gets.
        let long_id = "a".repeat(40);
        let long_tool = "t".repeat(40);
        let m = mod_with_tools(&long_id, &[long_tool.as_str(), "short"]);
        let (table, clashes, too_long) = ToolTable::build([&m]);
        assert!(clashes.is_empty());
        assert_eq!(too_long.len(), 1);
        assert_eq!(too_long[0].mod_id, long_id);
        assert_eq!(too_long[0].tool, long_tool);
        assert_eq!(too_long[0].name.len(), 85);
        assert_eq!(table.len(), 1, "the short tool still routes");
        assert!(table.route(&too_long[0].name).is_none());
    }

    #[test]
    fn the_longest_allowed_name_fits_the_mcp_limit_with_its_prefix() {
        // The boundary: 4 + 40 + 1 + 7 = 52, the most the agent can see, and "mcp__flare__" in front makes 64.
        let id = "a".repeat(40);
        let m = mod_with_tools(&id, &["seven77"]);
        let (table, _, too_long) = ToolTable::build([&m]);
        assert!(too_long.is_empty());
        let r = table.tools().next().unwrap();
        assert_eq!(r.name.len(), MAX_AGENT_NAME);
        assert_eq!(format!("mcp__flare__{}", r.name).len(), 64);
    }
}

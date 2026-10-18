//! The agent launch probe (Task 28): `launch.state`, which tabs show the loading ring and what each
//! tab's mark says. Read-only: it asks the tab rows the window shows and sets nothing. The resume
//! skeleton's own flag is not reported here: the window holds it as an input property that no probe
//! can read back (the launchSkeleton notes in scenarios/agent_launch.rs say what that drops).

use serde_json::{Value, json};
use slint::Model;

use super::agent_hooks::state_word;
use super::{ProbeCtx, ProbeRegistry};

/// Adds this file's probes to `r`.
pub fn register(r: &mut ProbeRegistry) {
    r.add("launch.state", launch_state);
}

/// One tab as the row holds it: its id, whether the loading ring turns on it, and its mark code.
#[derive(Clone, Debug, PartialEq)]
pub struct LaunchFacts {
    pub id: String,
    pub loading: bool,
    pub mark: i32,
}

/// `{ tabs: [ { id, loading, state } ], rings }`: `rings` counts the tabs whose loading ring is on,
/// and `state` is the word for the tab's mark (the words `app.state` uses).
pub fn answer(tabs: &[LaunchFacts]) -> Value {
    let rings = tabs.iter().filter(|t| t.loading).count();
    let rows: Vec<Value> = tabs
        .iter()
        .map(|t| {
            json!({
                "id": t.id,
                "loading": t.loading,
                "state": state_word(t.mark),
            })
        })
        .collect();
    json!({ "tabs": rows, "rings": rings })
}

/// `launch.state`: every tab row the window shows (the settings tab is not a terminal tab).
fn launch_state(ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    let tabs: Vec<LaunchFacts> = ctx
        .app
        .get_tab_rows()
        .iter()
        .filter(|row| !row.settings)
        .map(|row| LaunchFacts {
            id: row.id.to_string(),
            loading: row.loading,
            mark: row.mark_state,
        })
        .collect();
    Ok(answer(&tabs))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(id: &str, loading: bool, mark: i32) -> LaunchFacts {
        LaunchFacts {
            id: id.into(),
            loading,
            mark,
        }
    }

    #[test]
    fn rings_count_only_the_tabs_whose_loading_ring_is_on() {
        let a = answer(&[tab("a", true, 0), tab("b", false, 1), tab("c", true, 0)]);
        assert_eq!(a["rings"], 2);
        assert_eq!(a["tabs"][0]["loading"], true);
        assert_eq!(a["tabs"][1]["loading"], false);
    }

    #[test]
    fn no_loading_tab_means_no_ring() {
        let a = answer(&[tab("a", false, 0), tab("b", false, 3)]);
        assert_eq!(a["rings"], 0);
    }

    #[test]
    fn a_tab_with_no_tabs_has_no_rings_and_no_rows() {
        let a = answer(&[]);
        assert_eq!(a["rings"], 0);
        assert_eq!(a["tabs"].as_array().map(Vec::len), Some(0));
    }

    #[test]
    fn each_row_says_its_mark_in_the_words_app_state_uses() {
        let a = answer(&[
            tab("a", false, 0),
            tab("b", false, 1),
            tab("c", false, 2),
            tab("d", false, 3),
            tab("e", false, 4),
        ]);
        let words: Vec<&str> = a["tabs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["state"].as_str().unwrap())
            .collect();
        assert_eq!(words, ["none", "working", "question", "finished", "failed"]);
    }

    #[test]
    fn the_probe_is_registered_under_its_name() {
        let mut r = ProbeRegistry::new();
        register(&mut r);
        assert!(r.names().contains(&"launch.state"));
    }
}

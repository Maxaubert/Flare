//! The agent hooks probe (Task 26): `agent.state`, what the hooks did to each tab. Read-only: it
//! asks the tab rows the window shows and the agent hub, and sets nothing.

use serde_json::{Value, json};
use slint::Model;

use super::{ProbeCtx, ProbeRegistry};

/// Adds this file's probes to `r`.
pub fn register(r: &mut ProbeRegistry) {
    r.add("agent.state", agent_state);
}

/// What one tab shows and what the hub knows of its agent.
#[derive(Clone, Debug, PartialEq)]
pub struct TabFacts {
    pub id: String,
    /// The agent kind ("claude", "codex") or "shell" when no agent is here.
    pub kind: String,
    pub present: bool,
    /// The row's mark code: 1 working, 2 question, 3 finished, 4 failed, anything else none.
    pub mark: i32,
    pub failed_kind: Option<String>,
    pub tooltip: String,
    pub polled: bool,
}

/// The word for a mark code, as `app.state` says it.
pub fn state_word(mark: i32) -> &'static str {
    match mark {
        1 => "working",
        2 => "question",
        3 => "finished",
        4 => "failed",
        _ => "none",
    }
}

/// The probe's answer. `polled` is the asked tab's flag, or every tab's when none was asked.
pub fn answer(tabs: &[TabFacts], only: Option<&str>, badge: &str) -> Value {
    let polled = match only {
        Some(id) => tabs.iter().any(|t| t.id == id && t.polled),
        None => !tabs.is_empty() && tabs.iter().all(|t| t.polled),
    };
    let rows: Vec<Value> = tabs
        .iter()
        .map(|t| {
            json!({
                "id": t.id,
                "kind": t.kind,
                "present": t.present,
                "state": state_word(t.mark),
                "question": t.mark == 2,
                "failed_kind": t.failed_kind,
                "tooltip": t.tooltip,
                "polled": t.polled,
            })
        })
        .collect();
    json!({ "tabs": rows, "badge": badge, "polled": polled })
}

/// `agent.state`: `{ tabs: [ { id, kind, present, state, question, failed_kind, tooltip, polled } ],
/// badge, polled }`. `badge` is what the taskbar button was last told ("1 tab needs a look"), empty
/// when it is clear. `polled` says the process poll has answered for the tab asked (`{ tab }`), or
/// for every tab.
fn agent_state(ctx: &ProbeCtx, arg: &Value) -> Result<Value, String> {
    let only = arg.get("tab").and_then(Value::as_str);
    let c = ctx.ctl.try_borrow().map_err(|_| "the controller is busy")?;
    let hub = c.hub();
    let tabs: Vec<TabFacts> = ctx
        .app
        .get_tab_rows()
        .iter()
        .filter(|row| !row.settings)
        .map(|row| {
            let id = row.id.to_string();
            let present = hub.agent_here(&id);
            let kind = hub
                .agent_kind(&id)
                .filter(|_| present)
                .map_or_else(|| "shell".to_string(), |k| format!("{k:?}").to_lowercase());
            TabFacts {
                failed_kind: hub.failed_kind(&id).map(str::to_string),
                polled: hub.polled(&id),
                kind,
                present,
                mark: row.mark_state,
                tooltip: row.tip.to_string(),
                id,
            }
        })
        .collect();
    Ok(answer(&tabs, only, &flare_win::taskbar::last_description()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(id: &str, mark: i32, polled: bool) -> TabFacts {
        TabFacts {
            id: id.into(),
            kind: "claude".into(),
            present: true,
            mark,
            failed_kind: None,
            tooltip: String::new(),
            polled,
        }
    }

    #[test]
    fn the_mark_codes_have_the_words_app_state_uses() {
        let words: Vec<&str> = (0..6).map(state_word).collect();
        assert_eq!(
            words,
            ["none", "working", "question", "finished", "failed", "none"]
        );
    }

    #[test]
    fn a_question_tab_says_so_and_a_working_one_does_not() {
        let a = answer(&[tab("a", 2, true), tab("b", 1, true)], None, "");
        assert_eq!(a["tabs"][0]["state"], "question");
        assert_eq!(a["tabs"][0]["question"], true);
        assert_eq!(a["tabs"][1]["state"], "working");
        assert_eq!(a["tabs"][1]["question"], false);
    }

    #[test]
    fn a_failed_tab_carries_its_kind_and_tooltip() {
        let mut t = tab("a", 4, true);
        t.failed_kind = Some("rate_limit".into());
        t.tooltip = "proj\nFailed: rate limit".into();
        let a = answer(&[t], None, "1 tab needs a look");
        assert_eq!(a["tabs"][0]["failed_kind"], "rate_limit");
        assert_eq!(a["tabs"][0]["tooltip"], "proj\nFailed: rate limit");
        assert_eq!(a["badge"], "1 tab needs a look");
    }

    #[test]
    fn polled_is_the_asked_tabs_flag_else_every_tabs() {
        let tabs = [tab("a", 0, true), tab("b", 0, false)];
        assert_eq!(answer(&tabs, Some("a"), "")["polled"], true);
        assert_eq!(answer(&tabs, Some("b"), "")["polled"], false);
        assert_eq!(answer(&tabs, Some("zzz"), "")["polled"], false);
        assert_eq!(answer(&tabs, None, "")["polled"], false);
        assert_eq!(answer(&[tab("a", 0, true)], None, "")["polled"], true);
        assert_eq!(answer(&[], None, "")["polled"], false, "no tab, no verdict");
    }

    #[test]
    fn the_probe_is_registered_once() {
        let mut r = ProbeRegistry::new();
        register(&mut r);
        assert_eq!(r.names(), ["agent.state"]);
    }
}

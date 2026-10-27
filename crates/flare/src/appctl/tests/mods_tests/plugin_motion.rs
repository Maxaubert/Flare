//! Agent icon motions picked by a plugin (owner decision 2026-10-26, PR 2): `agents.motion.set` and
//! `agents.motion.clear` need `agent-looks`, take a motion from Flare's fixed list that fits the agent, are spent from
//! the decor rate, and the newest pick wins; a plugin turned off takes its picks with it, and a working Claude tab
//! wears the pick. The plugins are real folders, the hub and the dispatcher the real ones, the background pages the
//! fake browser's.

use super::*;
use flare_agent::icon_motion::IconMotion;

/// A protocol 2 plugin with a background page and the given permissions.
fn looks_plugin(root: &Path, id: &str, perms: &[&str]) {
    let text = format!(
        r#"{{"id":"{id}","name":"Plugin {id}","version":"1.0.0","protocol":2,"description":"A test plugin.","author":"me","homepage":"https://example.com/{id}","permissions":{perms:?},"background":"bg.html"}}"#
    );
    let to = root.join(id);
    std::fs::create_dir_all(&to).unwrap(); // sync-ok: test code only, never the app
    std::fs::write(to.join("plugin.json"), text).unwrap(); // sync-ok: test code only, never the app
    std::fs::write(to.join("bg.html"), "<p>bg</p>").unwrap(); // sync-ok: test code only, never the app
}

fn code_of(v: &Value) -> String {
    v["error"]["code"].as_str().unwrap_or("").to_string()
}

fn wears(f: &Fx, agent: AgentKind) -> IconMotion {
    f.ctl.borrow().motions.of(agent)
}

#[test]
fn a_plugin_with_agent_looks_picks_a_motion_from_the_list_and_nothing_else() {
    let (f, _a, _b) = two_tabs();
    let slot = with_fake(&f);
    let _w = world(&f, |r| {
        looks_plugin(r, "moves", &["agent-looks"]);
        looks_plugin(r, "plain", &["tabs", "decorate"]);
    });
    for id in ["moves", "plain"] {
        turn(&f, id, true);
        bg_up(&f, &slot, id);
    }
    let set = |n, params: Value| bg_call(&f, &slot, "moves", n, "agents.motion.set", params);

    let r = set(1, json!({"agent": "claude", "motion": "hop-blink"}));
    assert_eq!(
        r["result"],
        json!({"agent": "claude", "motion": "hop-blink"}),
        "{r}"
    );
    assert_eq!(wears(&f, AgentKind::Claude), IconMotion::HopBlink);
    let r = set(2, json!({"agent": "agy", "motion": "rainbow-flow"}));
    assert_eq!(r["result"]["motion"], "rainbow-flow", "{r}");

    // What is not on the list, or does not fit the agent, or is not a field, is bad_params and changes nothing.
    for (n, params, says) in [
        (
            3,
            json!({"agent": "aider", "motion": "hop"}),
            "agent is one of",
        ),
        (4, json!({"motion": "hop"}), "agent is required"),
        (
            5,
            json!({"agent": "claude", "motion": "glow"}),
            "motion for claude",
        ),
        (6, json!({"agent": "claude"}), "motion is required"),
        (
            7,
            json!({"agent": "codex", "motion": "rainbow-flow"}),
            "spin-fade",
        ),
        (
            8,
            json!({"agent": "claude", "motion": "hop", "art": "M0 0"}),
            "art is not a field",
        ),
        (9, json!({"agent": 1, "motion": "hop"}), "must be a string"),
    ] {
        let r = set(n, params);
        assert_eq!(code_of(&r), "bad_params", "{r}");
        let msg = r["error"]["message"].as_str().unwrap();
        assert!(msg.contains(says), "{msg}");
    }
    assert_eq!(wears(&f, AgentKind::Claude), IconMotion::HopBlink);
    assert_eq!(wears(&f, AgentKind::Codex), IconMotion::None);

    // Without the permission: forbidden, both methods.
    for (n, m) in [(1, "agents.motion.set"), (2, "agents.motion.clear")] {
        let r = bg_call(
            &f,
            &slot,
            "plain",
            n,
            m,
            json!({"agent": "claude", "motion": "spin"}),
        );
        assert_eq!(code_of(&r), "forbidden", "{r}");
    }

    // Clear: it had one, and the agent wears none now.
    let r = bg_call(
        &f,
        &slot,
        "moves",
        10,
        "agents.motion.clear",
        json!({"agent": "claude"}),
    );
    assert_eq!(
        r["result"],
        json!({"agent": "claude", "cleared": true, "motion": "none"}),
        "{r}"
    );
    let r = bg_call(
        &f,
        &slot,
        "moves",
        11,
        "agents.motion.clear",
        json!({"agent": "claude"}),
    );
    assert_eq!(r["result"]["cleared"], false, "{r}");
}

#[test]
fn the_newest_pick_wins_and_a_plugin_turned_off_takes_its_picks_with_it() {
    let (f, _a, _b) = two_tabs();
    let slot = with_fake(&f);
    let _w = world(&f, |r| {
        looks_plugin(r, "first", &["agent-looks"]);
        looks_plugin(r, "second", &["agent-looks"]);
    });
    for id in ["first", "second"] {
        turn(&f, id, true);
        bg_up(&f, &slot, id);
    }
    let set = |id, n, agent: &str, motion: &str| {
        bg_call(
            &f,
            &slot,
            id,
            n,
            "agents.motion.set",
            json!({"agent": agent, "motion": motion}),
        )
    };
    set("first", 1, "codex", "spin-fade");
    set("second", 1, "codex", "breathe");
    assert_eq!(wears(&f, AgentKind::Codex), IconMotion::Breathe);
    set("first", 2, "codex", "spin");
    assert_eq!(
        wears(&f, AgentKind::Codex),
        IconMotion::Spin,
        "the last writer wins"
    );
    set("first", 3, "agy", "rainbow-flow");

    // Turned off: every pick of `first` goes, and the older pick of `second` shows again.
    turn(&f, "first", false);
    assert_eq!(wears(&f, AgentKind::Codex), IconMotion::Breathe);
    assert_eq!(wears(&f, AgentKind::Agy), IconMotion::None);
    turn(&f, "second", false);
    assert_eq!(wears(&f, AgentKind::Codex), IconMotion::None);
}

#[test]
fn a_working_claude_tab_wears_the_pick_and_the_rate_is_the_decors() {
    let (f, a, _b) = two_tabs();
    let slot = with_fake(&f);
    let _w = world(&f, |r| looks_plugin(r, "moves", &["agent-looks"]));
    turn(&f, "moves", true);
    bg_up(&f, &slot, "moves");
    f.made_with("C:\\a")[0].feed(b"\x1b]777;flare-agent;state=working\x07");
    let row_of = |f: &Fx| {
        let rows = f.h.comp.get_tab_rows();
        (0..rows.row_count())
            .filter_map(|i| rows.row_data(i))
            .find(|r| r.id.as_str() == a)
            .unwrap()
    };
    f.pump_until("working", |f| row_of(f).mark_state == 1);
    assert_eq!(row_of(&f).motion, 0);
    let r = bg_call(
        &f,
        &slot,
        "moves",
        1,
        "agents.motion.set",
        json!({"agent": "claude", "motion": "hop-blink"}),
    );
    assert_eq!(code_of(&r), "", "{r}");
    f.h.advance(0);
    assert_eq!(row_of(&f).motion, IconMotion::HopBlink.code());
    // The plugin goes off: the icon stands still again.
    turn(&f, "moves", false);
    f.h.advance(0);
    assert_eq!(row_of(&f).motion, 0);

    // Ten changes a second for one agent, as decor on one tab; another agent has its own second.
    turn(&f, "moves", true);
    bg_up(&f, &slot, "moves");
    let mut limited = None;
    for n in 0..40u64 {
        let m = if n % 2 == 0 { "hop" } else { "spin" };
        let r = bg_call(
            &f,
            &slot,
            "moves",
            100 + n,
            "agents.motion.set",
            json!({"agent": "claude", "motion": m}),
        );
        if code_of(&r) == "rate_limited" {
            limited = Some(n);
            break;
        }
    }
    // The bucket holds ten (one spent above); the real clock refills it while the calls run.
    assert!(limited.is_some_and(|n| n >= 9), "{limited:?}");
    let r = bg_call(
        &f,
        &slot,
        "moves",
        200,
        "agents.motion.set",
        json!({"agent": "codex", "motion": "spin"}),
    );
    assert_eq!(code_of(&r), "", "{r}");
}

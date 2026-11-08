//! The plugins' sidebar sections and actions in the window (plugins phase 4): `sidebar.set` and `actions.set` checked
//! on the worker and drawn, a click or a tick turned into the plugin's event, click scope given and taken back, and
//! everything a plugin set gone when it stops. The plugins are real folders, the hub and the dispatcher the real
//! ones, and the background pages are played by the fake browser (`bg_*`).

use super::*;
use crate::plugin_ui::{EMPTY_NOTE, FOLDED_PREF, ORDER_PREF};

/// A protocol 2 plugin with a background page, the given permissions and `contributes`.
fn surface_plugin(root: &Path, id: &str, perms: &[&str], contributes: &str) {
    let text = format!(
        r#"{{"id":"{id}","name":"Plugin {id}","version":"1.0.0","protocol":2,"description":"A test plugin.","author":"me","homepage":"https://example.com/{id}","permissions":{perms:?},"background":"bg.html","contributes":{contributes}}}"#
    );
    let to = root.join(id);
    std::fs::create_dir_all(&to).unwrap(); // sync-ok: test code only, never the app
    std::fs::write(to.join("plugin.json"), text).unwrap(); // sync-ok: test code only, never the app
    std::fs::write(to.join("bg.html"), "<p>bg</p>").unwrap(); // sync-ok: test code only, never the app
}

/// The events of `name` the plugin's background page was handed, their data in order.
fn events_of(slot: &Slot, id: &str, name: &str) -> Vec<Value> {
    bg_posts(slot, id)
        .into_iter()
        .filter(|v| v["t"] == "event" && v["name"] == name)
        .map(|v| v["data"].clone())
        .collect()
}

/// The sections' rows as (kind, text).
fn rows_of(f: &Fx) -> Vec<(i32, String)> {
    f.ctl
        .borrow()
        .plugin_rows()
        .iter()
        .map(|r| (r.kind, r.text.to_string()))
        .collect()
}

/// The code of a call's error, or "" when it was answered.
fn code_of_reply(v: &Value) -> String {
    v["error"]["code"].as_str().unwrap_or("").to_string()
}

/// The tab's row as Slint draws it.
fn slint_row(f: &Fx, id: &str) -> TabRow {
    (0..f.h.comp.get_tab_rows().row_count())
        .filter_map(|i| f.h.comp.get_tab_rows().row_data(i))
        .find(|r| r.id.as_str() == id)
        .expect("the tab's row")
}

#[test]
fn a_section_is_drawn_under_its_plugins_name_set_by_sidebar_set_and_its_ticks_and_clicks_are_events()
 {
    let (f, a, b) = two_tabs();
    let slot = with_fake(&f);
    let _w = world(&f, |r| {
        surface_plugin(
            r,
            "lister",
            &["sidebar"],
            &format!(
                r#"{{"sidebar":[{{"key":"todos","title":"To-dos"}}],"icons":{{"tick":"{TICK}"}}}}"#
            ),
        );
        surface_plugin(
            r,
            "wide",
            &["sidebar", "tabs"],
            r#"{"sidebar":[{"key":"jobs","title":"Plugin wide"}]}"#,
        );
    });
    // Off: no section.
    assert!(rows_of(&f).is_empty());
    turn(&f, "lister", true);
    bg_up(&f, &slot, "lister");
    // On: its header under its name, with the section's title, and the empty note until it sets items.
    assert_eq!(
        rows_of(&f),
        [
            (0, "Plugin lister".to_string()),
            (2, EMPTY_NOTE.to_string())
        ]
    );
    assert_eq!(f.ctl.borrow().plugin_rows()[0].sub.as_str(), "To-dos");
    // What the worker refuses, each with its reason and nothing drawn.
    let item = |extra: Value| {
        let mut o = json!({"id": "a", "text": "Check the build"});
        for (k, v) in extra.as_object().unwrap() {
            o[k] = v.clone();
        }
        o
    };
    let many: Vec<Value> = (0..51)
        .map(|i| json!({"id": format!("i{i}"), "text": "x"}))
        .collect();
    let cases: Vec<(Value, &str)> = vec![
        (json!({"key": "nope", "items": []}), "bad_params"),
        (json!({"key": "todos"}), "bad_params"),
        (
            json!({"key": "todos", "items": [], "bogus": 1}),
            "bad_params",
        ),
        (json!({"key": "todos", "items": many}), "bad_params"),
        (
            json!({"key": "todos", "items": [item(json!({"text": "x".repeat(121)}))]}),
            "bad_params",
        ),
        (
            json!({"key": "todos", "items": [item(json!({"sub": "y".repeat(121)}))]}),
            "bad_params",
        ),
        (
            json!({"key": "todos", "items": [item(json!({"text": "two\nlines"}))]}),
            "bad_params",
        ),
        (
            json!({"key": "todos", "items": [item(json!({"text": "hid\u{200b}den"}))]}),
            "bad_params",
        ),
        (
            json!({"key": "todos", "items": [item(json!({})), item(json!({}))]}),
            "bad_params",
        ),
        (
            json!({"key": "todos", "items": [item(json!({"colour": "red"}))]}),
            "bad_params",
        ),
        (
            json!({"key": "todos", "items": [item(json!({"icon": "nope"}))]}),
            "bad_params",
        ),
        (
            json!({"key": "todos", "items": [item(json!({"checked": "yes"}))]}),
            "bad_params",
        ),
        // A tab the plugin may not see (it has no `tabs`, and nobody clicked it there).
        (
            json!({"key": "todos", "items": [item(json!({"tab": b}))]}),
            "forbidden",
        ),
    ];
    for (n, (params, want)) in cases.into_iter().enumerate() {
        let r = bg_call(&f, &slot, "lister", 10 + n as u64, "sidebar.set", params);
        assert_eq!(code_of_reply(&r), want, "case {n}: {r}");
    }
    assert_eq!(rows_of(&f).len(), 2, "nothing refused was drawn");

    // Two items: one with a checkbox, a second line and its own icon, one plain.
    let r = bg_call(
        &f,
        &slot,
        "lister",
        40,
        "sidebar.set",
        json!({"key": "todos", "items": [
            {"id": "a", "text": "Check the build", "sub": "api", "icon": "tick", "checked": false},
            {"id": "b", "text": "Read the log"},
        ]}),
    );
    assert_eq!(code_of_reply(&r), "", "{r}");
    let rows = f.ctl.borrow().plugin_rows();
    assert_eq!(rows.len(), 3);
    assert_eq!(
        (
            rows[1].kind,
            rows[1].text.as_str(),
            rows[1].sub.as_str(),
            rows[1].icon.as_str(),
            rows[1].check
        ),
        (1, "Check the build", "api", TICK, 0)
    );
    assert_eq!((rows[2].check, rows[2].jump), (-1, false));
    // The window holds the same rows.
    assert_eq!(f.h.comp.get_plugin_rows().row_count(), 3);

    // A tick on the box: drawn at once, and the plugin hears it. A click beside it says no `checked`.
    bg_message(
        &f,
        &slot,
        "lister",
        r#"{"t":"listen","name":"sidebar.item"}"#,
    );
    f.ctl.borrow_mut().plugin_item("lister/todos", "a", true);
    f.ctl.borrow_mut().plugin_item("lister/todos", "b", false);
    // An item that is not there, or a section that is not drawn, says nothing.
    f.ctl.borrow_mut().plugin_item("lister/todos", "zz", false);
    f.ctl.borrow_mut().plugin_item("wide/jobs", "a", false);
    assert_eq!(
        events_of(&slot, "lister", "sidebar.item"),
        [
            json!({"key": "todos", "item": "a", "checked": true}),
            json!({"key": "todos", "item": "b"}),
        ]
    );
    assert_eq!(f.ctl.borrow().plugin_rows()[1].check, 1);

    // A plugin with `tabs` may lead an item to any tab; a click brings that tab to the front.
    turn(&f, "wide", true);
    bg_up(&f, &slot, "wide");
    let r = bg_call(
        &f,
        &slot,
        "wide",
        1,
        "sidebar.set",
        json!({"key": "jobs", "items": [{"id": "j", "text": "Go to b", "tab": b}]}),
    );
    assert_eq!(code_of_reply(&r), "", "{r}");
    let rows = rows_of(&f);
    assert_eq!(rows[0], (0, "Plugin lister".to_string()));
    assert!(rows.contains(&(0, "Plugin wide".to_string())));
    let jump = f
        .ctl
        .borrow()
        .plugin_rows()
        .into_iter()
        .find(|r| r.id.as_str() == "j")
        .unwrap();
    assert!(jump.jump);
    assert_eq!(f.ctl.borrow().front_id().as_deref(), Some(a.as_str()));
    f.ctl.borrow_mut().plugin_item("wide/jobs", "j", false);
    assert_eq!(f.ctl.borrow().front_id().as_deref(), Some(b.as_str()));

    // Turned off: its section and everything it set go; on again, it starts empty.
    turn(&f, "lister", false);
    assert!(!rows_of(&f).iter().any(|(_, t)| t == "Plugin lister"));
    turn(&f, "lister", true);
    bg_up(&f, &slot, "lister");
    let rows = rows_of(&f);
    let at = rows.iter().position(|(_, t)| t == "Plugin lister").unwrap();
    assert_eq!(rows[at + 1], (2, EMPTY_NOTE.to_string()));
    // Its background page ends: what it set goes too (it sets it again when it starts).
    let r = bg_call(
        &f,
        &slot,
        "lister",
        2,
        "sidebar.set",
        json!({"key": "todos", "items": [{"id": "a", "text": "x"}]}),
    );
    assert_eq!(code_of_reply(&r), "", "{r}");
    bg_say(
        &slot,
        "lister",
        WebEvent::Gone {
            tab: "lister".into(),
            why: "x".into(),
        },
    );
    f.pump();
    let rows = rows_of(&f);
    let at = rows.iter().position(|(_, t)| t == "Plugin lister").unwrap();
    assert_eq!(rows[at + 1], (2, EMPTY_NOTE.to_string()));
}

#[test]
fn sidebar_set_is_five_a_second_for_one_section_and_only_a_plugin_calls_it() {
    let (f, a, _b) = two_tabs();
    let slot = with_fake(&f);
    let w = world(&f, |r| {
        surface_plugin(
            r,
            "busy",
            &["sidebar"],
            r#"{"sidebar":[{"key":"k","title":"K"}]}"#,
        );
    });
    turn(&f, "busy", true);
    bg_up(&f, &slot, "busy");
    let mut limited = 0;
    for n in 0..12u64 {
        let r = bg_call(
            &f,
            &slot,
            "busy",
            n + 1,
            "sidebar.set",
            json!({"key": "k", "items": [{"id": "a", "text": format!("n{n}")}]}),
        );
        if code_of_reply(&r) == "rate_limited" {
            limited += 1;
        }
    }
    assert!(
        limited >= 1,
        "twelve calls in a moment are over five a second"
    );
    // The agent cannot set a plugin's section.
    let rx = call_on_thread(
        &w.dispatcher,
        agent_in(&a),
        Method::SidebarSet,
        json!({"key": "k", "items": []}),
    );
    assert_eq!(answer_of(&f, &rx).unwrap_err().code, code::FORBIDDEN);
}

#[test]
fn one_tab_menu_action_is_flat_with_the_plugins_logo_and_more_are_a_flyout_whose_entry_wears_it() {
    let (f, _a, b) = two_tabs();
    let slot = with_fake(&f);
    let _w = world(&f, |r| {
        surface_plugin(
            r,
            "solo",
            &["actions"],
            r##"{"actions":[{"key":"toggle","title":"Lock it","where":["tab-menu"]}]},"brand":{"color":"#175ddc","icon":"M3 3h18v18H3z"}"##,
        );
        surface_plugin(
            r,
            "duo",
            &["actions"],
            r##"{"actions":[{"key":"a","title":"First","where":["tab-menu"]},{"key":"b","title":"Second","where":["tab-menu"]}]},"brand":{"color":"#2a6a2a","icon":"M4 4h16v16H4z"}"##,
        );
        surface_plugin(
            r,
            "plain",
            &["actions"],
            r#"{"actions":[{"key":"a","title":"One","where":["tab-menu"]},{"key":"b","title":"Two","where":["tab-menu"]}]}"#,
        );
    });
    for id in ["solo", "duo", "plain"] {
        turn(&f, id, true);
        bg_up(&f, &slot, id);
    }
    f.ctl.borrow_mut().open_tab_menu(&b, 10.0, 10.0);
    let rows: Vec<(MenuRowData, String)> = f.ctl.borrow().menu.as_ref().unwrap().rows.clone();
    let at = |label: &str| {
        rows.iter()
            .position(|(r, _)| r.label == label)
            .unwrap_or_else(|| panic!("{label}"))
    };
    // One action: flat, the brand's logo in its colour, its title set by the plugin.
    let solo = at("Lock it");
    assert_eq!(rows[solo].0.parent, -1);
    assert!(!rows[solo].0.flyout);
    assert_eq!(
        rows[solo].0.logo,
        Some(("M3 3h18v18H3z".to_string(), "#175ddc".to_string()))
    );
    let drawn = f.h.comp.get_menu_rows().row_data(solo).unwrap();
    assert_eq!(drawn.logo.as_str(), "M3 3h18v18H3z");
    // Its title does not name the plugin: the plugin's name is the hint.
    assert_eq!(drawn.hint.as_str(), "Plugin solo");
    assert_eq!(drawn.logo_fill, slint::Color::from_rgb_u8(0x17, 0x5d, 0xdc));
    // Two actions: a flyout under the plugin's name, the logo on the entry, the actions inside.
    let duo = at("Plugin duo");
    assert!(rows[duo].0.flyout);
    assert_eq!(
        rows[duo].0.logo.as_ref().map(|l| l.1.as_str()),
        Some("#2a6a2a")
    );
    assert_eq!(rows[at("First")].0.parent, duo as i32);
    assert_eq!(rows[at("Second")].0.parent, duo as i32);
    // No brand: Flare's chip on the entry.
    let plain = at("Plugin plain");
    assert!(rows[plain].0.flyout && rows[plain].0.logo.is_none());
    assert_eq!(
        rows[plain].0.icon_cmd,
        flare_settings::icons::icon_path("chip").unwrap()
    );
    // The title the plugin sets is the flat row's words.
    bg_message(&f, &slot, "solo", r#"{"t":"listen","name":"action"}"#);
    let r = bg_call(
        &f,
        &slot,
        "solo",
        1,
        "actions.set",
        json!({"key": "toggle", "title": "Unlock it"}),
    );
    assert_eq!(code_of_reply(&r), "", "{r}");
    f.ctl.borrow_mut().open_tab_menu(&b, 10.0, 10.0);
    let rows: Vec<(MenuRowData, String)> = f.ctl.borrow().menu.as_ref().unwrap().rows.clone();
    assert!(rows.iter().any(|(r, _)| r.label == "Unlock it"));
}

#[test]
fn actions_show_in_the_tab_menu_on_hover_and_in_the_top_bar_and_a_click_is_an_event_with_click_scope()
 {
    let (f, a, b) = two_tabs();
    let slot = with_fake(&f);
    let w = world(&f, |r| {
        surface_plugin(
            r,
            "caller",
            &["actions"],
            r#"{"actions":[{"key":"add","title":"Add to-do","icon":"plus","where":["tab-menu","tab-hover"]},{"key":"top","title":"Top","where":["top-bar"]}]}"#,
        );
    });
    turn(&f, "caller", true);
    bg_up(&f, &slot, "caller");
    bg_message(&f, &slot, "caller", r#"{"t":"listen","name":"action"}"#);

    // The tab's menu: Flare's own rows, then the plugin's one tab-menu action, flat (no flyout for one item).
    f.ctl.borrow_mut().open_tab_menu(&b, 10.0, 10.0);
    let rows: Vec<(MenuRowData, String)> = f.ctl.borrow().menu.as_ref().unwrap().rows.clone();
    assert_eq!(rows[0].0.label, "Close tab");
    assert!(
        !rows.iter().any(|(r, _)| r.label == "Plugin caller"),
        "no flyout for one action"
    );
    let child = rows
        .iter()
        .position(|(r, _)| r.label == "Add to-do")
        .expect("the action");
    assert_eq!(rows[child].0.parent, -1);
    assert!(rows[child].0.group && !rows[child].0.flyout);
    assert_eq!(rows[child].0.logo, None, "no brand: no logo");
    // No brand: the flat row wears Flare's chip, never the action's own icon (so no plugin row can pass for one of
    // Flare's own items), and the plugin's name as its hint.
    let drawn = f.h.comp.get_menu_rows().row_data(child).unwrap();
    assert_eq!(
        drawn.icon.as_str(),
        flare_settings::icons::icon_path("chip").unwrap()
    );
    assert_ne!(
        drawn.icon.as_str(),
        glyph_path(crate::menus_ui::GLYPH_BADGE_PLUS)
    );
    assert_eq!(drawn.hint.as_str(), "Plugin caller");
    f.ctl.borrow_mut().menu_picked(child as i32);
    assert_eq!(
        events_of(&slot, "caller", "action"),
        [json!({"key": "add", "tab": b})]
    );

    // The click gave it scope on tab b, and on b only.
    let hub = Arc::clone(&w.hub);
    assert!(hub.has_click_scope("caller", &b) && !hub.has_click_scope("caller", &a));
    let r = bg_call(
        &f,
        &slot,
        "caller",
        1,
        "actions.set",
        json!({"key": "add", "tab": b, "checked": true}),
    );
    assert_eq!(code_of_reply(&r), "", "{r}");
    let r = bg_call(
        &f,
        &slot,
        "caller",
        2,
        "actions.set",
        json!({"key": "add", "tab": a, "checked": true}),
    );
    assert_eq!(code_of_reply(&r), "forbidden", "{r}");
    // What it set shows on that tab's hover button, ticked; tab a's is not.
    let rb = slint_row(&f, &b);
    assert_eq!(rb.hov_count, 1);
    assert!(rb.hov1_on && !rb.hov1_off);
    assert_eq!(rb.hov1_tip.as_str(), "Plugin caller: Add to-do");
    assert_eq!(
        rb.hov1_cmd.as_str(),
        glyph_path(crate::menus_ui::GLYPH_BADGE_PLUS)
    );
    assert!(!slint_row(&f, &a).hov1_on);

    // The hover button on tab a: an event for a, and scope there too.
    f.ctl.borrow_mut().tab_hover_action(&a, 0);
    assert_eq!(
        events_of(&slot, "caller", "action").last().unwrap(),
        &json!({"key": "add", "tab": a})
    );
    assert!(hub.has_click_scope("caller", &a));
    // Turned off on a: the button is faint, and a click on it says nothing.
    let r = bg_call(
        &f,
        &slot,
        "caller",
        3,
        "actions.set",
        json!({"key": "add", "tab": a, "enabled": false}),
    );
    assert_eq!(code_of_reply(&r), "", "{r}");
    assert!(slint_row(&f, &a).hov1_off);
    let n = events_of(&slot, "caller", "action").len();
    f.ctl.borrow_mut().tab_hover_action(&a, 0);
    assert_eq!(events_of(&slot, "caller", "action").len(), n);

    // What the worker refuses.
    for (i, (params, want)) in [
        (json!({"key": "nope"}), "bad_params"),
        (json!({"key": "add", "colour": "red"}), "bad_params"),
        (json!({"key": "add", "title": "t".repeat(81)}), "bad_params"),
        (json!({"key": "add", "checked": "yes"}), "bad_params"),
        (json!({"key": "add", "tab": "tab-404"}), "forbidden"),
    ]
    .into_iter()
    .enumerate()
    {
        let r = bg_call(&f, &slot, "caller", 20 + i as u64, "actions.set", params);
        assert_eq!(code_of_reply(&r), want, "case {i}: {r}");
    }

    // The top bar: the plugin-wide state a background page sets (it names no tab), and a click acts on the tab
    // in front.
    let r = bg_call(
        &f,
        &slot,
        "caller",
        30,
        "actions.set",
        json!({"key": "top", "checked": true, "title": "Top on"}),
    );
    assert_eq!(code_of_reply(&r), "", "{r}");
    let top = f.ctl.borrow().plugin_top();
    assert_eq!(top.len(), 1);
    assert_eq!(top[0].tip, "Plugin caller: Top on");
    assert!(top[0].checked && top[0].enabled);
    assert_eq!((top[0].cmd.as_str(), top[0].letter.as_str()), ("", "T"));
    assert_eq!(f.h.comp.get_plugin_top_buttons().row_count(), 1);
    f.ctl.borrow_mut().plugin_top_clicked(0);
    assert_eq!(
        events_of(&slot, "caller", "action").last().unwrap(),
        &json!({"key": "top", "tab": a})
    );

    // Tab b closes: its state and the plugin's scope on it go.
    f.ctl.borrow_mut().close_now(&b);
    f.pump_until("b to close", |f| !f.ctl.borrow().is_shell_tab(&b));
    f.pump();
    assert!(!hub.has_click_scope("caller", &b));

    // Turned off: the scope, the top-bar button and what it set go; on again, it starts afresh.
    turn(&f, "caller", false);
    assert!(f.ctl.borrow().plugin_top().is_empty());
    assert!(!hub.has_click_scope("caller", &a));
    turn(&f, "caller", true);
    bg_up(&f, &slot, "caller");
    assert_eq!(f.ctl.borrow().plugin_top()[0].tip, "Plugin caller: Top");
    let r = bg_call(
        &f,
        &slot,
        "caller",
        40,
        "actions.set",
        json!({"key": "add", "tab": a, "checked": true}),
    );
    assert_eq!(code_of_reply(&r), "forbidden", "{r}");
    assert!(!slint_row(&f, &a).hov1_off, "what it set went with it");
}

#[test]
fn sections_and_buttons_are_capped_and_the_persons_order_and_folds_are_kept() {
    let (f, a, _b) = two_tabs();
    let _slot = with_fake(&f);
    let _w = world(&f, |r| {
        for i in 1..=4 {
            surface_plugin(
                r,
                &format!("plug-{i}"),
                &["sidebar", "actions"],
                r#"{"sidebar":[{"key":"one","title":"One"},{"key":"two","title":"Two"}],"actions":[{"key":"top","title":"Top","where":["top-bar","tab-hover"]}]}"#,
            );
        }
    });
    for i in 1..=4 {
        turn(&f, &format!("plug-{i}"), true);
    }
    // Eight declared, six drawn; four top-bar buttons declared, three drawn; a tab draws two hover buttons.
    let headers = |f: &Fx| -> Vec<String> {
        f.ctl
            .borrow()
            .plugin_rows()
            .iter()
            .filter(|r| r.kind == 0)
            .map(|r| r.sid.to_string())
            .collect()
    };
    let first = headers(&f);
    assert_eq!(first.len(), crate::plugin_ui::SECTIONS_MAX);
    assert_eq!(first[0], "plug-1/one");
    assert_eq!(
        f.ctl.borrow().plugin_top().len(),
        crate::plugin_ui::TOP_BAR_MAX
    );
    assert_eq!(
        slint_row(&f, &a).hov_count,
        crate::plugin_ui::HOVER_MAX as i32
    );

    // The second section's menu: up, down and fold; "Move up" puts it first, and the order is kept.
    f.ctl.borrow_mut().plugin_header_menu(&first[1], 5.0, 5.0);
    let labels: Vec<String> = f
        .ctl
        .borrow()
        .menu
        .as_ref()
        .unwrap()
        .rows
        .iter()
        .map(|(r, _)| r.label.clone())
        .collect();
    assert_eq!(labels, ["Move up", "Move down", "Fold"]);
    f.ctl.borrow_mut().menu_picked(0);
    let now = headers(&f);
    assert_eq!(now[0], first[1]);
    assert_eq!(now[1], first[0]);
    let stored = f.ctl.borrow().prefs.get(ORDER_PREF).unwrap();
    assert!(
        stored.starts_with(&format!("{}\n{}", first[1], first[0])),
        "{stored}"
    );
    // The first section cannot move up.
    f.ctl.borrow_mut().plugin_header_menu(&now[0], 5.0, 5.0);
    assert!(f.ctl.borrow().menu.as_ref().unwrap().rows[0].0.disabled);
    f.ctl.borrow_mut().close_menu();

    // A fold leaves only the header, and is kept.
    f.ctl.borrow_mut().plugin_fold(&now[0]);
    let rows = f.ctl.borrow().plugin_rows();
    assert!(rows[0].folded);
    assert_eq!(rows[1].kind, 0, "no rows under a folded header");
    assert_eq!(f.ctl.borrow().prefs.get(FOLDED_PREF).unwrap(), now[0]);
    f.ctl.borrow_mut().plugin_fold(&now[0]);
    assert!(!f.ctl.borrow().plugin_rows()[0].folded);
}

#[test]
fn the_plugins_dropdown_opens_from_the_top_bar_only_while_a_section_is_drawn() {
    let (f, _a, _b) = two_tabs();
    let _slot = with_fake(&f);
    let _w = world(&f, |r| {
        surface_plugin(
            r,
            "lister",
            &["sidebar"],
            r#"{"sidebar":[{"key":"todos","title":"To-dos"}]}"#,
        );
    });
    // Nothing to show: the button does nothing.
    f.ctl.borrow_mut().plugins_clicked();
    assert!(!f.h.comp.get_plugins_open());
    turn(&f, "lister", true);
    f.ctl.borrow_mut().plugins_clicked();
    assert!(f.h.comp.get_plugins_open());
    assert!(
        f.h.comp.get_overlay_open(),
        "the dropdown hides a web panel"
    );
    f.ctl.borrow_mut().plugins_dismissed();
    assert!(!f.h.comp.get_plugins_open());
    // Open, and the plugin turned off: it closes with its last section.
    f.ctl.borrow_mut().plugins_clicked();
    turn(&f, "lister", false);
    assert!(!f.h.comp.get_plugins_open());
}

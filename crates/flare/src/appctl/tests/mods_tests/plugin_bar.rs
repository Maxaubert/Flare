//! The plugins' session bar in the window (plugins phase 6): `bar.show` and `bar.hide` checked on the worker and
//! applied, the terminal giving up the bar's height (one fit), one bar a tab (`busy` for a second plugin), the bar's
//! page made lazily for the tab in front and hidden behind, its `session-bar` actions drawn as chips, and every bar
//! gone when its plugin stops or its tab closes. The plugins are real folders, the hub and the dispatcher the real
//! ones, and the pages are played by the fake browser.

use super::*;
use flare_surfaces::app::AppSpec;
use flare_surfaces::web::{PhysRect, phys_rect};

/// A protocol 2 plugin with a background page and a bar `height` tall (0: no bar), the given permissions and
/// `contributes`.
fn bar_plugin(root: &Path, id: &str, perms: &[&str], height: u32, contributes: &str) {
    let bar = if height == 0 {
        String::new()
    } else {
        format!(r#","bar":{{"entry":"bar.html","height":{height}}}"#)
    };
    let text = format!(
        r#"{{"id":"{id}","name":"Plugin {id}","version":"1.0.0","protocol":2,"description":"A test plugin.","author":"me","homepage":"https://example.com/{id}","permissions":{perms:?},"background":"bg.html"{bar},"contributes":{contributes}}}"#
    );
    let to = root.join(id);
    std::fs::create_dir_all(&to).unwrap(); // sync-ok: test code only, never the app
    std::fs::write(to.join("plugin.json"), text).unwrap(); // sync-ok: test code only, never the app
    std::fs::write(to.join("bg.html"), "<p>bg</p>").unwrap(); // sync-ok: test code only, never the app
    std::fs::write(to.join("bar.html"), "<p>bar</p>").unwrap(); // sync-ok: test code only, never the app
}

/// The code of a call's error, or "" when it was answered.
fn code_of_reply(v: &Value) -> String {
    v["error"]["code"].as_str().unwrap_or("").to_string()
}

/// The events of `name` the plugin's background page was handed, their data in order.
fn events_of(slot: &Slot, id: &str, name: &str) -> Vec<Value> {
    bg_posts(slot, id)
        .into_iter()
        .filter(|v| v["t"] == "event" && v["name"] == name)
        .map(|v| v["data"].clone())
        .collect()
}

/// Counts the grid's fits (the window's `grid-resized`): each one resizes the pty.
fn count_fits(f: &Fx) -> Rc<Cell<u32>> {
    let n = Rc::new(Cell::new(0u32));
    let c = n.clone();
    // untimed: a test counter
    f.h.comp.on_grid_resized(move |_, _, _| c.set(c.get() + 1));
    f.h.advance(0);
    n
}

/// The bar views the browser was asked to open for the tab: their spec, rectangle and visibility.
fn bar_opens(slot: &Slot, tab: &str) -> Vec<(AppSpec, PhysRect, bool)> {
    fake(slot)
        .calls()
        .into_iter()
        .filter_map(|c| match c {
            Call::OpenView {
                slot,
                app,
                bounds,
                visible,
            } if slot == View::bar(tab) => Some((app, bounds, visible)),
            _ => None,
        })
        .collect()
}

/// What the tab's bar view was told about being on screen, in order.
fn bar_shown(slot: &Slot, tab: &str) -> Vec<bool> {
    fake(slot)
        .calls()
        .into_iter()
        .filter_map(|c| match c {
            Call::ViewVisible(s, v) if s == View::bar(tab) => Some(v),
            _ => None,
        })
        .collect()
}

fn bar_closes(slot: &Slot, tab: &str) -> usize {
    fake(slot)
        .calls()
        .into_iter()
        .filter(|c| matches!(c, Call::CloseView(s) if *s == View::bar(tab)))
        .count()
}

/// The rectangle the layout gives the bar's page now, in device pixels.
fn bar_rect(f: &Fx) -> PhysRect {
    let c = &f.h.comp;
    phys_rect(
        c.get_bar_web_x(),
        c.get_bar_web_y(),
        c.get_bar_web_w(),
        c.get_bar_web_h(),
        c.get_scale(),
    )
}

/// Plays the browser for the tab's bar page: it loads.
fn bar_page_up(f: &Fx, slot: &Slot, tab: &str, id: &str) {
    let url = modview::mod_url(&modview::mod_host(id));
    for e in [
        WebEvent::Loading {
            tab: tab.into(),
            url: url.clone(),
            nav: 77,
        },
        WebEvent::Loaded {
            tab: tab.into(),
            url,
            ok: true,
            status: "0".into(),
            nav: 77,
        },
    ] {
        fake(slot).say(WebEvent::View {
            slot: View::bar(tab),
            event: Box::new(e),
        });
    }
    f.pump();
}

/// What was posted to the tab's bar page.
fn bar_posts(slot: &Slot, tab: &str) -> Vec<Value> {
    fake(slot)
        .calls()
        .into_iter()
        .filter_map(|c| match c {
            Call::PostView(s, json) if s == View::bar(tab) => {
                Some(serde_json::from_str(&json).unwrap())
            }
            _ => None,
        })
        .collect()
}

#[test]
fn a_bar_shows_under_the_front_tabs_terminal_which_gives_up_its_height_once_and_a_second_plugin_is_busy()
 {
    let (f, a, b) = two_tabs();
    let slot = with_fake(&f);
    let _w = world(&f, |r| {
        bar_plugin(r, "player", &["bar", "tabs"], 48, "{}");
        bar_plugin(r, "other", &["bar", "tabs"], 30, "{}");
        bar_plugin(r, "nobar", &["tabs"], 0, "{}");
    });
    for id in ["player", "other"] {
        turn(&f, id, true);
        bg_up(&f, &slot, id);
    }
    f.h.advance(0);
    let fits = count_fits(&f);
    let (h0, scale) = (f.h.comp.get_grid_phys_height(), f.h.comp.get_scale());
    assert!(!f.h.comp.get_bar_shown());

    let r = bg_call(&f, &slot, "player", 1, "bar.show", json!({"tab": a}));
    assert_eq!(r["result"], json!({"tab": a, "height": 48}), "{r}");
    f.h.advance(0);
    // The terminal gave up the bar's height, in one fit (one pty resize).
    assert!(f.h.comp.get_bar_shown());
    assert_eq!(f.h.comp.get_bar_h(), 48.0);
    let h1 = f.h.comp.get_grid_phys_height();
    assert!(
        (h0 - h1 - (48.0 * scale).round() as i32).abs() <= 1,
        "{h0} -> {h1} at {scale}"
    );
    assert_eq!(fits.get(), 1, "one fit for the bar");
    // The plugin's page opened in the bar's place, sealed as a bar, on screen, with no network.
    let opens = bar_opens(&slot, &a);
    assert_eq!(opens.len(), 1);
    let (spec, rect, visible) = &opens[0];
    assert!(*visible);
    assert_eq!(spec.module.as_ref().unwrap().id, "player");
    assert_eq!(spec.module.as_ref().unwrap().entry, "bar.html");
    assert!(!spec.allow_network);
    assert_eq!(*rect, bar_rect(&f));
    assert!(rect.h > 0 && rect.w > 0);
    // Under the grid, as wide as it.
    assert!(f.h.comp.get_bar_web_y() >= f.h.comp.get_grid_top() + h1 as f32 / scale - 1.0);
    let (bars, views) = f.ctl.borrow().bar_state();
    assert_eq!(bars, [(a.clone(), "player".to_string(), 48)]);
    assert_eq!(views, [(a.clone(), "player".to_string(), true)]);

    // At most one bar.show a second on a tab (design 13).
    let r = bg_call(&f, &slot, "player", 2, "bar.show", json!({"tab": a}));
    assert_eq!(code_of_reply(&r), "rate_limited", "{r}");
    f.h.advance(0);
    assert_eq!((fits.get(), bar_opens(&slot, &a).len()), (1, 1));

    // One bar a tab: another plugin is busy there, and free on another tab. A plugin with no bar has none to show.
    let r = bg_call(&f, &slot, "other", 1, "bar.show", json!({"tab": a}));
    assert_eq!(code_of_reply(&r), "busy", "{r}");
    assert!(
        r["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Plugin player")
    );
    let r = bg_call(&f, &slot, "other", 2, "bar.show", json!({"tab": b}));
    assert_eq!(code_of_reply(&r), "", "{r}");
    turn(&f, "nobar", true);
    bg_up(&f, &slot, "nobar");
    let r = bg_call(&f, &slot, "nobar", 1, "bar.show", json!({"tab": b}));
    assert_eq!(code_of_reply(&r), "forbidden", "no bar permission: {r}");
    // Unknown fields and a closed tab are refused.
    let r = bg_call(&f, &slot, "other", 3, "bar.hide", json!({"tab": b, "x": 1}));
    assert_eq!(code_of_reply(&r), "bad_params", "{r}");
    // b is behind: its bar has no view yet (made lazily).
    assert!(bar_opens(&slot, &b).is_empty());

    // b in front: a's bar hides, b's opens at its own height, and the terminal fits once more.
    f.ctl.borrow_mut().model.pick(&b);
    f.ctl.borrow_mut().refresh();
    f.h.advance(0);
    assert_eq!(f.h.comp.get_bar_h(), 30.0);
    assert_eq!(bar_shown(&slot, &a), [false]);
    assert_eq!(bar_opens(&slot, &b).len(), 1);
    assert_eq!(fits.get(), 2);
    // And back: a's view is shown again, not made again.
    f.ctl.borrow_mut().model.pick(&a);
    f.ctl.borrow_mut().refresh();
    f.h.advance(0);
    assert_eq!(bar_shown(&slot, &a), [false, true]);
    assert_eq!(bar_opens(&slot, &a).len(), 1);

    // A Flare overlay over the window hides the bar's page (it would paint over the menu).
    f.ctl.borrow_mut().open_tab_menu(&a, 10.0, 10.0);
    f.h.advance(0);
    f.ctl.borrow_mut().web_moved();
    assert_eq!(bar_shown(&slot, &a).last(), Some(&false));
    f.ctl.borrow_mut().close_menu();
    f.h.advance(0);
    f.ctl.borrow_mut().web_moved();
    assert_eq!(bar_shown(&slot, &a).last(), Some(&true));

    // Hidden by its plugin: the view closes, the terminal takes the height back.
    let r = bg_call(&f, &slot, "player", 3, "bar.hide", json!({"tab": a}));
    assert_eq!(r["result"], json!({"hidden": true}), "{r}");
    f.h.advance(0);
    assert_eq!(bar_closes(&slot, &a), 1);
    assert!(!f.h.comp.get_bar_shown());
    assert_eq!(f.h.comp.get_grid_phys_height(), h0);
    // Another plugin cannot hide b's bar: it is not its own.
    let r = bg_call(&f, &slot, "player", 4, "bar.hide", json!({"tab": b}));
    assert_eq!(r["result"], json!({"hidden": false}), "{r}");

    // The plugin turned off: its bar goes from every tab.
    turn(&f, "other", false);
    f.h.advance(0);
    let (bars, _) = f.ctl.borrow().bar_state();
    assert!(bars.is_empty(), "{bars:?}");
}

#[test]
fn the_bars_actions_are_chips_beside_it_and_its_page_speaks_for_its_tab_until_the_tab_closes() {
    let (f, a, b) = two_tabs();
    let slot = with_fake(&f);
    let _w = world(&f, |r| {
        bar_plugin(
            r,
            "player",
            &["bar", "tabs", "actions"],
            40,
            r#"{"actions":[{"key":"skip","title":"Skip","icon":"bolt","where":["session-bar"]},{"key":"menu","title":"Menu","where":["tab-menu"]}]}"#,
        );
    });
    turn(&f, "player", true);
    bg_up(&f, &slot, "player");
    bg_message(&f, &slot, "player", r#"{"t":"listen","name":"action"}"#);
    // No bar yet: no chip.
    assert_eq!(f.h.comp.get_bar_chips().row_count(), 0);
    let r = bg_call(&f, &slot, "player", 1, "bar.show", json!({"tab": a}));
    assert_eq!(code_of_reply(&r), "", "{r}");
    f.h.advance(0);
    // Only its session-bar action, in Flare's chip at the bar's right end; the page's place leaves room for it.
    let chips = f.h.comp.get_bar_chips();
    assert_eq!(chips.row_count(), 1);
    let c = chips.row_data(0).unwrap();
    assert_eq!(c.tip.as_str(), "Plugin player: Skip");
    assert!(c.enabled && !c.checked);
    assert_eq!(
        c.cmd.as_str(),
        glyph_path(crate::menus_ui::GLYPH_BADGE_BOLT)
    );
    assert!(
        f.h.comp.get_bar_web_w() < f.h.comp.get_grid_phys_width() as f32 / f.h.comp.get_scale()
    );
    // A click is the plugin's action on the tab, with click scope there.
    f.ctl.borrow_mut().bar_chip_clicked(0);
    assert_eq!(
        events_of(&slot, "player", "action"),
        [json!({"key": "skip", "tab": a})]
    );
    // actions.set reaches the chip.
    let r = bg_call(
        &f,
        &slot,
        "player",
        2,
        "actions.set",
        json!({"key": "skip", "tab": a, "checked": true}),
    );
    assert_eq!(code_of_reply(&r), "", "{r}");
    f.h.advance(0);
    assert!(f.h.comp.get_bar_chips().row_data(0).unwrap().checked);

    // The bar's page: ready once loaded, and its calls act on the tab it sits in.
    bar_page_up(&f, &slot, &a, "player");
    assert!(
        f.ctl
            .borrow()
            .mods
            .as_ref()
            .unwrap()
            .is_ready(View::bar(&a))
    );
    fake(&slot).say(WebEvent::View {
        slot: View::bar(&a),
        event: Box::new(WebEvent::ModMessage {
            tab: a.clone(),
            host: modview::mod_host("player"),
            json: json!({"t": "call", "id": 9, "method": "tabs.list", "params": {}}).to_string(),
        }),
    });
    f.pump_until("the bar page's reply", |_| {
        bar_posts(&slot, &a)
            .iter()
            .any(|v| v["t"] == "reply" && v["id"] == 9)
    });
    let reply = bar_posts(&slot, &a)
        .into_iter()
        .find(|v| v["id"] == 9)
        .unwrap();
    let tabs = reply["result"]["tabs"].as_array().expect("the tabs");
    assert_eq!(tabs.len(), 2, "{reply}");
    // The page hears the plugin's events too: it is one of its pages.
    assert!(
        f.ctl
            .borrow()
            .mods
            .as_ref()
            .unwrap()
            .slots_of("player")
            .contains(&View::bar(&a))
    );
    let r = bg_call(&f, &slot, "player", 4, "bar.hide", json!({"tab": a}));
    assert_eq!(code_of_reply(&r), "", "{r}");
    f.h.advance(0);
    assert_eq!(f.h.comp.get_bar_chips().row_count(), 0);
    assert!(
        !f.ctl
            .borrow()
            .mods
            .as_ref()
            .unwrap()
            .slots_of("player")
            .contains(&View::bar(&a)),
        "its page is let go with its view"
    );

    // On b, then b closes: its bar and its view go with it.
    let r = bg_call(&f, &slot, "player", 5, "bar.show", json!({"tab": b}));
    assert_eq!(code_of_reply(&r), "", "{r}");
    f.ctl.borrow_mut().model.pick(&b);
    f.ctl.borrow_mut().refresh();
    f.h.advance(0);
    assert_eq!(bar_opens(&slot, &b).len(), 1);
    f.ctl.borrow_mut().request_close(&b);
    f.pump();
    f.ctl.borrow_mut().refresh();
    f.h.advance(0);
    let (bars, views) = f.ctl.borrow().bar_state();
    assert!(bars.is_empty() && views.is_empty(), "{bars:?} {views:?}");
    assert_eq!(bar_closes(&slot, &b), 1);
}

#[test]
fn a_plugin_that_holds_network_gets_networked_pages_and_one_without_it_does_not() {
    let (f, a, _b) = two_tabs();
    let slot = with_fake(&f);
    let _w = world(&f, |r| {
        bar_plugin(r, "radio", &["bar", "tabs", "network"], 48, "{}");
        bar_plugin(r, "quiet", &["bar", "tabs"], 48, "{}");
    });
    turn(&f, "radio", true);
    bg_up(&f, &slot, "radio");
    turn(&f, "quiet", true);
    bg_up(&f, &slot, "quiet");
    let bg_net = |id: &str| {
        fake(&slot)
            .calls()
            .into_iter()
            .find_map(|c| match c {
                Call::OpenView { slot, app, .. } if slot == View::background(id) => {
                    Some(app.allow_network)
                }
                _ => None,
            })
            .unwrap()
    };
    assert!(bg_net("radio"), "its background page is networked");
    assert!(!bg_net("quiet"));
    let r = bg_call(&f, &slot, "radio", 1, "bar.show", json!({"tab": a}));
    assert_eq!(code_of_reply(&r), "", "{r}");
    f.h.advance(0);
    assert!(bar_opens(&slot, &a)[0].0.allow_network, "and so is its bar");
}

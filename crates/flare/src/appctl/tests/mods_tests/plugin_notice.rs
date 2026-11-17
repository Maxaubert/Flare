//! A plugin's short notice (`notice.show`, rule 1275, 2026-11-17): it needs `actions` (the agent never has it), shows
//! one line with the plugin's logo under the title bar, takes the place of the plugin's last one, goes after 6 s with
//! the pointer off it (and stands still while the pointer is on it), goes with its close button or when the plugin is
//! turned off, and a plugin shows at most three in ten seconds. The plugins are real folders, the hub and the dispatcher
//! the real ones; the tests play the helper.

use super::*;
use slint::Model;

/// A plugin with a menu item (so it may hold `actions`) when `perms` has it, and the given permissions.
fn notice_plugin(root: &Path, id: &str, perms: &[&str]) {
    let contributes = if perms.contains(&"actions") {
        r#","contributes":{"actions":[{"key":"toggle","title":"Lock","icon":"lock","where":["tab-menu"]}]}"#
    } else {
        ""
    };
    let text = format!(
        r#"{{"id":"{id}","name":"Vault {id}","version":"1.0.0","protocol":2,"description":"A test plugin.","author":"me","homepage":"https://example.com/{id}","permissions":{perms:?}{contributes}}}"#
    );
    let to = root.join(id);
    std::fs::create_dir_all(&to).unwrap(); // sync-ok: test code only, never the app
    std::fs::write(to.join("plugin.json"), text).unwrap(); // sync-ok: test code only, never the app
}

fn helper(w: &World, id: &str) -> CallContext {
    CallContext {
        conn: 7,
        client: format!("helper:{id}"),
        client_version: String::new(),
        grant: w.hub.grant_of_mod(id).expect("the plugin is on"),
        tab: None,
        cancel: Default::default(),
    }
}

fn show(f: &Fx, w: &World, id: &str, params: Value) -> Result<Value, CtlError> {
    answer_of(
        f,
        &call_on_thread(&w.dispatcher, helper(w, id), Method::NoticeShow, params),
    )
}

/// What the window shows: (plugin, text, error) of each notice.
fn shown(f: &Fx) -> Vec<(String, String, bool)> {
    let m = f.h.comp.get_plugin_notices();
    (0..m.row_count())
        .filter_map(|i| m.row_data(i))
        .map(|r| (r.id.to_string(), r.text.to_string(), r.error))
        .collect()
}

/// `ms` of the window's time, in the tick's steps.
fn wait(f: &Fx, ms: u64) {
    for _ in 0..ms / crate::plugin_notice::TICK_MS {
        f.h.advance(crate::plugin_notice::TICK_MS);
    }
    f.pump();
}

#[test]
fn a_plugin_with_actions_shows_a_notice_and_nothing_else_may() {
    let (f, a, _b) = two_tabs();
    let _slot = with_fake(&f);
    let w = world(&f, |r| {
        notice_plugin(r, "vault", &["actions", "tabs"]);
        notice_plugin(r, "plain", &["tabs", "decorate"]);
    });
    turn(&f, "vault", true);
    turn(&f, "plain", true);
    assert!(!f.h.comp.get_overlay_open());

    let r = show(
        &f,
        &w,
        "vault",
        json!({"text": "Bitwarden could not lock the vault. It is still unlocked.", "kind": "error"}),
    )
    .unwrap();
    assert_eq!(r, json!({"shown": true}));
    assert_eq!(
        shown(&f),
        [(
            "vault".to_string(),
            "Bitwarden could not lock the vault. It is still unlocked.".to_string(),
            true
        )]
    );
    let row = f.h.comp.get_plugin_notices().row_data(0).unwrap();
    assert_eq!(
        row.name, "Vault vault",
        "the plugin's own name, for the screen reader"
    );
    assert!(!row.look.branded, "no brand: Flare's chip");
    assert!(
        f.h.comp.get_overlay_open(),
        "a web panel shows its still under the notice"
    );

    // A plugin without `actions` is refused by the channel (its grant), as the one with it is let through.
    let plain = w.hub.grant_of_mod("plain").unwrap();
    assert_eq!(
        plain.check(Method::NoticeShow).unwrap_err().code,
        code::FORBIDDEN
    );
    assert!(
        w.hub
            .grant_of_mod("vault")
            .unwrap()
            .check(Method::NoticeShow)
            .is_ok()
    );
    // The agent, with or without its eval switch, is refused by its grant and again by the dispatcher.
    for g in [Grant::agent(), Grant::Agent { eval: true }] {
        assert_eq!(
            g.check(Method::NoticeShow).unwrap_err().code,
            code::FORBIDDEN
        );
    }
    for grant in [Grant::agent(), Grant::Agent { eval: true }] {
        let mut ctx = agent_in(&a);
        ctx.grant = grant;
        let e = answer_of(
            &f,
            &call_on_thread(
                &w.dispatcher,
                ctx,
                Method::NoticeShow,
                json!({"text": "Hi"}),
            ),
        )
        .unwrap_err();
        assert_eq!(e.code, code::FORBIDDEN);
    }
    // Bad params are bad_params and show nothing.
    for p in [
        json!({}),
        json!({"text": ""}),
        json!({"text": "x".repeat(121)}),
        json!({"text": "a", "kind": "warn"}),
        json!({"text": "a", "button": "Retry"}),
    ] {
        let e = show(&f, &w, "vault", p.clone()).unwrap_err();
        assert_eq!(e.code, code::BAD_PARAMS, "{p}");
    }
    assert_eq!(shown(&f).len(), 1);
    // The method is listed in the plugin's hello only where there is a hub.
    assert!(w.dispatcher.capabilities().contains(&Method::NoticeShow));
}

#[test]
fn hidden_characters_are_taken_out_of_the_text() {
    let (f, _a, _b) = two_tabs();
    let _slot = with_fake(&f);
    let w = world(&f, |r| notice_plugin(r, "vault", &["actions"]));
    turn(&f, "vault", true);
    show(
        &f,
        &w,
        "vault",
        json!({"text": "Locked\u{202E}kcol\u{200B}\r\nnow\u{FEFF}"}),
    )
    .unwrap();
    assert_eq!(shown(&f)[0].1, "Lockedkcol now");
    assert!(!shown(&f)[0].2, "info is the default kind");
}

#[test]
fn a_new_notice_takes_the_place_of_the_plugins_last_and_others_stack() {
    let (f, _a, _b) = two_tabs();
    let _slot = with_fake(&f);
    let w = world(&f, |r| {
        notice_plugin(r, "vault", &["actions"]);
        notice_plugin(r, "other", &["actions"]);
    });
    turn(&f, "vault", true);
    turn(&f, "other", true);
    show(&f, &w, "vault", json!({"text": "One", "kind": "error"})).unwrap();
    show(&f, &w, "other", json!({"text": "Two"})).unwrap();
    wait(&f, 5_000);
    show(&f, &w, "vault", json!({"text": "Three"})).unwrap();
    assert_eq!(
        shown(&f),
        [
            ("vault".to_string(), "Three".to_string(), false),
            ("other".to_string(), "Two".to_string(), false)
        ],
        "in its place, with the new kind"
    );
    // "other" is out of time; "vault" started again at "Three".
    wait(&f, 1_000);
    assert_eq!(shown(&f).len(), 1);
    assert_eq!(shown(&f)[0].0, "vault");
    wait(&f, 4_900);
    assert_eq!(shown(&f).len(), 1);
    wait(&f, 100);
    assert!(shown(&f).is_empty());
    assert!(!f.h.comp.get_overlay_open());
    assert!(
        !crate::timer_registry::running("plugins.notice"),
        "no tick runs with nothing to show"
    );
}

#[test]
fn a_notice_goes_after_six_seconds_and_stands_still_while_the_pointer_is_on_it() {
    let (f, _a, _b) = two_tabs();
    let _slot = with_fake(&f);
    let w = world(&f, |r| notice_plugin(r, "vault", &["actions"]));
    turn(&f, "vault", true);
    show(&f, &w, "vault", json!({"text": "Could not lock"})).unwrap();
    wait(&f, 5_900);
    assert_eq!(shown(&f).len(), 1, "still up at 5.9 s");
    wait(&f, 100);
    assert!(shown(&f).is_empty(), "gone at 6 s");

    show(&f, &w, "vault", json!({"text": "Could not lock"})).unwrap();
    wait(&f, 3_000);
    f.h.comp.invoke_plugin_notice_hover("vault".into(), true);
    wait(&f, 20_000);
    assert_eq!(shown(&f).len(), 1, "the pointer is on it");
    let v = f.ctl.borrow().e2e_plugin_notices();
    assert_eq!(v["notices"][0]["left_ms"], 3_000);
    assert_eq!(v["notices"][0]["hovered"], true);
    f.h.comp.invoke_plugin_notice_hover("vault".into(), false);
    wait(&f, 2_900);
    assert_eq!(shown(&f).len(), 1);
    wait(&f, 100);
    assert!(shown(&f).is_empty(), "the rest of its time, then gone");
}

/// The pointer rests on the second notice and the first goes: the window builds the toasts anew and the second moves up
/// from under the pointer, so it counts down again and does not stand still for good.
#[test]
fn a_notice_that_moves_from_under_the_pointer_still_goes() {
    let (f, _a, _b) = two_tabs();
    let _slot = with_fake(&f);
    let w = world(&f, |r| {
        notice_plugin(r, "vault", &["actions"]);
        notice_plugin(r, "other", &["actions"]);
    });
    turn(&f, "vault", true);
    turn(&f, "other", true);
    show(&f, &w, "vault", json!({"text": "First"})).unwrap();
    wait(&f, 1_000);
    show(&f, &w, "other", json!({"text": "Second"})).unwrap();
    f.h.comp.invoke_plugin_notice_hover("other".into(), true);
    wait(&f, 5_000);
    assert_eq!(shown(&f).len(), 1, "the first went at 6 s");
    let v = f.ctl.borrow().e2e_plugin_notices();
    assert_eq!(v["notices"][0]["hovered"], false, "{v}");
    wait(&f, 6_000);
    assert!(shown(&f).is_empty(), "the second went too");
}

#[test]
fn the_close_button_and_turning_the_plugin_off_take_the_notice_away() {
    let (f, _a, _b) = two_tabs();
    let _slot = with_fake(&f);
    let w = world(&f, |r| {
        notice_plugin(r, "vault", &["actions"]);
        notice_plugin(r, "other", &["actions"]);
    });
    turn(&f, "vault", true);
    turn(&f, "other", true);
    show(&f, &w, "vault", json!({"text": "One"})).unwrap();
    show(&f, &w, "other", json!({"text": "Two"})).unwrap();
    f.h.comp.invoke_plugin_notice_closed("other".into());
    assert_eq!(shown(&f).len(), 1);
    assert_eq!(shown(&f)[0].0, "vault");

    // A call checked before the plugin went off (an older generation) shows nothing.
    let old = w.hub.generation("vault");
    turn(&f, "vault", false);
    assert!(shown(&f).is_empty(), "its notice went with it");
    turn(&f, "vault", true);
    let e = f
        .ctl
        .borrow_mut()
        .ctl_notice_show("vault", "Late", true, old)
        .unwrap_err();
    assert_eq!(e.code, code::UNAUTHORIZED);
    assert!(shown(&f).is_empty());
}

#[test]
fn a_plugin_shows_at_most_three_notices_in_ten_seconds() {
    let (f, _a, _b) = two_tabs();
    let _slot = with_fake(&f);
    let w = world_still(&f, |r| {
        notice_plugin(r, "vault", &["actions"]);
        notice_plugin(r, "other", &["actions"]);
    });
    turn(&f, "vault", true);
    turn(&f, "other", true);
    // A refused call spends nothing.
    show(&f, &w, "vault", json!({"text": ""})).unwrap_err();
    for i in 0..3 {
        show(&f, &w, "vault", json!({ "text": format!("Try {i}") })).unwrap();
    }
    let e = show(&f, &w, "vault", json!({"text": "Try 3"})).unwrap_err();
    assert_eq!(e.code, code::RATE_LIMITED);
    assert_eq!(e.message, "at most 3 notices in 10 seconds for one plugin");
    assert_eq!(shown(&f)[0].1, "Try 2", "the limited one changed nothing");
    // Another plugin has its own three.
    show(&f, &w, "other", json!({"text": "Fine"})).unwrap();
}

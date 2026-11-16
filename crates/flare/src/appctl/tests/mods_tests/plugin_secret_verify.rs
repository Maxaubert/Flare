//! A checked secret (`secret.ask` with `verify`, `secret.result`, rule 1274, 2026-11-16): the dialog stays open after
//! the person sends; the plugin gets the text with the ask's id and says right (a green check, then the dialog closes
//! by itself), wrong (the field comes back in place with the red line, and that call waits for the next try) or done
//! (its plain end line). Cancel while checking, a check with no word in time, an unknown id and an ask without
//! `verify` (an older plugin) are each held here. The plugin is a real folder, the hub and the dispatcher the real
//! ones; the tests play the helper and the timer.

use super::*;

const PASS: &str = "Hunter2-SECRET-pw";
const NEXT: &str = "Second-SECRET-try";

fn fill_plugin(root: &Path, id: &str) {
    let text = format!(
        r#"{{"id":"{id}","name":"Vault {id}","version":"1.0.0","protocol":2,"description":"A test plugin.","author":"me","homepage":"https://example.com/{id}","permissions":["fill"]}}"#
    );
    let to = root.join(id);
    std::fs::create_dir_all(&to).unwrap(); // sync-ok: test code only, never the app
    std::fs::write(to.join("plugin.json"), text).unwrap(); // sync-ok: test code only, never the app
}

fn helper_ctx(w: &World, id: &str) -> CallContext {
    CallContext {
        conn: 7,
        client: format!("helper:{id}"),
        client_version: String::new(),
        grant: w.hub.grant_of_mod(id).expect("the plugin is on"),
        tab: None,
        cancel: Default::default(),
    }
}

/// A call of the plugin's helper on its own thread (it may wait for the person).
fn start(w: &World, id: &str, m: Method, params: Value) -> Receiver<Result<Value, CtlError>> {
    call_on_thread(&w.dispatcher, helper_ctx(w, id), m, params)
}

fn ask_params() -> Value {
    json!({
        "title": "Your vault is locked",
        "label": "Master password",
        "button": "Unlock",
        "verify": true,
        "checking": "Checking the password...",
    })
}

fn asking(f: &Fx) -> bool {
    f.h.comp.get_pask_open()
}

/// The stage the e2e probe reads (`input`, `checking`, `shown`), or None with no checked secret up.
fn stage(f: &Fx) -> Option<String> {
    f.ctl.borrow().e2e_plugin_ask()["stage"]
        .as_str()
        .map(str::to_string)
}

/// The open question's timer runs out.
fn expire(f: &Fx) {
    let (seq, round) = f.ctl.borrow().plugin_ask_timer().expect("a question is up");
    f.ctl
        .borrow_mut()
        .on_msg(UiMsg::Mods(ModsMsgBox(ModsMsg::AskExpired { seq, round })));
}

/// Puts a checked secret up and sends `PASS`: gives the ask's id, with the dialog checking.
fn ask_and_send(f: &Fx, w: &World) -> String {
    let rx = start(w, "vault", Method::SecretAsk, ask_params());
    f.pump_until("the question", asking);
    assert!(f.h.comp.get_pask_verify());
    assert_eq!(f.h.comp.get_pask_stage(), 0);
    assert_eq!(f.h.comp.get_pask_checking(), "Checking the password...");
    f.h.comp.invoke_pask_secret(true, PASS.into());
    let got = answer_of(f, &rx).unwrap();
    assert_eq!(got["text"], PASS);
    let id = got["ask"].as_str().expect("the ask's id").to_string();
    assert_eq!(got.as_object().map(|o| o.len()), Some(2), "{got:?}");
    id
}

#[test]
fn verify_keeps_the_box_open_checking_and_right_shows_the_check_then_closes() {
    let (f, _a, _b) = two_tabs();
    let _slot = with_fake(&f);
    let w = world(&f, |r| fill_plugin(r, "vault"));
    turn(&f, "vault", true);
    let id = ask_and_send(&f, &w);
    // The box stays open: checking, the field read-only, the button off with its spinner.
    assert!(asking(&f), "the box never closes on Unlock");
    assert_eq!(f.h.comp.get_pask_stage(), 1);
    assert_eq!(stage(&f).as_deref(), Some("checking"));
    assert_eq!(f.ctl.borrow().e2e_plugin_ask()["waiting"], false);
    // A second press while checking sends nothing.
    f.h.comp.invoke_pask_secret(true, NEXT.into());
    assert_eq!(stage(&f).as_deref(), Some("checking"));

    let r = start(
        &w,
        "vault",
        Method::SecretResult,
        json!({"ask": id, "ok": true, "title": "Vault unlocked", "line": "Bitwarden can fill logins now."}),
    );
    assert_eq!(answer_of(&f, &r).unwrap(), json!({"shown": true}));
    assert!(asking(&f), "the check shows before the box closes");
    assert_eq!(f.h.comp.get_pask_stage(), 2);
    assert_eq!(f.h.comp.get_pask_result_title(), "Vault unlocked");
    assert_eq!(
        f.h.comp.get_pask_result_line(),
        "Bitwarden can fill logins now."
    );
    // Cancel during the result does nothing; the result's timer closes the box.
    f.h.comp.invoke_pask_secret(false, "".into());
    assert!(asking(&f));
    expire(&f);
    assert!(!asking(&f));
    assert!(f.ctl.borrow().plugin_ask.is_none());
    // The ask is over: its id is no longer open.
    let e = answer_of(
        &f,
        &start(
            &w,
            "vault",
            Method::SecretResult,
            json!({"ask": id, "ok": true}),
        ),
    )
    .unwrap_err();
    assert_eq!(e.code, code::NOT_FOUND);
}

#[test]
fn wrong_brings_the_field_back_in_place_and_the_call_waits_for_the_next_try() {
    let (f, _a, _b) = two_tabs();
    let _slot = with_fake(&f);
    let w = world(&f, |r| fill_plugin(r, "vault"));
    turn(&f, "vault", true);
    let id = ask_and_send(&f, &w);
    let seq_before = f.ctl.borrow().plugin_ask_seq;

    let r = start(
        &w,
        "vault",
        Method::SecretResult,
        json!({"ask": id, "ok": false, "error": "Wrong password, try again"}),
    );
    f.pump_until("the field back", |f| f.h.comp.get_pask_stage() == 0);
    // Same box, never closed: the red line, the field cleared and holding the keys; the call waits.
    assert!(asking(&f));
    assert_eq!(f.ctl.borrow().plugin_ask_seq, seq_before, "no new question");
    assert_eq!(f.h.comp.get_pask_error(), "Wrong password, try again");
    assert!(f.h.comp.get_pask_error_shown());
    assert!(r.try_recv().is_err(), "the call waits for the next try");
    assert_eq!(f.ctl.borrow().e2e_plugin_ask()["waiting"], true);
    // A second call while one waits is busy.
    let e = answer_of(
        &f,
        &start(
            &w,
            "vault",
            Method::SecretResult,
            json!({"ask": id, "ok": false}),
        ),
    )
    .unwrap_err();
    assert_eq!(e.code, code::BUSY);

    // The next try reaches the waiting call, with the same id, and the box checks again.
    f.h.comp.invoke_pask_secret(true, NEXT.into());
    assert_eq!(answer_of(&f, &r).unwrap(), json!({"text": NEXT, "ask": id}));
    assert_eq!(f.h.comp.get_pask_stage(), 1);
    assert!(!f.h.comp.get_pask_error_shown(), "no red while checking");

    // No error named: Flare's own red line.
    let r = start(
        &w,
        "vault",
        Method::SecretResult,
        json!({"ask": id, "ok": false}),
    );
    f.pump_until("the field back", |f| f.h.comp.get_pask_stage() == 0);
    assert_eq!(f.h.comp.get_pask_error(), crate::plugin_ask::DEFAULT_WRONG);
    // Cancel now answers the waiting call.
    f.h.comp.invoke_pask_secret(false, "".into());
    assert_eq!(answer_of(&f, &r).unwrap(), json!({"cancelled": true}));
    assert!(!asking(&f));
}

#[test]
fn done_ends_the_question_with_the_plugins_plain_line() {
    let (f, _a, _b) = two_tabs();
    let _slot = with_fake(&f);
    let w = world(&f, |r| fill_plugin(r, "vault"));
    turn(&f, "vault", true);
    let id = ask_and_send(&f, &w);
    let r = start(
        &w,
        "vault",
        Method::SecretResult,
        json!({"ask": id, "ok": false, "done": true, "line": "Bitwarden did not unlock after 5 tries."}),
    );
    assert_eq!(answer_of(&f, &r).unwrap(), json!({"shown": true}));
    assert_eq!(f.h.comp.get_pask_stage(), 3);
    // No heading of its own: the ask's title.
    assert_eq!(f.h.comp.get_pask_result_title(), "Your vault is locked");
    assert_eq!(
        f.h.comp.get_pask_result_line(),
        "Bitwarden did not unlock after 5 tries."
    );
    expire(&f);
    assert!(!asking(&f));
}

#[test]
fn cancel_while_checking_closes_the_box_and_tells_the_plugin() {
    let (f, _a, _b) = two_tabs();
    let _slot = with_fake(&f);
    let w = world(&f, |r| fill_plugin(r, "vault"));
    turn(&f, "vault", true);
    let id = ask_and_send(&f, &w);
    f.h.comp.invoke_pask_secret(false, "".into());
    assert!(!asking(&f), "Cancel works while checking");
    assert!(f.ctl.borrow().plugin_ask.is_none());
    // The plugin's word on that check comes later: it hears the cancel.
    let r = start(
        &w,
        "vault",
        Method::SecretResult,
        json!({"ask": id, "ok": true}),
    );
    assert_eq!(answer_of(&f, &r).unwrap(), json!({"cancelled": true}));
    // Once: the next word on it is an unknown id.
    let r = start(
        &w,
        "vault",
        Method::SecretResult,
        json!({"ask": id, "ok": true}),
    );
    assert_eq!(answer_of(&f, &r).unwrap_err().code, code::NOT_FOUND);
    // Another plugin's id, or one never given, is not found either.
    let r = start(
        &w,
        "vault",
        Method::SecretResult,
        json!({"ask": "s999", "ok": true}),
    );
    assert_eq!(answer_of(&f, &r).unwrap_err().code, code::NOT_FOUND);
}

#[test]
fn a_check_with_no_word_in_time_goes_back_to_the_field_with_a_plain_error() {
    let (f, _a, _b) = two_tabs();
    let _slot = with_fake(&f);
    let w = world(&f, |r| fill_plugin(r, "vault"));
    turn(&f, "vault", true);
    let id = ask_and_send(&f, &w);
    // The plugin says nothing for a minute: back to the field, Flare's red line, the box still up.
    expire(&f);
    assert!(asking(&f));
    assert_eq!(f.h.comp.get_pask_stage(), 0);
    assert_eq!(f.h.comp.get_pask_error(), crate::plugin_ask::TOOK_TOO_LONG);
    // The person tries again while no call waits: the text waits for the plugin's next word.
    f.h.comp.invoke_pask_secret(true, NEXT.into());
    assert_eq!(f.h.comp.get_pask_stage(), 1);
    // The plugin's late word on the first try is wrong: the call gets the waiting text at once.
    let r = start(
        &w,
        "vault",
        Method::SecretResult,
        json!({"ask": id, "ok": false}),
    );
    assert_eq!(answer_of(&f, &r).unwrap(), json!({"text": NEXT, "ask": id}));
    assert_eq!(f.h.comp.get_pask_stage(), 1, "the new try is checked");
    // An old timer of an earlier stage ends nothing.
    let (seq, round) = f.ctl.borrow().plugin_ask_timer().unwrap();
    f.ctl
        .borrow_mut()
        .on_msg(UiMsg::Mods(ModsMsgBox(ModsMsg::AskExpired {
            seq,
            round: round - 1,
        })));
    assert_eq!(f.h.comp.get_pask_stage(), 1);
    // Right: the check.
    let r = start(
        &w,
        "vault",
        Method::SecretResult,
        json!({"ask": id, "ok": true}),
    );
    assert_eq!(answer_of(&f, &r).unwrap(), json!({"shown": true}));
    assert_eq!(f.h.comp.get_pask_result_title(), "Done");
    expire(&f);
    assert!(!asking(&f));

    // A field left alone after a timeout closes like any unanswered question; the plugin hears it as a cancel.
    let id = ask_and_send(&f, &w);
    expire(&f); // the check ran out
    expire(&f); // then the field
    assert!(!asking(&f));
    let r = start(
        &w,
        "vault",
        Method::SecretResult,
        json!({"ask": id, "ok": true}),
    );
    assert_eq!(
        answer_of(&f, &r).unwrap(),
        json!({"cancelled": true, "timed_out": true})
    );
}

#[test]
fn turning_the_plugin_off_while_checking_ends_the_question() {
    let (f, _a, _b) = two_tabs();
    let _slot = with_fake(&f);
    let w = world(&f, |r| fill_plugin(r, "vault"));
    turn(&f, "vault", true);
    let id = ask_and_send(&f, &w);
    let r = start(
        &w,
        "vault",
        Method::SecretResult,
        json!({"ask": id, "ok": false}),
    );
    f.pump_until("the field back", |f| f.h.comp.get_pask_stage() == 0);
    turn(&f, "vault", false);
    assert_eq!(answer_of(&f, &r).unwrap_err().code, code::UNAUTHORIZED);
    assert!(!asking(&f));
}

#[test]
fn a_secret_result_is_checked_before_it_reaches_the_window() {
    let (f, a, _b) = two_tabs();
    let _slot = with_fake(&f);
    let w = world(&f, |r| fill_plugin(r, "vault"));
    turn(&f, "vault", true);
    for p in [
        json!({"ok": true}),
        json!({"ask": "s1", "ok": true, "error": "x"}),
        json!({"ask": "s1", "ok": false, "text": PASS}),
    ] {
        let e = answer_of(&f, &start(&w, "vault", Method::SecretResult, p.clone())).unwrap_err();
        assert_eq!(e.code, code::BAD_PARAMS, "{p}");
        assert!(!e.message.contains(PASS));
    }
    // The agent never has it.
    let e = answer_of(
        &f,
        &call_on_thread(
            &w.dispatcher,
            agent_in(&a),
            Method::SecretResult,
            json!({"ask": "s1", "ok": true}),
        ),
    )
    .unwrap_err();
    assert_eq!(e.code, code::FORBIDDEN);
    // `checking` without `verify` is refused; the old keys alone are today's ask.
    let e = answer_of(
        &f,
        &start(
            &w,
            "vault",
            Method::SecretAsk,
            json!({"title": "t", "label": "l", "checking": "c"}),
        ),
    )
    .unwrap_err();
    assert_eq!(e.code, code::BAD_PARAMS);
}

#[test]
fn an_empty_field_sends_nothing_and_the_box_stays_at_the_field() {
    let (f, _a, _b) = two_tabs();
    let _slot = with_fake(&f);
    let w = world(&f, |r| fill_plugin(r, "vault"));
    turn(&f, "vault", true);
    let rx = start(&w, "vault", Method::SecretAsk, ask_params());
    f.pump_until("the question", asking);
    // Unlock (or Enter) on an empty field: nothing goes to the plugin, and the box never sits checking.
    f.h.comp.invoke_pask_secret(true, "".into());
    assert_eq!(stage(&f).as_deref(), Some("input"));
    assert!(rx.try_recv().is_err(), "the plugin's call still waits");
    f.h.comp.invoke_pask_secret(true, PASS.into());
    let got = answer_of(&f, &rx).unwrap();
    assert_eq!(got["text"], PASS);
    assert_eq!(stage(&f).as_deref(), Some("checking"));
}

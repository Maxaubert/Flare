//! The `fill` permission (2026-10-28): `browser.fill` types a saved login into a tab's web page only when the page's
//! site is the login's and only after the person's Allow in Flare's own dialog; Deny and "Deny for this session" say
//! no; one question is up at a time and a plugin asks at most six a minute; `secret.ask` hands what the person typed
//! to the plugin that asked; and a plugin with only a helper hears the agent's tool calls as a `tool` event. No value
//! is in any answer, error or post the plugin's pages see, other than the secret it asked for.
//!
//! The plugins are real folders, the hub and the dispatcher the real ones; the page is the fake browser's, and its
//! script's answer is played by the test (the script itself runs in the e2e).

use super::*;

const PAGE: &str = "https://accounts.example.com/login";
const USER: &str = "max@example.com";
const PASS: &str = "Hunter2-SECRET-pw";
const CODE: &str = "731905";

/// A plugin with the given permissions and nothing else: a helper calls for it (the tests play the helper).
fn fill_plugin(root: &Path, id: &str, perms: &[&str]) {
    let text = format!(
        r#"{{"id":"{id}","name":"Vault {id}","version":"1.0.0","protocol":2,"description":"A test plugin.","author":"me","homepage":"https://example.com/{id}","permissions":{perms:?}}}"#
    );
    let to = root.join(id);
    std::fs::create_dir_all(&to).unwrap(); // sync-ok: test code only, never the app
    std::fs::write(to.join("plugin.json"), text).unwrap(); // sync-ok: test code only, never the app
}

/// A plugin whose tools only its helper answers (no background page, no panel).
fn helper_tool_plugin(root: &Path, id: &str) {
    let text = format!(
        r#"{{"id":"{id}","name":"Vault {id}","version":"1.0.0","protocol":2,"description":"A test plugin.","author":"me","homepage":"https://example.com/{id}","permissions":["process","tools","fill"],"helper":{{"command":"helper.exe"}},"tools":[{{"name":"fill_login","description":"Fills.","input_schema":{{"type":"object"}}}}]}}"#
    );
    let to = root.join(id);
    std::fs::create_dir_all(&to).unwrap(); // sync-ok: test code only, never the app
    std::fs::write(to.join("plugin.json"), text).unwrap(); // sync-ok: test code only, never the app
    std::fs::write(to.join("helper.exe"), b"MZ").unwrap(); // sync-ok: test code only, never the app
}

/// The plugin's helper on the channel: its live grant, no tab of its own.
fn helper_ctx(w: &World, id: &str) -> CallContext {
    CallContext {
        conn: 7,
        client: format!("helper:{id}"),
        client_version: String::new(),
        grant: w.hub.grant_of_mod(id).expect("the plugin is on"),
        tab: None,
    }
}

/// A call of the plugin's helper, run on its own thread while the window is pumped (the call may wait for it).
fn call(f: &Fx, w: &World, id: &str, m: Method, params: Value) -> Result<Value, CtlError> {
    answer_of(
        f,
        &call_on_thread(&w.dispatcher, helper_ctx(w, id), m, params),
    )
}

/// A call of the agent in `tab`, as [`call`].
fn agent_call(f: &Fx, w: &World, tab: &str, m: Method, params: Value) -> Result<Value, CtlError> {
    answer_of(f, &call_on_thread(&w.dispatcher, agent_in(tab), m, params))
}

/// The agent opens `url` in the front tab's web panel and the page loads.
fn web_up(f: &Fx, w: &World, slot: &Slot, tab: &str, url: &str) {
    let rx = call_on_thread(
        &w.dispatcher,
        agent_in(tab),
        Method::BrowserOpen,
        json!({ "url": url }),
    );
    f.pump_until("the page to open", |_| {
        slot.borrow()
            .as_ref()
            .is_some_and(|b| b.calls().iter().any(|c| matches!(c, Call::Open { .. })))
    });
    finish_page(&fake(slot), tab, url);
    answer_of(f, &rx).expect("the page opened");
}

fn fill_params(tab: &str, uris: &[&str]) -> Value {
    json!({
        "tab": tab,
        "login_name": "Example account",
        "login_uris": uris,
        "username": USER,
        "password": PASS,
        "totp": CODE,
    })
}

fn asking(f: &Fx) -> bool {
    f.h.comp.get_pask_open()
}

/// The fill scripts the page was given: (tab, id, script).
fn scripts(slot: &Slot) -> Vec<(String, u64, String)> {
    fake(slot)
        .calls()
        .into_iter()
        .filter_map(|c| match c {
            Call::Script(tab, id, js) => Some((tab, id, js)),
            _ => None,
        })
        .collect()
}

/// No value of the login is in `text` (an answer, an error, a post to a page, a debug form).
fn holds_no_secret(text: &str) -> bool {
    !text.contains(PASS) && !text.contains(USER) && !text.contains(CODE)
}

#[test]
fn a_fill_needs_the_logins_site_and_the_persons_allow_and_answers_kinds_of_field_only() {
    let (f, a, _b) = two_tabs();
    let slot = with_fake(&f);
    let w = world(&f, |r| fill_plugin(r, "vault", &["fill", "tabs"]));
    turn(&f, "vault", true);
    web_up(&f, &w, &slot, &a, PAGE);

    // The address of the page, to pick the login with.
    let page = call(&f, &w, "vault", Method::BrowserFillPage, json!({"tab": a}));
    assert_eq!(
        page.unwrap(),
        json!({"tab": a, "url": PAGE, "host": "accounts.example.com"})
    );

    // A login of another site is refused before anything is asked or typed.
    let e = call(
        &f,
        &w,
        "vault",
        Method::BrowserFill,
        fill_params(&a, &["https://example.org", "androidapp://com.example"]),
    )
    .unwrap_err();
    assert_eq!(e.code, code::FORBIDDEN);
    assert_eq!(e.message, "this login is not for accounts.example.com");
    assert!(!asking(&f) && scripts(&slot).is_empty());
    assert!(holds_no_secret(&format!("{e:?}")));

    // The login's own site: Flare asks, and nothing is typed before the answer.
    let rx = call_on_thread(
        &w.dispatcher,
        helper_ctx(&w, "vault"),
        Method::BrowserFill,
        fill_params(&a, &["example.com"]),
    );
    f.pump_until("the question", asking);
    assert_eq!(f.h.comp.get_pask_kind(), 0);
    assert_eq!(
        f.h.comp.get_pask_title(),
        "Vault vault wants to fill a login"
    );
    assert!(
        f.h.comp.get_overlay_open(),
        "the page hides under the question"
    );
    assert!(scripts(&slot).is_empty(), "nothing typed before Allow");
    assert!(rx.try_recv().is_err(), "the call waits for the person");

    // Allow: the page gets one script with the values as data, and the call answers what was filled.
    f.h.comp.invoke_pask_fill(1);
    assert!(!asking(&f));
    let run = scripts(&slot);
    assert_eq!(run.len(), 1);
    let (tab, id, js) = &run[0];
    assert_eq!(tab, &a);
    assert!(js.contains("JSON.parse(") && js.contains(PASS));
    fake(&slot).say(WebEvent::Script {
        id: *id,
        result: Ok(format!(
            r#"{{"ok":true,"filled":["username","password","{PASS}"],"missing":["totp"]}}"#
        )),
    });
    let answer = answer_of(&f, &rx).unwrap();
    assert_eq!(
        answer,
        json!({"filled": ["username", "password"], "missing": ["totp"]})
    );
    assert!(holds_no_secret(&answer.to_string()));
}

#[test]
fn deny_and_deny_for_this_session_say_no_and_the_second_holds_until_flare_quits() {
    let (f, a, _b) = two_tabs();
    let slot = with_fake(&f);
    let w = world(&f, |r| {
        fill_plugin(r, "vault", &["fill", "tabs"]);
        fill_plugin(r, "other", &["fill", "tabs"]);
    });
    turn(&f, "vault", true);
    turn(&f, "other", true);
    web_up(&f, &w, &slot, &a, PAGE);
    let ask = |id: &str| {
        call_on_thread(
            &w.dispatcher,
            helper_ctx(&w, id),
            Method::BrowserFill,
            fill_params(&a, &["https://accounts.example.com/"]),
        )
    };

    let rx = ask("vault");
    f.pump_until("the question", asking);
    f.h.comp.invoke_pask_fill(0);
    assert_eq!(answer_of(&f, &rx).unwrap(), json!({"declined": true}));

    let rx = ask("vault");
    f.pump_until("the question", asking);
    f.h.comp.invoke_pask_fill(2);
    assert_eq!(
        answer_of(&f, &rx).unwrap(),
        json!({"declined": true, "for_session": true})
    );
    // From now on that plugin's fills are declined at once, with no question; another plugin still asks.
    let rx = ask("vault");
    assert_eq!(
        answer_of(&f, &rx).unwrap(),
        json!({"declined": true, "for_session": true})
    );
    assert!(!asking(&f));
    let rx = ask("other");
    f.pump_until("the other plugin's question", asking);
    f.h.comp.invoke_pask_fill(0);
    assert_eq!(answer_of(&f, &rx).unwrap(), json!({"declined": true}));
    assert!(scripts(&slot).is_empty(), "no answer typed anything");
}

#[test]
fn one_question_at_a_time_six_asks_a_minute_and_an_unanswered_one_counts_as_deny() {
    let (f, a, _b) = two_tabs();
    let slot = with_fake(&f);
    let w = world(&f, |r| {
        fill_plugin(r, "vault", &["fill", "tabs"]);
        fill_plugin(r, "other", &["fill", "tabs"]);
    });
    turn(&f, "vault", true);
    turn(&f, "other", true);
    web_up(&f, &w, &slot, &a, PAGE);
    let good = fill_params(&a, &["example.com"]);

    let rx = call_on_thread(
        &w.dispatcher,
        helper_ctx(&w, "vault"),
        Method::BrowserFill,
        good.clone(),
    );
    f.pump_until("the question", asking);
    // A second question, from any plugin, waits for none: it is busy.
    let busy = call_on_thread(
        &w.dispatcher,
        helper_ctx(&w, "other"),
        Method::BrowserFill,
        good.clone(),
    );
    assert_eq!(answer_of(&f, &busy).unwrap_err().code, code::BUSY);
    let busy = call_on_thread(
        &w.dispatcher,
        helper_ctx(&w, "other"),
        Method::SecretAsk,
        json!({"title": "Unlock", "label": "Password"}),
    );
    assert_eq!(answer_of(&f, &busy).unwrap_err().code, code::BUSY);
    // Its time runs out: a no.
    let seq = f.ctl.borrow().plugin_ask_seq;
    f.ctl
        .borrow_mut()
        .on_msg(UiMsg::Mods(ModsMsgBox(ModsMsg::AskExpired { seq })));
    assert_eq!(
        answer_of(&f, &rx).unwrap(),
        json!({"declined": true, "timed_out": true})
    );
    assert!(!asking(&f));

    // "vault" asked once; five more fit in the minute (refused ones count), the seventh is rate_limited.
    let bad = fill_params(&a, &["example.org"]);
    for _ in 0..5 {
        let e = call(&f, &w, "vault", Method::BrowserFill, bad.clone()).unwrap_err();
        assert_eq!(e.code, code::FORBIDDEN);
    }
    let e = call(&f, &w, "vault", Method::BrowserFill, good.clone()).unwrap_err();
    assert_eq!(e.code, code::RATE_LIMITED);
    let e = call(
        &f,
        &w,
        "vault",
        Method::SecretAsk,
        json!({"title": "Unlock", "label": "Password"}),
    )
    .unwrap_err();
    assert_eq!(e.code, code::RATE_LIMITED, "secret asks share the count");
    // "other" spent two (both busy), and still has room.
    let e = call(&f, &w, "other", Method::BrowserFill, bad).unwrap_err();
    assert_eq!(e.code, code::FORBIDDEN);
}

#[test]
fn a_fill_is_refused_where_there_is_no_web_page_or_a_tab_the_plugin_may_not_name() {
    let (f, a, b) = two_tabs();
    let slot = with_fake(&f);
    let w = world(&f, |r| {
        fill_plugin(r, "vault", &["fill", "tabs"]);
        fill_plugin(r, "narrow", &["fill"]);
    });
    turn(&f, "vault", true);
    turn(&f, "narrow", true);
    // No web page in the tab yet.
    let e = call(
        &f,
        &w,
        "vault",
        Method::BrowserFill,
        fill_params(&a, &["example.com"]),
    )
    .unwrap_err();
    assert_eq!(e.code, code::NOT_FOUND, "{e:?}");
    web_up(&f, &w, &slot, &a, PAGE);
    // A plugin without `tabs`, no click and no call from that tab may not name it.
    for (m, params) in [
        (Method::BrowserFill, fill_params(&a, &["example.com"])),
        (Method::BrowserFillPage, json!({"tab": a})),
    ] {
        let e = call(&f, &w, "narrow", m, params).unwrap_err();
        assert_eq!(e.code, code::FORBIDDEN, "{}", m.name());
    }
    // The other tab has no web page.
    let e = call(
        &f,
        &w,
        "vault",
        Method::BrowserFill,
        fill_params(&b, &["example.com"]),
    )
    .unwrap_err();
    assert_eq!(e.code, code::NOT_FOUND);
    // The agent has no fill, whatever its tab.
    for m in [
        Method::BrowserFill,
        Method::BrowserFillPage,
        Method::SecretAsk,
    ] {
        let e = agent_call(&f, &w, &a, m, fill_params(&a, &["example.com"])).unwrap_err();
        assert_eq!(e.code, code::FORBIDDEN, "{}", m.name());
    }
    // Bad params name the key, never the value.
    let mut p = fill_params(&a, &["example.com"]);
    p["password"] = json!(format!("{PASS}\u{7}"));
    let e = call(&f, &w, "vault", Method::BrowserFill, p).unwrap_err();
    assert_eq!(e.code, code::BAD_PARAMS);
    assert!(holds_no_secret(&e.message), "{}", e.message);
    assert!(scripts(&slot).is_empty());
}

#[test]
fn a_page_that_moved_after_the_question_is_not_filled_and_turning_the_plugin_off_ends_the_question()
{
    let (f, a, _b) = two_tabs();
    let slot = with_fake(&f);
    let w = world(&f, |r| fill_plugin(r, "vault", &["fill", "tabs"]));
    turn(&f, "vault", true);
    web_up(&f, &w, &slot, &a, PAGE);
    let rx = call_on_thread(
        &w.dispatcher,
        helper_ctx(&w, "vault"),
        Method::BrowserFill,
        fill_params(&a, &["example.com"]),
    );
    f.pump_until("the question", asking);
    // While the person reads, the page goes to another site.
    finish_page(&fake(&slot), &a, "https://evil.example.net/login");
    f.pump();
    f.h.comp.invoke_pask_fill(1);
    let e = answer_of(&f, &rx).unwrap_err();
    assert_eq!(e.code, code::INTERRUPTED);
    assert!(
        scripts(&slot).is_empty(),
        "nothing typed into the other site"
    );

    finish_page(&fake(&slot), &a, PAGE);
    f.pump();
    let rx = call_on_thread(
        &w.dispatcher,
        helper_ctx(&w, "vault"),
        Method::BrowserFill,
        fill_params(&a, &["example.com"]),
    );
    f.pump_until("the question", asking);
    turn(&f, "vault", false);
    assert_eq!(answer_of(&f, &rx).unwrap_err().code, code::UNAUTHORIZED);
    assert!(!asking(&f), "the question goes with the plugin");
}

#[test]
fn secret_ask_hands_the_typed_text_to_the_plugin_that_asked_or_says_cancelled() {
    let (f, _a, _b) = two_tabs();
    let _slot = with_fake(&f);
    let w = world(&f, |r| fill_plugin(r, "vault", &["fill"]));
    turn(&f, "vault", true);
    let ask = || {
        call_on_thread(
            &w.dispatcher,
            helper_ctx(&w, "vault"),
            Method::SecretAsk,
            json!({"title": "Unlock the vault", "label": "Master password"}),
        )
    };
    let rx = ask();
    f.pump_until("the question", asking);
    assert_eq!(f.h.comp.get_pask_kind(), 1);
    assert_eq!(f.h.comp.get_pask_title(), "Vault vault: Unlock the vault");
    assert_eq!(f.h.comp.get_pask_label(), "Master password");
    f.h.comp.invoke_pask_secret(true, PASS.into());
    assert_eq!(answer_of(&f, &rx).unwrap(), json!({"text": PASS}));
    assert!(!asking(&f));

    let rx = ask();
    f.pump_until("the question", asking);
    f.h.comp.invoke_pask_secret(false, "".into());
    assert_eq!(answer_of(&f, &rx).unwrap(), json!({"cancelled": true}));
    // An answer with no question up does nothing.
    f.h.comp.invoke_pask_secret(true, PASS.into());
    f.h.comp.invoke_pask_fill(1);
    assert!(f.ctl.borrow().plugin_ask.is_none());
    // A bad title is refused before anything is shown.
    let e = call(
        &f,
        &w,
        "vault",
        Method::SecretAsk,
        json!({"title": "", "label": "x"}),
    )
    .unwrap_err();
    assert_eq!(e.code, code::BAD_PARAMS);
}

/// A plugin with only a helper hears the agent's call as a `tool` event on its connection, answers with
/// `tools.respond`, and while the call waits it may fill into the tab the call came from (tool scope).
#[test]
fn a_helper_hears_its_tools_and_may_fill_the_callers_tab_while_the_call_waits() {
    let (f, a, _b) = two_tabs();
    let slot = with_fake(&f);
    let w = world(&f, |r| helper_tool_plugin(r, "vault"));
    let heard: Arc<Mutex<Vec<(String, Value, String)>>> = Arc::default();
    {
        let heard = Arc::clone(&heard);
        f.ctl
            .borrow_mut()
            .set_plugin_owner_events(Arc::new(move |name, data, owner| {
                heard
                    .lock()
                    .unwrap()
                    .push((name.to_string(), data, owner.to_string()));
                1
            }));
    }
    turn(&f, "vault", true);
    assert!(w.hub.answers_with_helper("vault"));
    assert!(
        w.hub
            .tools()
            .iter()
            .any(|t| t.name == "mod_vault_fill_login")
    );
    web_up(&f, &w, &slot, &a, PAGE);

    // Before any call, the helper may not name the tab (no `tabs`, no click).
    let e = call(&f, &w, "vault", Method::BrowserFillPage, json!({"tab": a})).unwrap_err();
    assert_eq!(e.code, code::FORBIDDEN);

    let tool_call = call_on_thread(
        &w.dispatcher,
        agent_in(&a),
        Method::ModsCall,
        json!({"name": "mod_vault_fill_login", "input": {}}),
    );
    f.pump_until("the tool event", |_| !heard.lock().unwrap().is_empty());
    let (name, data, owner) = heard.lock().unwrap()[0].clone();
    assert_eq!((name.as_str(), owner.as_str()), ("tool", "vault"));
    assert_eq!(data["tool"], "fill_login");
    assert_eq!(data["tab"], json!(a));
    assert_eq!(data["input"], json!({}));
    let call_id = data["call_id"].as_str().unwrap().to_string();
    assert!(f.ctl.borrow().panels.mod_of(&a).is_none(), "no panel opens");

    // While the call waits: the page's address, then a fill, both on the caller's tab.
    let page = call(&f, &w, "vault", Method::BrowserFillPage, json!({"tab": a})).unwrap();
    assert_eq!(page["host"], "accounts.example.com");
    let fill = call_on_thread(
        &w.dispatcher,
        helper_ctx(&w, "vault"),
        Method::BrowserFill,
        fill_params(&a, &["example.com"]),
    );
    f.pump_until("the question", asking);
    f.h.comp.invoke_pask_fill(1);
    let (_, id, _) = scripts(&slot)[0].clone();
    fake(&slot).say(WebEvent::Script {
        id,
        result: Ok(r#"{"ok":true,"filled":["username","password","totp"],"missing":[]}"#.into()),
    });
    let filled = answer_of(&f, &fill).unwrap();
    assert_eq!(filled["filled"], json!(["username", "password", "totp"]));

    // The helper answers the agent with the outcome only.
    call(
        &f,
        &w,
        "vault",
        Method::ToolsRespond,
        json!({"call_id": call_id, "result": {"filled": true}}),
    )
    .unwrap();
    let agent_got = answer_of(&f, &tool_call).unwrap();
    assert_eq!(agent_got, json!({"result": {"filled": true}}));
    assert!(holds_no_secret(&agent_got.to_string()));
    // The call is answered: the scope is gone.
    let e = call(&f, &w, "vault", Method::BrowserFillPage, json!({"tab": a})).unwrap_err();
    assert_eq!(e.code, code::FORBIDDEN);
}

#[test]
fn a_helper_that_is_not_connected_fails_the_tool_call_at_once() {
    let (f, a, _b) = two_tabs();
    let _slot = with_fake(&f);
    let w = world(&f, |r| helper_tool_plugin(r, "vault"));
    f.ctl
        .borrow_mut()
        .set_plugin_owner_events(Arc::new(|_, _, _| 0));
    turn(&f, "vault", true);
    let tool_call = call_on_thread(
        &w.dispatcher,
        agent_in(&a),
        Method::ModsCall,
        json!({"name": "mod_vault_fill_login", "input": {}}),
    );
    let e = answer_of(&f, &tool_call).unwrap_err();
    assert_eq!(e.code, code::UNAVAILABLE);
    assert!(e.message.contains("helper"), "{}", e.message);
}

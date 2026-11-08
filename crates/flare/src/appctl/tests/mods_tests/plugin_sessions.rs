//! Sessions, typing and storage for plugins (plugins phase 5): a tab's transcript is followed only while a plugin that
//! holds `sessions` listens and may see the tab; its entries are `session` events of a fixed shape within each
//! plugin's share; `session.read` gives the last 200; `tabs.input` types through the paste path with its checks, and
//! the Enter only while the plugin is still on; `storage.*` is each plugin's own. The plugins are real folders, the
//! hub and the dispatcher the real ones, the background pages the fake browser's, and the transcript a file in a fake
//! Claude folder.

use super::*;
use flare_convo::follow::Said;
use flare_convo::said::{Entry, Role};

const SESSION: &str = "11111111-2222-4333-8444-555555555555";

/// A protocol 2 plugin with a background page and the given permissions.
fn bare_plugin(root: &Path, id: &str, perms: &[&str]) {
    let text = format!(
        r#"{{"id":"{id}","name":"Plugin {id}","version":"1.0.0","protocol":2,"description":"A test plugin.","author":"me","homepage":"https://example.com/{id}","permissions":{perms:?},"background":"bg.html"}}"#
    );
    let to = root.join(id);
    std::fs::create_dir_all(&to).unwrap(); // sync-ok: test code only, never the app
    std::fs::write(to.join("plugin.json"), text).unwrap(); // sync-ok: test code only, never the app
    std::fs::write(to.join("bg.html"), "<p>bg</p>").unwrap(); // sync-ok: test code only, never the app
}

/// A fake Claude folder the window reads, and tab `a` (in `C:\a`) told its Claude session by the agent's hook.
struct Claude {
    dir: tempfile::TempDir,
}

impl Claude {
    fn new(f: &Fx, tab_cwd: &str, tab: &str) -> Claude {
        let dir = tempfile::tempdir().unwrap();
        f.ctl
            .borrow_mut()
            .set_convo_claude_dir(Some(dir.path().to_path_buf()));
        let hook = format!("\x1b]777;flare-agent;state=working;session={SESSION}\x07");
        f.made_with(tab_cwd)[0].feed(hook.as_bytes());
        f.pump_until("the session", |f| {
            f.ctl.borrow().hub().claude_session(tab) == Some(SESSION)
        });
        Claude { dir }
    }

    fn append(&self, lines: &[String]) {
        use std::io::Write;
        let path = flare_convo::transcript_path_in(self.dir.path(), "C:\\a", SESSION).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap(); // sync-ok: test code only
        let mut file = std::fs::OpenOptions::new() // sync-ok: test code only
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        for l in lines {
            writeln!(file, "{l}").unwrap();
        }
    }
}

fn user_line(text: &str) -> String {
    json!({"type": "user", "timestamp": "2026-10-26T09:00:00Z", "message": {"content": text}})
        .to_string()
}

fn tool_line(name: &str, command: &str) -> String {
    json!({"type": "assistant", "message": {"content": [
        {"type": "text", "text": "Running it."},
        {"type": "tool_use", "id": "t", "name": name, "input": {"command": command}}
    ]}})
    .to_string()
}

fn session_events(slot: &Slot, id: &str) -> Vec<Value> {
    bg_posts(slot, id)
        .into_iter()
        .filter(|v| v["t"] == "event" && v["name"] == "session")
        .map(|v| v["data"].clone())
        .collect()
}

fn followed(f: &Fx) -> Vec<String> {
    f.ctl.borrow().followed_sessions()
}

fn listen(f: &Fx, slot: &Slot, id: &str) {
    bg_message(f, slot, id, r#"{"t":"listen","name":"session"}"#);
}

fn code_of_reply(v: &Value) -> String {
    v["error"]["code"].as_str().unwrap_or("").to_string()
}

#[test]
fn a_tab_is_followed_only_while_a_plugin_with_sessions_listens_and_may_see_it() {
    let (f, a, b) = two_tabs();
    let slot = with_fake(&f);
    let _w = world(&f, |r| {
        bare_plugin(r, "reader", &["sessions", "tabs"]);
        bare_plugin(r, "own-only", &["sessions"]);
        bare_plugin(r, "no-sessions", &["tabs"]);
    });
    let claude = Claude::new(&f, "C:\\a", &a);
    for id in ["reader", "own-only", "no-sessions"] {
        turn(&f, id, true);
        bg_up(&f, &slot, id);
    }
    // Nobody listens: nothing runs.
    f.ctl.borrow_mut().refresh();
    assert!(followed(&f).is_empty());
    // A plugin without `sessions`, and one whose background page sits in no tab and holds no `tabs`: still nothing.
    listen(&f, &slot, "no-sessions");
    listen(&f, &slot, "own-only");
    assert!(followed(&f).is_empty());
    // A plugin that holds both listens: tab a (the one with a Claude session) is followed, tab b (none) is not.
    listen(&f, &slot, "reader");
    assert_eq!(followed(&f), std::slice::from_ref(&a));
    let _ = &b;
    // What is said from now on reaches it, in the event's shape; the others hear nothing.
    std::thread::sleep(Duration::from_millis(300)); // sync-ok: test code only, the follower settles at the end
    claude.append(&[
        user_line("Look at the build"),
        tool_line("Bash", "cargo build"),
    ]);
    f.pump_until("the session events", |_| {
        session_events(&slot, "reader").len() >= 3
    });
    let got = session_events(&slot, "reader");
    assert_eq!(
        got[0],
        json!({"tab": a, "session": SESSION, "role": "user", "text": "Look at the build", "at": "2026-10-26T09:00:00Z"})
    );
    assert_eq!(
        got[1],
        json!({"tab": a, "session": SESSION, "role": "assistant", "text": "Running it."})
    );
    assert_eq!(
        got[2],
        json!({"tab": a, "session": SESSION, "role": "tool", "text": "cargo build", "tool": "Bash"})
    );
    assert!(session_events(&slot, "own-only").is_empty());
    assert!(session_events(&slot, "no-sessions").is_empty());
    // The plugin is turned off: the follower stops and its thread ends.
    let exit = f.ctl.borrow().sessions.exit_flag(&a).expect("a follower");
    turn(&f, "reader", false);
    assert!(followed(&f).is_empty());
    f.pump_until("the follower to end", |_| {
        exit.load(std::sync::atomic::Ordering::SeqCst)
    });
    // On again and listening: followed again; the tab closes: no longer.
    turn(&f, "reader", true);
    bg_up(&f, &slot, "reader");
    listen(&f, &slot, "reader");
    assert_eq!(followed(&f), std::slice::from_ref(&a));
    f.ctl.borrow_mut().close_now(&a);
    f.pump_until("a to close", |f| !f.ctl.borrow().is_shell_tab(&a));
    f.ctl.borrow_mut().refresh();
    assert!(followed(&f).is_empty());
}

#[test]
fn each_plugin_hears_fifty_entries_a_second_and_the_next_event_says_what_it_missed() {
    let (f, a, _b) = two_tabs();
    let slot = with_fake(&f);
    let _w = world(&f, |r| bare_plugin(r, "reader", &["sessions", "tabs"]));
    let _claude = Claude::new(&f, "C:\\a", &a);
    turn(&f, "reader", true);
    bg_up(&f, &slot, "reader");
    listen(&f, &slot, "reader");
    assert_eq!(followed(&f), std::slice::from_ref(&a));
    let entry = |i: usize| Entry {
        role: Role::User,
        text: format!("p{i}\u{200b}"),
        tool: None,
        at: None,
    };
    mods_msg(
        &f,
        ModsMsg::Said {
            tab: a.clone(),
            session: SESSION.into(),
            said: Said {
                entries: (0..60).map(entry).collect(),
                skipped: 0,
            },
        },
    );
    let got = session_events(&slot, "reader");
    assert_eq!(got.len(), 50, "fifty a second");
    // The characters that print nothing are not handed on.
    assert_eq!(got[0]["text"], "p0");
    assert!(got.iter().all(|e| e.get("dropped").is_none()));
    // A moment later the next one says the ten it missed, plus what the follower skipped.
    std::thread::sleep(Duration::from_millis(60)); // sync-ok: test code only
    mods_msg(
        &f,
        ModsMsg::Said {
            tab: a.clone(),
            session: SESSION.into(),
            said: Said {
                entries: vec![entry(99)],
                skipped: 5,
            },
        },
    );
    let got = session_events(&slot, "reader");
    assert_eq!(got.len(), 51);
    assert_eq!(got[50]["dropped"], 15);
    assert_eq!(got[50]["text"], "p99");
    // A batch for a session the tab is not followed for is nobody's.
    mods_msg(
        &f,
        ModsMsg::Said {
            tab: a.clone(),
            session: "22222222-2222-4333-8444-555555555555".into(),
            said: Said {
                entries: vec![entry(1)],
                skipped: 0,
            },
        },
    );
    assert_eq!(session_events(&slot, "reader").len(), 51);
}

#[test]
fn session_read_gives_the_last_two_hundred_entries_to_a_plugin_that_holds_sessions_only() {
    let (f, a, b) = two_tabs();
    let slot = with_fake(&f);
    let w = world(&f, |r| {
        bare_plugin(r, "reader", &["sessions", "tabs"]);
        bare_plugin(r, "narrow", &["sessions"]);
        bare_plugin(r, "blind", &["tabs"]);
    });
    let claude = Claude::new(&f, "C:\\a", &a);
    let lines: Vec<String> = (0..230)
        .map(|i| user_line(&format!("prompt {i}")))
        .collect();
    claude.append(&lines);
    for id in ["reader", "narrow", "blind"] {
        turn(&f, id, true);
        bg_up(&f, &slot, id);
    }
    let v = bg_call(&f, &slot, "reader", 1, "session.read", json!({"tab": a}));
    let r = &v["result"];
    assert_eq!(r["session"], SESSION);
    assert_eq!(r["tab"], a);
    let entries = r["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 200);
    assert_eq!(entries[0]["text"], "prompt 30");
    assert_eq!(
        entries[199],
        json!({"role": "user", "text": "prompt 229", "at": "2026-10-26T09:00:00Z"})
    );
    // Read once a second per tab: the next one at once is rate limited; another tab is its own.
    let v = bg_call(&f, &slot, "reader", 2, "session.read", json!({"tab": a}));
    assert_eq!(code_of_reply(&v), "rate_limited");
    // A tab with no known session has no entries.
    let v = bg_call(&f, &slot, "reader", 3, "session.read", json!({"tab": b}));
    assert_eq!(v["result"]["session"], Value::Null);
    assert_eq!(v["result"]["entries"], json!([]));
    // Without `sessions`: forbidden before anything runs. Without `tabs` a background page names no tab.
    let v = bg_call(&f, &slot, "blind", 4, "session.read", json!({"tab": a}));
    assert_eq!(code_of_reply(&v), "forbidden");
    let v = bg_call(&f, &slot, "narrow", 5, "session.read", json!({"tab": a}));
    assert_eq!(code_of_reply(&v), "forbidden");
    let v = bg_call(
        &f,
        &slot,
        "reader",
        6,
        "session.read",
        json!({"tab": b, "since": 3}),
    );
    assert_eq!(code_of_reply(&v), "bad_params");
    // The agent: refused by its grant over the pipe, and by the dispatcher itself.
    assert_eq!(
        Grant::agent().check(Method::SessionRead).unwrap_err().code,
        code::FORBIDDEN
    );
    let e = w
        .dispatcher
        .call(&agent_in(&a), Method::SessionRead, json!({}))
        .unwrap_err();
    assert_eq!(e.code, code::FORBIDDEN);
}

/// Bracketed paste is on in the tab (the program asked for it, as Claude Code does).
fn bracketed(f: &Fx, tab: &str) {
    let h = f.ctl.borrow().handle_of(tab).expect("the tab's terminal");
    feed(&h, "\x1b[?2004h");
}

fn written(f: &Fx, cwd: &str) -> Vec<u8> {
    f.made_with(cwd)[0].written()
}

#[test]
fn tabs_input_types_as_a_paste_with_its_checks_and_the_enter_comes_after() {
    let (f, a, b) = two_tabs();
    let slot = with_fake(&f);
    // The still clock: "two a second, the third at once is refused" holds however long the runner takes
    // between the calls (on the real clock a slow macOS runner refilled the bucket before the third).
    let w = world_still(&f, |r| {
        bare_plugin(r, "typist", &["input", "tabs"]);
        bare_plugin(r, "narrow", &["input"]);
        bare_plugin(r, "mute", &["tabs", "sessions"]);
    });
    for id in ["typist", "narrow", "mute"] {
        turn(&f, id, true);
        bg_up(&f, &slot, id);
    }
    bracketed(&f, &a);
    let v = bg_call(
        &f,
        &slot,
        "typist",
        1,
        "tabs.input",
        json!({"tab": a, "text": "/compact"}),
    );
    assert_eq!(v["result"], json!({"typed": true, "enter": false}));
    assert_eq!(written(&f, "C:\\a"), b"\x1b[200~/compact\x1b[201~");
    // Every check on the worker: nothing reaches the shell.
    let cases = [
        (
            json!({"tab": a, "text": "a\u{1b}[201~rm -rf ~"}),
            "bad_params",
        ),
        (json!({"tab": a, "text": "\u{3}"}), "bad_params"),
        (json!({"tab": a, "text": "c1\u{9b}201~"}), "bad_params"),
        (json!({"tab": a, "text": "x".repeat(4097)}), "bad_params"),
        (json!({"tab": a, "text": ""}), "bad_params"),
        (json!({"tab": a}), "bad_params"),
        (json!({"tab": a, "text": "x", "enter": "yes"}), "bad_params"),
        (json!({"tab": a, "text": "x", "raw": true}), "bad_params"),
    ];
    for (n, (params, want)) in cases.iter().enumerate() {
        let v = bg_call(
            &f,
            &slot,
            "typist",
            10 + n as u64,
            "tabs.input",
            params.clone(),
        );
        assert_eq!(code_of_reply(&v), *want, "{params}");
    }
    assert_eq!(written(&f, "C:\\a"), b"\x1b[200~/compact\x1b[201~");
    // With Enter: the paste now, the carriage return a moment later.
    let v = bg_call(
        &f,
        &slot,
        "typist",
        20,
        "tabs.input",
        json!({"tab": a, "text": "two\nlines", "enter": true}),
    );
    assert_eq!(v["result"]["enter"], true);
    let after = b"\x1b[200~/compact\x1b[201~\x1b[200~two\rlines\x1b[201~".to_vec();
    assert_eq!(written(&f, "C:\\a"), after);
    f.pump_until("the Enter", |f| {
        written(f, "C:\\a").ends_with(b"\x1b[201~\r")
    });
    // Two a second per tab: the third at once is rate limited.
    let v = bg_call(
        &f,
        &slot,
        "typist",
        21,
        "tabs.input",
        json!({"tab": a, "text": "x"}),
    );
    assert_eq!(code_of_reply(&v), "rate_limited");
    // A program that never asked for bracketed paste gets the paste as the person's would be (no final Enter).
    let v = bg_call(
        &f,
        &slot,
        "typist",
        22,
        "tabs.input",
        json!({"tab": b, "text": "echo hi\n"}),
    );
    assert_eq!(v["result"]["typed"], true);
    assert_eq!(written(&f, "C:\\b"), b"echo hi");
    // Without `input`: forbidden. Without `tabs`, a background page names no tab.
    let v = bg_call(
        &f,
        &slot,
        "mute",
        23,
        "tabs.input",
        json!({"tab": b, "text": "x"}),
    );
    assert_eq!(code_of_reply(&v), "forbidden");
    let v = bg_call(
        &f,
        &slot,
        "narrow",
        24,
        "tabs.input",
        json!({"tab": b, "text": "x"}),
    );
    assert_eq!(code_of_reply(&v), "forbidden");
    // The agent never types into a shell (rules 2 and 188): its grant, and the dispatcher itself, refuse it.
    assert_eq!(
        Grant::agent().check(Method::TabsInput).unwrap_err().code,
        code::FORBIDDEN
    );
    let e = w
        .dispatcher
        .call(&agent_in(&b), Method::TabsInput, json!({"text": "x"}))
        .unwrap_err();
    assert_eq!(e.code, code::FORBIDDEN);
    assert_eq!(written(&f, "C:\\b"), b"echo hi");
}

#[test]
fn the_enter_is_not_pressed_once_the_plugin_is_off_and_a_closed_tab_takes_nothing() {
    let (f, a, b) = two_tabs();
    let slot = with_fake(&f);
    let _w = world(&f, |r| bare_plugin(r, "typist", &["input", "tabs"]));
    turn(&f, "typist", true);
    bg_up(&f, &slot, "typist");
    bracketed(&f, &a);
    let v = bg_call(
        &f,
        &slot,
        "typist",
        1,
        "tabs.input",
        json!({"tab": a, "text": "hello", "enter": true}),
    );
    assert_eq!(v["result"]["enter"], true);
    // Switched off before the Enter is due: the paste stays, no Enter follows.
    turn(&f, "typist", false);
    std::thread::sleep(Duration::from_millis(
        crate::plugin_sessions::ENTER_AFTER_MS + 200,
    )); // sync-ok: test code only
    f.pump();
    assert_eq!(written(&f, "C:\\a"), b"\x1b[200~hello\x1b[201~");
    // On again: a tab that closed is `no_tab`, and nothing is written anywhere.
    turn(&f, "typist", true);
    bg_up(&f, &slot, "typist");
    f.ctl.borrow_mut().close_now(&b);
    f.pump_until("b to close", |f| !f.ctl.borrow().is_shell_tab(&b));
    let v = bg_call(
        &f,
        &slot,
        "typist",
        2,
        "tabs.input",
        json!({"tab": b, "text": "x"}),
    );
    assert_eq!(code_of_reply(&v), "no_tab");
}

#[test]
fn storage_is_each_plugins_own_and_never_a_protocol_1_mods_or_the_agents() {
    let (f, a, _b) = two_tabs();
    let slot = with_fake(&f);
    let w = world(&f, |r| {
        bare_plugin(r, "keeper", &[]);
        bare_plugin(r, "other", &[]);
        mod_with(r, "old", &["panel"]);
    });
    for id in ["keeper", "other", "old"] {
        turn(&f, id, true);
    }
    for id in ["keeper", "other"] {
        bg_up(&f, &slot, id);
    }
    // A plugin with no permission at all keeps its own store.
    let v = bg_call(
        &f,
        &slot,
        "keeper",
        1,
        "storage.set",
        json!({"key": "list", "value": [1, 2]}),
    );
    assert_eq!(v["result"], json!({}));
    let v = bg_call(
        &f,
        &slot,
        "keeper",
        2,
        "storage.get",
        json!({"key": "list"}),
    );
    assert_eq!(v["result"]["value"], json!([1, 2]));
    let v = bg_call(&f, &slot, "other", 3, "storage.get", json!({}));
    assert_eq!(
        v["result"]["value"],
        json!({}),
        "another plugin's store is its own"
    );
    let v = bg_call(
        &f,
        &slot,
        "keeper",
        4,
        "storage.set",
        json!({"key": "list"}),
    );
    assert_eq!(code_of_reply(&v), "bad_params");
    let v = bg_call(
        &f,
        &slot,
        "keeper",
        5,
        "storage.get",
        json!({"key": "list", "x": 1}),
    );
    assert_eq!(code_of_reply(&v), "bad_params");
    // Past 1 MB: too large, and the store is as it was. (Straight to the dispatcher: a page's message is smaller.)
    let keeper = CallContext {
        conn: 0,
        client: "mod:keeper".into(),
        client_version: String::new(),
        grant: w.hub.grant_of_mod("keeper").unwrap(),
        tab: None,
    };
    let big = "x".repeat(flare_mods::storage::MAX_BYTES);
    let e = w
        .dispatcher
        .call(
            &keeper,
            Method::StorageSet,
            json!({"key": "big", "value": big}),
        )
        .unwrap_err();
    assert_eq!(e.code, code::TOO_LARGE);
    let v = bg_call(&f, &slot, "keeper", 6, "storage.get", json!({}));
    assert_eq!(v["result"]["value"], json!({"list": [1, 2]}));
    // It lands in the profile, one file per plugin, written whole.
    let store = w.hub.storage();
    let t = Instant::now();
    while !store.is_idle("keeper") {
        assert!(t.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(5)); // sync-ok: test code only
    }
    let file = f.dir.path().join("plugins-storage").join("keeper.json");
    assert_eq!(store.file_of("keeper"), file);
    let on_disk: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap(); // sync-ok: test code only
    assert_eq!(on_disk, json!({"list": [1, 2]}));
    // A protocol 1 mod keeps exactly what it had: no store.
    let old = CallContext {
        conn: 0,
        client: "mod:old".into(),
        client_version: String::new(),
        grant: w.hub.grant_of_mod("old").unwrap(),
        tab: Some(a.clone()),
    };
    let e = w
        .dispatcher
        .call(&old, Method::StorageGet, json!({}))
        .unwrap_err();
    assert_eq!(e.code, code::FORBIDDEN);
    // The agent: refused by its grant, and by the dispatcher itself.
    assert_eq!(
        Grant::agent().check(Method::StorageSet).unwrap_err().code,
        code::FORBIDDEN
    );
    let e = w
        .dispatcher
        .call(&agent_in(&a), Method::StorageGet, json!({}))
        .unwrap_err();
    assert_eq!(e.code, code::FORBIDDEN);
}

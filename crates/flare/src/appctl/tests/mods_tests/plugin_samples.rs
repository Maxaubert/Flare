//! The sample plugins of `sample-plugins/` run (macOS port phase 6, regression rule 1060): each one is copied as it
//! ships into the plugins folder, turned on, its background page brought up, and one round trip of its protocol driven
//! with a call its own page makes (or, for the tour, the tool its page answers). The hub, the dispatcher and the grant
//! are the real ones; the page is played by the fake browser, so nothing is shown. The same test runs on Windows and
//! on a Mac: the samples are pages only (no helper), so nothing in them differs by system.

use super::*;

/// Copies the sample `id` as it ships, every file of it, into the plugins folder.
fn copy_sample(root: &Path, id: &str) {
    let from = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../sample-plugins")
        .join(id);
    let to = root.join(id);
    std::fs::create_dir_all(&to).unwrap(); // sync-ok: test code only, never the app
    for entry in std::fs::read_dir(&from).unwrap() {
        // sync-ok: test code only, never the app
        let entry = entry.unwrap();
        assert!(entry.file_type().unwrap().is_file(), "a sample is flat");
        std::fs::copy(entry.path(), to.join(entry.file_name())).unwrap(); // sync-ok: test code only, never the app
    }
}

/// Every sample, by its folder name.
fn samples() -> Vec<String> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sample-plugins");
    let mut out: Vec<String> = std::fs::read_dir(dir) // sync-ok: test code only, never the app
        .unwrap()
        .filter_map(|e| {
            let e = e.unwrap();
            e.file_type()
                .unwrap()
                .is_dir()
                .then(|| e.file_name().to_string_lossy().into_owned())
        })
        .collect();
    out.sort();
    out
}

#[test]
fn every_sample_plugin_loads_turns_on_and_answers_one_round_trip_of_its_protocol() {
    let ids = samples();
    assert_eq!(
        ids,
        [
            "agent-moves",
            "browser-badge",
            "look",
            "manifest-tour",
            "session-buttons",
            "sonara-bar",
            "todo",
            "what-next"
        ],
        "a new sample gets its round trip here"
    );
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let w = world(&f, |root| {
        for id in &ids {
            copy_sample(root, id);
        }
    });
    let snap = w.hub.snapshot();
    assert!(snap.errors.is_empty(), "{:?}", snap.errors);
    assert_eq!(snap.mods.len(), ids.len());
    let tab = f.ctl.borrow().front_id().unwrap();
    // One call each page makes, with the parameters it sends.
    let calls: [(&str, &str, Value); 7] = [
        ("agent-moves", "storage.get", json!({"key": "picks"})),
        (
            "browser-badge",
            "tabs.badge.set",
            json!({"tab": tab, "key": "browser", "icon": "globe", "tip": "A browser panel is open"}),
        ),
        (
            "look",
            "tabs.progress.set",
            json!({"tab": tab, "key": "steps", "value": 0.5, "label": "1 of 2"}),
        ),
        (
            "session-buttons",
            "actions.set",
            json!({"key": "bar", "tab": tab, "checked": true, "title": "Hide the buttons bar"}),
        ),
        ("sonara-bar", "storage.get", json!({"key": "now"})),
        ("todo", "sidebar.set", json!({"key": "todos", "items": []})),
        (
            "what-next",
            "sidebar.set",
            json!({"key": "next", "items": []}),
        ),
    ];
    // At most six background pages run at once, so each sample is turned on, driven and turned off in turn.
    for (n, id) in ids.iter().enumerate() {
        turn(&f, id, true);
        assert!(w.hub.is_enabled(id), "{id} is on");
        assert!(running(&f).contains(id), "{id}'s background page runs");
        bg_up(&f, &slot, id);
        if let Some((_, method, params)) = calls.iter().find(|c| c.0 == id) {
            let page = std::fs::read_to_string(w.root.join(id).join("background.html")).unwrap(); // sync-ok: test code only, never the app
            assert!(
                page.contains(&format!("'{method}'")),
                "{id}'s page calls {method}"
            );
            let reply = bg_call(&f, &slot, id, n as u64 + 1, method, params.clone());
            assert!(
                reply.get("error").is_none_or(Value::is_null) && reply.get("result").is_some(),
                "{id} {method}: {reply}"
            );
        } else {
            assert_eq!(id, "manifest-tour", "every other sample has its call above");
            tour_tool(&f, &w, &slot, &tab);
        }
        turn(&f, id, false);
        assert!(!running(&f).contains(id), "{id} is off");
    }
}

/// The tour makes no call of its own: its page answers the `add_item` tool, so its round trip is a tool call.
fn tour_tool(f: &Fx, w: &World, slot: &Slot, tab: &str) {
    let rx = call_on_thread(
        &w.dispatcher,
        agent_in(tab),
        Method::ModsCall,
        json!({"name": "mod_manifest_tour_add_item", "input": {"text": "x"}}),
    );
    f.pump_until("the tool event", |_| {
        bg_posts(slot, "manifest-tour")
            .iter()
            .any(|v| v["t"] == "tool")
    });
    let ev = bg_posts(slot, "manifest-tour")
        .into_iter()
        .find(|v| v["t"] == "tool")
        .unwrap();
    assert_eq!(ev["tool"], "add_item");
    bg_message(
        f,
        slot,
        "manifest-tour",
        &json!({"t": "respond", "call_id": ev["callId"], "result": {"added": "x"}}).to_string(),
    );
    assert_eq!(
        answer_of(f, &rx).unwrap(),
        json!({"result": {"added": "x"}})
    );
}

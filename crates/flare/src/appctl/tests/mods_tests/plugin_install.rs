//! Install, update and remove in the window, and the helper programs (plugins phase 7): Settings' asks reach the
//! workers and come back as a scan does, an install is off, an update that asks for more turns the plugin off, a
//! removal forgets it, and a plugin's helper runs only while the plugin is on and hears the events it may see. The
//! hub, the workers and the helper host are the real ones; the Recycle Bin is a stand-in that deletes.

use super::*;
use std::io::Write as _;

fn plugin_json(version: &str, perms: &str, extra: &str) -> String {
    format!(
        r#"{{"id":"demo","name":"Demo","version":"{version}","protocol":2,"description":"d","author":"me","permissions":{perms},"background":"bg.html"{extra}}}"#
    )
}

fn zip_at(dir: &Path, name: &str, manifest: &str) -> PathBuf {
    let mut cur = std::io::Cursor::new(Vec::new());
    {
        let mut w = zip::ZipWriter::new(&mut cur);
        let o = zip::write::SimpleFileOptions::default();
        w.start_file("plugin.json", o).unwrap();
        w.write_all(manifest.as_bytes()).unwrap();
        w.start_file("bg.html", o).unwrap();
        w.write_all(b"<p>bg</p>").unwrap();
        w.finish().unwrap();
    }
    let p = dir.join(name);
    std::fs::write(&p, cur.into_inner()).unwrap(); // sync-ok: test code only, never the app
    p
}

fn installing(f: &Fx) -> bool {
    f.ctl
        .borrow()
        .mods
        .as_ref()
        .is_some_and(|m| m.install.lock().unwrap().installing)
}

fn note(f: &Fx) -> Option<(bool, String)> {
    f.ctl
        .borrow()
        .mods
        .as_ref()
        .and_then(|m| m.install.lock().unwrap().note.clone())
}

fn install_file(f: &Fx, zip: PathBuf, expect: Option<&str>) {
    mods_msg(
        f,
        ModsMsg::Picked {
            path: Some(zip),
            expect: expect.map(str::to_string),
        },
    );
    assert!(installing(f), "the install runs");
    f.pump_until("the install ends", |f| !installing(f));
}

fn recycle_by_deleting(w: &World) {
    w.hub
        .set_recycle(Arc::new(|p: &Path| flare_mods::install::delete_path(p)));
}

#[test]
fn an_install_from_a_file_lands_off_and_says_so() {
    let f = fx();
    let w = world(&f, |_| {});
    let z = zip_at(
        f.dir.path(),
        "a.zip",
        &plugin_json("1.0.0", r#"["decorate"]"#, ""),
    );
    install_file(&f, z, None);
    let (failed, said) = note(&f).expect("a sentence");
    assert!(!failed, "{said}");
    assert!(
        said.contains("Demo 1.0.0 is installed. It is off"),
        "{said}"
    );
    assert!(w.hub.found("demo").is_some());
    assert!(!w.hub.is_enabled("demo"));
    assert!(w.root.join("demo").join("plugin.json").exists());
}

#[test]
fn a_zip_against_the_rules_is_one_sentence_and_nothing_lands() {
    let f = fx();
    let w = world(&f, |_| {});
    let z = zip_at(f.dir.path(), "a.zip", "{}");
    install_file(&f, z, None);
    let (failed, said) = note(&f).unwrap();
    assert!(failed && !said.is_empty());
    assert!(w.hub.snapshot().mods.is_empty());
    // An update must hold the plugin it updates.
    let z = zip_at(f.dir.path(), "b.zip", &plugin_json("1.0.0", "[]", ""));
    install_file(&f, z, Some("other"));
    assert_eq!(
        note(&f),
        Some((
            true,
            "That zip holds the plugin \"demo\", not \"other\".".to_string()
        ))
    );
    assert!(w.hub.found("demo").is_none());
}

#[test]
fn an_update_that_asks_for_more_stops_the_plugin_and_turns_it_off() {
    let f = fx();
    let slot = with_fake(&f);
    let w = world(&f, |_| {});
    recycle_by_deleting(&w);
    install_file(
        &f,
        zip_at(
            f.dir.path(),
            "a.zip",
            &plugin_json("1.0.0", r#"["decorate"]"#, ""),
        ),
        None,
    );
    turn(&f, "demo", true);
    assert!(f.ctl.borrow().mods.as_ref().unwrap().bg_running("demo"));
    // The same permissions: it stays on, and its background page is made again for the new copy.
    install_file(
        &f,
        zip_at(
            f.dir.path(),
            "b.zip",
            &plugin_json("1.1.0", r#"["decorate"]"#, ""),
        ),
        Some("demo"),
    );
    assert!(w.hub.is_enabled("demo"));
    assert!(f.ctl.borrow().mods.as_ref().unwrap().bg_running("demo"));
    assert!(!w.hub.is_paused("demo"));
    let _ = fake(&slot);
    // More: turned off until approved again, its page closed.
    install_file(
        &f,
        zip_at(
            f.dir.path(),
            "c.zip",
            &plugin_json("1.2.0", r#"["decorate","tabs"]"#, ""),
        ),
        Some("demo"),
    );
    let (failed, said) = note(&f).unwrap();
    assert!(
        !failed && said.contains("off until you turn it on again"),
        "{said}"
    );
    assert!(!w.hub.is_enabled("demo"));
    assert!(w.hub.snapshot().mods[0].needs_approval);
    assert!(!f.ctl.borrow().mods.as_ref().unwrap().bg_running("demo"));
}

#[test]
fn remove_turns_the_plugin_off_and_forgets_its_approval_and_storage() {
    let f = fx();
    let _slot = with_fake(&f);
    let w = world(&f, |_| {});
    recycle_by_deleting(&w);
    install_file(
        &f,
        zip_at(
            f.dir.path(),
            "a.zip",
            &plugin_json("1.0.0", r#"["decorate"]"#, ""),
        ),
        None,
    );
    turn(&f, "demo", true);
    f.pump_until("the switch is saved", |_| {
        std::fs::read_to_string(&w.state_file).is_ok_and(|t| t.contains(r#""on":true"#)) // sync-ok: test code only
    });
    w.hub.storage().set("demo", Some("k"), json!(1)).unwrap();
    mods_msg(&f, ModsMsg::Remove { id: "demo".into() });
    assert!(!w.hub.is_enabled("demo"), "off at once");
    f.pump_until("the removal ends", |f| !installing(f));
    let (failed, said) = note(&f).unwrap();
    assert!(!failed && said.contains("Recycle Bin"), "{said}");
    assert!(!w.root.join("demo").exists());
    assert!(w.hub.found("demo").is_none());
    let state = std::fs::read_to_string(&w.state_file).unwrap(); // sync-ok: test code only
    assert!(!state.contains("demo"), "{state}");
    assert!(!w.hub.storage().file_of("demo").exists());
}

#[test]
fn install_from_file_opens_the_picker_and_a_cancel_changes_nothing() {
    let f = fx();
    let w = world(&f, |_| {});
    mods_msg(&f, ModsMsg::InstallFile);
    assert_eq!(
        note(&f).map(|(failed, _)| failed),
        Some(true),
        "no picker in this run says so"
    );
    let zip = zip_at(f.dir.path(), "a.zip", &plugin_json("1.0.0", "[]", ""));
    let answers: Rc<RefCell<Vec<Option<PathBuf>>>> = Rc::new(RefCell::new(vec![None, Some(zip)]));
    {
        let answers = answers.clone();
        f.ctl.borrow_mut().set_pick_zip(Rc::new(move |answer| {
            answer(answers.borrow_mut().remove(0))
        }));
    }
    mods_msg(&f, ModsMsg::InstallFile);
    f.pump();
    assert!(!installing(&f), "a cancel starts nothing");
    mods_msg(&f, ModsMsg::InstallFile);
    f.pump_until("the install ends", |f| {
        f.ctl.borrow().mods.as_ref().is_some_and(|m| {
            m.install
                .lock()
                .unwrap()
                .note
                .as_ref()
                .is_some_and(|n| !n.0)
        })
    });
    assert!(w.hub.found("demo").is_some());
    // An empty address is a sentence; nothing starts.
    mods_msg(&f, ModsMsg::InstallUrl { url: "  ".into() });
    assert!(!installing(&f));
    assert!(note(&f).unwrap().1.contains("Paste the https address"));
}

/// A plugin folder with `helper.exe` (a copy of cmd.exe) that runs `cmd /c <line>`.
fn helper_plugin(root: &Path, perms: &str, line: &str) {
    let to = root.join("demo");
    std::fs::create_dir_all(&to).unwrap(); // sync-ok: test code only, never the app
    let sys = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    std::fs::copy(format!(r"{sys}\System32\cmd.exe"), to.join("helper.exe")).unwrap(); // sync-ok: test code only, never the app
    let helper = format!(
        r#","helper":{{"command":"helper.exe","args":["/d","/c",{}]}}"#,
        serde_json::to_string(line).unwrap()
    );
    std::fs::write(to.join("plugin.json"), plugin_json("1.0.0", perms, &helper)).unwrap(); // sync-ok: test code only, never the app
    std::fs::write(to.join("bg.html"), "<p>bg</p>").unwrap(); // sync-ok: test code only, never the app
}

fn helpers_running(f: &Fx) -> Vec<String> {
    f.ctl
        .borrow()
        .mods
        .as_ref()
        .and_then(|m| m.helpers.as_ref().map(|h| h.running()))
        .unwrap_or_default()
}

#[test]
fn a_helper_runs_only_while_its_plugin_is_on_and_hears_the_events_it_may_see() {
    let f = fx();
    let _slot = with_fake(&f);
    let w = world(&f, |root| {
        helper_plugin(root, r#"["process","tabs"]"#, "ping -n 60 127.0.0.1 >nul")
    });
    let heard: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
    {
        let heard = Arc::clone(&heard);
        f.ctl
            .borrow_mut()
            .set_plugin_owner_events(Arc::new(move |name, _data, owner| {
                heard
                    .lock()
                    .unwrap()
                    .push((name.to_string(), owner.to_string()));
            }));
    }
    f.ctl
        .borrow_mut()
        .set_plugin_helpers(f.dir.path().join("plugins-logs"), r"\\.\pipe\x".into());
    assert!(helpers_running(&f).is_empty(), "off: no helper");
    turn(&f, "demo", true);
    assert_eq!(helpers_running(&f), ["demo"]);
    let log = flare_mods::helper::log_file(&f.dir.path().join("plugins-logs"), "demo");
    f.pump_until("the helper's log is made", |_| log.exists());
    // A `tabs` event reaches it over the channel (it holds `tabs`).
    f.ctl.borrow_mut().deliver_events(&[(
        "tabs",
        "t1".to_string(),
        json!({"kind": "focused", "tab": {"id": "t1"}}),
    )]);
    assert_eq!(
        heard.lock().unwrap().as_slice(),
        [("tabs".to_string(), "demo".to_string())]
    );
    turn(&f, "demo", false);
    assert!(helpers_running(&f).is_empty(), "off: its helper is stopped");
    assert!(w.hub.helper_of("demo").is_none());
}

#[test]
fn a_helper_without_tabs_hears_no_tab_events_and_one_that_ends_too_often_is_stopped() {
    let f = fx();
    let _slot = with_fake(&f);
    let w = world(&f, |root| helper_plugin(root, r#"["process"]"#, "exit 2"));
    let heard: Arc<Mutex<Vec<String>>> = Arc::default();
    {
        let heard = Arc::clone(&heard);
        f.ctl
            .borrow_mut()
            .set_plugin_owner_events(Arc::new(move |name, _, _| {
                heard.lock().unwrap().push(name.to_string());
            }));
    }
    f.ctl
        .borrow_mut()
        .set_plugin_helpers(f.dir.path().join("plugins-logs"), "p".into());
    turn(&f, "demo", true);
    f.ctl.borrow_mut().deliver_events(&[(
        "tabs",
        "t1".to_string(),
        json!({"kind": "focused", "tab": {"id": "t1"}}),
    )]);
    assert!(heard.lock().unwrap().is_empty());
    // `exit 2` ends at once: three restarts, then it stays stopped and Settings says so.
    f.pump_until("the helper is stopped", |_| {
        w.hub.helper_stopped("demo").is_some()
    });
    assert_eq!(
        w.hub.snapshot().mods[0].stopped.as_deref(),
        Some(flare_mods::helper::STOPPED_HELPER)
    );
    assert!(helpers_running(&f).is_empty());
    // Switching it again lets it run again.
    turn(&f, "demo", false);
    assert!(w.hub.helper_stopped("demo").is_none());
}

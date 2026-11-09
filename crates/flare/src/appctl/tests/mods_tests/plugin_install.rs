//! Install, update and remove in the window, and the helper programs (plugins phase 7): Settings' asks reach the
//! workers and come back as a scan does, an install is off, an update that asks for more turns the plugin off, a
//! removal forgets it, and a plugin's helper runs only while the plugin is on and hears the events it may see. The
//! hub, the workers and the helper host are the real ones; the Recycle Bin is a stand-in that deletes.

use super::*;
use crate::plugin_ui::{FOLDED_PREF, ORDER_PREF};
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
    // The person ordered and folded a section of it and one of another plugin.
    for key in [ORDER_PREF, FOLDED_PREF] {
        f.ctl.borrow().prefs.set(key, "demo/list\nother/list");
    }
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
    // Its sidebar order and fold are forgotten; the other plugin's stay.
    for key in [ORDER_PREF, FOLDED_PREF] {
        assert_eq!(
            f.ctl.borrow().prefs.get(key).as_deref(),
            Some("other/list"),
            "{key}"
        );
    }
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
        f.ctl.borrow_mut().set_pick_zip(Rc::new(move |_, answer| {
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

#[test]
fn update_opens_the_picker_in_the_folder_the_zip_came_from_and_a_restart_keeps_it() {
    let f = fx();
    let w = world(&f, |_| {});
    recycle_by_deleting(&w);
    let from = f.dir.path().join("downloads");
    std::fs::create_dir_all(&from).unwrap(); // sync-ok: test code only, never the app
    let z = zip_at(&from, "a.zip", &plugin_json("1.0.0", "[]", ""));
    install_file(&f, z, None);
    let asked: Rc<RefCell<Vec<Option<PathBuf>>>> = Rc::default();
    {
        let asked = asked.clone();
        f.ctl
            .borrow_mut()
            .set_pick_zip(Rc::new(move |folder, answer| {
                asked.borrow_mut().push(folder);
                answer(None);
            }));
    }
    mods_msg(&f, ModsMsg::Update { id: "demo".into() });
    f.pump();
    assert_eq!(asked.borrow().as_slice(), [Some(from.clone())]);
    assert!(!installing(&f), "a cancel starts nothing");
    // A fresh install has no plugin to remember a folder for.
    mods_msg(&f, ModsMsg::InstallFile);
    f.pump();
    assert_eq!(asked.borrow().last().cloned(), Some(None));
    let saved = flare_mods::enabled::EnabledStore::load(&w.state_file).installed("demo");
    assert_eq!(saved.folder.as_deref(), from.to_str());
    assert_eq!(saved.source, None, "a folder is not a source");
}

/// A plugin folder with its helper, which runs `line` in the system's shell. Windows: `helper.exe` (a copy of cmd.exe)
/// runs `cmd /c <line>`, named by `helper` as a manifest always has. macOS (port phase 6, rule 288): `helper`, a `#!`
/// script that hands its arguments to `/bin/sh`, named by `helpers.macos`, runs `sh -c <line>`.
fn helper_plugin(root: &Path, perms: &str, line: &str) {
    #[cfg(target_os = "macos")]
    flare_proc::guard::use_test_entry("appctl::tests::mods_tests::plugin_install::guard_entry")
        .unwrap();
    let to = root.join("demo");
    std::fs::create_dir_all(&to).unwrap(); // sync-ok: test code only, never the app
    let (name, bytes) = helper_program();
    std::fs::write(to.join(name), bytes).unwrap(); // sync-ok: test code only, never the app
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(to.join(name), std::fs::Permissions::from_mode(0o755)).unwrap(); // sync-ok: test code only, never the app
    }
    std::fs::write(to.join("plugin.json"), helper_json("1.0.0", perms, line)).unwrap(); // sync-ok: test code only, never the app
    std::fs::write(to.join("bg.html"), "<p>bg</p>").unwrap(); // sync-ok: test code only, never the app
}

/// Not a test on its own: a Mac helper's guard started by this binary's tests runs as this test (rule 285).
#[cfg(target_os = "macos")]
#[test]
fn guard_entry() {
    flare_proc::guard::run_if_guard();
}

/// The helper program's file name in the plugin folder and its bytes.
fn helper_program() -> (&'static str, Vec<u8>) {
    if cfg!(windows) {
        let sys = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        (
            "helper.exe",
            std::fs::read(format!(r"{sys}\System32\cmd.exe")).unwrap(), // sync-ok: test code only, never the app
        )
    } else {
        ("helper", b"#!/bin/sh\nexec /bin/sh \"$@\"\n".to_vec())
    }
}

/// A line for the helper's shell that waits about a minute.
fn wait_a_minute() -> &'static str {
    if cfg!(windows) {
        "ping -n 60 127.0.0.1 >nul"
    } else {
        "sleep 60"
    }
}

/// The text of [`helper_plugin`]'s `plugin.json` at `version`.
fn helper_json(version: &str, perms: &str, line: &str) -> String {
    let line = serde_json::to_string(line).unwrap();
    let helper = if cfg!(windows) {
        format!(r#","helper":{{"command":"helper.exe","args":["/d","/c",{line}]}}"#)
    } else {
        format!(r#","helpers":{{"macos":{{"command":"helper","args":["-c",{line}]}}}}"#)
    };
    plugin_json(version, perms, &helper)
}

fn helpers_running(f: &Fx) -> Vec<String> {
    f.ctl
        .borrow()
        .mods
        .as_ref()
        .and_then(|m| m.helpers.as_ref().map(|h| h.running()))
        .unwrap_or_default()
}

#[cfg_attr(
    not(any(windows, target_os = "macos")),
    ignore = "plugin helpers are built on Windows and macOS only"
)]
#[test]
fn a_helper_runs_only_while_its_plugin_is_on_and_hears_the_events_it_may_see() {
    let f = fx();
    let _slot = with_fake(&f);
    let w = world(&f, |root| {
        helper_plugin(root, r#"["process","tabs"]"#, wait_a_minute())
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
                1
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

#[cfg_attr(
    not(any(windows, target_os = "macos")),
    ignore = "plugin helpers are built on Windows and macOS only"
)]
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
                1
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
    // Four starts and four ends, each end waited for: on a busy runner that passes the five seconds of `pump_until`.
    let t = std::time::Instant::now();
    while w.hub.helper_stopped("demo").is_none() {
        assert!(
            t.elapsed() < std::time::Duration::from_secs(30),
            "timed out waiting for the helper to be stopped"
        );
        f.pump();
        std::thread::sleep(std::time::Duration::from_millis(5)); // sync-ok: test code only, never the app
    }
    assert_eq!(
        w.hub.snapshot().mods[0].stopped.as_deref(),
        Some(flare_mods::helper::STOPPED_HELPER)
    );
    assert!(helpers_running(&f).is_empty());
    // Switching it again lets it run again.
    turn(&f, "demo", false);
    assert!(w.hub.helper_stopped("demo").is_none());
}

#[cfg_attr(
    not(any(windows, target_os = "macos")),
    ignore = "plugin helpers are built on Windows and macOS only"
)]
#[test]
fn an_update_of_a_plugin_whose_helper_runs_waits_for_it_to_end_and_starts_it_again() {
    // The helper (and its grandchild) run in the plugin folder, which the update moves aside: the update waits for
    // the stopped helper to end, or the move is refused.
    let f = fx();
    let _slot = with_fake(&f);
    let line = wait_a_minute();
    let w = world(&f, |root| helper_plugin(root, r#"["process"]"#, line));
    recycle_by_deleting(&w);
    f.ctl
        .borrow_mut()
        .set_plugin_helpers(f.dir.path().join("plugins-logs"), "p".into());
    turn(&f, "demo", true);
    assert_eq!(helpers_running(&f), ["demo"]);
    let log = flare_mods::helper::log_file(&f.dir.path().join("plugins-logs"), "demo");
    f.pump_until("the helper runs", |_| log.exists());
    let (name, exe) = helper_program();
    let mut cur = std::io::Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut cur);
        let o = zip::write::SimpleFileOptions::default();
        z.start_file("plugin.json", o).unwrap();
        z.write_all(helper_json("1.1.0", r#"["process"]"#, line).as_bytes())
            .unwrap();
        z.start_file("bg.html", o).unwrap();
        z.write_all(b"<p>bg</p>").unwrap();
        // The zip marks the program executable, and the install keeps that bit (rule 290).
        z.start_file(name, o.unix_permissions(0o755)).unwrap();
        z.write_all(&exe).unwrap();
        z.finish().unwrap();
    }
    let zip = f.dir.path().join("b.zip");
    std::fs::write(&zip, cur.into_inner()).unwrap(); // sync-ok: test code only, never the app
    install_file(&f, zip, Some("demo"));
    let (failed, said) = note(&f).unwrap();
    assert!(!failed, "{said}");
    assert_eq!(w.hub.found("demo").unwrap().manifest.version, "1.1.0");
    assert!(w.hub.is_enabled("demo"), "the same grant keeps it on");
    assert_eq!(helpers_running(&f), ["demo"], "its helper runs again");
    // Remove waits the same way before the folder goes.
    mods_msg(&f, ModsMsg::Remove { id: "demo".into() });
    f.pump_until("the removal ends", |f| !installing(f));
    let (failed, said) = note(&f).unwrap();
    assert!(!failed, "{said}");
    assert!(!w.root.join("demo").exists());
    assert!(helpers_running(&f).is_empty());
}

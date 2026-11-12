//! The web panel's CEF host on a Mac, end to end and headless: a dev bundle is built under the target dir around
//! the probe (`flare_web::cef_probe`), the probe drives the real `WebHost` through the `WebBackend` trait in one
//! scenario, and its report is checked. While it runs the window server is asked, every 50 ms, for any on-screen
//! window of the bundle's processes (the app and every helper): there must never be one (the screen rule). Empty
//! off macOS.

#![cfg(target_os = "macos")]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use flare_cef::macos::{CEF_DIR, bundle, screen, sign};
use serde_json::Value;

const NAME: &str = "Flare Web Probe";

fn bundle_for(test: &str) -> PathBuf {
    let out = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("flare-web-cef-dev")
        .join(test);
    let spec = bundle::BundleSpec {
        name: NAME,
        identifier: "dev.flare.web-probe",
        main_exe: Path::new(env!("CARGO_BIN_EXE_flare-web-cef-probe")),
        helper_exe: Path::new(env!("CARGO_BIN_EXE_flare-web-cef-helper")),
        cef_dir: Path::new(CEF_DIR),
        out_dir: &out,
        background_only: true,
        version: env!("CARGO_PKG_VERSION"),
        signing: sign::Signing::for_tests(),
        extras: Default::default(),
    };
    bundle::build(&spec).expect("the dev bundle builds")
}

struct Run {
    report: Value,
    shown: Vec<screen::Shown>,
    status: Option<i32>,
}

fn run_probe(scenario: &str) -> Run {
    run_probe_with(scenario, &[])
}

/// The probe with `args` on its own command line (CEF must not take them).
fn run_probe_with(scenario: &str, args: &[String]) -> Run {
    run_probe_in(scenario, scenario, args, &[])
}

/// The probe of `scenario` from the bundle named `bundle_name`, with `env` set as well.
fn run_probe_in(bundle_name: &str, scenario: &str, args: &[String], env: &[(&str, String)]) -> Run {
    let app = bundle_for(bundle_name);
    let dir = app.parent().expect("the bundle has a folder").to_path_buf();
    let out = dir.join(format!("{scenario}.json"));
    let _ = std::fs::remove_file(&out);
    let before = screen::windows_named(NAME);
    assert!(
        before.is_empty(),
        "no probe window before the run: {before:?}"
    );
    let mut child = Command::new(bundle::exe_in(&app, NAME))
        .env("FLARE_WEB_PROBE_SCENARIO", scenario)
        .env("FLARE_WEB_PROBE_OUT", &out)
        .envs(env.iter().map(|(k, v)| (*k, v.as_str())))
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("the probe starts");
    let mut shown = Vec::new();
    let end = Instant::now() + Duration::from_secs(240);
    let status = loop {
        shown.extend(screen::windows_named(NAME));
        if let Some(s) = child.try_wait().expect("the probe can be waited for") {
            break s.code();
        }
        if Instant::now() > end {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    shown.extend(screen::windows_named(NAME));
    let text = std::fs::read_to_string(&out).unwrap_or_default();
    let report: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    Run {
        report,
        shown,
        status,
    }
}

fn assert_clean(run: &Run, want: &[&str]) {
    let r = &run.report;
    assert!(r.get("fatal").is_none(), "the probe failed: {r:#}");
    assert_eq!(run.status, Some(0), "the probe exits 0: {r:#}");
    assert!(
        run.shown.is_empty(),
        "no window of the bundle ever on screen: {:?}",
        run.shown
    );
    assert_eq!(r["windows_at_start"], 0, "{r:#}");
    assert_eq!(r["windows_at_end"], 0, "{r:#}");
    assert_eq!(r["active_at_end"], false, "{r:#}");
    assert_eq!(
        r["activation_policy"], "NSApplicationActivationPolicy(2)",
        "Prohibited: {r:#}"
    );
    assert_eq!(r["cef_app_protocol"], true, "{r:#}");
    assert_eq!(r["shutdown"], true, "{r:#}");
    let checks = r["checks"].as_array().expect("checks");
    let failed: Vec<&Value> = checks.iter().filter(|c| c["ok"] != true).collect();
    assert!(failed.is_empty(), "failed checks: {failed:#?}");
    let ran: Vec<&str> = checks.iter().filter_map(|c| c["id"].as_str()).collect();
    for id in want {
        assert!(ran.contains(id), "{id} ran: {ran:?}");
    }
}

#[test]
fn the_web_panel_on_cef_navigates_draws_takes_input_and_holds_its_walls_with_no_window() {
    let run = run_probe("panel");
    assert_clean(
        &run,
        &[
            "panel.open",
            "panel.frame",
            "panel.still",
            "panel.history",
            "panel.same-document",
            "panel.external-schemes",
            "panel.new-window-same-view",
            "panel.popup",
            "panel.popup-closes-itself",
            "panel.popup-strip-close",
            "panel.popup-blocked",
            "panel.window-close-ignored",
            "panel.app-link-in-frame",
            "panel.cert",
            "panel.overlay-scrollbars",
            "panel.failed-load",
            "panel.dialogs",
            "panel.file-chooser",
            "panel.context-menu",
            "panel.leave-page",
            "panel.permissions",
            "panel.downloads",
            "panel.download-staging",
            "panel.no-native-pickers",
            "panel.zoom",
            "panel.pointer",
            "panel.keys-and-ime",
            "panel.key-filter",
            "panel.wheel",
            "panel.select-list",
            "panel.focus",
            "panel.cdp",
            "panel.gone",
            "panel.notes",
            "panel.closed",
        ],
    );
}

/// Rule 1152: the window pumps the host only when told (CEF's own work runs in AppKit's run loop), and drops it
/// when the last panel closes; a panel must still load and paint, the first and the one after.
#[test]
fn a_web_panel_turned_as_the_window_turns_loads_and_paints_and_again_after_the_last_closed() {
    let run = run_probe("app-loop");
    assert_clean(
        &run,
        &[
            "app-loop.open",
            "app-loop.navigate",
            "app-loop.nav-id",
            "app-loop.reopen",
            "app-loop.one-engine",
            "app-loop.gpu-note",
            "app-loop.quit-after-idle",
        ],
    );
}

#[test]
fn mini_apps_mods_and_plugin_views_on_cef_keep_their_walls_against_the_network_observers() {
    let run = run_probe("walls");
    assert_clean(
        &run,
        &[
            "walls.observers",
            "walls.control",
            "walls.mini-app-sealed",
            "walls.mini-app-profile",
            "walls.mini-app-stays",
            "walls.mini-app-shared-worker",
            "walls.mini-app-network",
            "walls.netproxy-literals",
            "walls.mod-bridge",
            "walls.mod-files",
            "walls.profiles",
            "walls.plugin-bar",
            "walls.bar-takes-plugins-only",
        ],
    );
}

#[test]
fn the_agent_drives_a_web_panel_on_cef_through_the_backend_as_on_windows_with_no_window() {
    let run = run_probe("agent");
    assert_clean(
        &run,
        &[
            "agent.look",
            "agent.click-ref",
            "agent.click-point",
            "agent.type",
            "agent.press-editing",
            "agent.scroll",
            "agent.frames",
            "agent.frame-sandboxed",
            "agent.wait",
            "agent.navigate-state",
            "agent.eval",
            "agent.download",
            "agent.input-is-not-the-persons",
        ],
    );
}

#[test]
fn the_engine_takes_only_flares_switches_and_calls_no_one_on_its_own() {
    // A free port for the switch that must open nothing.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .expect("a free port");
    let args = [
        format!("--remote-debugging-port={port}"),
        "--no-sandbox".to_string(),
        "--proxy-server=http://127.0.0.1:1".to_string(),
        "--disable-web-security".to_string(),
    ];
    let run = run_probe_with("quiet", &args);
    assert_clean(&run, &["quiet.own-switches-only", "quiet.no-outside-calls"]);
}

/// Rule 1175: the web panel's profile is Flare's own folder on disk. A cookie with an expiry and a page's
/// `localStorage` survive the engine's restart; a session cookie does not (it ends with the browser, as WebView2's).
/// Two runs of the probe on one data folder and one port (storage is per origin, port included).
#[test]
fn a_kept_sign_in_and_stored_data_survive_an_engine_restart_and_a_session_cookie_does_not() {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .expect("a free port");
    let data = Path::new(env!("CARGO_TARGET_TMPDIR")).join("flare-web-persist-data");
    let _ = std::fs::remove_dir_all(&data);
    let env = [
        ("FLARE_WEB_PROBE_DATA", data.to_string_lossy().into_owned()),
        ("FLARE_WEB_PROBE_PORT", port.to_string()),
    ];
    let first = run_probe_in("persist", "persist-write", &[], &env);
    assert_clean(&first, &["persist.write", "persist.own-folder"]);
    let second = run_probe_in("persist", "persist-read", &[], &env);
    let _ = std::fs::remove_dir_all(&data);
    assert_clean(&second, &["persist.read"]);
}

/// Rules 1176 and 1178: the page follows the app's appearance and names a sane browser; every mini-app Flare writes
/// (show_html, show_diagram, show_model on WebGL, show_audio, the markdown Compiled view) loads and paints windowless.
#[test]
fn mini_apps_load_and_paint_and_pages_follow_the_apps_appearance_on_cef() {
    let run = run_probe("content");
    assert_clean(
        &run,
        &[
            "look.color-scheme",
            "look.user-agent",
            "mini.html",
            "mini.diagram",
            "mini.model",
            "mini.audio",
            "mini.markdown",
        ],
    );
}

/// Rules 1177 and 1179: a plugin's fill types into a sign-in form through `run_script` and stops on a page that
/// moved; a plugin's bar and background page keep their seals in the web panel's host.
#[test]
fn a_plugins_fill_types_into_a_form_and_its_bar_and_background_page_stay_sealed_on_cef() {
    let run = run_probe("plugins");
    assert_clean(
        &run,
        &[
            "fill.types",
            "fill.moved",
            "plugin.background",
            "plugin.sealed",
        ],
    );
}

//! The CEF engine on a Mac, end to end and headless: the dev bundle is built under the target dir (framework,
//! five helpers, ad-hoc signature), the probe runs one scenario in it, and its report is checked. While it runs
//! the window server is asked, every 50 ms, for any on-screen window of the bundle's processes (the app and every
//! helper): there must never be one (the screen rule). Empty off macOS.

#![cfg(target_os = "macos")]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use flare_cef::macos::{CEF_DIR, bundle, screen, sign};
use serde_json::Value;

const NAME: &str = "Flare CEF Probe";

/// Builds the bundle for one test (its own folder: nextest runs tests in parallel processes).
fn bundle_for(test: &str) -> PathBuf {
    let out = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("flare-cef-dev")
        .join(test);
    let spec = bundle::BundleSpec {
        name: NAME,
        identifier: "dev.flare.cef-probe",
        main_exe: Path::new(env!("CARGO_BIN_EXE_flare-cef-probe")),
        helper_exe: Path::new(env!("CARGO_BIN_EXE_flare-cef-helper")),
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
    /// On-screen windows of the bundle's processes seen while it ran.
    shown: Vec<screen::Shown>,
    status: Option<i32>,
    /// The last lines the probe (and CEF in it) wrote to stderr: the evidence when it does not exit 0.
    stderr_tail: String,
}

fn run_probe(test: &str, scenario: &str) -> Run {
    let app = bundle_for(test);
    let dir = app.parent().expect("the bundle has a folder").to_path_buf();
    let out = dir.join(format!("{scenario}.json"));
    let _ = std::fs::remove_file(&out);
    let err_path = dir.join(format!("{scenario}.stderr"));
    let err_file = std::fs::File::create(&err_path).expect("the probe's stderr file");
    let before = screen::windows_named(NAME);
    assert!(
        before.is_empty(),
        "no probe window before the run: {before:?}"
    );
    let mut child = Command::new(bundle::exe_in(&app, NAME))
        .env("FLARE_CEF_PROBE_SCENARIO", scenario)
        .env("FLARE_CEF_PROBE_OUT", &out)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(err_file))
        .spawn()
        .expect("the probe starts");
    let mut shown = Vec::new();
    let end = Instant::now() + Duration::from_secs(150);
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
    let err = std::fs::read_to_string(&err_path).unwrap_or_default();
    let lines: Vec<&str> = err.lines().collect();
    let stderr_tail = lines[lines.len().saturating_sub(40)..].join("\n");
    Run {
        report,
        shown,
        status,
        stderr_tail,
    }
}

/// Every check of the report passed; the screen rule held; the run ended cleanly.
fn assert_clean(run: &Run) {
    let r = &run.report;
    assert!(r.get("fatal").is_none(), "the probe failed: {r:#}");
    assert_eq!(
        run.status,
        Some(0),
        "the probe exits 0: {r:#}\nits stderr ends:\n{}",
        run.stderr_tail
    );
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
    assert!(!checks.is_empty(), "{r:#}");
    let failed: Vec<&Value> = checks.iter().filter(|c| c["ok"] != true).collect();
    assert!(failed.is_empty(), "failed checks: {failed:#?}");
}

fn ids(r: &Value) -> Vec<String> {
    r["checks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| c["id"].as_str().map(String::from))
        .collect()
}

#[test]
fn the_dev_bundle_holds_the_framework_and_five_signed_helpers() {
    let app = bundle_for("layout");
    let fw = app
        .join("Contents/Frameworks")
        .join(flare_cef::pin::FRAMEWORK);
    assert!(fw.join("Chromium Embedded Framework").exists());
    assert!(fw.join("Libraries/libcef_sandbox.dylib").exists());
    for suffix in flare_cef::pin::HELPER_SUFFIXES {
        let name = bundle::helper_name(NAME, suffix);
        let exe = app
            .join("Contents/Frameworks")
            .join(format!("{name}.app/Contents/MacOS"))
            .join(&name);
        assert!(exe.exists(), "{}", exe.display());
    }
    bundle::verify(&app).expect("the ad-hoc signature verifies, deep and strict");
    let plist = std::fs::read_to_string(app.join("Contents/Info.plist")).expect("Info.plist");
    assert!(plist.contains("<key>LSBackgroundOnly</key>"));
}

#[test]
fn a_windowless_page_paints_into_a_pixel_buffer_with_no_window_on_screen() {
    let run = run_probe("paint", "paint");
    assert_clean(&run);
    let got = ids(&run.report);
    for id in ["paint.buffer", "paint.pixels", "paint.no-popup-or-window"] {
        assert!(got.iter().any(|g| g == id), "{id} ran: {got:?}");
    }
}

#[test]
fn the_devtools_methods_agent_job_uses_answer_in_chromiums_shapes() {
    let run = run_probe("cdp", "cdp");
    assert_clean(&run);
    let got = ids(&run.report);
    for id in [
        "cdp.look",
        "cdp.click",
        "cdp.type",
        "cdp.press",
        "cdp.scroll",
        "cdp.wait",
        "cdp.eval",
        "cdp.agent-methods",
        "cdp.nothing-native",
    ] {
        assert!(got.iter().any(|g| g == id), "{id} ran: {got:?}");
    }
}

#[test]
fn a_runaway_eval_is_stopped_by_its_timeout_and_the_page_answers_again_with_its_state() {
    let run = run_probe("runaway", "runaway");
    assert_clean(&run);
    let got = ids(&run.report);
    for id in [
        "runaway.while-true",
        "runaway.page-function",
        "runaway.same-document",
        "runaway.trap-look",
        "runaway.trap-eval",
        "runaway.nothing-native",
    ] {
        assert!(got.iter().any(|g| g == id), "{id} ran: {got:?}");
    }
}

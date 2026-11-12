//! M9 Task 27: PRIVACY.md against the network inventory scan.
//!
//! The scan reads the source text of every app crate and of the e2e tool's driver code, and lists each
//! line that can reach the network. Each file with such a line must appear in `INVENTORY` with the
//! PRIVACY.md keyword that says what the call is for, and each keyword must appear in PRIVACY.md. A new
//! request therefore fails this test until PRIVACY.md names it (regression rule 113).

use std::path::{Path, PathBuf};

/// (file suffix, PRIVACY.md keyword). A suffix matches a path ending in it, with forward slashes.
/// Only the lines the scan really finds are listed; a hit is never silenced by deleting its row.
pub const INVENTORY: &[(&str, &str)] = &[
    // The update check and the install path. Off until the repository is public (docs/updater-flip.md).
    ("flare-update/src/velopack_feed.rs", "update check"),
    // Sonara: the question and the tab id go to 127.0.0.1 only.
    ("flare-agent/src/sonara.rs", "Sonara"),
    // The web-address test (a string compare, no request) and the About link, opened only on click.
    ("flare-term/src/web_url.rs", "Links"),
    ("flare-win/src/names.rs", "Links"),
    // A placeholder log link in the headless scene text (printed, never opened).
    ("flare/src/headless.rs", "Links"),
    ("flare/src/appctl.rs", "Links"),
    // The e2e scenarios: address literals typed into a terminal, shown in a fixture row or offered by
    // the fake update feed. The harness never requests them (its only server is the loopback stand-in).
    ("e2e/src/scenarios/input_b.rs", "Links"),
    ("e2e/src/scenarios/input_d.rs", "Links"),
    ("e2e/src/scenarios/update.rs", "update check"),
    // Dictation: the catalogue holds the fixed download addresses (whisper.cpp releases, Hugging Face), the
    // fetcher is the one client that requests them, and only after the user chose a model; the engine's
    // client talks to 127.0.0.1 only. The controller picks the real or a stalled fetcher.
    ("flare-dictation/src/catalog.rs", "Dictation"),
    ("flare-dictation/src/fetch_ureq.rs", "Dictation"),
    ("flare-dictation/src/engine/http.rs", "Dictation"),
    ("flare/src/dictation_ctl.rs", "Dictation"),
    // The e2e's explicit fetch step names the sample recording it may download for the dictation scenarios.
    ("e2e/src/fixtures.rs", "Dictation"),
    // The web panel: the address rules (a bare host gets https, a local one http: strings, no request), the
    // Runtime's download page (opened only on a click) and the e2e's local test host and refusal cases.
    ("flare-surfaces/src/web.rs", "Web panel"),
    // The live preview: the local-address rule compares printed text with the `http://` and `https://` prefixes (no
    // request is made), and the e2e names the loopback address of a stand-in server it runs itself.
    ("flare-term/src/dev_server.rs", "Web panel"),
    ("e2e/src/scenarios/live_preview.rs", "Web panel"),
    // The logins scenario's own site on 127.0.0.1 (a thread of the harness), never the network.
    ("e2e/src/scenarios/web_logins.rs", "Web panel"),
    ("e2e/src/scenarios/bot_checks.rs", "Web panel"),
    // Mini-apps: the host names a mini-app is served from (a local virtual host, no request), the policy
    // text that keeps a page off the network, and the e2e's loopback addresses a blocked request points at.
    ("flare-surfaces/src/app.rs", "Web panel"),
    // Mods: the host name a mod's page is served from (a local name, answered by Flare itself: no request) and
    // the rules that keep the page off the network.
    ("flare-surfaces/src/modview.rs", "Web panel"),
    // Issue #97: the networked browsers' filtering proxy (on 127.0.0.1) connects only to a public address it looked up
    // for a page's own CONNECT, and the address check builds an `https://` address from the host to parse it.
    ("flare-surfaces/src/netproxy.rs", "Web panel"),
    ("flare-surfaces/src/net.rs", "Web panel"),
    // The mod manifest's homepage check: a string compare on the `https://` prefix, no request (the address is
    // only shown on the mod's card in Settings).
    ("flare-mods/src/manifest.rs", "Mods"),
    ("e2e/src/scenarios/mini_app.rs", "Web panel"),
    // The mods scenario names the address a mod's page is served from (a local name Flare answers itself).
    ("e2e/src/scenarios/mods.rs", "Web panel"),
    // Plugins phase 7: the download of a plugin zip from the https address the person pasted, only on Install or
    // Update, under the phase 6 wall (https, a public host on every hop and every resolved address).
    ("flare-mods/src/fetch.rs", "Installing a plugin"),
    // The install field's placeholder text, `https://` (shown in the empty field; no request).
    ("flare-settings/src/page/mods.rs", "Installing a plugin"),
    // A saved login's bare host read as `https://<host>` to compare sites (no request).
    ("flare-mods/src/fill.rs", "Filling logins"),
    // The fill e2e: its stand-in plugin helper and scenario name test pages (no request of their own).
    ("e2e/src/fake_fill_helper.rs", "Filling logins"),
    ("e2e/src/scenarios/fill.rs", "Filling logins"),
    ("flare/src/web_ctl.rs", "Web panel"),
    ("e2e/src/scenarios/web.rs", "Web panel"),
    // The macOS engine (CEF, port phase 5): the dev bundle's Info.plist names Apple's plist DTD (text, no request),
    // and the engine tests' probe serves its pages from a loopback server of its own.
    ("flare-cef/src/macos/bundle.rs", "Web panel"),
    ("flare-cef/src/macos/probe/server.rs", "Web panel"),
    // The macOS web host (CEF, port phase 5, step 2): a walled profile's proxy is the dead one or Flare's filtering
    // proxy on 127.0.0.1; the host tests' probe drives its pages from a loopback server and the network observers.
    ("flare-web/src/macos/host.rs", "Web panel"),
    ("flare-web/src/walls.rs", "Web panel"),
    ("flare-web/src/macos/probe/mod.rs", "Web panel"),
    ("flare-web/src/macos/probe/observers.rs", "Web panel"),
    ("flare-web/src/macos/probe/pages.rs", "Web panel"),
    // The host tests' parity and certificate checks (#192, #193): the probe's own loopback servers, one plain and one
    // HTTPS with a certificate made for the run. Test-only, run by `tests/cef_macos.rs` in the dev bundle; the app
    // never calls the probe, so nothing here is reached from Flare itself.
    ("flare-web/src/macos/probe/parity.rs", "Web panel"),
    ("flare-web/src/macos/probe/tls.rs", "Web panel"),
    // The plugin fill API and brand (2026-10-28, 2026-11-08): string work on addresses, no request. The fill's site
    // check builds an `https://` address from a login's host to compare it with the page's; the details sheet and a
    // plugin's question show a homepage's or an install address's host.
    ("flare-mods/src/fill.rs", "Mods"),
    ("flare-mods/src/grants.rs", "Mods"),
    ("flare/src/plugin_ask.rs", "Mods"),
    // The fill and brand scenarios: a test login's addresses and a test plugin's homepage, never requested (the page
    // is served by the loopback stand-in).
    ("e2e/src/fake_fill_helper.rs", "Web panel"),
    ("e2e/src/scenarios/brand.rs", "Web panel"),
    ("e2e/src/scenarios/fill.rs", "Web panel"),
];

/// Tokens that mark a line as possibly reaching the network.
const TOKENS: &[&str] = &[
    "ureq::",
    "reqwest",
    "hyper::",
    "TcpStream::connect",
    "UdpSocket",
    "GithubSource",
    "HttpSource",
    "AutoSource",
    "download_updates",
    "check_for_updates",
    "\"http://",
    "\"https://",
];

/// Directories whose files are test code only (`tests` is the integration-test folder).
const TEST_DIRS: &[&str] = &["tests", "ui_tests", "ui_harness", "theme_tests"];

fn is_network_line(line: &str) -> bool {
    let t = line.trim_start();
    if t.starts_with("//") || line.contains("starts_with(\"https") {
        return false;
    }
    TOKENS.iter().any(|k| line.contains(k))
}

/// Feeds one line of a skipped item to its brace count; true when the item has ended on this line.
fn item_ends(line: &str, depth: &mut i32, opened: &mut bool) -> bool {
    for c in line.chars() {
        match c {
            '{' => {
                *depth += 1;
                *opened = true;
            }
            '}' => *depth -= 1,
            _ => {}
        }
    }
    (*opened && *depth <= 0) || (!*opened && line.trim_end().ends_with(';'))
}

/// The network-capable lines of one file's text, as (1-based line, trimmed line). An item marked
/// `#[cfg(test)]` (a test module, a test fn, a test-only use) is skipped through its closing brace, so
/// code after it is still scanned.
fn hits_in(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut pending = false; // saw #[cfg(test)]; the item has not started yet
    let mut skipping = false; // inside a skipped item
    let (mut depth, mut opened) = (0, false);
    for (i, line) in text.lines().enumerate() {
        let t = line.trim();
        if skipping {
            skipping = !item_ends(line, &mut depth, &mut opened);
        } else if t == "#[cfg(test)]" {
            pending = true;
        } else if pending && (t.is_empty() || t.starts_with("#[")) {
            continue;
        } else if pending {
            pending = false;
            (depth, opened) = (0, false);
            skipping = !item_ends(line, &mut depth, &mut opened);
        } else if is_network_line(line) {
            out.push((i + 1, t.to_string()));
        }
    }
    out
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if p.is_dir() {
            if !TEST_DIRS.contains(&name.as_str()) {
                rs_files(&p, out);
            }
        } else if name.ends_with(".rs") {
            out.push(p);
        }
    }
}

/// Every network-capable line under `root`: `crates/*/src/**/*.rs` and `tools/e2e/**/*.rs`.
/// Returns (path relative to `root` with forward slashes, line, text).
fn scan(root: &Path) -> Vec<(String, usize, String)> {
    let mut files = Vec::new();
    if let Ok(rd) = std::fs::read_dir(root.join("crates")) {
        for entry in rd.flatten() {
            rs_files(&entry.path().join("src"), &mut files);
        }
    }
    rs_files(&root.join("tools").join("e2e"), &mut files);
    files.sort();
    let mut hits = Vec::new();
    for f in files {
        let Ok(text) = std::fs::read_to_string(&f) else {
            continue;
        };
        let rel = f
            .strip_prefix(root)
            .unwrap_or(&f)
            .to_string_lossy()
            .replace('\\', "/");
        for (line, text) in hits_in(&text) {
            hits.push((rel.clone(), line, text));
        }
    }
    hits
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn privacy_text() -> String {
    std::fs::read_to_string(workspace_root().join("PRIVACY.md")).expect("PRIVACY.md is at the root")
}

fn inventory_row_for(file: &str) -> Option<&'static str> {
    INVENTORY
        .iter()
        .find(|(suffix, _)| file.ends_with(suffix))
        .map(|(_, keyword)| *keyword)
}

#[test]
fn privacy_has_no_em_dash_and_names_flare_only() {
    let text = privacy_text();
    assert!(!text.contains('\u{2014}'), "PRIVACY.md has an em-dash");
    assert!(text.contains("Flare"), "PRIVACY.md names Flare");
    assert!(
        !text.to_lowercase().contains("prism"),
        "PRIVACY.md is Flare's own statement and names no other app"
    );
}

#[test]
fn every_network_capable_file_is_inventoried() {
    let hits = scan(&workspace_root());
    let unlisted: Vec<String> = hits
        .iter()
        .filter(|(file, _, _)| inventory_row_for(file).is_none())
        .map(|(file, line, text)| format!("{file}:{line}: {text}"))
        .collect();
    assert!(
        unlisted.is_empty(),
        "network-capable lines in files missing from INVENTORY (add the row and the PRIVACY.md keyword):\n{}",
        unlisted.join("\n")
    );
    for (suffix, _) in INVENTORY {
        assert!(
            hits.iter().any(|(file, _, _)| file.ends_with(suffix)),
            "INVENTORY row {suffix} names no network-capable line any more; remove the row"
        );
    }
}

#[test]
fn every_inventory_keyword_is_in_privacy_md() {
    let text = privacy_text().to_lowercase();
    for (suffix, keyword) in INVENTORY {
        assert!(
            text.contains(&keyword.to_lowercase()),
            "{suffix} lists the keyword {keyword}, which PRIVACY.md does not mention"
        );
    }
}

#[test]
fn a_new_file_with_a_network_call_fails_the_scan() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let src = dir.path().join("crates").join("demo").join("src");
    std::fs::create_dir_all(&src).expect("the src folder");
    std::fs::write(src.join("lib.rs"), "fn f() { ureq::get(\"x\"); }\n").expect("the file");
    let hits = scan(dir.path());
    assert_eq!(hits.len(), 1, "the scan reports the call: {hits:?}");
    assert_eq!(hits[0].0, "crates/demo/src/lib.rs");
    assert_eq!(inventory_row_for(&hits[0].0), None, "and no row lists it");
}

#[test]
fn the_scan_ignores_comments_test_items_and_string_compares() {
    let text = "// ureq::get(\"x\") in a comment\n\
                let ok = s.starts_with(\"https://\");\n\
                #[cfg(test)]\n\
                mod t {\n    fn h() { reqwest::get(\"y\"); }\n}\n\
                #[cfg(test)]\n\
                use ureq::Agent;\n\
                fn after() { let u = \"https://a.io/\"; }\n";
    let hits = hits_in(text);
    assert_eq!(
        hits.len(),
        1,
        "only the code after the test items: {hits:?}"
    );
    assert_eq!(hits[0].0, 9);
}

#[test]
fn the_update_check_sentence_says_off() {
    assert!(
        privacy_text().contains("switched off"),
        "PRIVACY.md says the update check is switched off"
    );
}

#[test]
fn no_telemetry_words() {
    let lock = std::fs::read_to_string(workspace_root().join("Cargo.lock")).expect("Cargo.lock");
    for name in ["sentry", "analytics", "posthog", "segment", "mixpanel"] {
        assert!(
            !lock.contains(&format!("name = \"{name}")),
            "Cargo.lock has a {name} crate"
        );
    }
    for line in privacy_text().lines() {
        let l = line.to_lowercase();
        for word in ["telemetry", "analytics"] {
            if l.contains(word) {
                assert!(
                    l.contains("no ") || l.contains("never") || l.contains("not "),
                    "{word} may appear only inside a negation: {line}"
                );
            }
        }
    }
}

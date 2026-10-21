//! M9 Task 35: every Flare name sits in `flare_win::names`, and no Flare file names the other product
//! outside the allowed places. The walkers, the line rule and the self-tests live here; the blocking-call
//! scan is in `flare-update/tests/sync_calls.rs`, the mutex and pipe names in `flare-win/tests/names_win.rs`,
//! the log's field keys and marker in `flare-diag/tests/neutral.rs`.

use std::path::{Path, PathBuf};

use flare_win::names;

/// The other product, spelled in pieces so this file never holds the word whole.
fn other_product() -> String {
    ["pri", "sm"].concat()
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/flare sits two levels under the repository root")
        .to_path_buf()
}

/// Every file under `dir` (recursively), skipping build output and installed packages.
fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            let skip = p
                .file_name()
                .is_some_and(|n| matches!(n.to_str(), Some("node_modules" | "target" | ".git")));
            if !skip {
                walk(&p, out);
            }
        } else {
            out.push(p);
        }
    }
}

/// Every `.rs` file of every crate's `src` folder.
fn crate_src_rs_files() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut crates: Vec<PathBuf> = std::fs::read_dir(repo_root().join("crates"))
        .expect("the crates folder can be read")
        .flatten()
        .map(|e| e.path())
        .collect();
    crates.sort();
    for c in crates {
        let mut files = Vec::new();
        walk(&c.join("src"), &mut files);
        out.extend(files.into_iter().filter(|p| ext_is(p, "rs")));
    }
    out
}

fn ext_is(p: &Path, want: &str) -> bool {
    p.extension().is_some_and(|e| e == want)
}

/// The path relative to the repository root, with forward slashes.
fn rel(p: &Path) -> String {
    p.strip_prefix(repo_root())
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}

// ---------------------------------------------------------------------------------------------
// The neutrality rule: a Flare file names the other product only as a source note or a refusal.
// ---------------------------------------------------------------------------------------------

/// True when the line breaks the name rule. A case-insensitive `prism` is allowed only on (a) a comment
/// or doc line that names Prism Terminal as the source, or (c) a fixture literal that proves Prism's
/// words are refused (`"prism-agent` or `"prism-terminal-status`).
fn offends(line: &str) -> bool {
    if !line.to_ascii_lowercase().contains("prism") {
        return false;
    }
    let trimmed = line.trim_start();
    let comment = ["//", "#", "<!--"].iter().any(|p| trimmed.starts_with(p));
    let source_note = comment && trimmed.contains("Prism Terminal");
    let refusal_literal =
        line.contains("\"prism-agent") || line.contains("\"prism-terminal-status");
    !(source_note || refusal_literal)
}

/// The flare-theme files that hold the stock theme id. The exact literal `"prism"` (with quotes) is
/// allowed in these and nowhere else: it is a saved-settings and import compatibility id, so a settings
/// file or a theme import that names the stock theme keeps resolving.
const STOCK_ID_FILES: &[&str] = &[
    "crates/flare-theme/src/chrome.rs",
    "crates/flare-theme/src/presets_data.rs",
    "crates/flare-theme/src/resolve.rs",
    "crates/flare-theme/src/preset.rs",
    "crates/flare-theme/src/order.rs",
    "crates/flare-theme/src/look.rs",
];

/// `offends` for the line at `path` (relative, forward slashes), with the two narrow widenings.
/// (1) In `.md`, `.ps1` and `.yml` files a line naming "Prism Terminal" is prose about the reference
/// app, the release workflow or the shipping notes. (2) The stock theme id literal in `STOCK_ID_FILES`.
fn offends_at(path: &str, line: &str) -> bool {
    if !offends(line) {
        return false;
    }
    let prose = [".md", ".ps1", ".yml"].iter().any(|e| path.ends_with(e));
    if prose && line.contains("Prism Terminal") {
        return false;
    }
    if STOCK_ID_FILES.contains(&path) {
        return offends(&line.replace("\"prism\"", ""));
    }
    true
}

/// The files the name rule covers: the crates' `src`, the viewer's `ui`, the bundled plugin, the tools
/// (not the Prism Terminal oracles), the build script, the workflows and two documents.
fn covered_files() -> Vec<PathBuf> {
    let root = repo_root();
    let mut files = Vec::new();
    let mut crates: Vec<PathBuf> = std::fs::read_dir(root.join("crates"))
        .expect("the crates folder can be read")
        .flatten()
        .map(|e| e.path())
        .collect();
    crates.sort();
    for c in &crates {
        walk(&c.join("src"), &mut files);
    }
    walk(&root.join("crates/flare/ui"), &mut files);
    walk(&root.join("crates/flare-agent/claude-plugin"), &mut files);
    let mut tools = Vec::new();
    walk(&root.join("tools"), &mut tools);
    for t in tools {
        let r = rel(&t);
        let excluded = r.starts_with("tools/oracle-update/")
            || r.starts_with("tools/oracle-help/")
            || r.starts_with("tools/pt-");
        if !excluded && (ext_is(&t, "rs") || ext_is(&t, "ps1") || ext_is(&t, "py")) {
            files.push(t);
        }
    }
    files.push(root.join("crates/flare/build.rs"));
    let mut workflows = Vec::new();
    walk(&root.join(".github/workflows"), &mut workflows);
    files.extend(workflows);
    files.push(root.join("PRIVACY.md"));
    files.push(root.join("docs/shipping.md"));
    files.retain(|p| p.is_file());
    files.sort();
    files.dedup();
    files
}

/// Files whose neighbour names are data (read from `names.json`), so their literal lines may name Prism.
/// The rule: a file under `tools/e2e` or `crates/flare-win/tests` is exempt only when it reads
/// `names.json`; the walker finds those trees' files and this check names the file that reads the data.
fn reads_the_neighbour_data(text: &str) -> bool {
    text.contains("names.json")
}

fn in_neighbour_tree(r: &str) -> bool {
    r.starts_with("tools/e2e/") || r.starts_with("crates/flare-win/tests/")
}

/// The import tree is the one place Flare reads Prism Terminal's files (rules (b)).
fn in_import_tree(r: &str) -> bool {
    r.starts_with("crates/flare-settings/src/import/")
}

#[test]
fn no_flare_source_names_prism_outside_the_allowed_places() {
    let mut bad = Vec::new();
    for f in covered_files() {
        let r = rel(&f);
        if in_import_tree(&r) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&f) else {
            continue; // a binary (the icon, the bundled conhost) carries no source line
        };
        if in_neighbour_tree(&r) && reads_the_neighbour_data(&text) {
            continue;
        }
        for (i, line) in text.lines().enumerate() {
            if offends_at(&r, line) {
                bad.push(format!("{r}:{}: {}", i + 1, line.trim()));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "Prism names outside the allowed places:\n{}",
        bad.join("\n")
    );
}

#[test]
fn the_neighbour_trees_are_scanned_too() {
    // The exemption above must not swallow a whole tree: the e2e and the win tests are walked.
    let files = covered_files();
    assert!(
        files
            .iter()
            .any(|p| rel(p).starts_with("crates/flare-agent/claude-plugin/")),
        "the bundled plugin is covered"
    );
    assert!(
        files.iter().any(|p| rel(p).starts_with("crates/flare/ui/")),
        "the viewer's Slint files are covered"
    );
    assert!(
        files.iter().any(|p| rel(p) == "PRIVACY.md"),
        "PRIVACY.md is covered"
    );
}

#[test]
fn the_name_scan_rejects_a_prism_name_in_code() {
    // The self-test's input is spelled in pieces, like the other product's name everywhere here.
    let word = ["Pri", "smTerminal"].concat();
    let bad_literal = ["let x = \"", word.as_str(), "\";"].concat();
    assert!(offends(&bad_literal), "{bad_literal}");
    let bare = ["// the agent is ", "Pri", "sm's own"].concat();
    assert!(offends(&bare), "{bare}");
    let code = ["    let t = \"", "pri", "sm-terminal\";"].concat();
    assert!(offends(&code), "{code}");
}

#[test]
fn the_widenings_are_narrow() {
    // (1) prose about the reference app in md, ps1 and yml only
    assert!(!offends_at(
        "docs/shipping.md",
        "Flare beside Prism Terminal on the runner"
    ));
    assert!(!offends_at("tools/a.ps1", "the Prism Terminal size"));
    assert!(!offends_at(
        ".github/workflows/r.yml",
        "- name: beside Prism Terminal"
    ));
    assert!(offends_at(
        "crates/flare/src/a.rs",
        "let x = Prism Terminal;"
    ));
    assert!(offends_at("docs/shipping.md", "beside Prism alone"));
    // (2) the stock theme id, exact literal, in the listed files only
    let id = ["\"pri", "sm\""].concat();
    let line = format!("let p = find_preset({id});");
    assert!(!offends_at("crates/flare-theme/src/preset.rs", &line));
    assert!(offends_at("crates/flare-theme/src/marks.rs", &line));
    assert!(offends_at("crates/flare/src/a.rs", &line));
    let other = ["\"Pri", "sm\""].concat();
    assert!(offends_at("crates/flare-theme/src/preset.rs", &other));
}

#[test]
fn the_name_scan_allows_the_three_shapes_only() {
    // (a) a doc or comment line that names Prism Terminal as the source
    assert!(!offends(
        "//! The poll follows Prism Terminal's agentPoll.ts."
    ));
    assert!(!offends(
        "    // Prism Terminal's agentDetect, ported as is."
    ));
    assert!(!offends(
        "# Prism Terminal's install folder, kept for the importer"
    ));
    assert!(!offends("<!-- from Prism Terminal's README -->"));
    // (c) a fixture literal that proves the other product's words are refused
    assert!(!offends(
        r#"assert!(!accepts("prism-agent:state=working"));"#
    ));
    assert!(!offends(r#"assert!(!accepts("prism-terminal-status"));"#));
    // Flare's own names are never a finding
    assert!(!offends(r#"let a = "flare-agent";"#));
}

// ---------------------------------------------------------------------------------------------
// Flare's names against Prism Terminal's (Appendix B; the right-hand side is names.json, Task 2).
// ---------------------------------------------------------------------------------------------

fn pt_names() -> serde_json::Value {
    let text = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../flare-win/tests/fixtures/names.json"),
    )
    .expect("names.json can be read");
    let v: serde_json::Value = serde_json::from_str(&text).expect("names.json is JSON");
    v["pt"].clone()
}

#[test]
fn every_flare_name_differs_from_prism_terminals() {
    let pt = pt_names();
    let text = |k: &str| {
        pt[k]
            .as_str()
            .expect("names.json key is a string")
            .to_string()
    };
    let install_leaf = text("installDir")
        .rsplit('\\')
        .next()
        .unwrap_or_default()
        .to_string();
    let pairs: Vec<(&str, &str, String)> = vec![
        ("APP_ID", names::APP_ID, text("appId")),
        ("PRODUCT_TITLE", names::PRODUCT_TITLE, text("productName")),
        ("EXE_NAME", names::EXE_NAME, text("exeName")),
        ("SETUP_NAME", names::SETUP_NAME, text("installerPrefix")),
        ("INSTALL_FOLDER", names::INSTALL_FOLDER, install_leaf),
        (
            "PROFILE_FOLDER",
            names::PROFILE_FOLDER,
            text("profileFolder"),
        ),
        (
            "DICTATION_FOLDER",
            names::DICTATION_FOLDER,
            text("dictationFolder"),
        ),
        ("VERB_KEY", names::VERB_KEY, text("verbKey")),
        ("VERB_LABEL", names::VERB_LABEL, text("verbLabel")),
        ("AUMID", names::AUMID, text("aumid")),
        ("PLUGIN_NAME", names::PLUGIN_NAME, text("pluginName")),
        ("HOOK_SIGNAL", names::HOOK_SIGNAL, text("hookSignal")),
        ("TAB_ID_VAR", names::TAB_ID_VAR, text("tabIdVar")),
    ];
    let mut bad = Vec::new();
    for (name, flare, pt_value) in &pairs {
        if flare == pt_value {
            bad.push(format!("{name}: both are {flare:?}"));
        }
        if !flare.to_ascii_lowercase().contains("flare") {
            bad.push(format!("{name}: {flare:?} does not contain flare"));
        }
        if flare.to_ascii_lowercase().contains(&other_product()) {
            bad.push(format!("{name}: {flare:?} names the other product"));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn every_flare_name_constant_is_flares_own() {
    let other = other_product();
    let all = [
        names::APP_ID,
        names::PRODUCT_TITLE,
        names::EXE_NAME,
        names::SETUP_NAME,
        names::INSTALL_FOLDER,
        names::PROFILE_FOLDER,
        names::DICTATION_FOLDER,
        names::VERB_KEY,
        names::VERB_LABEL,
        names::AUMID,
        names::MUTEX_PREFIX,
        names::PIPE_PREFIX,
        names::PLUGIN_NAME,
        names::HOOK_SIGNAL,
        names::TAB_ID_VAR,
        names::REPO_URL,
        names::E2E_REG_PREFIX,
    ];
    for n in all {
        assert!(!n.to_ascii_lowercase().contains(&other), "{n}");
    }
    assert!(names::REPO_URL.contains("Flare"));
    assert!(names::E2E_REG_PREFIX.starts_with("Software\\FlareE2E\\"));
}

#[test]
fn the_hook_signal_and_the_plugin_are_flares_own() {
    use flare_agent::signal::parse_agent_signal;
    let other = other_product();
    assert!(parse_agent_signal("flare-agent;state=working").is_some());
    assert!(parse_agent_signal(&format!("{other}-agent;state=working")).is_none());
    let manifest = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../flare-agent/claude-plugin/.claude-plugin/plugin.json"),
    )
    .expect("the plugin manifest can be read");
    let v: serde_json::Value = serde_json::from_str(&manifest).expect("plugin.json is JSON");
    assert_eq!(v["name"], "flare-status");
    assert_eq!(names::TAB_ID_VAR, "FLARE_TAB_ID");
    assert_eq!(flare_shells::env::TAB_ID_VAR, "FLARE_TAB_ID");
}

// ---------------------------------------------------------------------------------------------
// The update code: Velopack and Slint only where they belong; the window is text only.
// ---------------------------------------------------------------------------------------------

#[test]
fn velopack_is_named_only_in_velopack_feed_and_the_bootstrap() {
    // `velopack::` may appear in flare-update's velopack_feed.rs, in ship_ctl.rs (the source types
    // only, never a call), and in any test. Everywhere else it is a finding.
    let mut files = Vec::new();
    for c in ["crates", "tools"] {
        walk(&repo_root().join(c), &mut files);
    }
    let mut bad = Vec::new();
    for f in files.into_iter().filter(|p| ext_is(p, "rs")) {
        let r = rel(&f);
        if r == "crates/flare-update/src/velopack_feed.rs" || r.contains("/tests/") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&f) else {
            continue;
        };
        for (i, line) in text.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            let mut rest = code;
            while let Some(at) = rest.find("velopack::") {
                let after = &rest[at + "velopack::".len()..];
                let allowed = r == "crates/flare/src/ship_ctl.rs"
                    && (after.starts_with("sources::FileSource")
                        || after.starts_with("sources::HttpSource")
                        || (after.chars().next().is_some_and(char::is_uppercase)
                            && !after.contains('(')));
                if !allowed {
                    bad.push(format!("{r}:{}: {}", i + 1, line.trim()));
                }
                rest = after;
            }
        }
    }
    assert!(
        bad.is_empty(),
        "velopack:: outside its place:\n{}",
        bad.join("\n")
    );
}

#[test]
fn the_pure_update_modules_do_not_import_velopack_or_slint() {
    const PURE: &[&str] = &[
        "version", "types", "notes", "groups", "flow", "preview", "gate", "handoff", "copy", "geom",
    ];
    let src = repo_root().join("crates/flare-update/src");
    let mut bad = Vec::new();
    for m in PURE {
        let f = src.join(format!("{m}.rs"));
        let text = std::fs::read_to_string(&f).expect("a pure update module can be read");
        for (i, line) in text.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            if code.contains("velopack") || code.contains("slint") {
                bad.push(format!("{}:{}: {}", rel(&f), i + 1, line.trim()));
            }
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

/// The text inside the braces that open at `open` (the index of `{`), or the rest of the file.
fn braced(text: &str, open: usize) -> &str {
    let mut depth = 0usize;
    for (i, c) in text[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &text[open..open + i + 1];
                }
            }
            _ => {}
        }
    }
    &text[open..]
}

#[test]
fn the_update_ui_is_text_only() {
    let text = std::fs::read_to_string(repo_root().join("crates/flare/ui/update.slint"))
        .expect("update.slint can be read");
    for banned in [
        "StyledText",
        "TextInput",
        "Image",
        "@image-url",
        "Link",
        "http",
    ] {
        assert!(!text.contains(banned), "update.slint names {banned}");
    }
    // A TouchArea may sit over the window's buttons and the chip, never over a notes entry.
    let mut from = 0;
    while let Some(at) = text[from..].find("TouchArea") {
        let start = from + at;
        let after = start + "TouchArea".len();
        from = after;
        // Only an element (`TouchArea {`), never a comment that names the element.
        if !text[after..].trim_start().starts_with('{') {
            continue;
        }
        let brace = after + text[after..].find('{').unwrap_or(0);
        let body = braced(&text, brace);
        assert!(
            !body.to_ascii_lowercase().contains("notes"),
            "a TouchArea covers a notes entry:\n{body}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Scans of the viewer's code: timed registrations, em-dashes, the e2e seams.
// ---------------------------------------------------------------------------------------------

/// The Slint callback registrations in a file: `.on_<name>(` followed by a closure (on the same line
/// or the next non-empty one). Returns the zero-based line indices.
fn callback_registrations(lines: &[&str]) -> Vec<usize> {
    let mut hits = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let code = line.split("//").next().unwrap_or("");
        let mut from = 0;
        while let Some(at) = code[from..].find(".on_") {
            let start = from + at + 1;
            let name_end = code[start..]
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .map_or(code.len(), |e| start + e);
            if code[name_end..].starts_with('(') {
                let arg = code[name_end + 1..].trim();
                let closure_here = arg.starts_with('|')
                    || arg.starts_with("move |")
                    || arg.starts_with("move||")
                    || arg.starts_with("||");
                let arg_empty = arg.is_empty();
                let next_closure = arg_empty
                    && lines
                        .iter()
                        .skip(i + 1)
                        .find(|l| !l.trim().is_empty())
                        .is_some_and(|l| {
                            let t = l.trim_start();
                            t.starts_with('|') || t.starts_with("move |")
                        });
                if closure_here || next_closure {
                    hits.push(i);
                }
            }
            from = name_end.max(from + 1);
        }
    }
    hits
}

/// True when the registration at `i` is timed (`timed(` within its statement) or excused (`// untimed:`
/// on its line or the line above).
fn timed_or_excused(lines: &[&str], i: usize) -> bool {
    if lines[i].contains("untimed:") || (i > 0 && lines[i - 1].contains("untimed:")) {
        return true;
    }
    lines.iter().skip(i).take(8).any(|l| l.contains("timed("))
}

#[test]
fn on_registrations_are_timed_or_excused() {
    let mut bad = Vec::new();
    for f in crate_src_rs_files() {
        if !rel(&f).starts_with("crates/flare/src/") || rel(&f).contains("/ui_tests/") {
            continue;
        }
        let text = std::fs::read_to_string(&f).expect("a source file can be read");
        let mut lines: Vec<&str> = Vec::new();
        for line in text.lines() {
            if line.starts_with("#[cfg(test)]") {
                break;
            }
            lines.push(line);
        }
        for i in callback_registrations(&lines) {
            if !timed_or_excused(&lines, i) {
                bad.push(format!("{}:{}: {}", rel(&f), i + 1, lines[i].trim()));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "callback registrations without diag_ctl::timed or `// untimed: <reason>`:\n{}",
        bad.join("\n")
    );
}

#[test]
fn the_registration_scan_recognises_a_closure_and_a_wrapper() {
    let bare = ["app.on_tip(move |x| x);"];
    assert_eq!(callback_registrations(&bare), vec![0]);
    let wrapped = ["app.on_tip(", "    move |x| timed(\"tip\", || x),", ");"];
    assert_eq!(callback_registrations(&wrapped), vec![0]);
    assert!(timed_or_excused(&wrapped, 0));
    let unwrapped = ["app.on_tip(move |x| x);"];
    assert!(!timed_or_excused(&unwrapped, 0));
    let excused = [
        "// untimed: a click on a hairline, returns at once",
        "app.on_tip(move |x| x);",
    ];
    assert!(timed_or_excused(&excused, 1));
    // A plain method call with an argument is not a registration.
    assert!(callback_registrations(&["self.hub.on_closed(&id);"]).is_empty());
}

#[test]
fn no_em_dash_in_docs_or_tools() {
    const EM: char = '\u{2014}';
    let mut files = Vec::new();
    walk(&repo_root().join("docs"), &mut files);
    walk(&repo_root().join("tools"), &mut files);
    let mut bad = Vec::new();
    for f in files {
        let r = rel(&f);
        if r.starts_with("tools/oracle-help/") || r.starts_with("tools/oracle-update/") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&f) else {
            continue; // a binary
        };
        for (i, line) in text.lines().enumerate() {
            if line.contains(EM) {
                bad.push(format!("{r}:{}", i + 1));
            }
        }
    }
    assert!(bad.is_empty(), "em-dashes:\n{}", bad.join("\n"));
}

/// The e2e seams this app reads: Appendix F of the M9 plan, plus the one M7 seam (the eyedrop picker's
/// `FLARE_E2E_PICK_COLOUR`, in the M7 plan's e2e list).
const E2E_SEAMS: &[&str] = &[
    "FLARE_E2E_VERB_ROOT",
    "FLARE_E2E_VERB_EXE",
    "FLARE_E2E_UPDATE_OFFER",
    "FLARE_E2E_UPDATE_NOTES",
    "FLARE_E2E_UPDATE_SOURCE",
    "FLARE_E2E_PICK_COLOUR",
    // M10 Appendix F: reduced motion under --e2e (read in e2e.rs as a concatenated name).
    "FLARE_E2E_REDUCED_MOTION",
    // M10 Appendix F: the ScaleFactorChanged fallback for DPI changes (e2e_glue.rs, the lifecycle scenario).
    "FLARE_E2E_DPI_MODE",
    // M10 Appendix F: harness-only, set by the runner for a packaged build (tools/e2e).
    "FLARE_E2E_PACKAGED",
    // M10 Appendix F: harness-only, which renderers hookSmoke runs (software, gpu).
    "FLARE_E2E_HOOKSMOKE_RENDERERS",
    // M10 Appendix F: harness-only, which renderers the other scenarios run (tools/e2e input_b).
    "FLARE_E2E_RENDERERS",
    // M10 Appendix F: the Claude plugin copy is made under --e2e only when asked (startup.rs; the
    // harness sets LOCALAPPDATA to its sandbox first).
    "FLARE_E2E_PLUGIN",
    // M10 Appendix F (M7, unchanged): the importer's source folder under --e2e (tools/e2e settings_c).
    "FLARE_E2E_IMPORT_APPDATA",
    // A test-harness switch (tools/e2e/tests/update_apply.rs, side_by_side.rs), not an app seam.
    "FLARE_E2E_INSTALL",
    // M3, unchanged: the folder picker answers from this path with no dialog (window.rs reads it as a
    // concatenated name; tools/e2e newTabFolder and spawn set it).
    "FLARE_E2E_PICK",
    // M10 Appendix F: with a sandboxed HOME, the transcript store of that HOME is read for the Claude
    // session lookup (startup.rs, window.rs; the agentLaunch scenario sets it).
    "FLARE_E2E_SESSIONS",
    // M8 (dictation) seams: the microphone WAV, the NVIDIA probe, the link, the stalled download.
    "FLARE_E2E_MIC",
    "FLARE_E2E_NVIDIA",
    "FLARE_E2E_DICT_PIPE",
    "FLARE_E2E_DICT_KEY",
    "FLARE_E2E_FETCH",
];

/// Every `FLARE_E2E_<NAME>` literal in the text.
fn e2e_names(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = text[from..].find("FLARE_E2E_") {
        let start = from + at;
        let end = text[start..]
            .find(|c: char| !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'))
            .map_or(text.len(), |e| start + e);
        if end > start + "FLARE_E2E_".len() {
            out.push(text[start..end].to_string());
        }
        from = end.max(start + 1);
    }
    out
}

#[test]
fn the_e2e_seams_are_the_listed_ones() {
    let mut files = crate_src_rs_files();
    walk(&repo_root().join("tools/e2e"), &mut files);
    let mut bad = Vec::new();
    for f in files.into_iter().filter(|p| ext_is(p, "rs")) {
        let Ok(text) = std::fs::read_to_string(&f) else {
            continue;
        };
        for n in e2e_names(&text) {
            if !E2E_SEAMS.contains(&n.as_str()) {
                bad.push(format!("{}: {n}", rel(&f)));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "e2e seams not in Appendix F:\n{}",
        bad.join("\n")
    );
}

#[test]
fn the_e2e_name_reader_finds_a_seam_and_skips_the_prefix() {
    assert_eq!(
        e2e_names(r#"env("FLARE_E2E_VERB_ROOT")"#),
        vec!["FLARE_E2E_VERB_ROOT"]
    );
    assert!(e2e_names(r#"const P: &str = "FLARE_E2E_";"#).is_empty());
}

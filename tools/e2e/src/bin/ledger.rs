//! The parity ledger (M10 Task 74): the e2e port, scenario by scenario.
//!
//! For each of Prism Terminal's 74 e2e scenarios it prints the `ok(` count of the original
//! (`run.mjs`, Appendix B of the M10 plan), the `ctx.ok(` count of the Flare port's run function,
//! and the `// DROPPED:` lines that account for any assertion the port does not keep. A port that
//! keeps fewer assertions than the original must list every drop; the ledger's status says so.
//! `docs/test-port.md` holds this table; the unit test `the_doc_holds_the_ledger_table` keeps them equal.

use std::fs;
use std::path::{Path, PathBuf};

/// The 74 scenarios of Prism Terminal's `tools/e2e/run.mjs`, in run order, with their `ok(` counts.
const PT: &[(&str, u32)] = &[
    ("spawn", 8),
    ("cwdLabel", 3),
    ("indicator", 12),
    ("attention", 25),
    ("agentHooks", 43),
    ("agentDiag", 12),
    ("theme", 8),
    ("newTabFolder", 6),
    ("caretHold", 6),
    ("mouseReport", 6),
    ("clickCaret", 3),
    ("selectionEdit", 18),
    ("findKey", 5),
    ("scrollbar", 5),
    ("paste", 4),
    ("tabDragFirst", 3),
    ("tabWidth", 6),
    ("tabLooks", 18),
    ("tabSwitch", 9),
    ("indicatorStyles", 8),
    ("verticalTabs", 82),
    ("windowColoursSave", 9),
    ("tooltips", 11),
    ("manyTabs", 13),
    ("backgroundWork", 3),
    ("systemMenu", 1),
    ("agentKeys", 10),
    ("cogToggle", 8),
    ("accentAsk", 7),
    ("titleBar", 9),
    ("settingsPage", 7),
    ("launchSkeleton", 9),
    ("restore", 4),
    ("handoff", 3),
    ("quitManyShells", 3),
    ("lastTab", 7),
    ("closeAsk", 12),
    ("pathLinks", 11),
    ("linkPaintStall", 6),
    ("sonaraQuestion", 10),
    ("termReplies", 17),
    ("termClipboard", 7),
    ("imagePaste", 14),
    ("bell", 3),
    ("hiddenResize", 5),
    ("links", 11),
    ("osc8Links", 11),
    ("reviewKeys", 16),
    ("dropAndMenu", 6),
    ("helpPanel", 89),
    ("options", 19),
    ("diagLog", 26),
    ("settingsLook", 16),
    ("settingsSearch", 15),
    ("settingsKeys", 8),
    ("themeEditorLive", 17),
    ("themeSwitch", 15),
    ("reviewSettings", 19),
    ("colourPicker", 46),
    ("pickedGround", 3),
    ("opacityAlpha", 19),
    ("seeThrough", 22),
    ("themeCards", 7),
    ("edges", 25),
    ("accent", 33),
    ("updateWindow", 78),
    ("updateGuard", 26),
    ("updateNotes", 9),
    ("updateQuiet", 6),
    ("dictation", 21),
    ("dictationPage", 36),
    ("dictationParakeet", 11),
    ("exitClosesTab", 3),
    ("promptLayout", 2),
];

/// Where a scenario's port lives, as far as this ledger can tell.
#[derive(Debug, Clone, PartialEq)]
pub enum Port {
    /// In `scenarios::ALL`: its group file and run function.
    Flare { group: String, func: String },
    /// Ported by M9 out of process (`updateQuiet`), not in `ALL`; not counted here.
    Elsewhere(&'static str),
    /// Waits for M8 (dictation); not ported.
    Waits(&'static str),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub name: String,
    pub pt_ok: u32,
    pub port: Port,
    pub flare_ok: usize,
    pub dropped: usize,
    pub status: &'static str,
    /// Why a port is judged by hand (`HAND_CHECKED`), empty otherwise.
    pub note: &'static str,
}

fn scenarios_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("scenarios")
}

/// The text after `key` up to the first `end` character.
fn field(chunk: &str, key: &str, end: &[char]) -> Option<String> {
    let start = chunk.find(key)? + key.len();
    let rest = &chunk[start..];
    let stop = rest.find(|c: char| end.contains(&c))?;
    Some(rest[..stop].to_string())
}

/// `(name, group, run function)` for every entry of `scenarios::ALL`, read from `mod.rs`.
pub fn all_table(mod_src: &str) -> Vec<(String, String, String)> {
    mod_src
        .split("Scenario {")
        .skip(1)
        .filter_map(|chunk| {
            let name = field(chunk, "name: \"", &['"'])?;
            let group = field(chunk, "group: \"", &['"'])?;
            let run = field(chunk, "run: ", &[',', '\n'])?;
            let func = run.rsplit("::").next()?.trim().to_string();
            Some((name, group, func))
        })
        .collect()
}

/// The body of `fn <func>(` in a scenario file: from its first `{` to the matching `}`.
#[cfg(test)]
fn fn_body<'a>(src: &'a str, func: &str) -> Option<&'a str> {
    let at = src.find(&format!("fn {func}("))?;
    braced_from(src, at)
}

/// Every `fn <name>(` of the scenario sources, with its body: the index a scenario's calls are followed in.
pub fn fn_index(sources: &[String]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for src in sources {
        let mut from = 0;
        while let Some(at) = src[from..].find("fn ") {
            let start = from + at + 3;
            from = start;
            let name: String = src[start..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            let after = start + name.len();
            if name.is_empty() || !src[after..].starts_with('(') {
                continue;
            }
            if let Some(body) = braced_from(src, after) {
                out.push((name, body.to_string()));
            }
        }
    }
    out
}

/// The braced text that starts at the first `{` at or after `from`.
fn braced_from(src: &str, from: usize) -> Option<&str> {
    let open = from + src[from..].find('{')?;
    let mut depth = 0usize;
    for (i, c) in src[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&src[open..open + i + 1]);
                }
            }
            _ => {}
        }
    }
    None
}

/// The identifiers a body calls (directly followed by `(`) or passes on as a function value (between
/// `(` or `,` on one side and `,` or `)` on the other, as `with_app(ctx, opts, find_key_run)` does).
fn calls(body: &str) -> Vec<String> {
    let bytes = body.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let next = bytes[i..]
                .iter()
                .find(|b| !b.is_ascii_whitespace())
                .copied();
            let prev = bytes[..start]
                .iter()
                .rev()
                .find(|b| !b.is_ascii_whitespace())
                .copied();
            let called = next == Some(b'(');
            let passed = matches!(prev, Some(b'(' | b',')) && matches!(next, Some(b',' | b')'));
            if called || passed {
                out.push(body[start..i].to_string());
            }
        } else {
            i += 1;
        }
    }
    out
}

/// The bodies a scenario reaches: its run function, then the functions it calls or passes on that
/// are defined in its own file or in `driver.rs` (the shared helpers), and so on. Each source file is
/// given with its name; a name is looked up in the caller's file first, then in `driver.rs`.
pub fn reachable<'a>(
    files: &'a [(String, Vec<(String, String)>)],
    file: usize,
    run: &str,
) -> Vec<&'a str> {
    let find = |fi: usize, name: &str| -> Option<(usize, &'a str)> {
        files[fi]
            .1
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, b)| (fi, b.as_str()))
    };
    let driver = files.iter().position(|(n, _)| n == "driver");
    let mut seen: Vec<&'a str> = Vec::new();
    let mut todo: Vec<(usize, &'a str)> = find(file, run).into_iter().collect();
    while let Some((fi, body)) = todo.pop() {
        if seen.contains(&body) {
            continue;
        }
        seen.push(body);
        for callee in calls(body) {
            let hit = find(fi, &callee).or_else(|| driver.and_then(|d| find(d, &callee)));
            if let Some(next) = hit {
                todo.push(next);
            }
        }
    }
    seen
}

/// Each `.rs` file of the scenario folder: its name without the extension, and its functions.
pub fn scenario_files(dir: &Path) -> Vec<(String, Vec<(String, String)>)> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(String, Vec<(String, String)>)> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "rs"))
        .filter_map(|p| {
            let stem = p.file_stem()?.to_string_lossy().into_owned();
            let src = fs::read_to_string(&p).ok()?;
            Some((stem, fn_index(&[src])))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Counts the rows of the ledger from the scenario sources of `dir`.
pub fn ledger(dir: &Path) -> Vec<Row> {
    let mod_src = fs::read_to_string(dir.join("mod.rs")).unwrap_or_default();
    let table = all_table(&mod_src);
    let files = scenario_files(dir);
    PT.iter()
        .map(|&(name, pt_ok)| {
            if name == "updateQuiet" {
                return row(
                    name,
                    pt_ok,
                    Port::Elsewhere("M9 out of process (Task 39)"),
                    0,
                    0,
                );
            }
            if matches!(name, "dictation" | "dictationPage" | "dictationParakeet") {
                return row(name, pt_ok, Port::Waits("M8"), 0, 0);
            }
            let Some((_, group, func)) = table.iter().find(|(n, _, _)| n == name) else {
                return row(name, pt_ok, Port::Waits("not in ALL"), 0, 0);
            };
            // Assertions live in the run function and in the helpers it calls (its own file, driver.rs).
            let Some(file) = files.iter().position(|(n, _)| n == group) else {
                return row(name, pt_ok, Port::Waits("no file"), 0, 0);
            };
            let bodies = reachable(&files, file, func);
            // `ctx.ok(` and the `check!(` macro (help.rs), which is `ctx.ok(` with the condition first.
            let flare_ok: usize = bodies
                .iter()
                .map(|b| b.matches("ctx.ok(").count() + b.matches("check!(").count())
                .sum();
            // A dropped assertion is a `// DROPPED:` note, or a `ctx.skip(` that records the reason.
            let dropped: usize = bodies
                .iter()
                .map(|b| b.matches("// DROPPED:").count() + b.matches("ctx.skip(").count())
                .sum();
            let port = Port::Flare {
                group: group.clone(),
                func: func.clone(),
            };
            let mut r = row(name, pt_ok, port, flare_ok, dropped);
            if r.status == "SHORT" {
                if let Some((_, why)) = HAND_CHECKED.iter().find(|(n, _)| *n == name) {
                    r.status = "by hand";
                    r.note = why;
                }
            }
            r
        })
        .collect()
}

/// Ports the static count cannot judge: a count of source sites reads a closure or a loop once. Each
/// is checked by reading it, and its reason says what the reading found. The test
/// `the_hand_checked_ports_are_short_for_a_stated_reason` keeps this list honest.
const HAND_CHECKED: &[(&str, &str)] = &[(
    "settingsKeys",
    "the `reach` closure runs three walks (5 checks run, 3 DROPPED notes; 8 of 8 PT checks accounted)",
)];

fn row(name: &str, pt_ok: u32, port: Port, flare_ok: usize, dropped: usize) -> Row {
    let status = match &port {
        Port::Flare { .. } if flare_ok + dropped >= pt_ok as usize && flare_ok > 0 => "ok",
        Port::Flare { .. } if flare_ok == 0 => "MISSING",
        Port::Flare { .. } => "SHORT",
        Port::Elsewhere(_) => "elsewhere",
        Port::Waits(_) => "waits",
    };
    Row {
        name: name.to_string(),
        pt_ok,
        port,
        flare_ok,
        dropped,
        status,
        note: "",
    }
}

/// The markdown table `docs/test-port.md` holds, one line per scenario.
pub fn markdown(rows: &[Row]) -> String {
    let mut out = String::from(
        "| Scenario | `ok(` in PT | Flare file, function | `ctx.ok(` in Flare | DROPPED (notes and skips) | status |\n\
         |---|---|---|---|---|---|\n",
    );
    for r in rows {
        let (place, flare) = match &r.port {
            Port::Flare { group, func } => (
                format!("`scenarios/{group}.rs`, `{func}`"),
                r.flare_ok.to_string(),
            ),
            Port::Elsewhere(where_) => (where_.to_string(), "-".to_string()),
            Port::Waits(why) => (why.to_string(), "-".to_string()),
        };
        out.push_str(&format!(
            "| `{}` | {} | {} | {} | {} | {} |\n",
            r.name, r.pt_ok, place, flare, r.dropped, r.status
        ));
    }
    for r in rows.iter().filter(|r| !r.note.is_empty()) {
        out.push_str(&format!("\n`{}` is judged by hand: {}.\n", r.name, r.note));
    }
    out
}

fn main() {
    let rows = ledger(&scenarios_dir());
    print!("{}", markdown(&rows));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pt_counts_are_the_74_scenarios_and_their_1094_calls() {
        assert_eq!(PT.len(), 74);
        assert_eq!(PT.iter().map(|(_, n)| n).sum::<u32>(), 1094);
    }

    #[test]
    fn the_run_function_is_found_by_its_name_and_its_body_is_braced() {
        let src = "pub fn spawn(ctx: &mut Ctx) -> R {\n    if x { ctx.ok(true, \"a\"); }\n}\nfn other() {}";
        let body = fn_body(src, "spawn").unwrap();
        assert!(body.starts_with('{') && body.ends_with('}'));
        assert_eq!(body.matches("ctx.ok(").count(), 1);
        assert!(!body.contains("other"));
    }

    #[test]
    fn a_scenario_entry_is_read_from_mod_rs() {
        let mod_src = "Scenario {\n        name: \"spawn\",\n        group: \"lifecycle\",\n        run: lifecycle::spawn,\n        limit_secs: 180,\n    },";
        assert_eq!(
            all_table(mod_src),
            vec![("spawn".into(), "lifecycle".into(), "spawn".into())]
        );
    }

    #[test]
    fn every_port_keeps_its_count_or_lists_each_drop() {
        let rows = ledger(&scenarios_dir());
        let bad: Vec<&Row> = rows
            .iter()
            .filter(|r| {
                matches!(r.port, Port::Flare { .. }) && r.status != "ok" && r.status != "by hand"
            })
            .collect();
        assert!(bad.is_empty(), "ports short of their count: {bad:?}");
    }

    #[test]
    fn the_hand_checked_ports_are_short_for_a_stated_reason() {
        let rows = ledger(&scenarios_dir());
        for (name, why) in HAND_CHECKED {
            let r = rows
                .iter()
                .find(|r| r.name == *name)
                .expect("a hand-checked port exists");
            assert_eq!(
                r.status, "by hand",
                "{name} is no longer short; drop it from HAND_CHECKED"
            );
            assert!(!why.is_empty());
        }
    }

    #[test]
    fn the_flare_ports_are_the_70_scenarios_of_all() {
        let rows = ledger(&scenarios_dir());
        let ported = rows
            .iter()
            .filter(|r| matches!(r.port, Port::Flare { .. }))
            .count();
        assert_eq!(ported, 70);
    }

    #[test]
    fn the_asserts_of_a_called_helper_count_for_the_scenario() {
        let src = "pub fn run(ctx: &mut Ctx) -> R { helper(ctx); ctx.ok(true, \"a\"); Ok(()) }\n\
                   fn helper(ctx: &mut Ctx) { ctx.ok(true, \"b\"); ctx.skip(\"why\"); }\n\
                   fn unrelated(ctx: &mut Ctx) { ctx.ok(true, \"c\"); }";
        let files = vec![("one".to_string(), fn_index(&[src.to_string()]))];
        let bodies = reachable(&files, 0, "run");
        let oks: usize = bodies.iter().map(|b| b.matches("ctx.ok(").count()).sum();
        let skips: usize = bodies.iter().map(|b| b.matches("ctx.skip(").count()).sum();
        assert_eq!((oks, skips), (2, 1));
        assert!(!bodies.iter().any(|b| b.contains("\"c\"")));
    }

    #[test]
    fn a_function_passed_on_as_a_value_is_followed() {
        let src = "pub fn run(ctx: &mut Ctx) -> R { with_app(ctx, inner) }\n\
                   fn with_app(ctx: &mut Ctx, f: fn(&mut Ctx) -> R) -> R { f(ctx) }\n\
                   fn inner(ctx: &mut Ctx) -> R { ctx.ok(true, \"x\"); Ok(()) }";
        let files = vec![("one".to_string(), fn_index(&[src.to_string()]))];
        let bodies = reachable(&files, 0, "run");
        let oks: usize = bodies.iter().map(|b| b.matches("ctx.ok(").count()).sum();
        assert_eq!(oks, 1);
    }

    #[test]
    fn the_doc_holds_the_ledger_table() {
        let doc = fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/test-port.md"),
        )
        .expect("docs/test-port.md");
        let want = markdown(&ledger(&scenarios_dir()));
        for line in want.lines() {
            assert!(
                doc.contains(line),
                "docs/test-port.md lacks the ledger line: {line}"
            );
        }
    }
}

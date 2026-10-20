//! The parity ratchet (M10 Task 4): reads `docs/parity.md` and holds its lines to three rules.
//! A pure file reader; it links nothing of the `flare` binary. Open lines may only go down
//! (`docs/parity-ratchet.txt`), every tick in M1 to M8 names a test or a recorded difference,
//! and every difference a tick names has a row with a basis in the "Deliberate differences" table.

use std::fs;
use std::path::PathBuf;

const DIFFERENCES_SECTION: &str = "Deliberate";

struct Line {
    section: String,
    number: usize,
    text: String,
}

fn parity_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/parity.md")
}

fn ratchet_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/parity-ratchet.txt")
}

fn read_parity() -> String {
    fs::read_to_string(parity_path()).expect("docs/parity.md is readable")
}

/// The section key of a `## ` heading: `## M1: Window, chrome` gives `M1`,
/// `## E2E scenarios (73 named ...)` gives `E2E`, `## Deliberate differences` gives `Deliberate`.
fn section_of(heading: &str) -> String {
    heading
        .trim_start_matches("## ")
        .split(|c: char| c.is_whitespace() || c == ':')
        .next()
        .unwrap_or("")
        .to_string()
}

fn lines_with_sections(doc: &str) -> Vec<Line> {
    let mut section = String::new();
    let mut out = Vec::new();
    for (index, raw) in doc.lines().enumerate() {
        if raw.starts_with("## ") {
            section = section_of(raw);
            continue;
        }
        out.push(Line {
            section: section.clone(),
            number: index + 1,
            text: raw.to_string(),
        });
    }
    out
}

fn is_open(text: &str) -> bool {
    text.trim_start().starts_with("- [ ]")
}

fn is_tick(text: &str) -> bool {
    let trimmed = text.trim_start();
    trimmed.starts_with("- [x]") || trimmed.starts_with("- [X]")
}

/// M9's open lines (the update window, the installer, the icon) are M9's own work, so the sweep
/// holds every other section to zero. M8 is no longer exempt.
fn is_m9(section: &str) -> bool {
    section == "M9"
}

fn is_m1_to_m8(section: &str) -> bool {
    matches!(
        section,
        "M1" | "M2" | "M3" | "M4" | "M5" | "M6" | "M7" | "M8"
    )
}

/// Open lines that the sweep must close: every section except M9.
fn open_sweep_lines(lines: &[Line]) -> Vec<&Line> {
    lines
        .iter()
        .filter(|l| is_open(&l.text) && !is_m9(&l.section))
        .collect()
}

/// Every `DIFF-<n>` id a line names, in order.
fn diff_ids(text: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("DIFF-") {
        let after = &rest[at + "DIFF-".len()..];
        let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() {
            ids.push(format!("DIFF-{digits}"));
        }
        rest = &after[digits.len()..];
    }
    ids
}

/// The "Rests on" cell of a row of the register table, keyed by its id cell.
/// Columns: | Id | Prism Terminal does | Flare does | Rests on | State |
fn register_rests_on(lines: &[Line], id: &str) -> Option<String> {
    lines
        .iter()
        .filter(|l| l.section == DIFFERENCES_SECTION && l.text.trim_start().starts_with('|'))
        .find_map(|l| {
            let cells: Vec<&str> = l.text.split('|').collect();
            if cells.len() >= 6 && cells[1].trim() == id {
                Some(cells[4].trim().to_string())
            } else {
                None
            }
        })
}

#[test]
fn the_open_count_never_rises_above_the_ratchet() {
    let doc = read_parity();
    let lines = lines_with_sections(&doc);
    let open = open_sweep_lines(&lines);
    let ratchet: usize = fs::read_to_string(ratchet_path())
        .expect("docs/parity-ratchet.txt is readable")
        .trim()
        .parse()
        .expect("docs/parity-ratchet.txt holds one integer");
    assert!(
        open.len() <= ratchet,
        "open parity lines outside M8 and M9 (not tagged [M8]) are {}, above the ratchet of {}. Lines:\n{}",
        open.len(),
        ratchet,
        open.iter()
            .map(|l| format!("  docs/parity.md:{} {}", l.number, l.text))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn every_tick_in_m1_to_m8_names_a_test_or_a_difference() {
    let doc = read_parity();
    let lines = lines_with_sections(&doc);
    let bad: Vec<String> = lines
        .iter()
        .filter(|l| is_m1_to_m8(&l.section) && is_tick(&l.text))
        .filter(|l| !has_milestone_or_diff_tag(&l.text))
        .map(|l| format!("  docs/parity.md:{} {}", l.number, l.text))
        .collect();
    assert!(
        bad.is_empty(),
        "ticks in M1 to M8 must name a test as (M<n>: ...) or a DIFF-<n>. Lines:\n{}",
        bad.join("\n")
    );
}

/// True when the line carries `(M` followed by a digit, or a `DIFF-` id.
fn has_milestone_or_diff_tag(text: &str) -> bool {
    let milestone = text.match_indices("(M").any(|(at, _)| {
        text[at + 2..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit())
    });
    milestone || text.contains("DIFF-")
}

#[test]
fn every_difference_has_a_register_row_and_a_basis() {
    let doc = read_parity();
    let lines = lines_with_sections(&doc);
    let mut problems = Vec::new();
    for line in lines.iter().filter(|l| is_tick(&l.text)) {
        for id in diff_ids(&line.text) {
            match register_rests_on(&lines, &id) {
                None => problems.push(format!(
                    "  docs/parity.md:{} {} has no row in the Deliberate differences table",
                    line.number, id
                )),
                Some(basis) if basis.is_empty() => problems.push(format!(
                    "  docs/parity.md:{} {} has a row with an empty \"Rests on\" cell",
                    line.number, id
                )),
                Some(_) => {}
            }
        }
    }
    assert!(
        problems.is_empty(),
        "every DIFF-<n> named by a tick needs a register row with a basis. Problems:\n{}",
        problems.join("\n")
    );
}

#[test]
fn m1_to_m8_have_no_open_lines() {
    let doc = read_parity();
    let lines = lines_with_sections(&doc);
    let open: Vec<String> = lines
        .iter()
        .filter(|l| is_m1_to_m8(&l.section) && is_open(&l.text))
        .map(|l| format!("  docs/parity.md:{} {}", l.number, l.text))
        .collect();
    assert!(
        open.is_empty(),
        "M1 to M8 must have no open lines. Open:\n{}",
        open.join("\n")
    );
}

/// Every section but M9 is closed: no open line anywhere the M8 sweep reaches. M9 is excluded
/// until its own lines are ticked (the update, installer and icon lines are not M8's).
#[test]
fn no_line_is_open_anywhere() {
    let doc = read_parity();
    let lines = lines_with_sections(&doc);
    let open: Vec<String> = open_sweep_lines(&lines)
        .iter()
        .map(|l| format!("  docs/parity.md:{} {}", l.number, l.text))
        .collect();
    assert!(
        open.is_empty(),
        "no parity line outside M9 may stay open. Open:\n{}",
        open.join("\n")
    );
}

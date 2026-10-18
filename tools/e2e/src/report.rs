//! The printed report: the scenario table. The stall report and the clipboard line are Task 14.

/// One scenario's line in the table.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub name: String,
    pub checks: u32,
    pub fails: u32,
    pub skips: u32,
    pub secs: f64,
}

/// The table: a header, then one line per scenario that ran (skips are not in the table; the
/// runner prints them apart).
pub fn table(rows: &[Row]) -> String {
    let width = rows
        .iter()
        .map(|r| r.name.chars().count())
        .max()
        .unwrap_or(0)
        .max("scenario".len());
    let mut out = format!(
        "{:<width$}  {:>6}  {:>5}  {:>6}\n",
        "scenario", "checks", "fails", "secs"
    );
    for r in rows {
        out.push_str(&format!(
            "{:<width$}  {:>6}  {:>5}  {:>6.1}\n",
            r.name, r.checks, r.fails, r.secs
        ));
    }
    out
}

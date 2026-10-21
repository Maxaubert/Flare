//! A small line diff for one edit (old text to new text), as hunks for the Changes panel.
//!
//! Plain longest-common-subsequence over lines, with no dependency. Output is capped at `MAX_DIFF_LINES` lines in all
//! (the rest is `truncated`). A pair too large for the table (more than `MAX_CELLS` cells) is shown as all removed
//! then all added, which is still correct, just without shared context.

/// Lines of unchanged text kept on each side of a change.
pub const CONTEXT: usize = 3;
/// The most lines a diff returns, across all hunks.
pub const MAX_DIFF_LINES: usize = 2000;
/// The largest LCS table built (cells of `u32`, about 16 MB at this size).
const MAX_CELLS: usize = 4_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Context,
    Removed,
    Added,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: Kind,
    pub text: String,
}

/// One run of changes with its context. `old_start` and `new_start` are 1-based line numbers of its first line.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Hunk {
    pub old_start: usize,
    pub new_start: usize,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Diff {
    pub hunks: Vec<Hunk>,
    /// Lines were left out because the diff passed `MAX_DIFF_LINES`.
    pub truncated: bool,
}

/// One step of the walk: what happened, and the line index on each side it touched.
#[derive(Clone, Copy)]
struct Op {
    kind: Kind,
    /// Index into the old lines (for Context and Removed).
    old: usize,
    /// Index into the new lines (for Context and Added).
    new: usize,
}

/// Diffs `old` against `new` line by line.
pub fn diff(old: &str, new: &str) -> Diff {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let ops = walk(&a, &b);
    hunks_of(&ops, &a, &b)
}

fn walk(a: &[&str], b: &[&str]) -> Vec<Op> {
    let (n, m) = (a.len(), b.len());
    if n.saturating_mul(m) > MAX_CELLS {
        let mut ops: Vec<Op> = (0..n)
            .map(|i| Op {
                kind: Kind::Removed,
                old: i,
                new: 0,
            })
            .collect();
        ops.extend((0..m).map(|j| Op {
            kind: Kind::Added,
            old: 0,
            new: j,
        }));
        return ops;
    }
    // lcs[i * (m + 1) + j] is the LCS length of a[i..] and b[j..].
    let width = m + 1;
    let mut lcs = vec![0u32; (n + 1) * width];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i * width + j] = if a[i] == b[j] {
                lcs[(i + 1) * width + j + 1] + 1
            } else {
                lcs[(i + 1) * width + j].max(lcs[i * width + j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut ops = Vec::with_capacity(n.max(m));
    while i < n || j < m {
        if i < n && j < m && a[i] == b[j] {
            ops.push(Op {
                kind: Kind::Context,
                old: i,
                new: j,
            });
            i += 1;
            j += 1;
        } else if j == m || (i < n && lcs[(i + 1) * width + j] >= lcs[i * width + j + 1]) {
            ops.push(Op {
                kind: Kind::Removed,
                old: i,
                new: 0,
            });
            i += 1;
        } else {
            ops.push(Op {
                kind: Kind::Added,
                old: 0,
                new: j,
            });
            j += 1;
        }
    }
    ops
}

/// Groups the changed steps into hunks with `CONTEXT` lines around each, merging hunks that touch.
fn hunks_of(ops: &[Op], a: &[&str], b: &[&str]) -> Diff {
    let mut out = Diff::default();
    // How many old and new lines lie before each step, so a hunk knows its starting line numbers.
    let mut before_old = Vec::with_capacity(ops.len() + 1);
    let mut before_new = Vec::with_capacity(ops.len() + 1);
    let (mut old_seen, mut new_seen) = (0usize, 0usize);
    for op in ops {
        before_old.push(old_seen);
        before_new.push(new_seen);
        if op.kind != Kind::Added {
            old_seen += 1;
        }
        if op.kind != Kind::Removed {
            new_seen += 1;
        }
    }

    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for (k, op) in ops.iter().enumerate() {
        if op.kind == Kind::Context {
            continue;
        }
        let start = k.saturating_sub(CONTEXT);
        let end = (k + CONTEXT + 1).min(ops.len());
        match ranges.last_mut() {
            Some(last) if last.1 >= start => last.1 = last.1.max(end),
            _ => ranges.push((start, end)),
        }
    }

    let mut budget = MAX_DIFF_LINES;
    for (start, end) in ranges {
        if budget == 0 {
            out.truncated = true;
            break;
        }
        let mut hunk = Hunk {
            old_start: before_old[start] + 1,
            new_start: before_new[start] + 1,
            lines: Vec::new(),
        };
        for op in &ops[start..end] {
            if budget == 0 {
                out.truncated = true;
                break;
            }
            let text = match op.kind {
                Kind::Context | Kind::Removed => a[op.old],
                Kind::Added => b[op.new],
            };
            hunk.lines.push(DiffLine {
                kind: op.kind,
                text: text.to_string(),
            });
            budget -= 1;
        }
        out.hunks.push(hunk);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(d: &Diff) -> Vec<(Kind, String)> {
        d.hunks
            .iter()
            .flat_map(|h| h.lines.iter().map(|l| (l.kind, l.text.clone())))
            .collect()
    }

    #[test]
    fn identical_text_has_no_hunks() {
        assert!(diff("a\nb\n", "a\nb\n").hunks.is_empty());
    }

    #[test]
    fn one_replaced_line_with_context_on_both_sides() {
        let old = "one\ntwo\nthree\nfour\nfive\n";
        let new = "one\ntwo\n3\nfour\nfive\n";
        let d = diff(old, new);
        assert_eq!(d.hunks.len(), 1);
        assert_eq!(d.hunks[0].old_start, 1);
        assert_eq!(
            kinds(&d),
            vec![
                (Kind::Context, "one".into()),
                (Kind::Context, "two".into()),
                (Kind::Removed, "three".into()),
                (Kind::Added, "3".into()),
                (Kind::Context, "four".into()),
                (Kind::Context, "five".into()),
            ]
        );
    }

    #[test]
    fn a_new_file_is_all_added() {
        let d = diff("", "x\ny\n");
        assert_eq!(
            kinds(&d),
            vec![(Kind::Added, "x".into()), (Kind::Added, "y".into())]
        );
        assert_eq!(d.hunks[0].new_start, 1);
    }

    #[test]
    fn far_apart_changes_make_two_hunks() {
        let old: String = (1..=30).map(|i| format!("line {i}\n")).collect();
        let new = old
            .replace("line 2\n", "two\n")
            .replace("line 28\n", "twenty-eight\n");
        let d = diff(&old, &new);
        assert_eq!(d.hunks.len(), 2);
        assert_eq!(d.hunks[1].old_start, 25);
    }

    #[test]
    fn changes_close_together_merge_into_one_hunk() {
        let old = "a\nb\nc\nd\ne\n";
        let new = "A\nb\nC\nd\ne\n";
        assert_eq!(diff(old, new).hunks.len(), 1);
    }

    #[test]
    fn output_is_capped_and_says_so() {
        let old: String = (0..3000).map(|i| format!("old {i}\n")).collect();
        let new: String = (0..3000).map(|i| format!("new {i}\n")).collect();
        let d = diff(&old, &new);
        let total: usize = d.hunks.iter().map(|h| h.lines.len()).sum();
        assert_eq!(total, MAX_DIFF_LINES);
        assert!(d.truncated);
    }

    #[test]
    fn a_pair_too_big_for_the_table_is_all_removed_then_added() {
        let old: String = (0..2500).map(|i| format!("o{i}\n")).collect();
        let new: String = (0..2500).map(|i| format!("n{i}\n")).collect();
        let d = diff(&old, &new);
        let total: usize = d.hunks.iter().map(|h| h.lines.len()).sum();
        assert_eq!(total, MAX_DIFF_LINES);
        assert!(d.truncated);
        assert_eq!(d.hunks[0].lines[0].kind, Kind::Removed);
    }
}

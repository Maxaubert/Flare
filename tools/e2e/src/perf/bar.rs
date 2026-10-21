//! The performance bar and its evaluator (Task 60, D5 and D7).
//!
//! `docs/perf/bar.json` holds the rules, never absolute numbers: "flare <= pt / 3" is read as
//! written, so a new Prism Terminal baseline re-derives the bar. A rule that needs the baseline
//! and has none is `Unknown`, never `Pass`; a number that already breaks its absolute limit is
//! `Fail` even without a baseline.

use serde_json::Value;

/// The rules, embedded so the driver never depends on the working folder.
pub const BAR_JSON: &str = include_str!("../../../../docs/perf/bar.json");

/// What the bar says about one measure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    Fail,
    /// A rule needs the Prism Terminal baseline (or a detail) that is not there.
    Unknown,
}

impl Verdict {
    pub fn word(self) -> &'static str {
        match self {
            Verdict::Pass => "pass",
            Verdict::Fail => "FAIL",
            Verdict::Unknown => "unknown",
        }
    }
}

/// How a Prism Terminal number is scaled in a rule.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Scale {
    One,
    Div(f64),
    Mul(f64),
}

/// A rule of the form `<= pt`, `<= pt / 3`, `>= pt` or `>= pt * 2`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PtRule {
    at_most: bool,
    scale: Scale,
}

/// Reads a rule string.
pub fn parse_rule(text: &str) -> Result<PtRule, String> {
    let t = text.trim();
    let (at_most, rest) = if let Some(r) = t.strip_prefix("<=") {
        (true, r)
    } else if let Some(r) = t.strip_prefix(">=") {
        (false, r)
    } else {
        return Err(format!("rule {text:?} starts with neither <= nor >="));
    };
    let Some(rest) = rest.trim().strip_prefix("pt") else {
        return Err(format!("rule {text:?} does not name pt"));
    };
    let rest = rest.trim();
    let scale = if rest.is_empty() {
        Scale::One
    } else if let Some(n) = rest.strip_prefix('/') {
        Scale::Div(num(n, text)?)
    } else if let Some(n) = rest.strip_prefix('*') {
        Scale::Mul(num(n, text)?)
    } else {
        return Err(format!("rule {text:?} has {rest:?} after pt"));
    };
    Ok(PtRule { at_most, scale })
}

fn num(s: &str, whole: &str) -> Result<f64, String> {
    s.trim()
        .parse::<f64>()
        .ok()
        .filter(|n| *n > 0.0)
        .ok_or_else(|| format!("rule {whole:?} has no usable number"))
}

impl PtRule {
    /// The limit the rule puts on a Flare number, given the Prism Terminal one.
    pub fn limit(&self, pt: f64) -> f64 {
        match self.scale {
            Scale::One => pt,
            Scale::Div(n) => pt / n,
            Scale::Mul(n) => pt * n,
        }
    }

    /// True when `value` meets the rule. The edge itself passes.
    pub fn holds(&self, value: f64, pt: f64) -> bool {
        let limit = self.limit(pt);
        if self.at_most {
            value <= limit
        } else {
            value >= limit
        }
    }
}

/// One check: `Some(held)`, or `None` when it cannot be judged.
type Check = Option<bool>;

fn combine(checks: &[Check]) -> Verdict {
    if checks.contains(&Some(false)) {
        Verdict::Fail
    } else if checks.iter().any(Option::is_none) {
        Verdict::Unknown
    } else {
        Verdict::Pass
    }
}

fn at<'a>(v: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter().try_fold(v, |cur, k| cur.get(*k))
}

fn number(v: &Value, path: &[&str]) -> Option<f64> {
    at(v, path).and_then(Value::as_f64)
}

fn pt_check(bar: &Value, path: &[&str], value: f64, pt: Option<f64>) -> Check {
    let rule = at(bar, path)?.as_str().and_then(|s| parse_rule(s).ok())?;
    Some(rule.holds(value, pt?))
}

fn le(value: f64, limit: Option<f64>) -> Check {
    limit.map(|l| value <= l)
}

/// The Prism Terminal number a bar compares with, from a baseline file (`tools/pt-perf`'s shape).
/// `shell` picks the start-up row (`cmd` or `pwsh`); every other bar is read on `cmd`.
pub fn pt_for(id: &str, shell: &str, baseline: &Value) -> Option<f64> {
    match id {
        "B1" => number(baseline, &["start_ms", shell, "median"]),
        "B2" => number(baseline, &["idle_private_ws_mb", "cmd", "total"]),
        "B3" => number(baseline, &["flood", "mb_per_s"]),
        "B4" => number(baseline, &["key_latency_ms", "median"]),
        "B5" => number(baseline, &["scroll", "fps"]),
        "B6" => number(baseline, &["idle_cpu_pct_of_core", "cmd"]),
        _ => None,
    }
}

/// Judges one measure against the bar. `detail` is the measure's detail object. The text is the
/// rule as the table prints it.
pub fn evaluate(
    bar: &Value,
    id: &str,
    renderer: &str,
    value: f64,
    detail: &Value,
    pt: Option<f64>,
) -> (Verdict, String) {
    let gpu = renderer == "gpu";
    let b = bar.get(id).unwrap_or(&Value::Null);
    let (checks, text): (Vec<Check>, String) = match id {
        "B1" => {
            let key = format!("{renderer}_max_ms");
            let cap = number(b, &[&key]);
            (
                vec![le(value, cap), pt_check(b, &["also"], value, pt)],
                format!(
                    "<= {} ms and {}",
                    cap.map_or("?".into(), |c| format!("{c}")),
                    at(b, &["also"]).and_then(Value::as_str).unwrap_or("?")
                ),
            )
        }
        "B2" => (
            vec![pt_check(b, &[renderer], value, pt)],
            at(b, &[renderer])
                .and_then(Value::as_str)
                .unwrap_or("?")
                .to_string(),
        ),
        "B3" => {
            let mut checks = vec![pt_check(b, &[renderer], value, pt)];
            let mut text = at(b, &[renderer])
                .and_then(Value::as_str)
                .unwrap_or("?")
                .to_string();
            if gpu {
                let cap = number(b, &["gpu_max_frame_ms"]);
                checks.push(number(detail, &["frames", "max_ms"]).and_then(|m| le(m, cap)));
                text.push_str(&format!(
                    " and no frame over {} ms",
                    cap.map_or("?".into(), |c| format!("{c}"))
                ));
            }
            (checks, text)
        }
        "B4" => {
            let cap = number(b, &["p95_max_ms"]);
            (
                vec![
                    pt_check(b, &["median"], value, pt),
                    number(detail, &["p95_ms"]).and_then(|p| le(p, cap)),
                ],
                format!(
                    "median {} and p95 <= {} ms",
                    at(b, &["median"]).and_then(Value::as_str).unwrap_or("?"),
                    cap.map_or("?".into(), |c| format!("{c}"))
                ),
            )
        }
        "B5" => {
            let floor = number(b, &["min_fps"]);
            let mut checks = vec![floor.map(|f| value >= f)];
            let mut text = format!(">= {} fps", floor.map_or("?".into(), |c| format!("{c}")));
            if gpu {
                let cap = number(b, &["max_frame_ms"]);
                checks.push(number(detail, &["max_ms"]).and_then(|m| le(m, cap)));
                text.push_str(&format!(
                    " and no frame over {} ms",
                    cap.map_or("?".into(), |c| format!("{c}"))
                ));
            }
            (checks, text)
        }
        "B6" => {
            let cap = number(b, &["max_pct_of_core"]);
            (
                vec![le(value, cap), pt_check(b, &["rule"], value, pt)],
                format!(
                    "<= {}% of a core and {}",
                    cap.map_or("?".into(), |c| format!("{c}")),
                    at(b, &["rule"]).and_then(Value::as_str).unwrap_or("?")
                ),
            )
        }
        "B7" => {
            let p95 = number(b, &["render_p95_max_ms"]);
            let gap = number(b, &["interval_max_ms"]);
            (
                vec![
                    le(value, p95),
                    number(detail, &["max_ms"]).and_then(|m| le(m, gap)),
                ],
                format!(
                    "render p95 <= {} ms and interval <= {} ms",
                    p95.map_or("?".into(), |c| format!("{c}")),
                    gap.map_or("?".into(), |c| format!("{c}"))
                ),
            )
        }
        other => (vec![None], format!("no bar named {other}")),
    };
    (combine(&checks), text)
}

/// The parsed bar file.
pub fn bar() -> Value {
    serde_json::from_str(BAR_JSON).expect("docs/perf/bar.json is valid JSON")
}

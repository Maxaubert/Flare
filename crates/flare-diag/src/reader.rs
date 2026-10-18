//! M9 Task 23: the reader tool (`cargo diag`). Port of Prism Terminal's `tools/diag.mjs`: the newest
//! problems of the diagnostics log, each with the crumbs that came just before it, or the agent
//! indicator's timeline. Its output is held exactly equal to Prism Terminal's by the golden test.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

/// What the command line asked for.
#[derive(Debug, Clone, PartialEq)]
pub struct Opts {
    /// A folder holding a `diag.jsonl` (wins over `profile`).
    pub dir: Option<PathBuf>,
    /// A profile folder; its `logs` folder is read.
    pub profile: Option<PathBuf>,
    /// Only lines from this long ago (`10m`, `2h`, ...).
    pub since: Option<String>,
    /// Every line, not only the problems.
    pub all: bool,
    /// Only these kinds.
    pub kinds: Option<BTreeSet<String>>,
    /// How many problems (default 20).
    pub n: usize,
    /// The agent indicator's timeline.
    pub agent: bool,
    /// One tab's lines (`--agent --tab t3`).
    pub tab: Option<String>,
    /// Print the usage.
    pub help: bool,
    /// Print times in UTC (tests and CI).
    pub utc: bool,
}

impl Default for Opts {
    fn default() -> Self {
        Self {
            dir: None,
            profile: None,
            since: None,
            all: false,
            kinds: None,
            n: 20,
            agent: false,
            tab: None,
            help: false,
            utc: false,
        }
    }
}

/// One line of the log: its time in ms since the Unix epoch, and its fields as written.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub ms: i64,
    pub raw: Map<String, Value>,
}

/// The command line. Unknown flags and a flag with no value are errors.
pub fn parse_args(argv: &[String]) -> Result<Opts, String> {
    fn take<'a>(it: &mut impl Iterator<Item = &'a String>, flag: &str) -> Result<String, String> {
        it.next()
            .cloned()
            .ok_or_else(|| format!("{flag} needs a value"))
    }
    let mut o = Opts::default();
    let mut it = argv.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--dir" => o.dir = Some(PathBuf::from(take(&mut it, a)?)),
            "--profile" => o.profile = Some(PathBuf::from(take(&mut it, a)?)),
            "--since" => o.since = Some(take(&mut it, a)?),
            "--all" => o.all = true,
            "--kinds" => {
                o.kinds = Some(take(&mut it, a)?.split(',').map(str::to_owned).collect());
            }
            "--n" => {
                // Prism Terminal's rule: a count that is not a positive number means 20.
                o.n = take(&mut it, a)?
                    .parse::<usize>()
                    .ok()
                    .filter(|n| *n > 0)
                    .unwrap_or(20);
            }
            "--agent" => o.agent = true,
            "--tab" => o.tab = Some(take(&mut it, a)?),
            "--help" | "-h" => o.help = true,
            "--utc" => o.utc = true,
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(o)
}

/// `10m`, `10`, `1.5h`, `250ms`, `2d` as ms; a bare number is minutes; None for nothing or nonsense.
pub fn duration_ms(s: Option<&str>) -> Option<u64> {
    let s = s?;
    let split = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let digits = |x: &str| !x.is_empty() && x.bytes().all(|b| b.is_ascii_digit());
    let well_formed = match num.split_once('.') {
        None => digits(num),
        Some((whole, frac)) => digits(whole) && digits(frac),
    };
    if !well_formed {
        return None;
    }
    let unit_ms = match unit {
        "ms" => 1.0,
        "s" => 1_000.0,
        "" | "m" => 60_000.0,
        "h" => 3_600_000.0,
        "d" => 86_400_000.0,
        _ => return None,
    };
    let value: f64 = num.parse().ok()?;
    Some((value * unit_ms) as u64)
}

/// The rotated files and the live one, oldest first.
const DIAG_FILES: [&str; 5] = [
    "diag.jsonl.4",
    "diag.jsonl.3",
    "diag.jsonl.2",
    "diag.jsonl.1",
    "diag.jsonl",
];

/// Every line of the live file and its rotations, oldest file first, sorted by time. A blank line, a
/// line that is not an object, and one whose `t` is not a time (a line cut by a crash) are skipped.
pub fn read_lines(dir: &Path) -> Vec<Line> {
    let mut lines = Vec::new();
    for name in DIAG_FILES {
        // sync-ok: the reader is a command-line tool that runs off the app, never on its UI thread
        let Ok(bytes) = std::fs::read(dir.join(name)) else {
            continue;
        };
        let text = String::from_utf8_lossy(&bytes);
        for raw in text.split('\n') {
            if raw.trim().is_empty() {
                continue;
            }
            let Ok(Value::Object(map)) = serde_json::from_str::<Value>(raw) else {
                continue;
            };
            let Some(ms) = map.get("t").and_then(Value::as_str).and_then(parse_iso_ms) else {
                continue;
            };
            lines.push(Line { ms, raw: map });
        }
    }
    // A stable sort: a page's batch arrives late, so time decides the order and the file keeps ties.
    lines.sort_by_key(|l| l.ms);
    lines
}

/// A cursor over an ISO-8601 time.
struct Cursor<'a> {
    s: &'a [u8],
    i: usize,
}

impl Cursor<'_> {
    /// Exactly `n` digits as a number.
    fn digits(&mut self, n: usize) -> Option<i64> {
        let part = self.s.get(self.i..self.i + n)?;
        if !part.iter().all(u8::is_ascii_digit) {
            return None;
        }
        self.i += n;
        std::str::from_utf8(part).ok()?.parse().ok()
    }

    /// Takes the byte if it is next.
    fn eat(&mut self, c: u8) -> bool {
        if self.s.get(self.i) == Some(&c) {
            self.i += 1;
            true
        } else {
            false
        }
    }
}

/// `2026-10-07T09:12:03.412Z` as ms since the epoch; None when it is not a timestamp. A time with no
/// zone is read as UTC (the writer always writes `Z`).
pub fn parse_iso_ms(t: &str) -> Option<i64> {
    let mut c = Cursor {
        s: t.as_bytes(),
        i: 0,
    };
    let year = c.digits(4)?;
    if !c.eat(b'-') {
        return None;
    }
    let month = c.digits(2)?;
    if !c.eat(b'-') {
        return None;
    }
    let day = c.digits(2)?;
    let (mut hour, mut minute, mut second, mut milli) = (0, 0, 0, 0);
    if c.eat(b'T') || c.eat(b' ') {
        hour = c.digits(2)?;
        if !c.eat(b':') {
            return None;
        }
        minute = c.digits(2)?;
        if c.eat(b':') {
            second = c.digits(2)?;
        }
        if c.eat(b'.') {
            // The first three digits are the milliseconds, shorter fractions padded on the right.
            let start = c.i;
            while c.s.get(c.i).is_some_and(u8::is_ascii_digit) {
                c.i += 1;
            }
            let frac = &c.s[start..c.i];
            if frac.is_empty() {
                return None;
            }
            let mut three = [b'0'; 3];
            for (slot, b) in three.iter_mut().zip(frac) {
                *slot = *b;
            }
            milli = std::str::from_utf8(&three).ok()?.parse().ok()?;
        }
    }
    let mut offset_min = 0;
    if c.eat(b'Z') {
        // UTC.
    } else if c.eat(b'+') || c.eat(b'-') {
        let sign = if c.s[c.i - 1] == b'-' { -1 } else { 1 };
        let oh = c.digits(2)?;
        c.eat(b':');
        let om = c.digits(2)?;
        offset_min = sign * (oh * 60 + om);
    }
    if c.i != c.s.len()
        || !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let days = days_from_civil(year, month, day);
    let local_ms = days * 86_400_000 + hour * 3_600_000 + minute * 60_000 + second * 1_000 + milli;
    Some(local_ms - offset_min * 60_000)
}

/// Days since 1970-01-01 of a proleptic Gregorian date (H. Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The date of a day count since 1970-01-01 (year, month, day).
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// The calendar fields of a UTC time.
fn utc_fields(ms: i64) -> [i32; 7] {
    let days = ms.div_euclid(86_400_000);
    let rem = ms.rem_euclid(86_400_000);
    let (y, mo, d) = civil_from_days(days);
    [
        y as i32,
        mo as i32,
        d as i32,
        (rem / 3_600_000) as i32,
        (rem / 60_000 % 60) as i32,
        (rem / 1_000 % 60) as i32,
        (rem % 1_000) as i32,
    ]
}

/// Calendar fields for a time: year, month, day, hour, minute, second, millisecond.
pub trait Tz {
    fn local(&self, ms: i64) -> [i32; 7];
}

/// UTC.
pub struct Utc;

/// The local time zone of this PC (the active one).
pub struct WinLocal;

impl Tz for Utc {
    fn local(&self, ms: i64) -> [i32; 7] {
        utc_fields(ms)
    }
}

/// Milliseconds from 1601-01-01 (the FILETIME epoch) to 1970-01-01.
const FILETIME_EPOCH_MS: i64 = 11_644_473_600_000;

impl Tz for WinLocal {
    fn local(&self, ms: i64) -> [i32; 7] {
        use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
        use windows_sys::Win32::System::Time::{
            FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime,
        };
        let Some(ticks) = ms
            .checked_add(FILETIME_EPOCH_MS)
            .and_then(|x| x.checked_mul(10_000))
            .and_then(|x| u64::try_from(x).ok())
        else {
            return utc_fields(ms);
        };
        let file_time = FILETIME {
            dwLowDateTime: ticks as u32,
            dwHighDateTime: (ticks >> 32) as u32,
        };
        // SAFETY: SYSTEMTIME is plain data; all zeros is a valid value that the call below overwrites.
        let mut utc: SYSTEMTIME = unsafe { std::mem::zeroed() };
        // SAFETY: SYSTEMTIME is plain data; all zeros is a valid value that the call below overwrites.
        let mut local: SYSTEMTIME = unsafe { std::mem::zeroed() };
        // SAFETY: both pointers are to live locals; the FILETIME is only read and the SYSTEMTIME written.
        let utc_ok = unsafe { FileTimeToSystemTime(&file_time, &mut utc) } != 0;
        // SAFETY: a null zone asks for the active time zone; the two SYSTEMTIMEs are live locals.
        let converted = utc_ok
            && unsafe { SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut local) } != 0;
        if !converted {
            return utc_fields(ms);
        }
        [
            i32::from(local.wYear),
            i32::from(local.wMonth),
            i32::from(local.wDay),
            i32::from(local.wHour),
            i32::from(local.wMinute),
            i32::from(local.wSecond),
            i32::from(local.wMilliseconds),
        ]
    }
}

/// The usage, printed by `--help` and after a bad argument.
const USAGE: &str = "\
cargo diag: the newest problems in Flare's diagnostics log (diag.jsonl), each with the crumbs before it.

  cargo diag                        newest 20 problems, the crumbs before each
  cargo diag -- --since 10m         only the last ten minutes (ms, s, m, h, d)
  cargo diag -- --all               every line, not only the problems
  cargo diag -- --kinds ui-stack,main-lag
  cargo diag -- --agent             the agent indicator's timeline (hooks, titles, marks, why)
  cargo diag -- --agent --tab t3    one tab's
  cargo diag -- --dir <folder>      any folder holding a diag.jsonl
  cargo diag -- --profile <folder>  a profile folder (its logs folder is read)
  cargo diag -- --n 50              how many problems (default 20)
  cargo diag -- --utc               print the times in UTC
";

/// Kinds that are problems, besides every `*-slow` kind. Prism Terminal's sixteen, then Flare's.
const PROBLEMS: &[&str] = &[
    "main-lag",
    "fs-slow",
    "mem-high",
    "ipc-slow",
    "ipc-error",
    "page-stall",
    "page-stack",
    "page-error",
    "page-rejection",
    "main-error",
    "main-rejection",
    "gone",
    "unresponsive",
    "logger-error",
    "logger-dropped",
    "mark",
    "frame-slow",
    "ui-stack",
    "work-slow",
    "work-error",
    "panic",
    "render-error",
];
/// The agent indicator's records (not problems, the context for one).
const AGENT: &[&str] = &["agent-hook", "agent-title", "agent-mark", "agent-restore"];
/// The crumbs an agent timeline shows beside the agent records.
const AGENT_CRUMBS: &[&str] = &[
    "tab-open",
    "tab-close",
    "tab-switch",
    "shell-spawn",
    "shell-exit",
];
/// Crumbs shown before a problem.
const CRUMBS_BEFORE: usize = 5;
/// A crumb older than this before a problem says nothing about it.
const CRUMB_WINDOW_MS: i64 = 120_000;

/// A field's value as a JavaScript template string would show it (`undefined` when absent).
fn show(v: Option<&Value>) -> String {
    match v {
        None => "undefined".to_owned(),
        Some(Value::Null) => "null".to_owned(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Number(n)) => {
            if let Some(i) = n.as_i64() {
                i.to_string()
            } else {
                let f = n.as_f64().unwrap_or(f64::NAN);
                if f.fract() == 0.0 && f.abs() < 1e15 {
                    format!("{}", f as i64)
                } else {
                    f.to_string()
                }
            }
        }
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|x| show(Some(x)))
            .collect::<Vec<_>>()
            .join(","),
        Some(Value::Object(_)) => "[object Object]".to_owned(),
    }
}

/// `x ?? fallback`: a field that is absent or null shows the fallback.
fn or(v: Option<&Value>, fallback: &str) -> String {
    match v {
        None | Some(Value::Null) => fallback.to_owned(),
        other => show(other),
    }
}

/// A value's truth as JavaScript sees it.
fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Some(Value::String(s)) => !s.is_empty(),
        Some(_) => true,
    }
}

/// The items of a list field (none when it is not a list).
fn items(v: Option<&Value>) -> &[Value] {
    match v {
        Some(Value::Array(a)) => a,
        _ => &[],
    }
}

/// A key of an object value (`undefined` when the value is not an object or lacks it).
fn sub<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    v.as_object().and_then(|m| m.get(key))
}

/// JSON as `JSON.stringify` writes it (compact, insertion order).
fn json(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_default()
}

/// The lines of a text indented by `n` spaces, blank lines dropped (`diag.mjs`'s `indent`).
fn indent(text: &str, n: usize) -> String {
    text.split('\n')
        .filter(|s| !s.trim().is_empty())
        .map(|s| format!("{}{}", " ".repeat(n), s.trim()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Milliseconds as seconds with one decimal, rounded as JavaScript's `toFixed(1)` rounds (an exact
/// half goes away from zero).
fn fixed1_ms(ms: i64) -> String {
    let neg = ms < 0;
    let a = ms.unsigned_abs();
    let body = if a % 500 == 250 {
        let tenths = a / 100 + 1;
        format!("{}.{}", tenths / 10, tenths % 10)
    } else {
        format!("{:.1}", a as f64 / 1000.0)
    };
    if neg {
        format!("-{body}")
    } else {
        body
    }
}

/// The kind of a line.
fn kind_of(l: &Line) -> String {
    show(l.raw.get("k"))
}

fn is_problem(l: &Line) -> bool {
    let k = kind_of(l);
    PROBLEMS.contains(&k.as_str()) || k.ends_with("-slow")
}

fn is_agent(l: &Line) -> bool {
    AGENT.contains(&kind_of(l).as_str())
}

fn is_context(l: &Line) -> bool {
    matches!(
        kind_of(l).as_str(),
        "crumb" | "agent-mark" | "agent-restore"
    )
}

fn in_agent_timeline(l: &Line) -> bool {
    let k = kind_of(l);
    is_agent(l)
        || matches!(k.as_str(), "session" | "quit" | "mark")
        || (k == "crumb" && AGENT_CRUMBS.contains(&show(l.raw.get("a")).as_str()))
}

/// A time as `YYYY-MM-DD HH:MM:SS.mmm` in the given zone.
fn clock(tz: &dyn Tz, ms: i64) -> String {
    let [y, mo, d, h, mi, s, milli] = tz.local(ms);
    format!("{y}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}.{milli:03}")
}

/// One line, said briefly: the fields that matter for its kind first (`diag.mjs`'s `summary`).
fn summary(raw: &Map<String, Value>) -> String {
    let g = |key: &str| show(raw.get(key));
    let kind = show(raw.get("k"));
    match kind.as_str() {
        "page-stall" => {
            let scripts: Vec<String> = items(raw.get("scripts"))
                .iter()
                .map(|x| {
                    format!(
                        "{} in {} ({}) {}ms",
                        or(sub(x, "fn"), "?"),
                        or(sub(x, "src"), "?"),
                        or(sub(x, "invoker"), "?"),
                        show(sub(x, "ms"))
                    )
                })
                .collect();
            let s = scripts.join("; ");
            let tail = if s.is_empty() {
                String::new()
            } else {
                format!(": {s}")
            };
            format!("{}ms, blocking {}ms{tail}", g("ms"), g("blocking"))
        }
        "frame-slow" => format!("{}ms", g("ms")),
        "main-lag" => {
            let calls: Vec<String> = items(raw.get("inflight"))
                .iter()
                .map(|x| {
                    let ch = sub(x, "ch").or(sub(x, "name"));
                    let ended = if truthy(sub(x, "done")) {
                        " (ended)"
                    } else {
                        ""
                    };
                    format!("{} {}ms{ended}", show(ch), show(sub(x, "ms")))
                })
                .collect();
            let c = calls.join(", ");
            if c.is_empty() {
                format!("{}ms, no IPC call around it", g("ms"))
            } else {
                format!("{}ms, calls: {c}", g("ms"))
            }
        }
        "ipc-slow" => format!(
            "{} {}ms{} args {}",
            g("ch"),
            g("ms"),
            if truthy(raw.get("sync")) {
                " (sync)"
            } else {
                ""
            },
            raw.get("args").map_or_else(|| "undefined".to_owned(), json)
        ),
        "ipc-error" => format!("{}: {}", g("ch"), g("err")),
        "page-stack" => format!(
            "{}ms without a heartbeat{}\n{}",
            g("ms"),
            if truthy(raw.get("unresponsive")) {
                ", unresponsive"
            } else {
                ""
            },
            indent(&g("stack"), 6)
        ),
        "ui-stack" => {
            // The frames when the sampler took them, else the names of the work in flight.
            let mut out = format!("{}ms without a heartbeat", g("ms"));
            if truthy(raw.get("stack")) {
                out.push('\n');
                out.push_str(&indent(&g("stack"), 6));
            } else {
                let names: Vec<String> = items(raw.get("inflight"))
                    .iter()
                    .map(|x| show(sub(x, "name")))
                    .collect();
                if !names.is_empty() {
                    out.push('\n');
                    out.push_str(&indent(&names.join("\n"), 6));
                }
            }
            out
        }
        "work-slow" if raw.contains_key("name") => format!(
            "{} {}ms{}",
            g("name"),
            g("ms"),
            if truthy(raw.get("sync")) {
                " (sync)"
            } else {
                ""
            }
        ),
        "work-error" if raw.contains_key("name") => format!("{}: {}", g("name"), g("msg")),
        "page-error" | "page-rejection" | "main-error" | "main-rejection" | "panic"
        | "render-error" => {
            let loc = if truthy(raw.get("loc")) {
                format!(" at {}", g("loc"))
            } else {
                String::new()
            };
            let more = if truthy(raw.get("repeats")) {
                format!(" (and {} more like it)", g("repeats"))
            } else {
                String::new()
            };
            let stack = if truthy(raw.get("stack")) {
                format!("\n{}", indent(&g("stack"), 6))
            } else {
                String::new()
            };
            format!("{}{loc}{more}{stack}", g("msg"))
        }
        "mark" => {
            if truthy(raw.get("note")) {
                format!("\"{}\"", g("note"))
            } else {
                "(no note)".to_owned()
            }
        }
        "crumb" => {
            let rest: Map<String, Value> = raw
                .iter()
                .filter(|(k, _)| !matches!(k.as_str(), "t" | "up" | "src" | "k" | "a"))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            let body = if rest.is_empty() {
                String::new()
            } else {
                json(&Value::Object(rest))
            };
            format!("{} {body}", g("a")).trim().to_owned()
        }
        "agent-hook" => format!(
            "{} {}{}{}",
            g("id"),
            g("state"),
            if truthy(raw.get("kind")) {
                format!(" ({})", g("kind"))
            } else {
                String::new()
            },
            if truthy(raw.get("repeats")) {
                format!(" x{} more", g("repeats"))
            } else {
                String::new()
            }
        ),
        "agent-title" => format!(
            "{} title {}{}",
            g("id"),
            g("state"),
            if truthy(raw.get("agent")) {
                format!(" ({})", g("agent"))
            } else {
                String::new()
            }
        ),
        "agent-mark" => {
            let to = raw.get("to");
            let held: Vec<String> = items(raw.get("held"))
                .iter()
                .filter(|m| Some(*m) != to)
                .map(|m| show(Some(m)))
                .collect();
            let holds = if held.is_empty() {
                String::new()
            } else {
                format!(" (holds {})", held.join(", "))
            };
            let restored = match raw.get("restored") {
                None => String::new(),
                Some(Value::Number(n)) => match n.as_i64() {
                    Some(ms) => format!(", {}s after restore", fixed1_ms(ms)),
                    None => format!(
                        ", {:.1}s after restore",
                        n.as_f64().unwrap_or(f64::NAN) / 1000.0
                    ),
                },
                Some(Value::Null) => ", 0.0s after restore".to_owned(),
                Some(_) => ", NaNs after restore".to_owned(),
            };
            format!(
                "{} {} -> {}{holds}: {}{restored}",
                g("id"),
                g("from"),
                g("to"),
                g("why")
            )
        }
        "agent-restore" => format!(
            "{} restored with resume in {}",
            g("id"),
            or(raw.get("cwd"), "?")
        ),
        _ => {
            let rest: Map<String, Value> = raw
                .iter()
                .filter(|(k, _)| !matches!(k.as_str(), "t" | "up" | "src" | "k"))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            json(&Value::Object(rest))
        }
    }
}

/// The report, as the same text `diag.mjs` prints.
pub fn render(o: &Opts, dir_label: &str, lines: &[Line], now_ms: i64, tz: &dyn Tz) -> String {
    use std::fmt::Write as _;
    if o.help {
        return USAGE.to_owned();
    }
    if lines.is_empty() {
        return format!("No diagnostics log in {dir_label}\n");
    }
    let since = duration_ms(o.since.as_deref());
    let from = since.map(|s| now_ms.saturating_sub(s as i64));
    let kept: Vec<&Line> = lines
        .iter()
        .filter(|l| from.is_none_or(|f| l.ms >= f))
        .collect();
    let sessions = kept.iter().filter(|l| kind_of(l) == "session").count();

    let mut out = String::new();
    let _ = writeln!(out, "{dir_label}");
    if let Some(last) = lines.iter().rev().find(|l| kind_of(l) == "session") {
        let r = &last.raw;
        let detailed = if truthy(r.get("verbose")) {
            ", detailed"
        } else {
            ""
        };
        let _ = writeln!(
            out,
            "last session {}: {} {}, pid {}{detailed}",
            clock(tz, last.ms),
            show(r.get("app")),
            show(r.get("version")),
            show(r.get("pid"))
        );
    }
    let window = match (since, o.since.as_deref()) {
        (Some(_), Some(s)) => format!(" in the last {s}"),
        _ => String::new(),
    };
    let _ = writeln!(out, "{} lines{window}, {sessions} session(s)\n", kept.len());

    let of_tab = |l: &Line| match (&o.tab, l.raw.get("id")) {
        (None, _) | (_, None) => true,
        (Some(tab), Some(id)) => show(Some(id)) == *tab,
    };

    if o.all || o.agent {
        // A tab's capped flood: how many lines before this one were not written.
        let shown: Vec<&Line> = kept
            .iter()
            .copied()
            .filter(|x| {
                let kind_ok = match &o.kinds {
                    Some(kinds) => kinds.contains(&kind_of(x)),
                    None => !o.agent || in_agent_timeline(x),
                };
                kind_ok && of_tab(x)
            })
            .collect();
        if o.agent && !shown.iter().any(|x| is_agent(x)) {
            out.push_str("No agent indicator lines here (an app older than #152 writes none).\n\n");
        }
        for l in shown {
            let dropped = match l.raw.get("dropped") {
                Some(d) if is_agent(l) && truthy(Some(d)) => {
                    format!("  [{} dropped before]", show(Some(d)))
                }
                _ => String::new(),
            };
            let _ = writeln!(
                out,
                "{}  {}  {}  {}{dropped}",
                clock(tz, l.ms),
                pad_end(&or(l.raw.get("src"), "?"), 4),
                pad_end(&kind_of(l), 14),
                summary(&l.raw)
            );
        }
        return out;
    }

    let problems: Vec<&Line> = kept
        .iter()
        .copied()
        .filter(|l| {
            let kind_ok = match &o.kinds {
                Some(kinds) => kinds.contains(&kind_of(l)),
                None => is_problem(l),
            };
            kind_ok && of_tab(l)
        })
        .collect();
    let problems = &problems[problems.len().saturating_sub(o.n)..];
    if problems.is_empty() {
        out.push_str("No stalls, errors or slow calls.\n");
        return out;
    }
    let crumbs: Vec<&Line> = lines.iter().filter(|l| is_context(l)).collect();
    for p in problems {
        let _ = writeln!(
            out,
            "{}  {}  {}  {}",
            clock(tz, p.ms),
            pad_end(&or(p.raw.get("src"), "?"), 4),
            pad_end(&kind_of(p), 14),
            summary(&p.raw)
        );
        let before: Vec<&Line> = crumbs
            .iter()
            .copied()
            .filter(|c| c.ms <= p.ms && c.ms >= p.ms - CRUMB_WINDOW_MS)
            .collect();
        for c in &before[before.len().saturating_sub(CRUMBS_BEFORE)..] {
            let _ = writeln!(
                out,
                "      {:>6}s  {}  {}",
                fixed1_ms(c.ms - p.ms),
                show(c.raw.get("src")),
                summary(&c.raw)
            );
        }
        out.push('\n');
    }
    out
}

/// A string padded on the right with spaces to `n` characters (JavaScript's `padEnd`).
fn pad_end(s: &str, n: usize) -> String {
    let len = s.chars().count();
    format!("{s}{}", " ".repeat(n.saturating_sub(len)))
}

/// Now, in ms since the Unix epoch.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// The default logs folder: `%APPDATA%\Flare\logs`, else `<temp>\Flare\logs` (the profile rule).
fn default_logs_dir() -> PathBuf {
    match std::env::var("APPDATA") {
        Ok(a) if !a.is_empty() => PathBuf::from(a).join("Flare").join("logs"),
        _ => std::env::temp_dir().join("Flare").join("logs"),
    }
}

/// The reader's entry point. 0 on success; 2 on a bad argument.
pub fn cli_main() -> i32 {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let o = match parse_args(&argv) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("{}\n{USAGE}", sanitize_for_terminal(&e));
            return 2;
        }
    };
    if o.help {
        print!("{USAGE}");
        return 0;
    }
    let dir = match (&o.dir, &o.profile) {
        (Some(d), _) => d.clone(),
        (None, Some(p)) => p.join("logs"),
        (None, None) => default_logs_dir(),
    };
    let label = dir.display().to_string();
    let lines = read_lines(&dir);
    let text = if o.utc {
        render(&o, &label, &lines, now_ms(), &Utc)
    } else {
        render(&o, &label, &lines, now_ms(), &WinLocal)
    };
    print!("{}", sanitize_for_terminal(&text));
    0
}

/// Text safe to print to a terminal: ESC and every other C0 / C1 control, DEL and the bidi
/// controls become a visible `\u{..}` escape. Newline and tab stay. A hostile log line cannot
/// inject terminal escapes through the reader.
pub fn sanitize_for_terminal(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        let bad = matches!(c,
            '\u{0}'..='\u{8}' | '\u{b}'..='\u{1f}' | '\u{7f}'..='\u{9f}'
            | '\u{61c}' | '\u{200e}' | '\u{200f}' | '\u{2028}' | '\u{2029}'
            | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}');
        if bad {
            out.push_str(&format!("\\u{{{:x}}}", c as u32));
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_escapes_terminal_and_bidi_controls() {
        let hostile = "a\u{1b}[2Jb\u{9b}31m\u{202e}c\u{7}d\r\u{85}e";
        let got = sanitize_for_terminal(hostile);
        assert_eq!(got, "a\\u{1b}[2Jb\\u{9b}31m\\u{202e}c\\u{7}d\\u{d}\\u{85}e");
        assert!(!got.chars().any(|c| c.is_control()));
        assert_eq!(sanitize_for_terminal("ok\n\tline é"), "ok\n\tline é");
    }

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    /// A line built from JSON fields with a time.
    fn line(t: &str, json: &str) -> Line {
        let ms = parse_iso_ms(t).expect("a test time");
        let mut raw: Map<String, Value> = serde_json::from_str(json).expect("test json");
        raw.insert("t".to_owned(), Value::String(t.to_owned()));
        Line { ms, raw }
    }

    fn opts(list: &[&str]) -> Opts {
        parse_args(&args(list)).expect("valid arguments")
    }

    #[test]
    fn a_duration_is_ms_s_m_h_d_and_defaults_to_minutes() {
        assert_eq!(duration_ms(Some("10m")), Some(600_000));
        assert_eq!(duration_ms(Some("10")), Some(600_000));
        assert_eq!(duration_ms(Some("1.5h")), Some(5_400_000));
        assert_eq!(duration_ms(Some("250ms")), Some(250));
        assert_eq!(duration_ms(Some("2d")), Some(172_800_000));
        assert_eq!(duration_ms(Some("x")), None);
        assert_eq!(duration_ms(Some("")), None);
        assert_eq!(duration_ms(None), None);
    }

    #[test]
    fn iso_times_parse_with_milliseconds_and_zones() {
        assert_eq!(
            parse_iso_ms("2026-10-07T09:12:03.412Z"),
            Some(1_791_364_323_412)
        );
        assert_eq!(parse_iso_ms("1970-01-01T00:00:01.5Z"), Some(1500));
        assert_eq!(parse_iso_ms("1970-01-01T01:00:00.000+01:00"), Some(0));
        assert_eq!(parse_iso_ms("not a time"), None);
    }

    #[test]
    fn since_keeps_only_the_recent_lines() {
        let lines = vec![
            line(
                "2026-10-17T09:00:01.000Z",
                r#"{"src":"main","k":"mark","note":"old"}"#,
            ),
            line(
                "2026-10-17T09:00:05.000Z",
                r#"{"src":"main","k":"mark","note":"middle"}"#,
            ),
            line(
                "2026-10-17T09:00:09.000Z",
                r#"{"src":"main","k":"mark","note":"new"}"#,
            ),
        ];
        // now is 09:00:10; two seconds back keeps only the 09:00:09 line.
        let now = parse_iso_ms("2026-10-17T09:00:10.000Z").unwrap();
        let o = opts(&["--all", "--since", "2s"]);
        let out = render(&o, "dir", &lines, now, &Utc);
        assert!(
            out.contains("1 lines in the last 2s, 0 session(s)"),
            "{out}"
        );
        assert!(out.contains("\"new\""), "{out}");
        assert!(!out.contains("\"middle\""), "{out}");
    }

    #[test]
    fn a_page_batch_that_arrives_late_is_sorted_by_t() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("diag.jsonl"),
            concat!(
                r#"{"t":"2026-10-17T09:00:05.000Z","src":"page","k":"mark","note":"later"}"#,
                "\n",
                r#"{"t":"2026-10-17T09:00:01.000Z","src":"page","k":"mark","note":"earlier"}"#,
                "\n"
            ),
        )
        .unwrap();
        let lines = read_lines(dir.path());
        let notes: Vec<&str> = lines
            .iter()
            .map(|l| l.raw["note"].as_str().unwrap())
            .collect();
        assert_eq!(notes, ["earlier", "later"]);
    }

    #[test]
    fn rotated_files_are_read_oldest_first() {
        let dir = tempfile::tempdir().unwrap();
        let same = r#"{"t":"2026-10-17T09:00:00.000Z","src":"main","k":"mark","note":"%"}"#;
        std::fs::write(
            dir.path().join("diag.jsonl"),
            same.replace('%', "live") + "\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("diag.jsonl.1"),
            same.replace('%', "one") + "\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("diag.jsonl.4"),
            same.replace('%', "four") + "\n",
        )
        .unwrap();
        let notes: Vec<String> = read_lines(dir.path())
            .iter()
            .map(|l| l.raw["note"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(notes, ["four", "one", "live"]);
    }

    #[test]
    fn a_line_cut_by_a_crash_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("diag.jsonl"),
            concat!(
                r#"{"t":"2026-10-17T09:00:00.000Z","src":"main","k":"mark","note":"whole"}"#,
                "\n",
                r#"{"t":"2026-10-17T09:00:0"#,
                "\n"
            ),
        )
        .unwrap();
        assert_eq!(read_lines(dir.path()).len(), 1);
    }

    #[test]
    fn kinds_filter_and_n_limit() {
        let lines = vec![
            line(
                "2026-10-17T09:00:01.000Z",
                r#"{"src":"main","k":"main-lag","ms":300,"inflight":[]}"#,
            ),
            line(
                "2026-10-17T09:00:02.000Z",
                r#"{"src":"main","k":"mark","note":"a"}"#,
            ),
            line(
                "2026-10-17T09:00:03.000Z",
                r#"{"src":"main","k":"mark","note":"b"}"#,
            ),
            line(
                "2026-10-17T09:00:04.000Z",
                r#"{"src":"main","k":"mark","note":"c"}"#,
            ),
        ];
        let now = 0;
        let only_lag = render(&opts(&["--kinds", "main-lag"]), "d", &lines, now, &Utc);
        assert!(only_lag.contains("main-lag"), "{only_lag}");
        assert!(!only_lag.contains("\"a\""), "{only_lag}");
        let two = render(&opts(&["--n", "2"]), "d", &lines, now, &Utc);
        assert!(two.contains("\"b\"") && two.contains("\"c\""), "{two}");
        assert!(!two.contains("\"a\""), "{two}");
    }

    #[test]
    fn agent_view_and_one_tabs_view() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/reader/agent");
        let lines = read_lines(&fixture);
        let all_agent = render(&opts(&["--agent"]), "a", &lines, 0, &Utc);
        assert!(
            all_agent.contains("t4 title working (codex)"),
            "{all_agent}"
        );
        let one = render(&opts(&["--agent", "--tab", "t3"]), "a", &lines, 0, &Utc);
        assert!(one.contains("t3 idle (stop)"), "{one}");
        assert!(!one.contains("t4"), "{one}");
    }

    #[test]
    fn flare_kinds_are_problems_and_summarised() {
        let lines = vec![
            line(
                "2026-10-17T09:00:01.000Z",
                r#"{"src":"app","k":"frame-slow","ms":1234,"crumbs":[]}"#,
            ),
            line(
                "2026-10-17T09:00:02.000Z",
                r#"{"src":"app","k":"ui-stack","ms":1500,"stack":"ui.rs+0x10\nmain.rs+0x20"}"#,
            ),
            line(
                "2026-10-17T09:00:03.000Z",
                r#"{"src":"app","k":"ui-stack","ms":1600,"inflight":[{"name":"save","ms":900,"done":false}]}"#,
            ),
            line(
                "2026-10-17T09:00:04.000Z",
                r#"{"src":"app","k":"work-slow","name":"save","ms":640,"sync":true}"#,
            ),
            line(
                "2026-10-17T09:00:05.000Z",
                r#"{"src":"app","k":"work-error","name":"save","ms":3,"msg":"boom"}"#,
            ),
            line(
                "2026-10-17T09:00:06.000Z",
                r#"{"src":"app","k":"panic","msg":"explode","loc":"main.rs:9:1","repeats":2,"stack":"frame one"}"#,
            ),
            line(
                "2026-10-17T09:00:07.000Z",
                r#"{"src":"app","k":"render-error","msg":"gpu lost"}"#,
            ),
        ];
        let out = render(&opts(&[]), "d", &lines, 0, &Utc);
        assert!(out.contains("frame-slow      1234ms"), "{out}");
        assert!(
            out.contains(
                "ui-stack        1500ms without a heartbeat\n      ui.rs+0x10\n      main.rs+0x20"
            ),
            "{out}"
        );
        assert!(
            out.contains("ui-stack        1600ms without a heartbeat\n      save"),
            "{out}"
        );
        assert!(out.contains("work-slow       save 640ms (sync)"), "{out}");
        assert!(out.contains("work-error      save: boom"), "{out}");
        assert!(
            out.contains(
                "panic           explode at main.rs:9:1 (and 2 more like it)\n      frame one"
            ),
            "{out}"
        );
        assert!(out.contains("render-error    gpu lost"), "{out}");
    }

    #[test]
    fn parse_args_rejects_unknown_flags_with_exit_2() {
        assert!(parse_args(&args(&["--bogus"])).is_err());
        assert!(parse_args(&args(&["--dir"])).is_err());
        assert!(parse_args(&args(&["--dir", "x", "--utc"])).is_ok());
    }

    #[test]
    fn an_empty_folder_says_no_diagnostics_log() {
        let dir = tempfile::tempdir().unwrap();
        let lines = read_lines(dir.path());
        assert!(lines.is_empty());
        let out = render(&opts(&[]), "somewhere", &lines, 0, &Utc);
        assert_eq!(out, "No diagnostics log in somewhere\n");
    }

    #[test]
    fn the_local_zone_gives_calendar_fields() {
        let fields = WinLocal.local(1_791_364_323_412);
        assert!((1..=12).contains(&fields[1]), "{fields:?}");
        assert_eq!(Utc.local(1_791_364_323_412), [2026, 10, 7, 9, 12, 3, 412]);
    }
}

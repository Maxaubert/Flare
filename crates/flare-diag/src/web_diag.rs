//! The web panel's kinds in the diagnostics log (rule 247): what the browser refused, a pop-up, a failed load, a
//! failed browser process, a certificate error, a denied permission and, in verbose mode, the chain of sites a page
//! went through. A 2FA hand-off that fails is a hop from a shop to a bank, so each line says the tab (`id`), the view
//! (`web`, `app` or `mod`), the `site` and the site of the page it came `from`.
//!
//! Sites only (rule 110): the lines are built from `flare_web::WebNote`, whose sites `flare_surfaces::web::site_of`
//! made, and fixed words. Never an address, a path, a query, a page's title or text, a cookie or a certificate.
//!
//! The gate: one line per `kind|site|detail` per `WEB_EVERY_MS`, its copies counted and said as `repeats` on the
//! next line (or by `tick` once the window has passed), and at most `WEB_BUDGET` lines per kind per window. A
//! process failure and a certificate error (`ALWAYS_FIRST`) always write their first line, over the budget.
//! Time is passed in: `tick(now)` writes what is held and `next_due()` says when the glue must call it next.

use std::collections::HashMap;
use std::sync::Arc;

use crate::agent_diag::{DiagSink, DiagValue};

/// One line per key per this long.
pub const WEB_EVERY_MS: u64 = 10_000;
/// At most this many lines per kind per `WEB_EVERY_MS`.
pub const WEB_BUDGET: u32 = 10;
/// The kinds whose first line of a key is written even over the budget.
pub const ALWAYS_FIRST: &[&str] = &["web-process", "web-cert"];
/// Every web kind. `web-nav` is written only in verbose mode (the sink drops it otherwise).
pub const WEB_KINDS: &[&str] = &[
    "web-refused",
    "web-popup",
    "web-close-ignored",
    "web-load-failed",
    "web-process",
    "web-cert",
    "web-permission",
    "web-nav",
];
/// The web kinds the reader counts as problems.
pub const WEB_PROBLEMS: &[&str] = &["web-process", "web-load-failed", "web-cert"];
/// Past this many keys, a new one that is held is not remembered.
const MAX_KEYS: usize = 200;

/// One line to offer the gate: its kind, the site and detail that key it, and its fields.
#[derive(Clone, Debug, PartialEq)]
pub struct WebLine {
    pub kind: &'static str,
    pub site: String,
    pub detail: &'static str,
    pub fields: Vec<(&'static str, DiagValue)>,
}

impl WebLine {
    fn key(&self) -> String {
        format!("{}|{}|{}", self.kind, self.site, self.detail)
    }
}

struct Entry {
    /// The newest copy, written by `tick` with the count.
    line: WebLine,
    last_sent: Option<u64>,
    held: u64,
}

/// The web kinds' gate over the sink.
pub struct WebDiag {
    sink: Arc<dyn DiagSink>,
    entries: HashMap<String, Entry>,
    /// Per kind: the window's start and the lines written in it.
    windows: HashMap<&'static str, (u64, u32)>,
}

impl WebDiag {
    pub fn new(sink: Arc<dyn DiagSink>) -> Self {
        Self {
            sink,
            entries: HashMap::new(),
            windows: HashMap::new(),
        }
    }

    /// Takes one line: written now, or held and counted.
    pub fn log(&mut self, line: WebLine, now: u64) {
        let key = line.key();
        let last_sent = self.entries.get(&key).and_then(|e| e.last_sent);
        if last_sent.is_some_and(|t| now.saturating_sub(t) < WEB_EVERY_MS) {
            if let Some(e) = self.entries.get_mut(&key) {
                e.held += 1;
                e.line = line;
            }
            return;
        }
        let first = last_sent.is_none();
        let over = !self.allow(line.kind, now);
        if over && !(first && ALWAYS_FIRST.contains(&line.kind)) {
            let room = self.entries.len() < MAX_KEYS;
            match self.entries.get_mut(&key) {
                Some(e) => {
                    e.held += 1;
                    e.line = line;
                }
                None if room => {
                    self.entries.insert(
                        key,
                        Entry {
                            line,
                            last_sent: None,
                            held: 1,
                        },
                    );
                }
                None => {}
            }
            return;
        }
        if over {
            // The first line of a process failure or a certificate error, over the budget: it still counts.
            self.count(line.kind, now);
        }
        // The copies held since the last line (or held over the budget before any): this line stands for them too.
        let held = self.entries.get(&key).map_or(0, |e| e.held);
        self.write(&line, (held > 0).then_some(held), now);
        if self.entries.len() < MAX_KEYS || self.entries.contains_key(&key) {
            self.entries.insert(
                key,
                Entry {
                    line,
                    last_sent: Some(now),
                    held: 0,
                },
            );
        }
    }

    /// Writes the held copies whose window has passed, one line each with `repeats`, within the budget, and
    /// forgets a key quiet for six windows.
    pub fn tick(&mut self, now: u64) {
        let mut due: Vec<String> = self
            .entries
            .iter()
            .filter(|(_, e)| {
                e.held > 0
                    && e.last_sent
                        .is_none_or(|t| now.saturating_sub(t) >= WEB_EVERY_MS)
            })
            .map(|(k, _)| k.clone())
            .collect();
        due.sort();
        for key in due {
            let Some(kind) = self.entries.get(&key).map(|e| e.line.kind) else {
                continue;
            };
            if !self.allow(kind, now) {
                continue;
            }
            let Some(e) = self.entries.get_mut(&key) else {
                continue;
            };
            let (line, held, sent_before) = (e.line.clone(), e.held, e.last_sent.is_some());
            e.held = 0;
            e.last_sent = Some(now);
            // A key never written says its first line now, with the copies after it as repeats.
            let repeats = if sent_before { held } else { held - 1 };
            self.write(&line, (repeats > 0).then_some(repeats), now);
        }
        self.entries.retain(|_, e| {
            e.held > 0
                || e.last_sent
                    .is_some_and(|t| now.saturating_sub(t) < WEB_EVERY_MS * 6)
        });
    }

    /// When `tick` has something to write: the earliest end of a window with copies held.
    pub fn next_due(&self) -> Option<u64> {
        self.entries
            .values()
            .filter(|e| e.held > 0)
            .map(|e| e.last_sent.map_or(0, |t| t + WEB_EVERY_MS))
            .min()
    }

    /// Takes one line of `kind`'s budget, when there is one left in its window.
    fn allow(&mut self, kind: &'static str, now: u64) -> bool {
        let w = self.windows.entry(kind).or_insert((now, 0));
        if now.saturating_sub(w.0) >= WEB_EVERY_MS {
            *w = (now, 0);
        }
        if w.1 >= WEB_BUDGET {
            return false;
        }
        w.1 += 1;
        true
    }

    fn count(&mut self, kind: &'static str, now: u64) {
        let w = self.windows.entry(kind).or_insert((now, 0));
        w.1 = w.1.saturating_add(1);
    }

    fn write(&self, line: &WebLine, repeats: Option<u64>, now: u64) {
        let mut fields: Vec<(&str, DiagValue)> =
            line.fields.iter().map(|(k, v)| (*k, v.clone())).collect();
        if let Some(n) = repeats {
            fields.push(("repeats", DiagValue::Num(n)));
        }
        self.sink.record(line.kind, &fields, now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_diag::MemorySink;

    fn line(kind: &'static str, site: &str, detail: &'static str) -> WebLine {
        WebLine {
            kind,
            site: site.to_string(),
            detail,
            fields: vec![
                ("id", DiagValue::Str("t1".into())),
                ("site", DiagValue::Str(site.into())),
            ],
        }
    }

    fn diag() -> (Arc<MemorySink>, WebDiag) {
        let sink = Arc::new(MemorySink::default());
        let d = WebDiag::new(sink.clone());
        (sink, d)
    }

    #[test]
    fn a_repeat_inside_the_window_is_counted_and_said_once_it_has_passed() {
        let (sink, mut d) = diag();
        d.log(line("web-popup", "bank.example", "panel"), 1_000);
        for t in [2_000, 3_000, 4_000] {
            d.log(line("web-popup", "bank.example", "panel"), t);
        }
        assert_eq!(sink.lines().len(), 1);
        assert_eq!(sink.lines()[0].num("repeats"), None);
        assert_eq!(d.next_due(), Some(11_000));
        d.tick(10_999);
        assert_eq!(sink.lines().len(), 1);
        d.tick(11_000);
        let lines = sink.lines();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1].k, "web-popup");
        assert_eq!(lines[1].num("repeats"), Some(3));
        assert_eq!(d.next_due(), None);
    }

    #[test]
    fn a_repeat_after_the_window_is_written_with_the_count_it_carries() {
        let (sink, mut d) = diag();
        d.log(line("web-refused", "a.example", "scheme"), 0);
        d.log(line("web-refused", "a.example", "scheme"), 5_000);
        d.log(line("web-refused", "a.example", "scheme"), 12_000);
        let lines = sink.lines();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1].num("repeats"), Some(1));
        assert_eq!(d.next_due(), None);
    }

    #[test]
    fn another_site_or_detail_is_another_key() {
        let (sink, mut d) = diag();
        d.log(line("web-load-failed", "a.example", "timeout"), 0);
        d.log(line("web-load-failed", "b.example", "timeout"), 1);
        d.log(line("web-load-failed", "a.example", "cert-expired"), 2);
        assert_eq!(sink.lines().len(), 3);
    }

    #[test]
    fn a_kind_writes_at_most_its_budget_per_window_and_the_rest_later() {
        let (sink, mut d) = diag();
        for i in 0..15 {
            d.log(
                line("web-permission", &format!("s{i}.example"), "camera"),
                100,
            );
        }
        assert_eq!(sink.lines().len(), WEB_BUDGET as usize);
        // Another kind has its own budget.
        d.log(line("web-popup", "x.example", "panel"), 100);
        assert_eq!(sink.lines().len(), WEB_BUDGET as usize + 1);
        // The five held sites are said in the next window, each as its own first line.
        d.tick(10_100);
        let lines = sink.lines();
        assert_eq!(lines.len(), WEB_BUDGET as usize + 6);
        assert!(lines[11..].iter().all(|l| l.num("repeats").is_none()));
    }

    #[test]
    fn a_process_failure_and_a_certificate_error_always_write_their_first_line() {
        let (sink, mut d) = diag();
        for kind in ALWAYS_FIRST {
            for i in 0..12 {
                d.log(line(kind, &format!("s{i}.example"), "crashed"), 100);
            }
        }
        assert_eq!(sink.lines().len(), 24);
        // A copy of one already written still waits for its window.
        d.log(line("web-process", "s0.example", "crashed"), 200);
        assert_eq!(sink.lines().len(), 24);
    }

    #[test]
    fn a_quiet_key_is_forgotten() {
        let (_sink, mut d) = diag();
        d.log(line("web-nav", "a.example", ""), 0);
        d.tick(WEB_EVERY_MS * 6);
        assert!(d.entries.is_empty());
    }

    #[test]
    fn every_kind_is_one_the_log_takes() {
        for k in WEB_KINDS {
            assert!(crate::summary::kind_ok(k), "{k}");
        }
        for k in WEB_PROBLEMS.iter().chain(ALWAYS_FIRST) {
            assert!(WEB_KINDS.contains(k), "{k}");
        }
    }
}

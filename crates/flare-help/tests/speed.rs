//! A keystroke is quick on the whole catalogue (M7 Task 27). Ported from `catalogue.test.ts`'s
//! "answers a keystroke quickly". This file is its own test binary, so the first search below really
//! is the first search of the process: it pays for the catalogue load and the index build, and the
//! warm searches after it are what a keystroke costs.

use flare_help::entries::all_help;
use flare_help::search::{HelpSearchOptions, search_help};
use flare_help::types::HelpShell;
use std::time::Instant;

/// Debug builds are an order of magnitude slower, so every bound is multiplied there. The bounds
/// stay tight in release, where the panel runs.
fn scale() -> f64 {
    if cfg!(debug_assertions) { 10.0 } else { 1.0 }
}

#[test]
fn a_keystroke_is_quick_on_the_whole_catalogue() {
    let opts = HelpSearchOptions {
        shell: Some(HelpShell::Powershell),
        category: None,
        limit: None,
    };

    // The cold path: the first touch of the catalogue, then the first search (the index build).
    let cold_start = Instant::now();
    let cold_hits = search_help(all_help(), "warm the index", &opts);
    let cold_ms = cold_start.elapsed().as_secs_f64() * 1000.0;
    assert!(!cold_hits.is_empty(), "the warm-up query found nothing");
    let cold_bound = 150.0 * scale();
    assert!(
        cold_ms < cold_bound,
        "first build took {cold_ms:.1} ms, bound {cold_bound} ms"
    );

    // The warm path: twenty keystrokes of one realistic question.
    let question = "how do I find the biggest files in a folder";
    let t0 = Instant::now();
    for _ in 0..20 {
        let hits = search_help(all_help(), question, &opts);
        assert!(!hits.is_empty());
    }
    let mean_ms = t0.elapsed().as_secs_f64() * 1000.0 / 20.0;
    // Well under a millisecond on a desktop; the bound leaves a busy CI runner an order of
    // magnitude of room and still catches a search gone quadratic.
    let mean_bound = 25.0 * scale();
    assert!(
        mean_ms < mean_bound,
        "mean keystroke {mean_ms:.2} ms, bound {mean_bound} ms"
    );
}

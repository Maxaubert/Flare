//! M9 Task 4: the update preview (`--preview-update`).
//!
//! The preview shows the update window with a mock offer (sample notes, no token) and runs a
//! three-second progress that installs nothing. It never touches the network, the registry or the
//! packages folder, so it needs no feature and no packaged build.

use std::time::Duration;

use crate::types::Offer;

/// The command-line flag that starts the preview.
pub const PREVIEW_FLAG: &str = "--preview-update";

/// True when the flag appears anywhere in the argument list, as an exact match.
pub fn wants_preview(argv: &[String]) -> bool {
    argv.iter().any(|a| a == PREVIEW_FLAG)
}

/// The version the preview pretends is on offer: the next minor after `current`, `{major}.{minor+1}.0`.
/// A leading `v` or `V` is dropped, and a part that is not plain digits counts as 0.
pub fn preview_version(current: &str) -> String {
    let v = current.strip_prefix(['v', 'V']).unwrap_or(current);
    let mut parts = v.split('.').map(plain_number);
    let major = parts.next().unwrap_or(0);
    let minor = parts.next().unwrap_or(0);
    format!("{major}.{}.0", minor.saturating_add(1))
}

fn plain_number(part: &str) -> u64 {
    if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
        return 0;
    }
    part.parse::<u64>().unwrap_or(0)
}

/// The release body the preview shows: Prism Terminal's sample body with Flare's names. Generated from
/// `tests/fixtures/preview.json` (the `sample` notes), with the two Flare words substituted.
pub const SAMPLE_NOTES: &str = "<!-- Release notes generated using configuration in .github/release.yml at main -->\n\n## What's Changed\n* The update button opens a window with the patch notes, Cancel and Install by @Maxaubert in https://github.com/Maxaubert/Flare/pull/30\n* Explorer entries read \"Open in Flare\", and a window edges setting (#27) by @Maxaubert in https://github.com/Maxaubert/Flare/pull/29\n* Dictation: speak into the terminal, transcribed on your own machine by @Maxaubert in https://github.com/Maxaubert/Flare/pull/22\n* fix(terminal): a prompt survives the window getting narrower and wider again by @Maxaubert in https://github.com/Maxaubert/Flare/pull/31\n* fix(tabs): closing a tab that hosts an agent asks first, working or idle by @Maxaubert in https://github.com/Maxaubert/Flare/pull/32\n* Links are painted in a colour that follows the theme by @Maxaubert in https://github.com/Maxaubert/Flare/pull/33\n* Bump electron from 43.1.0 to 43.1.1 by @dependabot[bot] in https://github.com/Maxaubert/Flare/pull/34\n\n## New Contributors\n* @dependabot[bot] made their first contribution in https://github.com/Maxaubert/Flare/pull/34\n\n**Full Changelog**: https://github.com/Maxaubert/Flare/compare/v0.4.0...v0.5.0";

/// How long the preview's progress takes, in milliseconds.
pub const PREVIEW_MS: u64 = 3000;

/// Waits for a number of milliseconds. Production sleeps; tests record.
pub trait Waiter {
    fn wait(&self, ms: u64);
}

/// The real clock. The preview runs on a worker thread, so a sleep here never blocks the UI.
pub struct SleepWaiter;

impl Waiter for SleepWaiter {
    fn wait(&self, ms: u64) {
        std::thread::sleep(Duration::from_millis(ms)); // sync-ok: the preview runs on a worker thread
    }
}

/// The mock offer the preview window shows: version from `preview_version`, the sample notes, a mock
/// flag and token 0.
pub fn preview_update(current: &str) -> Offer {
    Offer {
        version: preview_version(current),
        notes: SAMPLE_NOTES.to_string(),
        mock: true,
        token: 0,
    }
}

/// Round half up for a non-negative value (JavaScript's `Math.round`).
fn round_half_up(x: f64) -> u64 {
    (x + 0.5).floor() as u64
}

/// Runs the preview's progress over `ms` milliseconds: 20 steps over 80% of the time, then a hold for
/// the rest (the last part, "installing"). It reports 0 first, then 5, 10 ... 100 percent, and stops
/// where it is when `cancelled` says so (checked after each step's wait and before its report).
/// It always resolves false: nothing was installed.
pub fn run_preview_install(
    on_pct: &mut dyn FnMut(u8),
    ms: u64,
    wait: &dyn Waiter,
    cancelled: &dyn Fn() -> bool,
) -> bool {
    const STEPS: u64 = 20;
    let hold = round_half_up(ms as f64 * 0.2);
    let tick = (ms - hold) as f64 / STEPS as f64;
    on_pct(0);
    let mut waited = 0;
    for i in 1..=STEPS {
        let target = round_half_up(tick * i as f64);
        wait.wait(target - waited);
        waited = target;
        if cancelled() {
            return false;
        }
        on_pct(round_half_up(100.0 * i as f64 / STEPS as f64) as u8);
    }
    wait.wait(hold);
    false
}

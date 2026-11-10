//! Safe mode for plugins (design 13): a run that ends without closing properly, twice in a row while plugins were
//! on, makes the next start keep every plugin off and say so in Settings > Plugins. `--no-plugins` and Shift held at
//! launch do the same for one start. The switches are not touched: the run after a safe one has them back.
//!
//! The record is one small file in the profile, `plugins-run.json`: `open` is set at start and cleared by a clean
//! quit, `plugins_on` is set (on a worker) once a plugin is on in the run, and `unclean` counts the runs in a row that
//! ended open with plugins on. It is read and written at start and at quit, before the window shows and after it
//! is gone, and the one write in between runs on a worker (rule 34).

use std::path::{Path, PathBuf};

/// The record's file name in the profile folder.
pub const FILE: &str = "plugins-run.json";
/// Unclean runs in a row (with plugins on) that make the next start safe.
pub const UNCLEAN_LIMIT: u32 = 2;

/// Why this start keeps the plugins off, as Settings says it.
pub const WHY_UNCLEAN: &str = "Flare did not close properly twice in a row while plugins were on, so they are off for this start. They are back the next time Flare starts.";
pub const WHY_ASKED: &str =
    "Flare was started without plugins this time. They are back the next time Flare starts.";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Run {
    pub open: bool,
    pub plugins_on: bool,
    pub unclean: u32,
}

impl Run {
    fn read(file: &Path) -> Option<Run> {
        let text = std::fs::read_to_string(file).ok()?; // sync-ok: startup or a worker (see the callers)
        let v: serde_json::Value = serde_json::from_str(&text).ok()?;
        Some(Run {
            open: v.get("open")?.as_bool()?,
            plugins_on: v.get("plugins_on")?.as_bool()?,
            unclean: u32::try_from(v.get("unclean")?.as_u64()?).ok()?,
        })
    }

    fn text(self) -> String {
        serde_json::json!({"open": self.open, "plugins_on": self.plugins_on, "unclean": self.unclean})
            .to_string()
    }
}

pub fn path_in(profile: &Path) -> PathBuf {
    profile.join(FILE)
}

/// What the last run left decides this one: the count of unclean runs in a row with plugins on (the last one
/// counts when it was left open with plugins on; anything else starts the count again), and whether this start
/// is safe.
pub fn decide(prev: Option<Run>) -> (u32, bool) {
    let unclean = match prev {
        Some(r) if r.open && r.plugins_on => r.unclean.saturating_add(1),
        _ => 0,
    };
    (unclean, unclean >= UNCLEAN_LIMIT)
}

/// The start: reads the last run's record, writes this one's (open, no plugin on yet) and says why the plugins are
/// off, if they are. `asked` is `--no-plugins` or Shift held at launch. A safe start keeps the count, so a third
/// unclean run in a row is still safe until one closes properly.
pub fn start(profile: &Path, asked: bool) -> Option<&'static str> {
    let file = path_in(profile);
    let prev = Run::read(&file);
    let (unclean, safe) = decide(prev);
    let now = Run {
        open: true,
        // A safe start runs no plugin, so the count it carries is the last run's.
        plugins_on: safe,
        unclean: if safe { unclean - 1 } else { unclean },
    };
    write(&file, now);
    if asked {
        Some(WHY_ASKED)
    } else if safe {
        Some(WHY_UNCLEAN)
    } else {
        None
    }
}

/// A plugin is on in this run: an unclean end now counts. Runs on a worker.
pub fn plugins_on(file: &Path) {
    let prev = Run::read(file).unwrap_or_default();
    if prev.plugins_on {
        return;
    }
    write(
        file,
        Run {
            plugins_on: true,
            open: true,
            ..prev
        },
    );
}

/// Whether Shift is held now (at launch, it asks for a start without plugins).
#[cfg(windows)]
pub fn shift_held() -> bool {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_SHIFT};
    // SAFETY: a plain read of the key state; no pointer is passed.
    let state = unsafe { GetAsyncKeyState(i32::from(VK_SHIFT)) };
    (state as u16) & 0x8000 != 0
}

/// macOS: phase 3 of the port plan (NSEvent modifierFlags). Never held until then.
#[cfg(not(windows))]
pub fn shift_held() -> bool {
    false
}

/// A clean quit: the run is closed and the count starts again.
pub fn clean_exit(profile: &Path) {
    write(&path_in(profile), Run::default());
}

fn write(file: &Path, run: Run) {
    let text = run.text();
    // sync-ok: startup, quit or a worker (see the callers)
    if let Err(e) = flare_settings::atomic::write_atomic(file, &text) {
        log::debug!("the plugins' run record was not written: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(open: bool, plugins_on: bool, unclean: u32) -> Option<Run> {
        Some(Run {
            open,
            plugins_on,
            unclean,
        })
    }

    #[test]
    fn two_unclean_runs_in_a_row_with_plugins_on_make_the_next_start_safe() {
        assert_eq!(decide(None), (0, false));
        assert_eq!(decide(run(false, true, 0)), (0, false), "closed properly");
        assert_eq!(decide(run(true, false, 1)), (0, false), "no plugin was on");
        assert_eq!(decide(run(true, true, 0)), (1, false), "one");
        assert_eq!(decide(run(true, true, 1)), (2, true), "two in a row");
    }

    #[test]
    fn the_record_follows_starts_plugins_and_quits() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        let file = path_in(p);
        // A run with a plugin on that ends open, twice: the third start is safe.
        assert_eq!(start(p, false), None);
        plugins_on(&file);
        assert_eq!(start(p, false), None);
        plugins_on(&file);
        assert_eq!(start(p, false), Some(WHY_UNCLEAN));
        // The safe run ends open too (it ran no plugin): the next is still safe, until one closes properly.
        assert_eq!(start(p, false), Some(WHY_UNCLEAN));
        clean_exit(p);
        assert_eq!(start(p, false), None);
        // Asked for: one start without plugins, whatever the record says.
        clean_exit(p);
        assert_eq!(start(p, true), Some(WHY_ASKED));
        // A record that cannot be read starts afresh.
        std::fs::write(&file, b"not json").unwrap(); // sync-ok: test code only
        assert_eq!(start(p, false), None);
    }
}

//! M9 Task 16: the verb switch and the Settings backend (RegistryVerb implements ExplorerVerb).
//!
//! The switch decides when the Explorer verb is written: on by default, a remembered "no" (the
//! off marker) is honoured across launches, a repair puts back an absent verb, and a stale label
//! is rewritten alone. Only a build that is the installed app (`allowed`) touches the registry or
//! the marker. The pure rules are in `verb_keys.rs` (Task 14), the operations in `verb.rs` (Task 15).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use flare_settings::verb::{ExplorerVerb, VerbState};

use crate::names::OFF_MARKER_FILE;
use crate::registry::Registry;
use crate::verb::{install_verb, relabel_verb, remove_verb, stale_label_keys, verb_installed};
use crate::verb_keys::should_write_verb;

/// The switch: the launch-time default, the Settings toggle, and the one rule over both (a build
/// that is not the installed app never writes anything).
pub struct VerbSwitch {
    reg: Arc<dyn Registry>,
    root: String,
    allowed: bool,
    exe: Option<String>,
    off_marker: PathBuf,
    writes: AtomicU32,
}

impl VerbSwitch {
    pub fn new(
        reg: Arc<dyn Registry>,
        root: String,
        allowed: bool,
        exe: Option<String>,
        off_marker: PathBuf,
    ) -> Self {
        Self {
            reg,
            root,
            allowed,
            exe,
            off_marker,
            writes: AtomicU32::new(0),
        }
    }

    /// The executable the verb may point at: only in a build that is the installed app.
    fn live_exe(&self) -> Option<&str> {
        if self.allowed {
            self.exe.as_deref()
        } else {
            None
        }
    }

    fn bump(&self) {
        self.writes.fetch_add(1, Ordering::SeqCst);
    }

    /// Repairs the verb on launch. Never over a remembered "no"; relabels a stale entry alone.
    pub fn reconcile(&self) {
        let Some(exe) = self.live_exe() else {
            return;
        };
        // sync-ok: runs on the settings worker, never the UI thread
        let said_no = self.off_marker.exists();
        let installed = said_no || verb_installed(self.reg.as_ref(), &self.root, exe);
        if should_write_verb(said_no, installed) {
            self.bump();
            install_verb(self.reg.as_ref(), &self.root, exe);
            return;
        }
        if said_no {
            return;
        }
        let stale = stale_label_keys(self.reg.as_ref(), &self.root, exe);
        if stale.is_empty() {
            return;
        }
        self.bump();
        relabel_verb(self.reg.as_ref(), &self.root, &stale);
    }

    /// True when the registry holds this executable's verb (never the marker). False without an
    /// executable, and false in a build that is not the installed app.
    pub fn status(&self) -> bool {
        self.live_exe()
            .is_some_and(|exe| verb_installed(self.reg.as_ref(), &self.root, exe))
    }

    /// Turns the verb on or off. False when this build may not write, or has no executable.
    /// The marker is the record of a "no": written on off, removed on on. A marker that cannot be
    /// written does not stop the change, as in Prism Terminal.
    pub fn set(&self, on: bool) -> bool {
        let Some(exe) = self.live_exe() else {
            return false;
        };
        if on {
            // sync-ok: runs on the settings worker, never the UI thread
            let _ = fs::remove_file(&self.off_marker);
        } else {
            if let Some(parent) = self.off_marker.parent() {
                // sync-ok: runs on the settings worker, never the UI thread
                let _ = fs::create_dir_all(parent);
            }
            // sync-ok: runs on the settings worker, never the UI thread
            let _ = fs::write(&self.off_marker, format_rfc3339(unix_now()));
        }
        self.bump();
        if on {
            install_verb(self.reg.as_ref(), &self.root, exe)
        } else {
            remove_verb(self.reg.as_ref(), &self.root)
        }
    }

    /// Registry writes attempted this session (adds, removes and relabels alike).
    pub fn writes(&self) -> u32 {
        self.writes.load(Ordering::SeqCst)
    }
}

/// The Explorer row: reads the registry, and answers what the registry says after a change.
pub struct RegistryVerb(Arc<VerbSwitch>);

impl RegistryVerb {
    pub fn new(switch: Arc<VerbSwitch>) -> Self {
        Self(switch)
    }
}

impl ExplorerVerb for RegistryVerb {
    fn status(&self) -> VerbState {
        // sync-ok: a registry read, on the settings worker, never the UI thread
        state_of(self.0.status())
    }

    fn set(&self, on: bool) -> VerbState {
        self.0.set(on);
        // What the registry says after the write, never what was asked for.
        // sync-ok: a registry read, on the settings worker, never the UI thread
        state_of(self.0.status())
    }
}

fn state_of(installed: bool) -> VerbState {
    if installed {
        VerbState::On
    } else {
        VerbState::Off
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// The off marker in the profile folder (`<profile>\shell-verb-off`).
pub fn off_marker_path(profile: &Path) -> PathBuf {
    profile.join(OFF_MARKER_FILE)
}

/// `secs` since the Unix epoch as UTC RFC 3339 text, for example `2023-11-14T22:13:20Z`.
pub fn format_rfc3339(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let of_day = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        of_day / 3600,
        of_day % 3600 / 60,
        of_day % 60
    )
}

/// Days since 1970-01-01 to (year, month, day), proleptic Gregorian (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097); // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::names::VERB_LABEL;
    use crate::registry::{FakeRegistry, RegError, SandboxRoot, WinRegistry};
    use crate::verb::{install_verb, verb_installed};
    use crate::verb_keys::{DEFAULT_CLASSES_ROOT, command_key, verb_keys};
    use std::sync::atomic::AtomicUsize;

    const ROOT: &str = r"Software\Classes";
    const EXE: &str = r"C:\Users\me\AppData\Local\Flare\current\Flare.exe";
    const OTHER_EXE: &str = r"D:\Build\Flare.exe";

    /// Counts `get_string` calls, and forwards everything else.
    struct Counting {
        inner: Arc<dyn Registry>,
        gets: AtomicUsize,
    }

    impl Registry for Counting {
        fn set_string(&self, k: &str, n: Option<&str>, v: &str) -> Result<(), RegError> {
            self.inner.set_string(k, n, v)
        }
        fn get_string(&self, k: &str, n: Option<&str>) -> Result<Option<String>, RegError> {
            self.gets.fetch_add(1, Ordering::SeqCst);
            self.inner.get_string(k, n)
        }
        fn delete_tree(&self, k: &str) -> Result<(), RegError> {
            self.inner.delete_tree(k)
        }
        fn delete_if_empty(&self, k: &str) -> Result<bool, RegError> {
            self.inner.delete_if_empty(k)
        }
        fn key_exists(&self, k: &str) -> Result<bool, RegError> {
            self.inner.key_exists(k)
        }
    }

    /// A registry that refuses every write and holds nothing.
    struct Refusing;

    impl Registry for Refusing {
        fn set_string(&self, _: &str, _: Option<&str>, _: &str) -> Result<(), RegError> {
            Err(RegError {
                code: 5,
                what: "refused",
            })
        }
        fn get_string(&self, _: &str, _: Option<&str>) -> Result<Option<String>, RegError> {
            Ok(None)
        }
        fn delete_tree(&self, _: &str) -> Result<(), RegError> {
            Err(RegError {
                code: 5,
                what: "refused",
            })
        }
        fn delete_if_empty(&self, _: &str) -> Result<bool, RegError> {
            Ok(false)
        }
        fn key_exists(&self, _: &str) -> Result<bool, RegError> {
            Ok(false)
        }
    }

    fn make_labels_old(reg: &FakeRegistry) {
        let [folder, background] = verb_keys(ROOT);
        reg.set_string(&folder, None, "Open in Flare Terminal")
            .unwrap();
        reg.set_string(&background, None, "Open Flare here")
            .unwrap();
    }

    fn installed_fake(exe: &str) -> Arc<FakeRegistry> {
        let reg = Arc::new(FakeRegistry::new());
        assert!(install_verb(reg.as_ref(), ROOT, exe));
        reg
    }

    fn switch(reg: &Arc<dyn Registry>, allowed: bool, dir: &Path) -> VerbSwitch {
        VerbSwitch::new(
            reg.clone(),
            ROOT.to_owned(),
            allowed,
            Some(EXE.to_owned()),
            off_marker_path(dir),
        )
    }

    fn as_dyn(f: &Arc<FakeRegistry>) -> Arc<dyn Registry> {
        f.clone()
    }

    #[test]
    fn writes_nothing_at_all_when_the_build_is_not_the_installed_app() {
        let dir = tempfile::tempdir().unwrap();
        let fake = Arc::new(FakeRegistry::new());
        let reg = as_dyn(&fake);
        let sw = switch(&reg, false, dir.path());
        sw.reconcile();
        assert!(!sw.set(true));
        assert!(!sw.set(false));
        assert_eq!(sw.writes(), 0);
        assert!(fake.keys().is_empty());
        assert_eq!(fake.writes(), 0);
        assert!(!off_marker_path(dir.path()).exists());
    }

    #[test]
    fn puts_an_absent_verb_back_on_launch_and_leaves_a_present_one_alone() {
        let dir = tempfile::tempdir().unwrap();
        let fake = Arc::new(FakeRegistry::new());
        let reg = as_dyn(&fake);
        let sw = switch(&reg, true, dir.path());
        sw.reconcile();
        assert!(verb_installed(fake.as_ref(), ROOT, EXE));
        assert_eq!(sw.writes(), 1);
        let before = fake.writes();
        sw.reconcile();
        assert_eq!(fake.writes(), before);
        assert_eq!(sw.writes(), 1);
    }

    #[test]
    fn honours_an_explicit_off_across_launches_until_turned_on_again() {
        let dir = tempfile::tempdir().unwrap();
        let fake = Arc::new(FakeRegistry::new());
        let reg = as_dyn(&fake);
        let sw = switch(&reg, true, dir.path());
        assert!(sw.set(false));
        assert!(off_marker_path(dir.path()).exists());
        assert!(!verb_installed(fake.as_ref(), ROOT, EXE));
        // A new launch: the marker is the "no", so nothing is written.
        let next = switch(&reg, true, dir.path());
        let before = fake.writes();
        next.reconcile();
        assert_eq!(fake.writes(), before);
        assert_eq!(next.writes(), 0);
        assert!(!verb_installed(fake.as_ref(), ROOT, EXE));
        // Turned on again: the marker goes and the verb is written.
        assert!(next.set(true));
        assert!(!off_marker_path(dir.path()).exists());
        assert!(verb_installed(fake.as_ref(), ROOT, EXE));
    }

    #[test]
    fn relabels_an_entry_that_is_on_ours_and_says_the_old_thing_the_label_alone() {
        let dir = tempfile::tempdir().unwrap();
        let fake = installed_fake(EXE);
        make_labels_old(&fake);
        let reg = as_dyn(&fake);
        let sw = switch(&reg, true, dir.path());
        let before = fake.writes();
        sw.reconcile();
        // Two label writes and nothing else: no reinstall of the icon or the command.
        assert_eq!(fake.writes() - before, 2);
        assert_eq!(sw.writes(), 1);
        for key in verb_keys(ROOT) {
            assert_eq!(
                fake.get_string(&key, None).unwrap().as_deref(),
                Some(VERB_LABEL)
            );
        }
    }

    #[test]
    fn leaves_an_entry_whose_label_is_already_right_completely_alone() {
        let dir = tempfile::tempdir().unwrap();
        let fake = installed_fake(EXE);
        let reg = as_dyn(&fake);
        let sw = switch(&reg, true, dir.path());
        let before = fake.writes();
        sw.reconcile();
        assert_eq!(fake.writes(), before);
        assert_eq!(sw.writes(), 0);
    }

    #[test]
    fn never_relabels_an_entry_the_user_turned_off_and_does_not_even_look() {
        let dir = tempfile::tempdir().unwrap();
        let fake = installed_fake(EXE);
        make_labels_old(&fake);
        let counting = Arc::new(Counting {
            inner: as_dyn(&fake),
            gets: AtomicUsize::new(0),
        });
        let reg: Arc<dyn Registry> = counting.clone();
        let no = switch(&reg, true, dir.path());
        assert!(no.set(false));
        let before = fake.writes();
        let gets_before = counting.gets.load(Ordering::SeqCst);
        let next = switch(&reg, true, dir.path());
        next.reconcile();
        assert_eq!(counting.gets.load(Ordering::SeqCst), gets_before);
        assert_eq!(fake.writes(), before);
        assert_eq!(next.writes(), 0);
    }

    #[test]
    fn does_not_relabel_on_top_of_a_fresh_write() {
        let dir = tempfile::tempdir().unwrap();
        let fake = Arc::new(FakeRegistry::new());
        let reg = as_dyn(&fake);
        let sw = switch(&reg, true, dir.path());
        sw.reconcile();
        assert_eq!(sw.writes(), 1);
        // A full install is six values; a relabel on top would add two more.
        assert_eq!(fake.writes(), 6);
        assert!(verb_installed(fake.as_ref(), ROOT, EXE));
        for key in verb_keys(ROOT) {
            assert_eq!(
                fake.get_string(&key, None).unwrap().as_deref(),
                Some(VERB_LABEL)
            );
        }
    }

    #[test]
    fn relabels_nothing_in_a_build_that_is_not_the_installed_app() {
        let dir = tempfile::tempdir().unwrap();
        let fake = installed_fake(EXE);
        make_labels_old(&fake);
        let counting = Arc::new(Counting {
            inner: as_dyn(&fake),
            gets: AtomicUsize::new(0),
        });
        let reg: Arc<dyn Registry> = counting.clone();
        let sw = switch(&reg, false, dir.path());
        let before = fake.writes();
        sw.reconcile();
        assert_eq!(counting.gets.load(Ordering::SeqCst), 0);
        assert_eq!(fake.writes(), before);
        assert_eq!(sw.writes(), 0);
        let [folder, _] = verb_keys(ROOT);
        assert_eq!(
            fake.get_string(&folder, None).unwrap().as_deref(),
            Some("Open in Flare Terminal")
        );
    }

    #[test]
    fn the_row_reads_the_registry_not_the_marker() {
        let dir = tempfile::tempdir().unwrap();
        let fake = Arc::new(FakeRegistry::new());
        let reg = as_dyn(&fake);
        // A marker alone reads Off.
        std::fs::write(off_marker_path(dir.path()), b"x").unwrap(); // sync-ok: test setup
        let row = RegistryVerb::new(Arc::new(switch(&reg, true, dir.path())));
        assert_eq!(row.status(), VerbState::Off);
        // A verb for another copy reads Off too.
        assert!(install_verb(fake.as_ref(), ROOT, OTHER_EXE));
        assert_eq!(row.status(), VerbState::Off);
        // This copy's verb reads On, and never Busy.
        assert!(install_verb(fake.as_ref(), ROOT, EXE));
        assert_eq!(row.status(), VerbState::On);
    }

    #[test]
    fn set_answers_what_the_registry_says_afterwards() {
        let dir = tempfile::tempdir().unwrap();
        let fake = Arc::new(FakeRegistry::new());
        let reg = as_dyn(&fake);
        let row = RegistryVerb::new(Arc::new(switch(&reg, true, dir.path())));
        assert_eq!(row.set(true), VerbState::On);
        assert_eq!(row.set(false), VerbState::Off);
        assert!(!verb_installed(fake.as_ref(), ROOT, EXE));
    }

    #[test]
    fn set_reads_off_when_the_registry_refuses_the_write() {
        let dir = tempfile::tempdir().unwrap();
        let reg: Arc<dyn Registry> = Arc::new(Refusing);
        let row = RegistryVerb::new(Arc::new(switch(&reg, true, dir.path())));
        assert_eq!(row.set(true), VerbState::Off);
    }

    #[test]
    fn a_dev_build_row_reads_off_and_set_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let fake = installed_fake(EXE);
        let reg = as_dyn(&fake);
        let row = RegistryVerb::new(Arc::new(switch(&reg, false, dir.path())));
        let before = fake.writes();
        assert_eq!(row.status(), VerbState::Off);
        assert_eq!(row.set(true), VerbState::Off);
        assert_eq!(row.set(false), VerbState::Off);
        assert_eq!(fake.writes(), before);
        assert!(!off_marker_path(dir.path()).exists());
    }

    #[test]
    fn a_non_boolean_is_unrepresentable() {
        // The Rust signature takes a bool; the Prism Terminal test that refused a non-boolean
        // has no Rust counterpart. This test pins the bool signatures.
        let dir = tempfile::tempdir().unwrap();
        let reg: Arc<dyn Registry> = Arc::new(FakeRegistry::new());
        let sw = switch(&reg, true, dir.path());
        let set: fn(&VerbSwitch, bool) -> bool = VerbSwitch::set;
        assert!(set(&sw, true));
        assert_eq!(sw.writes(), 1);
    }

    #[test]
    fn formats_the_marker_time_as_rfc_3339_utc() {
        assert_eq!(format_rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_rfc3339(1_700_000_000), "2023-11-14T22:13:20Z");
        assert_eq!(format_rfc3339(951_782_400), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn real_round_trip_through_the_row_under_a_sandbox_root() {
        let sandbox = SandboxRoot::new();
        let root = sandbox.classes();
        assert_ne!(root, DEFAULT_CLASSES_ROOT);
        let dir = tempfile::tempdir().unwrap();
        let reg: Arc<dyn Registry> = Arc::new(WinRegistry::current_user());
        let sw = VerbSwitch::new(
            reg.clone(),
            root.clone(),
            true,
            Some(EXE.to_owned()),
            off_marker_path(dir.path()),
        );
        let row = RegistryVerb::new(Arc::new(sw));
        assert_eq!(row.set(true), VerbState::On);
        let [folder, _] = verb_keys(&root);
        assert_eq!(
            reg.get_string(&command_key(&folder), None)
                .unwrap()
                .as_deref(),
            Some(format!("\"{EXE}\" \"%1\"").as_str())
        );
        assert_eq!(row.set(false), VerbState::Off);
        assert!(!reg.key_exists(&folder).unwrap());
    }
}

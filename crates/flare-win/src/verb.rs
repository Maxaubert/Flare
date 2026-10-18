//! M9 Task 15: the Explorer verb operations (install, remove, status, stale labels, relabel).
//!
//! Every write goes through the `Registry` trait, so the rules are tested on `FakeRegistry`; the
//! one real round trip runs under a `SandboxRoot`, never the real classes hive. Keys are relative to
//! the current user's hive. The key names and the pure rules are in `verb_keys.rs` (Task 14).

use crate::registry::Registry;
use crate::verb_keys::{add_writes, command_key, points_at, remove_keys, verb_keys, verb_spec};

/// Writes every value of the verb, in order, and stops at the first error (false).
pub fn install_verb(reg: &dyn Registry, root: &str, exe: &str) -> bool {
    add_writes(root, exe)
        .iter()
        .all(|w| reg.set_string(&w.key, w.name.as_deref(), &w.value).is_ok())
}

/// Deletes both verb keys, then the parents that `install_verb` may have created, deepest first.
/// A parent that still holds anything (someone else's verb) stays. True when every delete went
/// through; a key that never existed counts as gone.
pub fn remove_verb(reg: &dyn Registry, root: &str) -> bool {
    let mut ok = true;
    for key in remove_keys(root) {
        ok &= reg.delete_tree(&key).is_ok();
    }
    let directory = format!("{root}\\Directory");
    let parents = [
        format!("{directory}\\Background\\shell"),
        format!("{directory}\\Background"),
        format!("{directory}\\shell"),
        directory,
    ];
    for parent in parents {
        let _ = reg.delete_if_empty(&parent);
    }
    ok
}

/// The folder verb's command exists and names this executable.
pub fn verb_installed(reg: &dyn Registry, root: &str, exe: &str) -> bool {
    let [folder, _] = verb_keys(root);
    matches!(
        reg.get_string(&command_key(&folder), None),
        Ok(Some(command)) if points_at(&command, exe)
    )
}

/// The keys that are on, point at this executable, and carry a label other than today's. Each key
/// is judged on its own; an absent or foreign key is never named, and an unreadable label is not
/// stale.
pub fn stale_label_keys(reg: &dyn Registry, root: &str, exe: &str) -> Vec<String> {
    verb_keys(root)
        .into_iter()
        .filter(|key| {
            let Ok(Some(command)) = reg.get_string(&command_key(key), None) else {
                return false;
            };
            if !points_at(&command, exe) {
                return false;
            }
            let Ok(Some(label)) = reg.get_string(key, None) else {
                return false;
            };
            label != verb_spec(root, key).label
        })
        .collect()
}

/// Rewrites the default value (the label) of the named keys and nothing else. Refuses, without a
/// write, any key that is not one of the two this verb owns.
pub fn relabel_verb(reg: &dyn Registry, root: &str, keys: &[String]) -> bool {
    let owned = verb_keys(root);
    let mut targets = Vec::with_capacity(keys.len());
    for key in keys {
        match owned.iter().find(|own| own.eq_ignore_ascii_case(key)) {
            Some(own) => targets.push(own.clone()),
            None => return false,
        }
    }
    for key in &targets {
        let label = verb_spec(root, key).label;
        if reg.set_string(key, None, label).is_err() {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::names::VERB_LABEL;
    use crate::registry::{FakeRegistry, SandboxRoot, WinRegistry};
    use crate::verb_keys::{DEFAULT_CLASSES_ROOT, command_key, verb_keys};

    const ROOT: &str = r"Software\Classes";
    const EXE: &str = r"C:\Program Files\Flare\flare.exe";
    const OTHER_EXE: &str = r"D:\Build\flare.exe";

    fn installed_fake() -> FakeRegistry {
        let reg = FakeRegistry::new();
        assert!(install_verb(&reg, ROOT, EXE));
        reg
    }

    fn make_labels_old(reg: &FakeRegistry, folder: &str, background: &str) {
        reg.set_string(folder, None, "Open in Flare Terminal")
            .unwrap();
        reg.set_string(background, None, "Open Flare here").unwrap();
    }

    #[test]
    fn names_both_keys_of_an_install_made_before_the_label_changed() {
        let reg = installed_fake();
        let [folder, background] = verb_keys(ROOT);
        make_labels_old(&reg, &folder, &background);
        assert_eq!(stale_label_keys(&reg, ROOT, EXE), vec![folder, background]);
    }

    #[test]
    fn names_nothing_once_the_label_is_right() {
        let reg = installed_fake();
        assert!(stale_label_keys(&reg, ROOT, EXE).is_empty());
    }

    #[test]
    fn names_nothing_when_the_entry_is_off() {
        let reg = FakeRegistry::new();
        assert!(stale_label_keys(&reg, ROOT, EXE).is_empty());
    }

    #[test]
    fn never_names_a_key_that_points_at_another_copy() {
        let reg = FakeRegistry::new();
        assert!(install_verb(&reg, ROOT, OTHER_EXE));
        let [folder, background] = verb_keys(ROOT);
        make_labels_old(&reg, &folder, &background);
        assert!(stale_label_keys(&reg, ROOT, EXE).is_empty());
    }

    #[test]
    fn judges_each_key_on_its_own() {
        let reg = installed_fake();
        let [folder, background] = verb_keys(ROOT);
        reg.delete_tree(&background).unwrap();
        reg.set_string(&folder, None, "Open in Flare Terminal")
            .unwrap();
        assert_eq!(stale_label_keys(&reg, ROOT, EXE), vec![folder.clone()]);
        assert!(relabel_verb(&reg, ROOT, std::slice::from_ref(&folder)));
        assert!(!reg.key_exists(&background).unwrap());
    }

    #[test]
    fn rewrites_the_label_and_nothing_else_and_the_entry_then_reads_current() {
        let reg = installed_fake();
        let [folder, background] = verb_keys(ROOT);
        make_labels_old(&reg, &folder, &background);
        let keys = stale_label_keys(&reg, ROOT, EXE);
        let before = reg.writes();
        assert!(relabel_verb(&reg, ROOT, &keys));
        assert_eq!(reg.writes() - before, 2);
        for key in [&folder, &background] {
            assert_eq!(
                reg.get_string(key, None).unwrap().as_deref(),
                Some(VERB_LABEL)
            );
            assert_eq!(
                reg.get_string(key, Some("Icon")).unwrap(),
                Some(format!("{EXE},0"))
            );
        }
        assert_eq!(
            reg.get_string(&command_key(&folder), None).unwrap(),
            Some(format!("\"{EXE}\" \"%1\""))
        );
        assert!(stale_label_keys(&reg, ROOT, EXE).is_empty());
    }

    #[test]
    fn refuses_a_key_that_is_not_one_of_the_two_it_owns() {
        let reg = installed_fake();
        let [folder, _] = verb_keys(ROOT);
        let before = reg.writes();
        let other = r"Software\Classes\Directory\shell\OpenWithPrism".to_owned();
        assert!(!relabel_verb(&reg, ROOT, std::slice::from_ref(&other)));
        assert!(!relabel_verb(&reg, ROOT, &[folder, other]));
        assert_eq!(reg.writes(), before);
    }

    #[test]
    fn install_then_installed_then_remove() {
        let reg = FakeRegistry::new();
        assert!(install_verb(&reg, ROOT, EXE));
        assert!(verb_installed(&reg, ROOT, EXE));
        assert!(remove_verb(&reg, ROOT));
        assert!(!verb_installed(&reg, ROOT, EXE));
    }

    #[test]
    fn install_replaces_a_stale_verb() {
        let reg = FakeRegistry::new();
        let [folder, _] = verb_keys(ROOT);
        reg.set_string(&command_key(&folder), None, r#""C:\old\flare.exe" "%1""#)
            .unwrap();
        assert!(!verb_installed(&reg, ROOT, EXE));
        assert!(install_verb(&reg, ROOT, EXE));
        assert!(verb_installed(&reg, ROOT, EXE));
    }

    #[test]
    fn remove_is_true_when_it_never_existed() {
        let reg = FakeRegistry::new();
        assert!(remove_verb(&reg, ROOT));
    }

    #[test]
    fn install_then_remove_on_an_empty_registry_leaves_no_key_at_all() {
        let reg = FakeRegistry::new();
        assert!(install_verb(&reg, ROOT, EXE));
        assert!(remove_verb(&reg, ROOT));
        let root = ROOT.to_lowercase();
        for key in reg.keys() {
            assert!(
                root == key || root.starts_with(&format!("{key}\\")),
                "stray key left behind: {key}"
            );
        }
    }

    #[test]
    fn remove_keeps_a_parent_that_holds_someone_elses_verb() {
        let reg = installed_fake();
        let other = format!("{ROOT}\\Directory\\shell\\Other");
        // The neighbour's key name, built from parts so the name scan passes.
        let neighbour = ["Prism", "Terminal"].concat();
        let prism = format!("{ROOT}\\Directory\\Background\\shell\\{neighbour}");
        reg.set_string(&other, None, "Other").unwrap();
        reg.set_string(&prism, None, "Prism").unwrap();
        assert!(remove_verb(&reg, ROOT));
        assert!(reg.key_exists(&other).unwrap());
        assert!(reg.key_exists(&prism).unwrap());
        assert!(
            reg.key_exists(&format!("{ROOT}\\Directory\\shell"))
                .unwrap()
        );
        assert!(
            reg.key_exists(&format!("{ROOT}\\Directory\\Background\\shell"))
                .unwrap()
        );
        assert!(
            reg.key_exists(&format!("{ROOT}\\Directory\\Background"))
                .unwrap()
        );
        assert!(reg.key_exists(&format!("{ROOT}\\Directory")).unwrap());
    }

    #[test]
    fn real_install_and_remove_under_a_sandbox_root() {
        let sandbox = SandboxRoot::new();
        let root = sandbox.classes();
        assert_ne!(root, DEFAULT_CLASSES_ROOT);
        let reg = WinRegistry::current_user();
        assert!(install_verb(&reg, &root, EXE));
        assert!(verb_installed(&reg, &root, EXE));
        let [folder, _] = verb_keys(&root);
        assert_eq!(
            reg.get_string(&folder, None).unwrap().as_deref(),
            Some(VERB_LABEL)
        );
        assert!(remove_verb(&reg, &root));
        assert!(!verb_installed(&reg, &root, EXE));
    }
}

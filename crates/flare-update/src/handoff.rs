//! M9 Task 8: the restart arguments.
//!
//! After an install Velopack restarts the app. The restart must name the same profile the copy was
//! started with, so an update of a copy on its own profile comes back on that profile, not on the
//! default one.

use std::path::Path;

/// The arguments the restarted app is started with: none for the default profile, or the one
/// `--user-data-dir=<path>` (the equals form M3's parser reads). One string per argument, so a
/// path with spaces stays one argument.
pub fn restart_args(user_data_dir: Option<&Path>) -> Vec<String> {
    match user_data_dir {
        None => Vec::new(),
        Some(dir) => vec![format!("--user-data-dir={}", dir.display())],
    }
}

/// The arguments the new exe starts with after a staged install (2026-10-29): [`restart_args`], plus the flags of
/// `argv` (this run's arguments, without the exe) that the restarted run must keep. `--e2e` above all: a test run's
/// window must come back offscreen. Folders named on the command line are not repeated (the tabs come back from
/// `tabs.json`).
pub fn restart_args_keeping(user_data_dir: Option<&Path>, argv: &[String]) -> Vec<String> {
    let mut out = restart_args(user_data_dir);
    let mut i = 0;
    while i < argv.len() {
        let a = argv[i].as_str();
        match a {
            "--e2e" | "--no-import" | "--no-plugins" => out.push(a.to_string()),
            "--renderer" => {
                if let Some(v) = argv.get(i + 1) {
                    out.push(a.to_string());
                    out.push(v.clone());
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn a_staged_restart_keeps_the_profile_and_the_test_flags_but_no_folders() {
        let argv: Vec<String> = [
            "--user-data-dir=C:\\p",
            "--e2e",
            "--renderer",
            "software",
            "C:\\some\\folder",
            "--no-plugins",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(
            restart_args_keeping(Some(Path::new("C:\\p")), &argv),
            vec![
                "--user-data-dir=C:\\p",
                "--e2e",
                "--renderer",
                "software",
                "--no-plugins"
            ]
        );
        assert_eq!(
            restart_args_keeping(None, &["C:\\x".to_string()]),
            Vec::<String>::new()
        );
    }

    #[test]
    fn restarts_plainly_when_no_profile_was_named() {
        assert_eq!(restart_args(None), Vec::<String>::new());
    }

    #[test]
    fn restarts_on_the_same_profile_it_was_started_with() {
        assert_eq!(
            restart_args(Some(Path::new("C:\\p"))),
            vec!["--user-data-dir=C:\\p".to_string()]
        );
    }

    #[test]
    fn a_path_with_spaces_stays_one_argument() {
        let dir = PathBuf::from("C:\\Users\\Admin\\My Flare Profile");
        let args = restart_args(Some(&dir));
        assert_eq!(args.len(), 1);
        assert_eq!(
            args[0],
            "--user-data-dir=C:\\Users\\Admin\\My Flare Profile"
        );
    }
}

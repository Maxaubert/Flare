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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

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

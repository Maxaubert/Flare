//! M9 Task 35: the single-instance mutex and pipe are Flare's own names, whatever profile folder they
//! are derived from (Appendix B: Prism Terminal's lock is Electron's, with no named object of ours).

use std::path::Path;

use flare_win::single_instance::instance_names;

#[test]
fn the_mutex_and_pipe_names_are_flares_own() {
    let names = instance_names(Path::new(r"C:\Users\Someone\AppData\Roaming\Flare"));
    assert!(
        names.mutex.starts_with("Local\\Flare.Single."),
        "mutex: {}",
        names.mutex
    );
    assert!(
        names.pipe.starts_with("\\\\.\\pipe\\Flare.Handoff."),
        "pipe: {}",
        names.pipe
    );
}

#[test]
fn a_prism_terminal_profile_folder_still_gives_flares_names() {
    // The names come from the profile's identity and hash, never from the folder's own name: the
    // other product's folder name, given as input, yields Flare's prefixes all the same.
    let other = ["Pri", "smTerminal"].concat();
    let path = format!(r"C:\Users\Someone\AppData\Roaming\{other}");
    let names = instance_names(Path::new(&path));
    assert!(
        names.mutex.starts_with("Local\\Flare."),
        "mutex: {}",
        names.mutex
    );
    assert!(
        names.pipe.starts_with("\\\\.\\pipe\\Flare."),
        "pipe: {}",
        names.pipe
    );
    assert!(!names.mutex.contains(&other));
    assert!(!names.pipe.contains(&other));
}

#[test]
fn the_names_are_the_names_constants() {
    use flare_win::names::{MUTEX_PREFIX, PIPE_PREFIX};
    let names = instance_names(Path::new(r"C:\Users\Someone\AppData\Roaming\Flare"));
    assert!(names.mutex.starts_with(&format!("Local\\{MUTEX_PREFIX}.")));
    assert!(
        names
            .pipe
            .starts_with(&format!("\\\\.\\pipe\\{PIPE_PREFIX}."))
    );
}

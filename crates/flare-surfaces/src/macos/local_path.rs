//! macOS: which paths a panel may read (see `preview::loadable`). A local path is absolute (`/`)
//! and reaches no server: the same refusal a tab's folder gets (`flare_shells::cwd::is_remote`:
//! no `//` path, no `.` or `..` segment, nothing under an automount point). Twin of the
//! drive-letter rule on Windows.

/// True for an absolute path on this machine.
pub fn is_local_path(path: &str) -> bool {
    path.starts_with('/') && !flare_shells::cwd::is_remote(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_local_path_is_absolute_and_reaches_no_server() {
        assert!(is_local_path("/Users/me/p.png"));
        assert!(is_local_path("/Volumes/USB/p.png"));
        assert!(is_local_path("/netbsd/p.png"));
        for bad in [
            "p.png",
            "a/p.png",
            "~/p.png",
            "//server/share/p.png",
            "/net/host/p.png",
            "/NET/host/p.png",
            "/Network/Servers/h/p.png",
            "/tmp/../net/evil/p.png",
            "/Users/./me/p.png",
            "/System/Volumes/Data/net/evil/p.png",
            "/home/someone/p.png",
            r"C:\a\p.png",
            "",
        ] {
            assert!(!is_local_path(bad), "{bad:?}");
        }
    }

    #[test]
    fn a_dot_dot_path_to_an_automount_is_never_loadable() {
        // `panels.json` is anyone's to edit: the text is checked before anything reads it.
        assert!(!crate::preview::loadable(
            "/tmp/../net/evil/p.png",
            &crate::preview::PanelKind::Image
        ));
    }
}

//! macOS: how a folder reads in the + menu (`places::whereabouts`). POSIX folders hang off one
//! root `/` and use `/` between names; the Windows forms (a drive letter, `\`) stay in the shared
//! modules. The sidebar's folder line knows both forms on every build (`sidebar_rows::folder_line_by`,
//! rule 1279: a remote tab's folder is its host's).

use crate::folder_names::path_parts;

/// Where a folder lives: the last two folders above it, with a leading ellipsis when more exist,
/// else from the root (`/Users/me`).
pub fn whereabouts(p: &str) -> String {
    let parts = path_parts(p);
    let above = &parts[..parts.len().saturating_sub(1)];
    let near = above[above.len().saturating_sub(2)..].join("/");
    if above.len() > 2 {
        format!("\u{2026}/{near}")
    } else if p.starts_with('/') {
        format!("/{near}")
    } else {
        near
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whereabouts_shows_the_last_two_folders_above() {
        assert_eq!(
            whereabouts("/Users/admin/Documents/Claude/Github/Atlas"),
            "\u{2026}/Claude/Github"
        );
        assert_eq!(whereabouts("/Users/admin/atlas"), "/Users/admin");
        assert_eq!(whereabouts("/work"), "/");
        assert_eq!(whereabouts("/"), "/");
    }
}

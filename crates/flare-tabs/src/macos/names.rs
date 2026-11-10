//! macOS: how a folder reads in the sidebar and the + menu (`sidebar_rows::folder_line`,
//! `places::whereabouts`). POSIX folders hang off one root `/` and use `/` between names; the
//! Windows forms (a drive letter, `\`) stay in the shared modules.

use crate::folder_names::path_parts;

/// The folder a tab's folder sits in, the user's own folder as `~` (`~/code`). A folder right
/// under the root says `/`.
pub fn folder_line(cwd: &str, home: &str) -> String {
    let path = cwd.trim_end_matches('/');
    if path.is_empty() {
        return cwd.to_string();
    }
    let Some(cut) = path.rfind('/') else {
        return path.to_string();
    };
    if cut == 0 {
        return "/".to_string();
    }
    let parent = &path[..cut];
    let h = home.trim_end_matches('/');
    if !h.is_empty() {
        if parent.eq_ignore_ascii_case(h) {
            return "~".to_string();
        }
        if let Some(head) = parent.get(..h.len()) {
            let rest = &parent[h.len()..];
            if head.eq_ignore_ascii_case(h) && rest.starts_with('/') {
                return format!("~{rest}");
            }
        }
    }
    parent.to_string()
}

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

    const HOME: &str = "/Users/admin";

    #[test]
    fn folder_line_is_the_parent_with_home_as_tilde() {
        assert_eq!(
            folder_line("/Users/admin/Documents/Claude/Github/Atlas", HOME),
            "~/Documents/Claude/Github"
        );
        assert_eq!(folder_line("/Users/admin/dotfiles", HOME), "~");
        assert_eq!(folder_line("/users/Admin/code/api/", HOME), "~/code");
        assert_eq!(
            folder_line("/Users/administrator/x", HOME),
            "/Users/administrator"
        );
        assert_eq!(folder_line("/Users/admin/x", ""), "/Users/admin");
    }

    #[test]
    fn folder_line_says_the_root_right_under_it() {
        assert_eq!(folder_line("/Volumes", HOME), "/");
        assert_eq!(folder_line("/", HOME), "/");
        assert_eq!(folder_line("/Volumes/USB/", HOME), "/Volumes");
    }

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

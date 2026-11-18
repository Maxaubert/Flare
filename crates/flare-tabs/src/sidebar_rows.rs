//! folder_line and find_keeps for the sidebar rows (Task 9).

use crate::tabs::Os;

/// The rules a folder's path follows: a drive letter and `\` (a PC), or one root `/` (a Mac or Linux). A local tab's
/// folder follows this machine's; a remote tab's follows its host's, on any viewer (rule 1279).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathRules {
    Windows,
    Posix,
}

impl PathRules {
    /// This machine's own rules.
    pub const HERE: PathRules = if cfg!(windows) {
        PathRules::Windows
    } else {
        PathRules::Posix
    };

    /// A host's rules: by its system when the machines name it, else by the path itself (a drive letter or a `\` is a
    /// PC's; a Linux host, which has no kind of its own, reads as POSIX).
    pub fn of_host(kind: Option<Os>, path: &str) -> PathRules {
        match kind {
            Some(Os::Pc) => PathRules::Windows,
            Some(Os::Mac) => PathRules::Posix,
            None => {
                let b = path.as_bytes();
                let drive = b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':';
                if drive || path.contains('\\') {
                    PathRules::Windows
                } else {
                    PathRules::Posix
                }
            }
        }
    }
}

/// The folder a tab's folder sits in, the user's own folder as `~`. A drive's root says the drive.
/// macOS: POSIX folders (a folder right under the root says `/`).
pub fn folder_line(cwd: &str, home: &str) -> String {
    folder_line_by(cwd, home, PathRules::HERE)
}

/// `folder_line` by the given rules, whatever this machine is: a remote tab's line follows its host's (rule 1279).
pub fn folder_line_by(cwd: &str, home: &str, rules: PathRules) -> String {
    match rules {
        PathRules::Windows => folder_line_win(cwd, home),
        PathRules::Posix => folder_line_posix(cwd, home),
    }
}

fn folder_line_win(cwd: &str, home: &str) -> String {
    let path = cwd.trim_end_matches(['\\', '/']);
    let Some(cut) = path.rfind(['\\', '/']).filter(|&i| i > 0) else {
        return path.to_string();
    };
    let mut parent = path[..cut].to_string();
    let bytes = parent.as_bytes();
    if bytes.len() == 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return parent.to_ascii_uppercase();
    }
    let h = home.trim_end_matches(['\\', '/']);
    if !h.is_empty() {
        if parent.eq_ignore_ascii_case(h) {
            parent = "~".to_string();
        } else if let Some(head) = parent.get(..h.len()) {
            let rest = &parent[h.len()..];
            if head.eq_ignore_ascii_case(h) && rest.starts_with('\\') {
                parent = format!("~{rest}");
            }
        }
    }
    parent
}

/// POSIX folders hang off one root `/` and use `/` between names (`~/code`).
fn folder_line_posix(cwd: &str, home: &str) -> String {
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

/// Whether a row stays while finding: every word of the query, any order, any case.
pub fn find_keeps(query: &str, name: &str, cwd: &str) -> bool {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if words.is_empty() {
        return true;
    }
    let hay = format!("{name}\n{cwd}").to_lowercase();
    words.iter().all(|w| hay.contains(w.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = r"C:\Users\Admin";
    const MAC_HOME: &str = "/Users/admin";

    fn win(cwd: &str, home: &str) -> String {
        folder_line_by(cwd, home, PathRules::Windows)
    }

    fn posix(cwd: &str, home: &str) -> String {
        folder_line_by(cwd, home, PathRules::Posix)
    }

    #[test]
    fn folder_line_is_the_parent_with_home_as_tilde() {
        assert_eq!(
            win(r"C:\Users\Admin\Documents\Claude\Github\Atlas", HOME),
            r"~\Documents\Claude\Github"
        );
        assert_eq!(win(r"C:\Users\Admin\dotfiles", HOME), "~");
        assert_eq!(win(r"c:\users\admin\code\api", HOME), r"~\code");
        assert_eq!(
            posix("/Users/admin/Documents/Claude/Github/Atlas", MAC_HOME),
            "~/Documents/Claude/Github"
        );
        assert_eq!(posix("/Users/admin/dotfiles", MAC_HOME), "~");
        assert_eq!(posix("/users/Admin/code/api/", MAC_HOME), "~/code");
        assert_eq!(
            posix("/Users/administrator/x", MAC_HOME),
            "/Users/administrator"
        );
        assert_eq!(posix("/Users/admin/x", ""), "/Users/admin");
    }

    #[test]
    fn folder_line_says_the_drive_at_a_root() {
        assert_eq!(win(r"X:\Emulators", HOME), "X:");
        assert_eq!(win(r"D:\work\site\", HOME), r"D:\work");
        assert_eq!(
            win(r"C:\Users\Administrator\x", HOME),
            r"C:\Users\Administrator"
        );
    }

    #[test]
    fn folder_line_says_the_root_right_under_it() {
        assert_eq!(posix("/Volumes", MAC_HOME), "/");
        assert_eq!(posix("/", MAC_HOME), "/");
        assert_eq!(posix("/Volumes/USB/", MAC_HOME), "/Volumes");
    }

    /// The plain `folder_line` is this machine's rules.
    #[test]
    fn folder_line_follows_this_machines_rules() {
        #[cfg(windows)]
        assert_eq!(folder_line(r"C:\Users\Admin\dotfiles", HOME), "~");
        #[cfg(not(windows))]
        assert_eq!(folder_line("/Users/admin/dotfiles", MAC_HOME), "~");
        assert_eq!(
            folder_line("~", ""),
            "~",
            "a folder not named yet stays itself"
        );
    }

    /// Rule 1279: a remote tab's line follows its host's rules on any viewer, with no home of this machine's.
    #[test]
    fn a_hosts_folder_reads_by_the_hosts_rules_on_any_viewer() {
        let line =
            |kind: Option<Os>, cwd: &str| folder_line_by(cwd, "", PathRules::of_host(kind, cwd));
        assert_eq!(line(Some(Os::Pc), r"C:\Users\Admin"), r"C:\Users");
        assert_eq!(line(Some(Os::Pc), r"C:\Users"), "C:");
        assert_eq!(line(Some(Os::Mac), "/Users/max/proj"), "/Users/max");
        assert_eq!(line(Some(Os::Mac), "/Users"), "/");
        // A machine of no known kind (a Linux host, or one the machines do not list yet): the path's shape says.
        assert_eq!(line(None, "/home/max/proj"), "/home/max");
        assert_eq!(line(None, r"D:\work\site"), r"D:\work");
        assert_eq!(line(None, "c:/w/api"), "c:/w");
        assert_eq!(line(Some(Os::Pc), "~"), "~");
        assert_eq!(line(Some(Os::Mac), "~"), "~");
        assert_eq!(PathRules::of_host(None, "~"), PathRules::Posix);
    }

    #[test]
    fn find_keeps_every_word_any_case_any_order() {
        assert!(find_keeps("", "Atlas", r"C:\x"));
        assert!(find_keeps("atl", "Atlantic", r"C:\x"));
        assert!(find_keeps(
            "github atlas",
            "Atlas",
            r"C:\Users\Admin\Documents\Claude\Github\Atlas"
        ));
        assert!(!find_keeps("sonara", "Atlas", r"C:\x"));
    }
}

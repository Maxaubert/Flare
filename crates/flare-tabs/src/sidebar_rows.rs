//! folder_line and find_keeps for the sidebar rows (Task 9).

/// The folder a tab's folder sits in, the user's own folder as `~`. A drive's root says the drive.
pub fn folder_line(cwd: &str, home: &str) -> String {
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

    #[test]
    fn folder_line_is_the_parent_with_home_as_tilde() {
        assert_eq!(
            folder_line(r"C:\Users\Admin\Documents\Claude\Github\Prism", HOME),
            r"~\Documents\Claude\Github"
        );
        assert_eq!(folder_line(r"C:\Users\Admin\dotfiles", HOME), "~");
        assert_eq!(folder_line(r"c:\users\admin\code\api", HOME), r"~\code");
    }

    #[test]
    fn folder_line_says_the_drive_at_a_root() {
        assert_eq!(folder_line(r"X:\Emulators", HOME), "X:");
        assert_eq!(folder_line(r"D:\work\site\", HOME), r"D:\work");
        assert_eq!(
            folder_line(r"C:\Users\Administrator\x", HOME),
            r"C:\Users\Administrator"
        );
    }

    #[test]
    fn find_keeps_every_word_any_case_any_order() {
        assert!(find_keeps("", "Prism", r"C:\x"));
        assert!(find_keeps("pri", "PrismTerminal", r"C:\x"));
        assert!(find_keeps(
            "github prism",
            "Prism",
            r"C:\Users\Admin\Documents\Claude\Github\Prism"
        ));
        assert!(!find_keeps("sonara", "Prism", r"C:\x"));
    }
}

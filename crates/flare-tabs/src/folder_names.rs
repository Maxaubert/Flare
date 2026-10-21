//! path_parts, base_name, parent_name: the folder names the tabs and rows show.

/// The path's folders, root first. Either separator, any run of them, a trailing one or not.
pub fn path_parts(p: &str) -> Vec<&str> {
    p.split(['\\', '/']).filter(|s| !s.is_empty()).collect()
}

/// The last folder. A drive root has only its letter, so `D:\` reads `D:`. No parts: `p` itself.
pub fn base_name(p: &str) -> String {
    match path_parts(p).last() {
        Some(last) => (*last).to_string(),
        None => p.to_string(),
    }
}

/// The folder before the last one, or "" at a root.
pub fn parent_name(p: &str) -> String {
    let parts = path_parts(p);
    match parts.len() {
        0 | 1 => String::new(),
        n => parts[n - 2].to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_parts_split_on_either_separator_runs() {
        assert_eq!(path_parts("C:\\w\\api"), vec!["C:", "w", "api"]);
        assert_eq!(path_parts("c:/w/api/"), vec!["c:", "w", "api"]);
        assert_eq!(path_parts("C:\\\\w//api"), vec!["C:", "w", "api"]);
        assert!(path_parts("").is_empty());
    }

    #[test]
    fn base_and_parent_names() {
        assert_eq!(base_name("D:\\"), "D:");
        assert_eq!(base_name(""), "");
        assert_eq!(base_name("C:\\w\\api\\"), "api");
        assert_eq!(parent_name("C:\\w\\api"), "w");
        assert_eq!(parent_name("D:\\"), "");
        assert_eq!(parent_name(""), "");
    }
}

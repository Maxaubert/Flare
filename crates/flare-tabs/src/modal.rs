//! returns_focus and escape_markdown for the dialog (Tasks 9 and 12).

/// Where the focus is when a dialog closes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActiveFocus {
    /// On the document body.
    Body,
    /// Nowhere.
    Nothing,
    /// On some other element.
    Other,
}

/// Whether a dialog gives the focus back on close: only when nothing else has taken it.
pub fn returns_focus(before_connected: bool, before_is_body: bool, active: ActiveFocus) -> bool {
    before_connected
        && !before_is_body
        && matches!(active, ActiveFocus::Body | ActiveFocus::Nothing)
}

/// Escapes markup in text that goes into styled text. A newline becomes a space.
pub fn escape_markdown(s: &str) -> String {
    const SPECIAL: &str = "\\`*_{}[]()#+-.!|<>~";
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\n' => out.push(' '),
            c if SPECIAL.contains(c) => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_returns_only_when_nothing_else_took_it() {
        assert!(returns_focus(true, false, ActiveFocus::Body));
        assert!(returns_focus(true, false, ActiveFocus::Nothing));
        assert!(!returns_focus(true, false, ActiveFocus::Other));
        assert!(!returns_focus(false, false, ActiveFocus::Body));
        assert!(!returns_focus(true, true, ActiveFocus::Body));
    }

    #[test]
    fn escape_markdown_escapes_markup() {
        assert_eq!(escape_markdown("api_v2*final"), r"api\_v2\*final");
        assert_eq!(escape_markdown("a`b"), "a\\`b");
        assert_eq!(escape_markdown("plain"), "plain");
        assert_eq!(escape_markdown(r"C:\x"), r"C:\\x");
        assert_eq!(escape_markdown("a\nb"), "a b");
    }
}

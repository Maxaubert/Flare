//! The words of a plugin's question (the `fill` permission, 2026-10-28): a fill to approve and a secret to type, in
//! Flare's own dialog (`ui/plugin_ask.slint`). Every name in it comes from a plugin or a page, so each is cleaned,
//! cut and escaped before it reaches the markdown body, as the download question does.

use flare_tabs::modal::escape_markdown;

/// What the dialog shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AskView {
    /// 0 a fill to approve, 1 a secret to type (the component's `kind`).
    pub kind: i32,
    pub title: String,
    /// Markdown.
    pub body: String,
    /// What the field is for (a secret only).
    pub label: String,
}

/// The longest name or host the question prints (the rest is cut with an ellipsis).
const SHOWN_MAX: usize = 80;

fn cut(s: &str) -> String {
    let clean: String = s.chars().filter(|c| !c.is_control()).collect();
    let head: String = clean.chars().take(SHOWN_MAX).collect();
    if clean.chars().count() > SHOWN_MAX {
        format!("{head}...")
    } else {
        head
    }
}

fn named(s: &str, ink: &str) -> String {
    format!("<font color=\"{ink}\">{}</font>", escape_markdown(&cut(s)))
}

/// "Fill <login> into <host>?", asked for the plugin by name. `ink` is the theme's text colour.
pub fn fill_view(plugin_name: &str, login_name: &str, host: &str, ink: &str) -> AskView {
    AskView {
        kind: 0,
        title: format!("{} wants to fill a login", cut(plugin_name)),
        body: format!(
            "Fill {} into {}? The page gets the login; the agent never sees it.",
            named(login_name, ink),
            named(host, ink)
        ),
        label: String::new(),
    }
}

/// A plugin's ask for a secret, titled with the plugin's name.
pub fn secret_view(plugin_name: &str, title: &str, label: &str, ink: &str) -> AskView {
    AskView {
        kind: 1,
        title: format!("{}: {}", cut(plugin_name), cut(title)),
        body: format!(
            "What you type goes to {} only. Flare does not keep it or show it.",
            named(plugin_name, ink)
        ),
        label: cut(label),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fill_question_names_the_login_the_host_and_the_plugin() {
        let v = fill_view("Bitwarden", "Example account", "example.com", "#fff");
        assert_eq!(v.kind, 0);
        assert_eq!(v.title, "Bitwarden wants to fill a login");
        assert!(
            v.body
                .starts_with("Fill <font color=\"#fff\">Example account</font> into ")
        );
        // Escaped for the markdown body; it reads `example.com`.
        assert!(v.body.contains(">example\\.com</font>?"), "{}", v.body);
        assert!(!v.body.contains('\u{2014}') && !v.title.contains('\u{2014}'));
    }

    #[test]
    fn names_from_a_plugin_are_escaped_cut_and_cleaned() {
        let v = fill_view("P", "**bold** [link](x) <b>", "h", "#fff");
        assert!(
            !v.body.contains("**bold**") && !v.body.contains("[link](x)"),
            "{}",
            v.body
        );
        let v = fill_view("P", &"x".repeat(200), "h", "#fff");
        // The ellipsis is escaped like the rest of the name.
        assert!(
            v.body
                .contains(&format!("{}\\.\\.\\.", "x".repeat(SHOWN_MAX))),
            "{}",
            v.body
        );
        let v = secret_view("Bit\nwarden", "Unlock\u{7}", "Master password", "#fff");
        assert_eq!(v.title, "Bitwarden: Unlock");
        assert_eq!(v.kind, 1);
        assert_eq!(v.label, "Master password");
    }
}

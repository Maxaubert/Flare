//! The words of a plugin's question (the `fill` permission, 2026-10-28; centred in the plugin's own look,
//! 2026-11-08): a fill to approve and a secret to type, in Flare's own dialog (`ui/plugin_ask.slint`). Every name in it
//! comes from a plugin or a page, so each is cleaned and cut before it is shown (the dialog draws plain text, so
//! nothing in a name is read as markup). The line under each question, whose question it is, is Flare's: a plugin
//! dressed as another cannot change it.

/// What the dialog shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AskView {
    /// 0 a fill to approve, 1 a secret to type (the component's `kind`).
    pub kind: i32,
    /// The heading: "Fill <login>?", or the secret's title.
    pub title: String,
    /// A secret's line under the heading (an account), or empty.
    pub sub: String,
    /// What the field is for (a secret only): its placeholder.
    pub label: String,
    /// A secret's button.
    pub button: String,
    /// A fill's card: the page's host, who asked and in which tab, and the letter on its tile.
    pub site: String,
    pub asker: String,
    pub letter: String,
    /// Whose question it is, in Flare's words.
    pub foot: String,
}

/// The longest name or host the question prints (the rest is cut with an ellipsis).
const SHOWN_MAX: usize = 80;
/// The secret's button when the plugin names none.
pub const DEFAULT_BUTTON: &str = "OK";

fn cut(s: &str) -> String {
    let clean: String = s.chars().filter(|c| !c.is_control()).collect();
    let head: String = clean.chars().take(SHOWN_MAX).collect();
    if clean.chars().count() > SHOWN_MAX {
        format!("{head}...")
    } else {
        head
    }
}

/// "Fill <login>?" for the page on `host`, asked for the plugin by name; `tab` is the title of the tab it fills, and
/// `by_agent` says the agent's call asked (else the plugin did, after a click).
pub fn fill_view(
    plugin_name: &str,
    login_name: &str,
    host: &str,
    tab: &str,
    by_agent: bool,
) -> AskView {
    let tab = cut(tab);
    let asker = match (by_agent, tab.is_empty()) {
        (true, false) => format!("asked by the agent in {tab}"),
        (true, true) => "asked by the agent".to_string(),
        (false, false) => format!("asked by {} in {tab}", cut(plugin_name)),
        (false, true) => format!("asked by {}", cut(plugin_name)),
    };
    AskView {
        kind: 0,
        title: format!("Fill {}?", cut(login_name)),
        site: cut(host),
        asker,
        letter: host
            .trim_start_matches("www.")
            .chars()
            .find(|c| c.is_alphanumeric())
            .map(|c| c.to_uppercase().collect())
            .unwrap_or_default(),
        foot: format!(
            "{} fills it into the page; the agent never sees it.",
            cut(plugin_name)
        ),
        ..AskView::default()
    }
}

/// A plugin's ask for a secret: its title as the heading, its sub under it, its label in the field and its word on the
/// button ([`DEFAULT_BUTTON`] when it names none).
pub fn secret_view(
    plugin_name: &str,
    title: &str,
    label: &str,
    sub: &str,
    button: &str,
) -> AskView {
    AskView {
        kind: 1,
        title: cut(title),
        sub: cut(sub),
        label: cut(label),
        button: if button.trim().is_empty() {
            DEFAULT_BUTTON.to_string()
        } else {
            cut(button)
        },
        foot: format!(
            "What you type goes to {} only. Flare does not keep it or show it.",
            cut(plugin_name)
        ),
        ..AskView::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fill_question_names_the_login_the_site_the_tab_and_the_plugin() {
        let v = fill_view("Vault", "GitHub", "github.com", "Makro", true);
        assert_eq!(v.kind, 0);
        assert_eq!(v.title, "Fill GitHub?");
        assert_eq!(v.site, "github.com");
        assert_eq!(v.asker, "asked by the agent in Makro");
        assert_eq!(v.letter, "G");
        assert_eq!(
            v.foot,
            "Vault fills it into the page; the agent never sees it."
        );
        let v = fill_view("Vault", "Bank", "www.bank.example", "", false);
        assert_eq!(
            (v.asker.as_str(), v.letter.as_str()),
            ("asked by Vault", "B")
        );
        for v in [v, fill_view("V", "L", "h", "T", false)] {
            assert!(!format!("{v:?}").contains('\u{2014}'));
        }
    }

    #[test]
    fn names_from_a_plugin_are_cut_and_cleaned() {
        let v = fill_view("P", &"x".repeat(200), "h", "t", true);
        assert_eq!(v.title, format!("Fill {}...?", "x".repeat(SHOWN_MAX)));
        let v = secret_view("Bit\nwarden", "Unlock\u{7}", "Master password", "me@x", "");
        assert_eq!(v.title, "Unlock");
        assert_eq!(v.kind, 1);
        assert_eq!(v.label, "Master password");
        assert_eq!(v.sub, "me@x");
        assert_eq!(v.button, DEFAULT_BUTTON);
        assert!(v.foot.contains("Bitwarden only"), "{}", v.foot);
        assert_eq!(secret_view("P", "t", "l", "", "Unlock").button, "Unlock");
    }
}

//! The words of a plugin's question (the `fill` permission, 2026-10-28; centred in the plugin's own look,
//! 2026-11-08): a fill to approve and a secret to type, in Flare's own dialog (`ui/plugin_ask.slint`). Every name in it
//! comes from a plugin or a page, so each is cleaned and cut before it is shown (the dialog draws plain text, so
//! nothing in a name is read as markup). The line under a secret's question, whose question it is, is Flare's: a
//! plugin dressed as another cannot change it, and one installed from an address is named with that address's host,
//! so a copy that took another's name and colour is told apart. A fill has no such line (the owner took it out,
//! 2026-11-10). A tab's title can be set by what runs in it, so the asker line quotes it as the tab's own words.

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
    /// A secret's: whose question it is, in Flare's words. Empty for a fill.
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

/// The plugin as the question's foot names it: its name, and the host it was installed from when Settings installed
/// it from an address (`source`, empty for a file or a folder put there by hand).
fn plugin_of(plugin_name: &str, source: &str) -> String {
    let host = source
        .trim()
        .trim_start_matches("https://")
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    if host.is_empty() {
        cut(plugin_name)
    } else {
        format!("{} (from {})", cut(plugin_name), cut(host))
    }
}

/// "Fill <login>?" for the page on `host`, asked for the plugin by name; `tab` is the title of the tab it fills, and
/// `by_agent` says the agent's call asked (else the plugin did, after a click). No line under it (2026-11-10).
pub fn fill_view(
    plugin_name: &str,
    login_name: &str,
    host: &str,
    tab: &str,
    by_agent: bool,
) -> AskView {
    let tab = cut(tab);
    let asker = match (by_agent, tab.is_empty()) {
        (true, false) => format!("asked by the agent in the tab \"{tab}\""),
        (true, true) => "asked by the agent".to_string(),
        (false, false) => format!("asked by {} in the tab \"{tab}\"", cut(plugin_name)),
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
        ..AskView::default()
    }
}

/// A plugin's ask for a secret: its title as the heading, its sub under it, its label in the field and its word on the
/// button ([`DEFAULT_BUTTON`] when it names none). `source`: where the plugin was installed from, or empty.
pub fn secret_view(
    plugin_name: &str,
    source: &str,
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
            plugin_of(plugin_name, source)
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
        // The tab's title is its own words (what runs in it can set it): quoted, never Flare's sentence.
        assert_eq!(v.asker, "asked by the agent in the tab \"Makro\"");
        assert_eq!(v.letter, "G");
        // No line under a fill (the owner took it out, 2026-11-10).
        assert_eq!(v.foot, "");
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
    fn a_plugin_installed_from_an_address_is_named_with_its_host() {
        let v = secret_view("Vault", "https://example.com/v.zip", "t", "l", "", "");
        assert!(
            v.foot.contains("Vault (from example.com) only"),
            "{}",
            v.foot
        );
        assert!(
            secret_view("Vault", "", "t", "l", "", "")
                .foot
                .contains("Vault only")
        );
    }

    #[test]
    fn names_from_a_plugin_are_cut_and_cleaned() {
        let v = fill_view("P", &"x".repeat(200), "h", "t", true);
        assert_eq!(v.title, format!("Fill {}...?", "x".repeat(SHOWN_MAX)));
        let v = secret_view(
            "Bit\nwarden",
            "",
            "Unlock\u{7}",
            "Master password",
            "me@x",
            "",
        );
        assert_eq!(v.title, "Unlock");
        assert_eq!(v.kind, 1);
        assert_eq!(v.label, "Master password");
        assert_eq!(v.sub, "me@x");
        assert_eq!(v.button, DEFAULT_BUTTON);
        assert!(v.foot.contains("Bitwarden only"), "{}", v.foot);
        assert_eq!(
            secret_view("P", "", "t", "l", "", "Unlock").button,
            "Unlock"
        );
    }
}

//! The words of a plugin's question (the `fill` permission, 2026-10-28; centred in the plugin's own look,
//! 2026-11-08): a fill to approve and a secret to type, in Flare's own dialog (`ui/plugin_ask.slint`). Every name in it
//! comes from a plugin or a page, so each is cleaned and cut before it is shown (the dialog draws plain text, so
//! nothing in a name is read as markup). Neither question has a line under it: the owner took the fill's out
//! (2026-11-10) and the secret's "Asked by <plugin>" line too (2026-11-16, rule 1266). A tab's title can be set by
//! what runs in it, so the asker line quotes it as the tab's own words.

/// What the dialog shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AskView {
    /// 0 a fill to approve, 1 a secret to type (the component's `kind`).
    pub kind: i32,
    /// The heading: "Fill <login>?", or the secret's title.
    pub title: String,
    /// A secret's line under the heading (an account), or empty.
    pub sub: String,
    /// A secret's error line (why the last answer failed), or empty. When set, the dialog shows it in the sub's
    /// place in the error colour and gives the field a red edge.
    pub error: String,
    /// What the field is for (a secret only): its placeholder.
    pub label: String,
    /// A secret's button.
    pub button: String,
    /// A fill's card: the page's host, who asked and in which tab, and the letter on its tile.
    pub site: String,
    pub asker: String,
    pub letter: String,
    /// A `verify` secret's line while it is checked (rule 1274): the plugin's `checking`, or [`DEFAULT_CHECKING`].
    /// Empty for an ask without `verify`.
    pub checking: String,
}

/// The checking line when the plugin names none.
pub const DEFAULT_CHECKING: &str = "Checking...";
/// A right answer's heading when the plugin names none.
pub const DEFAULT_RIGHT_TITLE: &str = "Done";
/// The red line of a wrong answer when the plugin names none.
pub const DEFAULT_WRONG: &str = "That was not right. Try again.";
/// The red line when the plugin did not say in time whether the answer was right ([`flare_mods::fill::VERIFY_WAIT`]).
pub const TOOK_TOO_LONG: &str = "The check took too long. Try again.";

/// A `verify` ask's checking line: the plugin's words, cleaned and cut, or [`DEFAULT_CHECKING`].
pub fn checking_line(checking: &str) -> String {
    let c = cut(checking.trim());
    if c.trim().is_empty() {
        DEFAULT_CHECKING.to_string()
    } else {
        c
    }
}

/// A wrong answer's red line: the plugin's words, cleaned and cut, or [`DEFAULT_WRONG`].
pub fn wrong_line(error: &str) -> String {
    let e = cut(error.trim());
    if e.trim().is_empty() {
        DEFAULT_WRONG.to_string()
    } else {
        e
    }
}

/// A result's heading and line, cleaned and cut. An empty heading is `fallback` (Flare's "Done" for a right answer,
/// the ask's own title for an end).
pub fn result_view(title: &str, line: &str, fallback: &str) -> (String, String) {
    let t = cut(title.trim());
    let t = if t.trim().is_empty() {
        cut(fallback)
    } else {
        t
    };
    (t, cut(line.trim()))
}

/// The longest name or host the question prints (the rest is cut with an ellipsis).
const SHOWN_MAX: usize = 80;
/// The secret's button when the plugin names none.
pub const DEFAULT_BUTTON: &str = "OK";

/// A character a name may not carry into the dialog: a control character, or one that is drawn as nothing or turns
/// the text around (the bidi marks, embeddings, overrides and isolates, the zero-width ones and the BOM). A name with
/// U+202E could otherwise show its letters backwards.
fn hidden(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{061C}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2069}' | '\u{FEFF}'
        )
}

fn cut_to(s: &str, max: usize) -> String {
    let clean: String = s.chars().filter(|c| !hidden(*c)).collect();
    let head: String = clean.chars().take(max).collect();
    if clean.chars().count() > max {
        format!("{head}...")
    } else {
        head
    }
}

fn cut(s: &str) -> String {
    cut_to(s, SHOWN_MAX)
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

/// A plugin's ask for a secret: its title as the heading, its sub under it (or its error, in red, in the sub's place),
/// its label in the field and its word on the button ([`DEFAULT_BUTTON`] when it names none). No line under Cancel:
/// Max took the "Asked by <plugin>" line out (2026-11-16, rule 1266).
pub fn secret_view(title: &str, label: &str, sub: &str, button: &str, error: &str) -> AskView {
    AskView {
        kind: 1,
        title: cut(title),
        sub: cut(sub),
        error: cut(error.trim()),
        label: cut(label),
        button: if button.trim().is_empty() {
            DEFAULT_BUTTON.to_string()
        } else {
            cut(button)
        },
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
        let v = secret_view("Unlock\u{7}", "Master password", "me@x", "", "");
        assert_eq!(v.title, "Unlock");
        assert_eq!(v.kind, 1);
        assert_eq!(v.label, "Master password");
        assert_eq!(v.sub, "me@x");
        assert_eq!(v.error, "");
        assert_eq!(v.button, DEFAULT_BUTTON);
        assert_eq!(secret_view("t", "l", "", "Unlock", "").button, "Unlock");
        // Bidi overrides, isolates and zero-width characters are taken out of every name the dialog shows.
        let v = secret_view(
            "Bit\u{202E}nedraw\u{202C}\u{2066}\u{200B}\u{FEFF}",
            "l",
            "",
            "",
            "",
        );
        assert_eq!(v.title, "Bitnedraw");
    }

    #[test]
    fn the_secret_names_no_plugin_under_cancel() {
        // Max took the "Asked by <plugin>" line out (2026-11-16, rule 1266): nothing in the view says who asked, so
        // no plugin's name or install host is shown, whatever the ask says.
        let v = secret_view("Bitwarden", "Master password", "me@x", "Unlock", "");
        let shown = format!("{v:?}");
        assert!(!shown.contains("Asked by"), "{shown}");
        assert!(!shown.contains(" from "), "{shown}");
    }

    #[test]
    fn a_checked_secret_has_flares_words_where_the_plugin_names_none() {
        assert_eq!(checking_line(""), DEFAULT_CHECKING);
        assert_eq!(checking_line("  \u{200B} "), DEFAULT_CHECKING);
        assert_eq!(
            checking_line(" Checking the password...\u{7} "),
            "Checking the password..."
        );
        assert_eq!(wrong_line(""), DEFAULT_WRONG);
        assert_eq!(
            wrong_line("Wrong password, try again"),
            "Wrong password, try again"
        );
        assert_eq!(
            wrong_line(&"w".repeat(200)),
            format!("{}...", "w".repeat(SHOWN_MAX))
        );
        assert_eq!(
            result_view("", "", DEFAULT_RIGHT_TITLE),
            ("Done".to_string(), String::new())
        );
        assert_eq!(
            result_view(
                "Vault unlocked",
                "Bitwarden can fill logins now.",
                DEFAULT_RIGHT_TITLE
            ),
            (
                "Vault unlocked".to_string(),
                "Bitwarden can fill logins now.".to_string()
            )
        );
        // An end with no heading keeps the ask's own title.
        assert_eq!(
            result_view(
                "",
                "Bitwarden did not unlock after 5 tries.",
                "Your vault is locked"
            )
            .0,
            "Your vault is locked"
        );
        for s in [
            DEFAULT_CHECKING,
            DEFAULT_RIGHT_TITLE,
            DEFAULT_WRONG,
            TOOK_TOO_LONG,
        ] {
            assert!(!s.contains('\u{2014}'));
        }
    }

    #[test]
    fn an_error_is_kept_beside_the_sub_and_cleaned() {
        let v = secret_view(
            "Your vault is locked",
            "Master password",
            "me@x",
            "Unlock",
            " Wrong password.\u{7} Try again. ",
        );
        assert_eq!(v.error, "Wrong password. Try again.");
        assert_eq!(v.sub, "me@x");
        assert_eq!(v.button, "Unlock");
        let v = secret_view("t", "l", "", "", &"e".repeat(200));
        assert_eq!(v.error, format!("{}...", "e".repeat(SHOWN_MAX)));
    }
}

//! The words of a plugin's question (the `fill` permission, 2026-10-28; centred in the plugin's own look,
//! 2026-11-08): a fill to approve and a secret to type, in Flare's own dialog (`ui/plugin_ask.slint`). Every name in it
//! comes from a plugin or a page, so each is cleaned and cut before it is shown (the dialog draws plain text, so
//! nothing in a name is read as markup). The short line under a secret's Cancel, "Asked by <plugin>", is Flare's: it
//! comes from the plugin's resolved name, never from the ask, so a plugin dressed as another (Bitwarden's name in its
//! title) cannot change it. The name is quoted and always followed by an origin Flare writes ("from <host>" for an
//! address, parsed with `url`, the host never cut from the right; "from a file" otherwise), so a copy that took another's
//! name and colour, or a name like "Bitwarden (from bitwarden.com)", is told apart (rule 1266, 2026-11-15). A fill has no such line (the owner took it out,
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
    /// A secret's line under Cancel, "Asked by <plugin>", in Flare's words. Empty for a fill.
    pub foot: String,
}

/// The longest name or host the question prints (the rest is cut with an ellipsis).
const SHOWN_MAX: usize = 80;
/// The longest plugin name the secret's "Asked by" line prints, so the origin after it stays near.
const FOOT_NAME_MAX: usize = 40;
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

/// A host as the "Asked by" line shows it: whole, or when longer than [`SHOWN_MAX`] cut from the LEFT, keeping its
/// last labels (the registrable end), so `bitwarden.com.evil.net` can never read as `bitwarden.com...`.
fn host_shown(host: &str) -> String {
    if host.chars().count() <= SHOWN_MAX {
        return host.to_string();
    }
    let room = SHOWN_MAX - 3;
    let mut kept: Vec<&str> = Vec::new();
    let mut len = 0;
    for label in host.rsplit('.') {
        let add = label.chars().count() + usize::from(!kept.is_empty());
        if len + add > room {
            break;
        }
        len += add;
        kept.push(label);
    }
    if kept.is_empty() {
        let skip = host.chars().count() - room;
        return format!("...{}", host.chars().skip(skip).collect::<String>());
    }
    kept.reverse();
    format!("...{}", kept.join("."))
}

/// Where the plugin came from, in Flare's words: "from <host>" for a plugin Settings installed from an address
/// (`source`, parsed with the same `url` parser the download check uses; only the host is shown, never a user name,
/// port or path), "from a file" for one installed from a file or a folder put there by hand (`source` empty), and
/// "from an address" when the address has no host to show.
fn origin_of(source: &str) -> String {
    let source = source.trim();
    if source.is_empty() {
        return "from a file".to_string();
    }
    match url::Url::parse(source)
        .ok()
        .as_ref()
        .and_then(url::Url::host_str)
    {
        Some(host) if !host.is_empty() => format!("from {}", host_shown(host)),
        _ => "from an address".to_string(),
    }
}

/// The plugin as the secret's "Asked by" line names it: its name in quotes (its own words, so they cannot run into
/// Flare's), then where it came from ([`origin_of`]), which Flare always writes. A quote mark in the name becomes an
/// apostrophe, so the name cannot close its quotes early.
fn plugin_of(plugin_name: &str, source: &str) -> String {
    let name: String = cut_to(plugin_name.trim(), FOOT_NAME_MAX)
        .chars()
        .map(|c| {
            if matches!(c, '"' | '\u{201C}' | '\u{201D}') {
                '\''
            } else {
                c
            }
        })
        .collect();
    format!("\"{name}\" \u{00B7} {}", origin_of(source))
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
/// its label in the field and its word on the button ([`DEFAULT_BUTTON`] when it names none), and Flare's "Asked by
/// <plugin>" under Cancel. `plugin_name` is the plugin's resolved name and `source` where it was installed from (or
/// empty); nothing the plugin sends in the ask reaches that line.
pub fn secret_view(
    plugin_name: &str,
    source: &str,
    title: &str,
    label: &str,
    sub: &str,
    button: &str,
    error: &str,
) -> AskView {
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
        foot: format!("Asked by {}", plugin_of(plugin_name, source)),
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
        let v = secret_view("Vault", "https://example.com/v.zip", "t", "l", "", "", "");
        assert_eq!(v.foot, "Asked by \"Vault\" \u{00B7} from example.com");
        // No address: Flare still names the origin.
        for empty in ["", "   "] {
            assert_eq!(
                secret_view("Vault", empty, "t", "l", "", "", "").foot,
                "Asked by \"Vault\" \u{00B7} from a file"
            );
        }
        // A user name, password and port are never shown, only the host.
        assert_eq!(
            plugin_of("Vault", "https://bitwarden.com:pw@evil.net:8443/v.zip?x#y"),
            "\"Vault\" \u{00B7} from evil.net"
        );
        assert_eq!(
            plugin_of("Vault", "https://user@Example.COM:444/v.zip"),
            "\"Vault\" \u{00B7} from example.com"
        );
        // An address with no host to show.
        assert_eq!(
            plugin_of("Vault", "not an address"),
            "\"Vault\" \u{00B7} from an address"
        );
    }

    #[test]
    fn a_host_is_never_cut_from_the_right() {
        // The security review's case: a look-alike host shows whole, its real end visible.
        assert_eq!(
            plugin_of("Bitwarden", "https://bitwarden.com.evil.net/b.zip"),
            "\"Bitwarden\" \u{00B7} from bitwarden.com.evil.net"
        );
        // A host longer than the line keeps its end, cut from the left.
        let long = format!("bitwarden.com.{}.evil.net", "a".repeat(70));
        let foot = plugin_of("Bitwarden", &format!("https://{long}/b.zip"));
        assert_eq!(foot, "\"Bitwarden\" \u{00B7} from ...evil.net");
        assert!(!foot.contains("bitwarden.com"));
        let many = format!("bitwarden.com.{}evil.net", "ab.".repeat(40));
        let shown = host_shown(&many);
        assert!(
            shown.starts_with("...") && shown.ends_with(".ab.evil.net"),
            "{shown}"
        );
        assert!(shown.chars().count() <= SHOWN_MAX);
        assert!(!shown.contains("bitwarden"));
        // One label too long for the line: its last characters.
        let one = "x".repeat(100);
        assert_eq!(
            host_shown(&one),
            format!("...{}", "x".repeat(SHOWN_MAX - 3))
        );
    }

    #[test]
    fn a_name_cannot_pass_for_another_plugins_origin() {
        // A plugin from a file named like the real one from an address: the quotes and Flare's origin tell them apart.
        let real = plugin_of("Bitwarden", "https://bitwarden.com/b.zip");
        let fake = plugin_of("Bitwarden (from bitwarden.com)", "");
        assert_eq!(real, "\"Bitwarden\" \u{00B7} from bitwarden.com");
        assert_eq!(
            fake,
            "\"Bitwarden (from bitwarden.com)\" \u{00B7} from a file"
        );
        assert!(!fake.ends_with("from bitwarden.com"));
        // A name that closes its own quotes and writes Flare's words: the quotes become apostrophes.
        let fake = plugin_of("Bitwarden\" \u{00B7} from bitwarden.com", "");
        assert_eq!(
            fake,
            "\"Bitwarden' \u{00B7} from bitwarden.com\" \u{00B7} from a file"
        );
        assert_eq!(fake.matches('"').count(), 2);
        // Bidi overrides, isolates and zero-width characters are taken out.
        let fake = plugin_of("Bit\u{202E}nedraw\u{202C}\u{2066}\u{200B}\u{FEFF}", "");
        assert_eq!(fake, "\"Bitnedraw\" \u{00B7} from a file");
        // A long name is cut, the origin stays.
        let fake = plugin_of(&"B".repeat(200), "https://evil.net/x.zip");
        assert_eq!(
            fake,
            format!(
                "\"{}...\" \u{00B7} from evil.net",
                "B".repeat(FOOT_NAME_MAX)
            )
        );
    }

    #[test]
    fn the_asked_by_line_names_the_real_plugin_whatever_the_ask_says() {
        // A plugin dressed as Bitwarden: its title, sub, label, button and error all say Bitwarden, but the line is
        // built from its resolved name and install host only.
        let v = secret_view(
            "Vault",
            "https://example.com/v.zip",
            "Bitwarden",
            "Bitwarden master password",
            "Asked by Bitwarden",
            "Asked by Bitwarden",
            "Asked by Bitwarden",
        );
        assert_eq!(v.foot, "Asked by \"Vault\" \u{00B7} from example.com");
        let plain = secret_view("Vault", "https://example.com/v.zip", "t", "l", "", "", "");
        assert_eq!(v.foot, plain.foot, "the ask's words never change the line");
        assert!(!v.foot.contains('\u{2014}'));
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
            "",
        );
        assert_eq!(v.title, "Unlock");
        assert_eq!(v.kind, 1);
        assert_eq!(v.label, "Master password");
        assert_eq!(v.sub, "me@x");
        assert_eq!(v.error, "");
        assert_eq!(v.button, DEFAULT_BUTTON);
        assert_eq!(v.foot, "Asked by \"Bitwarden\" \u{00B7} from a file");
        assert_eq!(
            secret_view("P", "", "t", "l", "", "Unlock", "").button,
            "Unlock"
        );
    }

    #[test]
    fn an_error_is_kept_beside_the_sub_and_cleaned() {
        let v = secret_view(
            "Bitwarden",
            "",
            "Your vault is locked",
            "Master password",
            "me@x",
            "Unlock",
            " Wrong password.\u{7} Try again. ",
        );
        assert_eq!(v.error, "Wrong password. Try again.");
        assert_eq!(v.sub, "me@x");
        assert_eq!(v.button, "Unlock");
        let v = secret_view("P", "", "t", "l", "", "", &"e".repeat(200));
        assert_eq!(v.error, format!("{}...", "e".repeat(SHOWN_MAX)));
    }
}

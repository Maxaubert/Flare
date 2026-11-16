//! The words of a plugin's question (the `fill` permission, 2026-10-28; centred in the plugin's own look,
//! 2026-11-08): a fill to approve and a secret to type, in Flare's own dialog (`ui/plugin_ask.slint`). Every name in it
//! comes from a plugin or a page, so each is cleaned and cut before it is shown (the dialog draws plain text, so
//! nothing in a name is read as markup). The short line under a secret's Cancel, "Asked by <plugin>", is Flare's: it
//! comes from the plugin's resolved name, never from the ask, so a plugin dressed as another (Bitwarden's name in its
//! title) cannot change it. The name is quoted, and for a plugin installed from an address Flare adds "from <host>"
//! (parsed with `url`, the host never cut from the right; nothing for a plugin from a file, 2026-11-16), so a copy that
//! took another's name and colour, or a name like "Bitwarden (from bitwarden.com)", is told apart (rule 1266,
//! 2026-11-15). The name loses its quote-like characters and is cut at its first dot-like separator and at " from ",
//! so the line holds " · from " only where Flare wrote it (a security review, 2026-11-16). A fill has no such line (the owner took it out, 2026-11-10). A tab's title can be set by what runs in it, so the asker line quotes it as the tab's own words.

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
/// port or path). None for one installed from a file or a folder put there by hand (`source` empty), or when the
/// address has no host to show (the owner took "from a file" out, 2026-11-16).
fn origin_of(source: &str) -> Option<String> {
    let source = source.trim();
    if source.is_empty() {
        return None;
    }
    url::Url::parse(source)
        .ok()
        .as_ref()
        .and_then(url::Url::host_str)
        .filter(|host| !host.is_empty())
        .map(|host| format!("from {}", host_shown(host)))
}

/// A quote-like character: the ASCII quote and apostrophe, the backtick, the curly and low quotes (U+2018 to U+201F),
/// the angle quotes, the primes (U+2032 to U+2037, U+2057), the modifier letters drawn as quotes (U+02B9 to U+02BC,
/// U+02BA, U+02DD, U+02EE), the acute accent, Hebrew geresh and gershayim, the ditto mark (U+3003), the CJK and
/// fullwidth quotes (U+301D to U+301F, U+FF02, U+FF07, U+FF40) and the dingbat quotes. A name loses them all, so it
/// can never seem to close the quotes Flare puts around it.
fn quote_like(c: char) -> bool {
    matches!(
        c,
        '"' | '\''
            | '`'
            | '\u{00AB}'
            | '\u{00B4}'
            | '\u{00BB}'
            | '\u{02B9}'..='\u{02BC}'
            | '\u{02C8}'
            | '\u{02DD}'
            | '\u{02EE}'
            | '\u{05F3}'
            | '\u{05F4}'
            | '\u{2018}'..='\u{201F}'
            | '\u{2032}'..='\u{2037}'
            | '\u{2039}'
            | '\u{203A}'
            | '\u{2057}'
            | '\u{275B}'..='\u{2760}'
            | '\u{2E42}'
            | '\u{3003}'
            | '\u{301D}'..='\u{301F}'
            | '\u{FF02}'
            | '\u{FF07}'
            | '\u{FF40}'
            | '\u{1F676}'..='\u{1F678}'
    )
}

/// A dot-like separator, such as the middle dot (U+00B7) Flare puts before "from <host>": the Greek ano teleia,
/// the bullets (U+2022, U+2023, U+2043, U+25CF, U+25E6), the one-dot leader, the hyphenation point (U+2027), the
/// bullet and dot operators (U+2219, U+22C5), the katakana and halfwidth middle dots (U+30FB, U+FF65) and other
/// raised dots. The "Asked by" line cuts a name at the first one.
fn separator_like(c: char) -> bool {
    matches!(
        c,
        '\u{00B7}'
            | '\u{0387}'
            | '\u{16EB}'
            | '\u{1427}'
            | '\u{2022}'
            | '\u{2023}'
            | '\u{2024}'
            | '\u{2027}'
            | '\u{2043}'
            | '\u{2219}'
            | '\u{22C5}'
            | '\u{25CF}'
            | '\u{25E6}'
            | '\u{2981}'
            | '\u{2E31}'
            | '\u{2E33}'
            | '\u{30FB}'
            | '\u{A78F}'
            | '\u{FF65}'
            | '\u{10101}'
    )
}

/// The plugin's name as the "Asked by" line shows it (rule 1266, hardened 2026-11-16). Hidden characters go, every
/// space-like character becomes one plain space, and quote-like characters ([`quote_like`]) are taken out. Then the
/// name is cut at its first dot-like separator ([`separator_like`]) and at its first " from " (any case), so a name
/// such as `Bitwarden″ · from bitwarden.com` shows as `Bitwarden`. Last, it is trimmed and cut at 40 characters.
/// The line can so only hold " · from " where Flare wrote it, for a plugin installed from an address.
fn foot_name(plugin_name: &str) -> String {
    let mut name = String::new();
    for c in plugin_name
        .chars()
        .filter(|c| !hidden(*c) && !quote_like(*c))
    {
        if separator_like(c) {
            break;
        }
        let c = if c.is_whitespace() { ' ' } else { c };
        if c == ' ' && (name.is_empty() || name.ends_with(' ')) {
            continue;
        }
        name.push(c);
    }
    if let Some(at) = name.to_ascii_lowercase().find(" from ") {
        name.truncate(at);
    }
    cut_to(name.trim(), FOOT_NAME_MAX)
}

/// The plugin as the secret's "Asked by" line names it: its name in quotes ([`foot_name`]: its own words, so they
/// cannot run into Flare's), then, for a plugin from an address, its host ([`origin_of`]).
fn plugin_of(plugin_name: &str, source: &str) -> String {
    let name = foot_name(plugin_name);
    match origin_of(source) {
        Some(origin) => format!("\"{name}\" \u{00B7} {origin}"),
        None => format!("\"{name}\""),
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
        // No address (a plugin from a file): the name only.
        for empty in ["", "   "] {
            assert_eq!(
                secret_view("Vault", empty, "t", "l", "", "", "").foot,
                "Asked by \"Vault\""
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
        // An address with no host to show: the name only.
        assert_eq!(plugin_of("Vault", "not an address"), "\"Vault\"");
        assert_eq!(plugin_of("Vault", "file:///C:/v.zip"), "\"Vault\"");
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
        // A plugin from a file named like the real one from an address: the quotes tell them apart.
        let real = plugin_of("Bitwarden", "https://bitwarden.com/b.zip");
        let fake = plugin_of("Bitwarden (from bitwarden.com)", "");
        assert_eq!(real, "\"Bitwarden\" \u{00B7} from bitwarden.com");
        assert_eq!(fake, "\"Bitwarden (from bitwarden.com)\"");
        assert!(!fake.ends_with("from bitwarden.com"));
        // A name that closes its own quotes and writes Flare's words: the quote goes and the name is cut at the dot.
        let fake = plugin_of("Bitwarden\" \u{00B7} from bitwarden.com", "");
        assert_eq!(fake, "\"Bitwarden\"");
        assert_eq!(fake.matches('"').count(), 2);
        // Bidi overrides, isolates and zero-width characters are taken out.
        let fake = plugin_of("Bit\u{202E}nedraw\u{202C}\u{2066}\u{200B}\u{FEFF}", "");
        assert_eq!(fake, "\"Bitnedraw\"");
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

    /// Flare's own separator before an origin. A line holds it only when Flare wrote it for an address install.
    const FLARE_SEP: &str = " \u{00B7} from ";

    #[test]
    fn look_alike_quotes_are_taken_out_of_the_name() {
        for q in [
            '"',
            '\'',
            '`',
            '\u{00AB}',
            '\u{00BB}',
            '\u{00B4}',
            '\u{02BA}',
            '\u{02BC}',
            '\u{02DD}',
            '\u{05F4}',
            '\u{2018}',
            '\u{2019}',
            '\u{201A}',
            '\u{201B}',
            '\u{201C}',
            '\u{201D}',
            '\u{201E}',
            '\u{201F}',
            '\u{2032}',
            '\u{2033}',
            '\u{2036}',
            '\u{2039}',
            '\u{203A}',
            '\u{2057}',
            '\u{275E}',
            '\u{3003}',
            '\u{301E}',
            '\u{FF02}',
            '\u{FF07}',
            '\u{FF40}',
            '\u{1F677}',
        ] {
            let foot = secret_view(&format!("Bit{q}warden{q}"), "", "t", "l", "", "", "").foot;
            assert_eq!(foot, "Asked by \"Bitwarden\"", "{:04X}", q as u32);
        }
        // A plain apostrophe is a quote too: "Max's Vault" shows as "Maxs Vault".
        assert_eq!(plugin_of("Max's Vault", ""), "\"Maxs Vault\"");
    }

    #[test]
    fn a_name_is_cut_at_a_dot_like_separator() {
        for dot in [
            '\u{00B7}', '\u{0387}', '\u{2022}', '\u{2023}', '\u{2024}', '\u{2027}', '\u{2043}',
            '\u{2219}', '\u{22C5}', '\u{25CF}', '\u{2E31}', '\u{30FB}', '\u{FF65}',
        ] {
            let name = format!("Bitwarden {dot} example.org");
            assert_eq!(plugin_of(&name, ""), "\"Bitwarden\"", "{:04X}", dot as u32);
            assert_eq!(
                plugin_of(&name, "https://evil.net/b.zip"),
                "\"Bitwarden\" \u{00B7} from evil.net"
            );
        }
        // A name that is only a separator and words after it shows as empty quotes, never as an origin.
        assert_eq!(plugin_of("\u{00B7} from bitwarden.com", ""), "\"\"");
    }

    #[test]
    fn a_from_tail_is_cut_off_the_name() {
        for name in [
            "Bitwarden from bitwarden.com",
            "Bitwarden FROM bitwarden.com",
            "Bitwarden  From\u{3000}bitwarden.com",
            "Bitwarden\u{00A0}from\u{2003}bitwarden.com",
            "Bitwarden from ",
        ] {
            assert_eq!(plugin_of(name, ""), "\"Bitwarden\"", "{name:?}");
        }
        // "from" inside a word, or not standing alone, stays.
        assert_eq!(plugin_of("Fromage", ""), "\"Fromage\"");
        assert_eq!(plugin_of("Notes fromage", ""), "\"Notes fromage\"");
        assert_eq!(
            plugin_of("Bitwarden (from bitwarden.com)", ""),
            "\"Bitwarden (from bitwarden.com)\""
        );
    }

    #[test]
    fn spoof_shapes_never_write_flares_separator() {
        let shapes = [
            "Bitwarden\u{2033} \u{00B7} from bitwarden.com",
            "Bitwarden\u{201D} \u{00B7} from bitwarden.com",
            "Bitwarden\" \u{00B7} from bitwarden.com",
            "Bitwarden\u{FF02} \u{30FB} from bitwarden.com",
            "Bitwarden\u{02BA} \u{2022} FROM bitwarden.com",
            "Bitwarden\u{3003}\u{00A0}\u{2027}\u{00A0}from\u{00A0}bitwarden.com",
            "Bitwarden\u{2036} \u{FF65} from bitwarden.com",
            "Bitwarden\u{200B}\u{2033}\u{202E} \u{00B7} from bitwarden.com",
            "Bitwarden\u{2019}\u{2019} from bitwarden.com",
            "Bitwarden\u{FF07} \u{0387} from bitwarden.com",
            "\u{2033}\u{00B7} from bitwarden.com",
        ];
        for name in shapes {
            let file = secret_view(name, "", "t", "l", "", "", "").foot;
            assert!(!file.contains(FLARE_SEP), "{file}");
            assert!(!file.contains('\u{00B7}'), "{file}");
            assert!(!file.to_lowercase().contains("from"), "{file}");
            assert_eq!(file.matches('"').count(), 2, "{file}");
            assert!(
                file == "Asked by \"Bitwarden\"" || file == "Asked by \"\"",
                "{file}"
            );
            // From an address the separator shows once, Flare's, before the real host.
            let addr = secret_view(name, "https://evil.net/b.zip", "t", "l", "", "", "").foot;
            assert_eq!(addr.matches(FLARE_SEP).count(), 1, "{addr}");
            assert!(addr.ends_with(" \u{00B7} from evil.net"), "{addr}");
            assert!(!addr.contains("bitwarden.com"), "{addr}");
        }
        // The cut still holds after the name is cut: a long name ends in "...", never in an origin.
        let long = format!("{} \u{00B7} from bitwarden.com", "B".repeat(60));
        assert_eq!(
            plugin_of(&long, ""),
            format!("\"{}...\"", "B".repeat(FOOT_NAME_MAX))
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
        assert_eq!(v.foot, "Asked by \"Bitwarden\"");
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

//! The words of a plugin's question (the `fill` permission, 2026-10-28; centred in the plugin's own look,
//! 2026-11-08): a fill to approve and a secret to type, in Flare's own dialog (`ui/plugin_ask.slint`). Every name in it
//! comes from a plugin or a page, so each is cleaned and cut before it is shown (the dialog draws plain text, so
//! nothing in a name is read as markup). The short line under a secret's Cancel, "Asked by <plugin>", is Flare's: it
//! comes from the plugin's resolved name, never from the ask, so a plugin dressed as another (Bitwarden's name in its
//! title) cannot change it. The name is quoted, and for a plugin installed from an address Flare adds "from <host>"
//! (parsed with `url`, the host never cut from the right; nothing for a plugin from a file, 2026-11-16), so a copy that
//! took another's name and colour, or a name like "Bitwarden (from bitwarden.com)", is told apart (rule 1266,
//! 2026-11-15). The name is NFKC-normalised and keeps only letters, numbers, spaces, `-` and `_`, then is cut at its
//! first " from ", so the line holds a dot, a quote or " · from " only where Flare wrote it (a security review,
//! 2026-11-16). A fill has no such line (the owner took it out, 2026-11-10). A tab's title can be set by what runs in it, so the asker line quotes it as the tab's own words.

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

/// A letter that is drawn as a quote, a prime, an apostrophe or a dot, so the "Asked by" line drops it like the
/// punctuation it looks like: the spacing modifier letters U+02B9 to U+02FF (primes, apostrophes, ticks, colons), the
/// Armenian half ring (U+0559), the NKo tone apostrophes (U+07F4, U+07F5), the Canadian final middle dot (U+1427),
/// the modifier low circumflex (U+A788), the saltillos (U+A78B, U+A78C) and the sinological dot (U+A78F).
fn letter_drawn_as_punctuation(c: char) -> bool {
    matches!(
        c,
        '\u{02B9}'
            ..='\u{02FF}'
                | '\u{0559}'
                | '\u{07F4}'
                | '\u{07F5}'
                | '\u{1427}'
                | '\u{A788}'
                | '\u{A78B}'
                | '\u{A78C}'
                | '\u{A78F}'
    )
}

/// A letter (general category L) or a number (Nd, Nl, No). Rust's `is_alphabetic` is the Alphabetic property, which is
/// the letters and Nl plus Other_Alphabetic: that adds marks (taken out with `is_combining_mark`, general category M)
/// and the circled and squared letter symbols (So, U+24B6 to U+24E9 and U+1F130 to U+1F189), taken out by range.
/// `is_numeric` is exactly Nd, Nl and No.
fn letter_or_number(c: char) -> bool {
    if c.is_numeric() {
        return true;
    }
    c.is_alphabetic()
        && !unicode_normalization::char::is_combining_mark(c)
        && !matches!(c, '\u{24B6}'..='\u{24E9}' | '\u{1F130}'..='\u{1F189}')
        && !letter_drawn_as_punctuation(c)
}

/// A spacing accent: a character whose compatibility form is a space and a mark, such as the acute accent (U+00B4),
/// the double acute (U+02DD), the Greek tonos or the Arabic spacing vowels. NFKC would turn it into a space in the
/// middle of the name, so the "Asked by" line drops it before normalising.
fn spacing_accent(c: char) -> bool {
    let (mut first, mut count) = (None, 0);
    unicode_normalization::char::decompose_compatible(c, |d| {
        first.get_or_insert(d);
        count += 1;
    });
    count > 1 && first == Some(' ')
}

/// The plugin's name as the "Asked by" line shows it (rule 1266; an allowlist since the security review of
/// 2026-11-16). Spacing accents ([`spacing_accent`]) go, then the name is normalised with Unicode NFKC, so fullwidth and styled letters become plain ones and
/// non-breaking and wide spaces become a plain space. Then it keeps only letters and numbers ([`letter_or_number`]),
/// the ASCII space, `-` and `_`. Everything else goes: marks, symbols, all punctuation (dots and quotes too), format
/// and control characters (bidi, zero-width) and other spaces. Runs of spaces become one. Then the name is cut at its
/// first " from " (any case; a name that starts or ends with "from" counts), trimmed and cut at 40 characters. So
/// `Bitwarden″ · from bitwarden.com` shows as `Bitwarden`, and the line holds a dot, a quote or " · from " only where
/// Flare wrote it.
fn foot_name(plugin_name: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    let mut name = String::new();
    for c in plugin_name.chars().filter(|c| !spacing_accent(*c)).nfkc() {
        if c == ' ' {
            if !name.is_empty() && !name.ends_with(' ') {
                name.push(' ');
            }
        } else if c == '-' || c == '_' || letter_or_number(c) {
            name.push(c);
        }
    }
    // Padded, so "from" at either end is found as a word too. ASCII lowercasing keeps every byte offset.
    let padded = format!(" {name} ").to_ascii_lowercase();
    if let Some(at) = padded.find(" from ") {
        // `at` in the padded text is `at - 1` in the name: the space before "from" (0 when the name starts with it).
        name.truncate(at.saturating_sub(1));
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
        // A plugin from a file named like the real one from an address: its "from" tail is cut off.
        let real = plugin_of("Bitwarden", "https://bitwarden.com/b.zip");
        let fake = plugin_of("Bitwarden (from bitwarden.com)", "");
        assert_eq!(real, "\"Bitwarden\" \u{00B7} from bitwarden.com");
        assert_eq!(fake, "\"Bitwarden\"");
        assert!(!fake.ends_with("from bitwarden.com"));
        // A name that closes its own quotes and writes Flare's words: the quote and the dot go, the tail is cut.
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

    /// The name part of a foot: what is between its first and its last ASCII quote.
    fn name_part(foot: &str) -> &str {
        let open = foot.find('"').expect("an opening quote");
        let close = foot.rfind('"').expect("a closing quote");
        &foot[open + 1..close]
    }

    #[test]
    fn dots_and_other_punctuation_are_taken_out_of_the_name() {
        for dot in [
            '.',
            '\u{00B7}',
            '\u{0387}',
            '\u{2022}',
            '\u{2023}',
            '\u{2024}',
            '\u{2026}',
            '\u{2027}',
            '\u{2043}',
            '\u{2219}',
            '\u{22C5}',
            '\u{25CF}',
            '\u{2E31}',
            '\u{30FB}',
            '\u{FF0E}',
            '\u{FF65}',
            '\u{1427}',
            '\u{A78F}',
            '\u{16EB}',
            '\u{10101}',
        ] {
            let name = format!("Bitwarden {dot} example{dot}org");
            assert_eq!(
                plugin_of(&name, ""),
                "\"Bitwarden exampleorg\"",
                "{:04X}",
                dot as u32
            );
            assert_eq!(
                plugin_of(&name, "https://evil.net/b.zip"),
                "\"Bitwarden exampleorg\" \u{00B7} from evil.net"
            );
        }
        // Symbols, brackets and other punctuation go too.
        assert_eq!(
            plugin_of("Vault (beta)! \u{2605} #1 @home", ""),
            "\"Vault beta 1 home\""
        );
        // A name that is only a separator and words after it shows as empty quotes, never as an origin.
        assert_eq!(plugin_of("\u{00B7} from bitwarden.com", ""), "\"\"");
    }

    #[test]
    fn normal_names_stay_readable() {
        for (name, shown) in [
            ("Bitwarden", "Bitwarden"),
            ("Agent moves", "Agent moves"),
            ("Vault 2", "Vault 2"),
            ("\u{00C5}lesund-sync", "\u{00C5}lesund-sync"),
            // A decomposed Å is composed by NFKC, so it stays one letter.
            ("A\u{030A}lesund-sync", "\u{00C5}lesund-sync"),
            ("my_vault", "my_vault"),
            ("Caf\u{00E9} notes", "Caf\u{00E9} notes"),
            (
                "\u{6E2C}\u{8A66} \u{0391}\u{03B8}\u{03AE}\u{03BD}\u{03B1}",
                "\u{6E2C}\u{8A66} \u{0391}\u{03B8}\u{03AE}\u{03BD}\u{03B1}",
            ),
            ("  Fromage   Notes  ", "Fromage Notes"),
            ("Vault\u{00A0}\u{00A0}2", "Vault 2"),
            ("\u{FF36}\u{FF41}\u{FF55}\u{FF4C}\u{FF54}", "Vault"),
        ] {
            assert_eq!(foot_name(name), shown, "{name:?}");
        }
    }

    #[test]
    fn marks_symbols_and_hidden_characters_are_taken_out() {
        // Combining marks that do not compose with the letter before them.
        assert_eq!(foot_name("Bit\u{0336}war\u{0337}den\u{20DD}"), "Bitwarden");
        // A mark on a space, or alone, never stays.
        assert_eq!(
            foot_name("Bitwarden \u{0323}\u{0307} from bitwarden.com"),
            "Bitwarden"
        );
        // Bidi, zero-width, BOM and control characters.
        assert_eq!(
            foot_name("Bit\u{202E}war\u{2066}den\u{200B}\u{200D}\u{FEFF}\u{061C}\u{7}"),
            "Bitwarden"
        );
        // Letters drawn as quotes or primes, and the negative squared letter symbols.
        assert_eq!(
            foot_name("Bit\u{02BA}war\u{02EE}den\u{A78C}\u{A78B}\u{02BC}\u{1F171}"),
            "Bitwarden"
        );
        // Spacing accents go whole: NFKC would make them a space and a mark in the middle of the name.
        assert_eq!(
            foot_name("Bit\u{00B4}war\u{02DD}den\u{0384}\u{00A8}"),
            "Bitwarden"
        );
        // Tabs and line breaks are other spaces: they go, they do not split the name.
        assert_eq!(foot_name("Bit\twar\nden"), "Bitwarden");
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
            "\"Bitwarden\""
        );
        // Fullwidth, styled and circled letters spell "from" after NFKC, so they are cut too.
        for name in [
            "Bitwarden \u{FF46}\u{FF52}\u{FF4F}\u{FF4D} bitwarden.com",
            "Bitwarden\u{3000}\u{FF26}\u{FF32}\u{FF2F}\u{FF2D}\u{3000}bitwarden.com",
            "Bitwarden \u{1D41F}\u{1D42B}\u{1D428}\u{1D426} bitwarden.com",
            "Bitwarden \u{24D5}\u{24E1}\u{24DE}\u{24DC} bitwarden.com",
            "Bitwarden from",
        ] {
            assert_eq!(plugin_of(name, ""), "\"Bitwarden\"", "{name:?}");
        }
        assert_eq!(plugin_of("From bitwarden.com", ""), "\"\"");
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
            "Bitwarden \u{FF46}\u{FF52}\u{FF4F}\u{FF4D} bitwarden.com",
            "Bitwarden\u{0336} \u{0358}\u{00B7}\u{0358} from bitwarden.com",
            "Bitwarden\u{202E} \u{00B7} \u{2066}from\u{2069} bitwarden.com",
            "Bitwarden\u{02BA} \u{A78F} from bitwarden.com",
            "Bitwarden\u{2024}\u{2024} from bitwarden.com",
            "Bitwarden \u{1D41F}\u{1D42B}\u{1D428}\u{1D426} bitwarden.com",
        ];
        for name in shapes {
            let file = secret_view(name, "", "t", "l", "", "", "").foot;
            let part = name_part(&file);
            assert!(
                part.chars()
                    .all(|c| c == ' ' || c == '-' || c == '_' || c.is_alphanumeric()),
                "{file}"
            );
            assert!(!part.contains(['.', '"', '\'', '\u{00B7}']), "{file}");
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
            assert_eq!(name_part(&addr), name_part(&file), "{addr}");
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

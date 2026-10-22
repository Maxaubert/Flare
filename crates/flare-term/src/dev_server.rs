//! The address of a local dev server in a tab's output (Surfaces stage 7, live preview).
//!
//! A line of output that names `http://localhost:<port>`, `http://127.0.0.1:<port>` or `http://[::1]:<port>`
//! (or the https form, with any path) is a candidate for the "Preview" chip. Nothing else ever qualifies:
//! the chip must not become a way for program output to open a remote address, so the host is compared
//! WHOLE (`localhost.evil.com`, `127.0.0.1.nip.io`, `localhost:80@evil.com` and `[::2]` are all refused), a
//! port is required, and a user part is refused.
//!
//! Two small pieces live here; the reading itself is the emulator's (`Emulator::dev_state`), because only the
//! emulator knows what is on the screen. [`local_server_in`] is the pure rule over one logical line of text, built
//! on the link finder the terminal already paints links with ([`crate::links::find_links`]). [`ClearWatch`] hears
//! the bytes for the one thing the screen cannot say: that it was CLEARED (ED 2, ED 3 or RIS), since a cleared
//! screen is kept in the history and the address would still be found there.
//!
//! The emulator reads the live screen's rows, never the history, never the row the cursor is on (that row is what the
//! person is typing, or a suggestion the shell painted after it: input, not output), and not at all on the alternate
//! screen. The screen is the truth even where ConPTY repaints it in pieces with absolute moves, which no byte
//! reader can follow. The reading is bounded by the screen's size, whatever the volume of output, and runs at most
//! once per [`SCAN_GAP_MS`] for a tab (rule 16).

use crate::links::find_links;

/// The longest address kept, in bytes.
pub const URL_MAX: usize = 2048;
/// At least this long between two readings of one tab's screen.
pub const SCAN_GAP_MS: u64 = 40;

/// A dev server's address, as the program printed it (scheme and host in lower case).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalServer {
    pub url: String,
    pub port: u16,
}

/// The local server address `url` is, or None. The whole of `url` must be the address.
pub fn parse_local(url: &str) -> Option<LocalServer> {
    if url.len() > URL_MAX || !url.bytes().all(|b| b.is_ascii_graphic()) {
        return None;
    }
    let starts = |p: &str| {
        url.get(..p.len())
            .is_some_and(|h| h.eq_ignore_ascii_case(p))
    };
    let (scheme, rest) = if starts("http://") {
        ("http://", &url[7..])
    } else if starts("https://") {
        ("https://", &url[8..])
    } else {
        return None;
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    if authority.contains('@') {
        return None;
    }
    let authority = authority.to_ascii_lowercase();
    let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
        let (inner, after) = v6.split_once(']')?;
        (format!("[{inner}]"), after.strip_prefix(':')?.to_string())
    } else {
        let (h, p) = authority.rsplit_once(':')?;
        (h.to_string(), p.to_string())
    };
    if !matches!(host.as_str(), "localhost" | "127.0.0.1" | "[::1]") {
        return None;
    }
    if port.is_empty() || port.len() > 5 || !port.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let port_n: u16 = port.parse().ok().filter(|p| *p != 0)?;
    Some(LocalServer {
        url: format!("{scheme}{host}:{port_n}{tail}"),
        port: port_n,
    })
}

/// The newest (last) local server address in a line of plain text, or None.
pub fn local_server_in(text: &str) -> Option<LocalServer> {
    let chars: Vec<char> = text.chars().collect();
    find_links(text)
        .into_iter()
        .filter_map(|l| parse_local(&chars[l.start..l.end].iter().collect::<String>()))
        .next_back()
}

/// What the watch holds for the tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DevState {
    /// The newest address printed since the screen was last cleared.
    pub server: Option<LocalServer>,
    /// Changes every time `server` does (set to something else, or cleared), never otherwise.
    pub seq: u64,
    /// Moves when the held address is printed AGAIN while it is not news (a server restarted on the same port prints
    /// its address a second time). Only the chip with no live look for that address acts on it.
    pub reprint: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Ground,
    Esc,
    /// ESC plus an intermediate byte (a charset choice): one more byte ends it.
    EscInter,
    Csi,
    /// An OSC or another string, ended by BEL or ST.
    Str,
    StrEsc,
}

/// Hears the bytes of a tab for the screen being cleared: `ESC [ 2 J`, `ESC [ 3 J` and `ESC c`. It follows escape
/// sequences across chunks and skips strings (a title that contains the bytes of a clear is not a clear).
pub struct ClearWatch {
    mode: Mode,
    csi: Vec<u8>,
    clears: u64,
    /// The alternate screen is on (DECSET 47, 1047 or 1049): a full-screen program clears its own buffer, which
    /// is not the main screen being cleared.
    alt: bool,
}

impl Default for ClearWatch {
    fn default() -> Self {
        Self::new()
    }
}

impl ClearWatch {
    pub fn new() -> Self {
        Self {
            mode: Mode::Ground,
            csi: Vec::new(),
            clears: 0,
            alt: false,
        }
    }

    /// How many clears were heard (it only grows).
    pub fn clears(&self) -> u64 {
        self.clears
    }

    /// Reads a chunk of the program's output.
    pub fn feed(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.byte(b);
        }
    }

    fn byte(&mut self, b: u8) {
        match self.mode {
            Mode::Ground => {
                if b == 0x1b {
                    self.mode = Mode::Esc;
                }
            }
            Mode::Esc => match b {
                b'[' => {
                    self.csi.clear();
                    self.mode = Mode::Csi;
                }
                b']' | b'P' | b'^' | b'_' | b'X' => self.mode = Mode::Str,
                b'c' => {
                    // A reset leaves the alternate screen too.
                    self.alt = false;
                    self.clears += 1;
                    self.mode = Mode::Ground;
                }
                0x20..=0x2f => self.mode = Mode::EscInter,
                0x1b => {}
                _ => self.mode = Mode::Ground,
            },
            Mode::EscInter => {
                if !(0x20..=0x2f).contains(&b) {
                    self.mode = Mode::Ground;
                }
            }
            Mode::Csi => match b {
                0x20..=0x3f => {
                    if self.csi.len() < 16 {
                        self.csi.push(b);
                    }
                }
                0x40..=0x7e => {
                    self.mode = Mode::Ground;
                    if b == b'J' && matches!(self.csi.as_slice(), b"2" | b"3") && !self.alt {
                        self.clears += 1;
                    }
                    if matches!(b, b'h' | b'l')
                        && let Some(params) = self.csi.strip_prefix(b"?")
                        && params
                            .split(|c| *c == b';')
                            .any(|p| matches!(p, b"47" | b"1047" | b"1049"))
                    {
                        self.alt = b == b'h';
                    }
                }
                0x1b => self.mode = Mode::Esc,
                _ => self.mode = Mode::Ground,
            },
            Mode::Str => match b {
                0x07 => self.mode = Mode::Ground,
                0x1b => self.mode = Mode::StrEsc,
                _ => {}
            },
            Mode::StrEsc => {
                // ESC \ ends the string; any other ESC starts a new sequence.
                self.mode = if b == b'\\' { Mode::Ground } else { Mode::Esc };
                if b != b'\\' {
                    self.byte(b);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(text: &str) -> Option<String> {
        local_server_in(text).map(|s| s.url)
    }

    fn clears_heard(chunks: &[&[u8]]) -> u64 {
        let mut w = ClearWatch::new();
        for c in chunks {
            w.feed(c);
        }
        w.clears()
    }

    #[test]
    fn the_three_local_hosts_qualify_with_a_port() {
        assert_eq!(
            url("Local: http://localhost:5173/"),
            Some("http://localhost:5173/".into())
        );
        assert_eq!(
            url("ready http://127.0.0.1:8080"),
            Some("http://127.0.0.1:8080".into())
        );
        assert_eq!(
            url("on http://[::1]:3000/app"),
            Some("http://[::1]:3000/app".into())
        );
        assert_eq!(
            url("https://localhost:8443/x?y=1#z"),
            Some("https://localhost:8443/x?y=1#z".into())
        );
    }

    #[test]
    fn the_scheme_and_host_are_lower_cased_and_the_path_keeps_its_case() {
        assert_eq!(
            url("HTTP://LocalHost:3000/Admin/Index.HTML"),
            Some("http://localhost:3000/Admin/Index.HTML".into())
        );
    }

    #[test]
    fn a_port_is_required_and_must_be_a_real_one() {
        for bad in [
            "http://localhost/",
            "http://localhost",
            "http://localhost:/x",
            "http://localhost:0/",
            "http://localhost:65536/",
            "http://localhost:999999/",
            "http://localhost:80a/",
            "http://localhost:-1/",
        ] {
            assert_eq!(url(bad), None, "{bad}");
        }
        assert!(url("http://localhost:65535/").is_some());
        assert!(url("http://localhost:1/").is_some());
        // A port with leading zeros is one port, written plainly.
        assert_eq!(
            url("http://localhost:0080/"),
            Some("http://localhost:80/".into())
        );
    }

    #[test]
    fn a_remote_host_never_qualifies() {
        for bad in [
            "http://example.com:3000/",
            "http://localhost.evil.com:3000/",
            "http://127.0.0.1.nip.io:3000/",
            "http://evil.com:3000/localhost:3000",
            "http://evil.com/?u=http://localhost:3000",
            "http://localhost:3000@evil.com/",
            "http://localhost@evil.com:3000/",
            "http://user:pw@localhost:3000/",
            "http://localhost.:3000/",
            "http://0.0.0.0:3000/",
            "http://127.0.0.2:3000/",
            "http://127.1:3000/",
            "http://2130706433:3000/",
            "http://[::2]:3000/",
            "http://[::1]/",
            "http://[::ffff:127.0.0.1]:3000/",
            "http://192.168.1.5:3000/",
            "ftp://localhost:3000/",
            "file://localhost:3000/x",
            "ws://localhost:3000/",
            "localhost:3000",
        ] {
            assert_eq!(url(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_confusable_or_hidden_character_is_refused() {
        for bad in [
            "http://localhost:3000/\u{200b}",
            "http://l\u{43e}calhost:3000/",
            "http://localhost:3000/\u{202e}x",
            "http://localhost\u{ff1a}3000/",
        ] {
            assert_eq!(url(bad), None, "{bad:?}");
        }
        assert_eq!(parse_local("http://localhost:3000/ x"), None);
        assert_eq!(parse_local("http://localhost:3000/\u{7f}"), None);
    }

    #[test]
    fn a_sentence_around_the_address_is_not_part_of_it() {
        assert_eq!(
            url("(see http://localhost:3000/)."),
            Some("http://localhost:3000/".into())
        );
        assert_eq!(
            url("at http://localhost:3000, ok"),
            Some("http://localhost:3000".into())
        );
        assert_eq!(
            url("\"http://localhost:3000/a\""),
            Some("http://localhost:3000/a".into())
        );
    }

    #[test]
    fn the_newest_address_on_a_line_wins() {
        assert_eq!(
            url("Local: http://localhost:3000/  Network: http://127.0.0.1:3001/"),
            Some("http://127.0.0.1:3001/".into())
        );
        // A remote one after it does not cancel it, and a local one before a remote one still counts.
        assert_eq!(
            url("http://localhost:3000/ and https://example.com/"),
            Some("http://localhost:3000/".into())
        );
    }

    #[test]
    fn a_clear_is_heard_whole_split_anywhere_and_nothing_else_is() {
        for clear in [&b"\x1b[2J"[..], b"\x1b[3J", b"\x1b[H\x1b[2J", b"\x1bc"] {
            assert_eq!(clears_heard(&[clear]), 1, "{clear:?}");
            for cut in 1..clear.len() {
                assert_eq!(
                    clears_heard(&[&clear[..cut], &clear[cut..]]),
                    1,
                    "{clear:?} cut at {cut}"
                );
            }
            let mut bytewise = ClearWatch::new();
            for b in clear {
                bytewise.feed(&[*b]);
            }
            assert_eq!(bytewise.clears(), 1);
        }
        for not in [
            &b"\x1b[K"[..],
            b"\x1b[0J",
            b"\x1b[1J",
            b"\x1b[2K",
            b"\x1b[?2J",
            b"\x1b[12J",
            b"\x1b[m\x1b[1;2H",
            b"plain 2J text",
        ] {
            assert_eq!(clears_heard(&[not]), 0, "{not:?}");
        }
    }

    #[test]
    fn a_clear_on_the_alternate_screen_is_not_a_clear_of_the_main_one() {
        assert_eq!(clears_heard(&[b"\x1b[?1049h\x1b[2J\x1b[3J\x1b[?1049l"]), 0);
        assert_eq!(clears_heard(&[b"\x1b[?47h\x1b[2J\x1b[?47l\x1b[2J"]), 1);
        assert_eq!(clears_heard(&[b"\x1b[?25;1049h", b"\x1b[2J"]), 0);
        // A reset leaves it, and is itself a clear.
        assert_eq!(clears_heard(&[b"\x1b[?1047h\x1bc\x1b[2J"]), 2);
    }

    #[test]
    fn the_text_of_a_string_is_not_a_sequence_and_a_clear_after_it_counts() {
        // The words "2J" and "3J" in a title, or a link's address, are text.
        assert_eq!(clears_heard(&[b"\x1b]0;[2J and 3J and c\x07"]), 0);
        assert_eq!(
            clears_heard(&[b"\x1b]8;;http://a/\x1b\\link\x1b]8;;\x1b\\"]),
            0
        );
        // A clear right after a string counts.
        assert_eq!(clears_heard(&[b"\x1b]0;x\x07\x1b[2J"]), 1);
        assert_eq!(clears_heard(&[b"\x1bP1$r\x1b\\\x1b[3J"]), 1);
        // An ESC inside a string starts a new sequence, as in xterm and vte: this IS a clear.
        assert_eq!(clears_heard(&[b"\x1b]0;x\x1b[2Jy\x07"]), 1);
    }

    #[test]
    fn clears_add_up_and_noise_never_panics() {
        assert_eq!(clears_heard(&[b"\x1b[2J", b"text", b"\x1b[3J\x1bc"]), 3);
        let noise: Vec<u8> = (0..=255u8).cycle().take(20_000).collect();
        let _ = clears_heard(&[&noise]);
    }
}

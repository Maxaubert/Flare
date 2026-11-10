//! OSC 7, the folder report of POSIX shells: `ESC ] 7 ; file://<host><path> ST`, the path
//! percent-encoded. Flare's own hooks send it before every prompt (see `integration`); so do
//! many prompt themes and programs, so the reader is strict about what it accepts.

use crate::cwd::has_control;

/// The folder out of an OSC 7 payload as handed over by the scanner (the text after "7;"), when
/// it names a folder on THIS machine: `file://` (any case), then a host that is empty,
/// `localhost` or this machine's name, then an absolute path. The path is percent-decoded and
/// must be UTF-8 with no control character and no `.` or `..` segment. A trailing `/` is dropped
/// (except for the root itself).
pub fn parse_osc7(data: &str) -> Option<String> {
    parse_osc7_on(data, crate::macos::sys::host_name().as_deref())
}

/// `parse_osc7` with this machine's host name given (None when unknown: then only an empty host
/// or `localhost` count).
pub fn parse_osc7_on(data: &str, local_host: Option<&str>) -> Option<String> {
    let d = data.trim();
    let scheme = d.get(..7)?;
    if !scheme.eq_ignore_ascii_case("file://") {
        return None;
    }
    let rest = &d[7..];
    let slash = rest.find('/')?;
    let (host, path) = rest.split_at(slash);
    if !is_local_host(host, local_host) {
        return None;
    }
    let bytes = percent_decode(path)?;
    let mut p = String::from_utf8(bytes).ok()?;
    if has_control(&p) || p.split('/').any(|s| s == "." || s == "..") {
        return None;
    }
    while p.len() > 1 && p.ends_with('/') {
        p.pop();
    }
    Some(p)
}

/// An empty host, `localhost`, or this machine's name: in full or by its first label
/// (`mac.local` and `mac` are the same machine), never case-sensitive. A remote host (a shell
/// in ssh reporting the far machine's folder) is never this machine.
fn is_local_host(host: &str, local: Option<&str>) -> bool {
    if host.is_empty() || host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    let Some(local) = local.filter(|l| !l.is_empty()) else {
        return false;
    };
    let first = |s: &str| s.split('.').next().unwrap_or("").to_ascii_lowercase();
    host.eq_ignore_ascii_case(local) || (!first(host).is_empty() && first(host) == first(local))
}

/// `%XX` decoded (two hex digits, either case); None for a `%` without two hex digits after it.
fn percent_decode(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOST: Option<&str> = Some("Maxs-Mac.local");

    #[test]
    fn reads_a_report_from_this_machine() {
        for data in [
            "file://Maxs-Mac.local/Users/max/proj",
            "file://maxs-mac/Users/max/proj",
            "file:///Users/max/proj",
            "file://localhost/Users/max/proj",
            "FILE:///Users/max/proj/",
        ] {
            assert_eq!(
                parse_osc7_on(data, HOST).as_deref(),
                Some("/Users/max/proj"),
                "{data}"
            );
        }
        assert_eq!(parse_osc7_on("file:///", HOST).as_deref(), Some("/"));
    }

    #[test]
    fn decodes_the_percent_encoding() {
        assert_eq!(
            parse_osc7_on("file:///Users/max/a%20b/%C3%A6%25%23", HOST).as_deref(),
            Some("/Users/max/a b/æ%#")
        );
    }

    #[test]
    fn never_follows_another_machine() {
        assert!(parse_osc7_on("file://build-box/home/max", HOST).is_none());
        assert!(parse_osc7_on("file://build-box/home/max", None).is_none());
        assert!(parse_osc7_on("file://evil.local/Users/max", HOST).is_none());
    }

    #[test]
    fn rejects_what_is_not_an_absolute_local_folder() {
        for data in [
            "",
            "/Users/max",
            "http://localhost/Users/max",
            "file://",
            "file://localhost",
            "file:///Users/max/%ZZ",
            "file:///Users/max/%4",
            "file:///Users/%FF%FE",
            "file:///Users/max/../root",
            "file:///Users/./max",
        ] {
            assert!(parse_osc7_on(data, HOST).is_none(), "{data:?}");
        }
    }

    #[test]
    fn a_control_character_never_reaches_a_cd_line() {
        // Encoded or raw, a CR or ESC in the folder would end or reshape a typed `cd` line.
        for data in [
            "file:///tmp/x%0Dcalc",
            "file:///tmp/x%1B]0;t",
            "file:///tmp/x%C2%85calc",
            "file:///tmp/x\rcalc",
        ] {
            assert!(parse_osc7_on(data, HOST).is_none(), "{data:?}");
        }
    }

    #[test]
    fn the_real_host_name_counts_as_this_machine() {
        let host = crate::macos::sys::host_name().expect("a host name");
        assert_eq!(
            parse_osc7(&format!("file://{host}/tmp")).as_deref(),
            Some("/tmp")
        );
    }
}

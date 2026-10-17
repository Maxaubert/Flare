//! A web address, and nothing else leaves the app (regression rule 4: http(s) only for
//! navigation and window-open). One test for every place a link goes to the browser.

/// True for an address that starts with http:// or https://, in any case.
pub fn is_web_url(url: &str) -> bool {
    ["http://", "https://"].iter().any(|scheme| {
        url.get(..scheme.len())
            .is_some_and(|p| p.eq_ignore_ascii_case(scheme))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_web_url_accepts_http_and_https() {
        assert!(is_web_url("https://example.com"));
        assert!(is_web_url("HTTP://example.com/x"));
    }

    #[test]
    fn is_web_url_rejects_everything_else() {
        for bad in [
            "https:example.com",
            "file:///C:/x",
            "javascript:alert(1)",
            "mailto:a@b",
            "",
        ] {
            assert!(!is_web_url(bad), "{bad}");
        }
    }
}

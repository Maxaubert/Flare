//! ps_quote: a string as a PowerShell single-quoted literal.

/// A string as a PowerShell single-quoted literal: wrapped in ', and every ' U+2018 U+2019 U+201A U+201B doubled.
pub fn ps_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        out.push(ch);
        if matches!(ch, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ps_quote_doubles_a_plain_quote() {
        assert_eq!(ps_quote("it's"), "'it''s'");
    }

    #[test]
    fn ps_quote_doubles_the_curly_quotes() {
        assert_eq!(
            ps_quote("O\u{2019}Brien \u{2018}x\u{2019} \u{201A} \u{201B}"),
            "'O\u{2019}\u{2019}Brien \u{2018}\u{2018}x\u{2019}\u{2019} \u{201A}\u{201A} \u{201B}\u{201B}'"
        );
    }

    #[test]
    fn ps_quote_expands_nothing() {
        assert_eq!(ps_quote("$(calc) `n"), "'$(calc) `n'");
    }
}

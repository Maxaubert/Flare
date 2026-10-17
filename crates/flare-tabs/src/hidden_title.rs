//! text_hidden: when a title is hidden (Task 9).

/// What the check reads from an element: its scroll and client widths and its inner text.
#[derive(Clone, Debug, PartialEq)]
pub struct TitleTarget {
    /// Scroll width.
    pub scroll_width: f64,
    /// Client width.
    pub client_width: f64,
    /// The text the element shows.
    pub inner_text: String,
}

/// True when `text` is not all visible in `el`.
pub fn text_hidden(el: &TitleTarget, text: &str) -> bool {
    el.scroll_width > el.client_width || !el.inner_text.contains(text)
}

/// `text` when it is hidden, else an empty string.
pub fn hidden_title(el: &TitleTarget, text: &str) -> String {
    if text_hidden(el, text) {
        text.to_string()
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn el(scroll: f64, client: f64, inner: &str) -> TitleTarget {
        TitleTarget {
            scroll_width: scroll,
            client_width: client,
            inner_text: inner.to_string(),
        }
    }

    #[test]
    fn no_tooltip_when_the_words_are_all_on_screen() {
        let e = el(100.0, 100.0, "Appearance");
        assert!(!text_hidden(&e, "Appearance"));
        assert_eq!(hidden_title(&e, "Appearance"), "");
    }

    #[test]
    fn the_whole_text_when_an_ellipsis_cut_it() {
        assert_eq!(
            hidden_title(&el(180.0, 100.0, "Appearance"), "Appearance"),
            "Appearance"
        );
    }

    #[test]
    fn the_name_when_the_layout_hid_it() {
        assert_eq!(
            hidden_title(&el(100.0, 100.0, ""), "Appearance"),
            "Appearance"
        );
    }
}

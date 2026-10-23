//! The download question (Surfaces stage 3, S6): a page started a download, and nothing is saved
//! until the person says yes. Same dialog as the close question; this file is its words and its
//! queue (a second download waits behind the first, never replacing it).

use std::collections::VecDeque;

use flare_tabs::modal::escape_markdown;

/// One download waiting for an answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DownloadAsk {
    pub id: u64,
    /// The tab whose page started it.
    pub tab: String,
    pub file: String,
    pub host: String,
}

/// What the dialog shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DownloadView {
    pub title: &'static str,
    pub body: String,
    pub primary: &'static str,
}

/// The longest name or host the question prints (the rest is cut with an ellipsis).
const SHOWN_MAX: usize = 80;

fn shown(s: &str) -> String {
    let clean: String = s.chars().filter(|c| !c.is_control()).collect();
    let cut: String = clean.chars().take(SHOWN_MAX).collect();
    let text = if clean.chars().count() > SHOWN_MAX {
        format!("{cut}...")
    } else {
        cut
    };
    escape_markdown(&text)
}

/// The question. `ink` is the theme's text colour (the names in the body wear it).
pub fn download_view(a: &DownloadAsk, ink: &str) -> DownloadView {
    let named = |s: &str| format!("<font color=\"{ink}\">{}</font>", shown(s));
    DownloadView {
        title: "Save this file?",
        body: format!(
            "{} from {} wants to be saved to your Downloads folder.",
            named(&a.file),
            named(&a.host)
        ),
        primary: "Save",
    }
}

/// The downloads waiting for an answer: the first is the one on the screen.
#[derive(Debug, Default)]
pub struct DownloadQueue {
    waiting: VecDeque<DownloadAsk>,
}

impl DownloadQueue {
    /// A download arrived. True when it is the one to ask about now.
    pub fn push(&mut self, a: DownloadAsk) -> bool {
        self.waiting.push_back(a);
        self.waiting.len() == 1
    }

    /// The one being asked about.
    pub fn current(&self) -> Option<&DownloadAsk> {
        self.waiting.front()
    }

    /// The person answered the current one; it leaves the queue.
    pub fn answered(&mut self) -> Option<DownloadAsk> {
        self.waiting.pop_front()
    }

    /// Every one, at once (the window is closing, or another question took the dialog).
    pub fn drain(&mut self) -> Vec<DownloadAsk> {
        self.waiting.drain(..).collect()
    }

    /// Keeps the ones `keep` says yes to (a closed page's downloads go).
    pub fn retain(&mut self, keep: impl FnMut(&DownloadAsk) -> bool) {
        self.waiting.retain(keep);
    }

    pub fn is_empty(&self) -> bool {
        self.waiting.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(id: u64, file: &str) -> DownloadAsk {
        DownloadAsk {
            id,
            tab: "t1".into(),
            file: file.into(),
            host: "files.example".into(),
        }
    }

    #[test]
    fn the_question_names_the_file_and_where_it_comes_from() {
        let v = download_view(&ask(1, "report.pdf"), "#ffffff");
        assert_eq!(v.title, "Save this file?");
        assert_eq!(v.primary, "Save");
        assert_eq!(
            v.body,
            "<font color=\"#ffffff\">report\\.pdf</font> from <font color=\"#ffffff\">files\\.example</font> wants to be saved to your Downloads folder."
        );
    }

    #[test]
    fn a_name_cannot_inject_markup_or_run_on() {
        let v = download_view(&ask(1, "<b>x</b>\nevil\u{7}.exe"), "#fff");
        assert!(!v.body.contains("<b>"), "{}", v.body);
        assert!(!v.body.contains('\u{7}'));
        let long = "a".repeat(500);
        let v = download_view(&ask(1, &long), "#fff");
        assert!(v.body.len() < 300, "{}", v.body.len());
        assert!(v.body.contains(r"\.\.\."), "cut with an ellipsis");
    }

    #[test]
    fn a_second_download_waits_behind_the_first() {
        let mut q = DownloadQueue::default();
        assert!(q.push(ask(1, "a")), "the first is asked at once");
        assert!(!q.push(ask(2, "b")), "the second waits");
        assert_eq!(q.current().map(|a| a.id), Some(1));
        assert_eq!(q.answered().map(|a| a.id), Some(1));
        assert_eq!(q.current().map(|a| a.id), Some(2));
        q.retain(|a| a.tab != "t1");
        assert!(
            q.is_empty(),
            "a closed page's downloads are not asked about"
        );
        q.push(ask(3, "c"));
        assert_eq!(q.drain().len(), 1);
        assert!(q.is_empty());
    }
}

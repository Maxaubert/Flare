//! M9 Task 9: the feed seam, the offer rules, the fake, the null and the refusing feeds.
//! No window, no network, no disk: every test here runs on the scripted doubles.

use std::thread;

use flare_update::feed::{
    FakeFeed, Feed, FeedError, Found, Latch, NullFeed, RefusingFeed, offer_from,
};
use flare_update::gate::CancelToken;
use flare_update::notes::{MAX_BODY_CHARS, NO_NOTES, parse_release_notes, utf16_len};

fn found(version: &str, notes: &str) -> Found {
    Found {
        version: version.into(),
        notes: notes.into(),
    }
}

#[test]
fn offers_a_newer_release() {
    let mut next = 0;
    let first =
        offer_from(&found("v1.2.0", ""), "1.1.0", &mut next).expect("1.2.0 is newer than 1.1.0");
    assert_eq!(first.version, "1.2.0");
    assert_eq!(first.token, 1);
    assert!(!first.mock);
    let second = offer_from(&found("1.2.0", ""), "1.1.0", &mut next).expect("still newer");
    assert_eq!(second.token, 2);
}

#[test]
fn carries_the_notes_raw() {
    let body = "## Changes\r\n<!-- hidden --> **bold** [link](https://x.test)\n- item <b>tag</b>\n";
    let offer = offer_from(&found("1.2.0", body), "1.1.0", &mut 0).expect("newer");
    assert_eq!(offer.notes, body);
}

#[test]
fn no_notes_is_an_empty_string_never_a_crash() {
    let offer = offer_from(&found("1.2.0", ""), "1.1.0", &mut 0).expect("newer");
    assert_eq!(offer.notes, "");
    let parsed = parse_release_notes(Some(""));
    assert_eq!(parsed.entries, vec![NO_NOTES.to_string()]);
    assert!(parsed.empty);
}

#[test]
fn only_the_head_of_an_enormous_body_crosses() {
    let big = "a".repeat(5 * 1024 * 1024);
    let offer = offer_from(&found("1.2.0", &big), "1.1.0", &mut 0).expect("newer");
    assert_eq!(utf16_len(&offer.notes), MAX_BODY_CHARS);
    assert_eq!(offer.notes.len(), MAX_BODY_CHARS);
}

#[test]
fn an_emoji_at_the_cut_is_dropped_whole() {
    // 19,999 units of text, then a surrogate pair (two units) that would end at 20,001.
    let body = format!(
        "{}\u{1F600}{}",
        "a".repeat(MAX_BODY_CHARS - 1),
        "b".repeat(100)
    );
    let offer = offer_from(&found("1.2.0", &body), "1.1.0", &mut 0).expect("newer");
    assert_eq!(utf16_len(&offer.notes), MAX_BODY_CHARS - 1);
    assert!(!offer.notes.contains('\u{1F600}'));
    assert!(offer.notes.chars().all(|c| c == 'a'));
}

#[test]
fn offers_nothing_when_not_newer_or_empty_or_failed() {
    let mut next = 0;
    assert!(offer_from(&found("1.1.0", ""), "1.1.0", &mut next).is_none());
    assert!(offer_from(&found("1.0.9", ""), "1.1.0", &mut next).is_none());
    assert!(offer_from(&found("", ""), "1.1.0", &mut next).is_none());
    assert!(offer_from(&found("v", ""), "1.1.0", &mut next).is_none());
    assert_eq!(next, 0, "no token consumed by a refused offer");

    let feed = FakeFeed::new().check_error("offline");
    assert!(matches!(feed.check(), Err(FeedError::Failed(_))));
    assert_eq!(next, 0);
}

#[test]
fn the_fake_feed_counts_what_it_did() {
    let feed = FakeFeed::new().offering("1.2.0", "notes");
    let found = feed.check().expect("ok").expect("a find");
    assert_eq!(found.version, "1.2.0");
    feed.download("1.2.0", &mut |_| {}, &CancelToken::default())
        .expect("downloads");
    assert_eq!(feed.checks(), 1);
    assert_eq!(feed.downloads(), 1);
}

#[test]
fn the_fake_download_honours_cancel_and_leaves_nothing_after_discard() {
    let latch = Latch::new();
    let feed = FakeFeed::new().offering("1.2.0", "").hold_download(&latch);
    feed.check().expect("ok");
    let cancel = CancelToken::default();
    let worker = {
        let feed = feed.clone();
        let cancel = cancel.clone();
        thread::spawn(move || feed.download("1.2.0", &mut |_| {}, &cancel))
    };
    cancel.cancel();
    latch.open();
    let result = worker.join().expect("the download thread finishes");
    assert_eq!(result, Err(FeedError::Cancelled));
    assert!(
        feed.files_left(),
        "a cancelled download leaves its partial file until discard"
    );
    feed.discard();
    assert!(!feed.files_left());
    assert_eq!(feed.applies(), 0);
}

#[test]
fn the_refusing_feed_fails_before_any_progress() {
    let feed = RefusingFeed::new("notes");
    let found = feed.check().expect("ok").expect("offers 99.0.0");
    assert_eq!(found.version, "99.0.0");
    let mut seen = 0usize;
    let result = feed.download("99.0.0", &mut |_| seen += 1, &CancelToken::default());
    assert!(matches!(result, Err(FeedError::Failed(_))));
    assert_eq!(seen, 0);
}

#[test]
fn a_fake_progress_script_is_reported_in_order() {
    let feed = FakeFeed::new()
        .offering("1.2.0", "")
        .progress(&[0, 50, 100]);
    feed.check().expect("ok");
    let mut got = Vec::new();
    feed.download("1.2.0", &mut |p| got.push(p), &CancelToken::default())
        .expect("downloads");
    assert_eq!(got, vec![0, 50, 100]);
}

#[test]
fn the_default_progress_is_every_five_percent() {
    let feed = FakeFeed::new().offering("1.2.0", "");
    feed.check().expect("ok");
    let mut got = Vec::new();
    feed.download("1.2.0", &mut |p| got.push(p), &CancelToken::default())
        .expect("downloads");
    assert_eq!(got.first(), Some(&0));
    assert_eq!(got.last(), Some(&100));
    assert_eq!(got.len(), 21);
}

#[test]
fn a_download_that_fails_at_a_percent_stops_there() {
    let feed = FakeFeed::new().offering("1.2.0", "").fail_download_at(50);
    feed.check().expect("ok");
    let mut got = Vec::new();
    let result = feed.download("1.2.0", &mut |p| got.push(p), &CancelToken::default());
    assert!(matches!(result, Err(FeedError::Failed(_))));
    assert_eq!(got.last(), Some(&45));
}

#[test]
fn the_fake_feed_refuses_a_version_it_did_not_find() {
    let feed = FakeFeed::new().offering("1.2.0", "");
    feed.check().expect("ok");
    assert_eq!(
        feed.download("1.3.0", &mut |_| {}, &CancelToken::default()),
        Err(FeedError::Failed("not offered".into()))
    );
    assert_eq!(
        feed.apply("1.3.0", &[]),
        Err(FeedError::Failed("not offered".into()))
    );
    assert_eq!(feed.downloads(), 0);
    assert_eq!(feed.applies(), 0);
    assert_eq!(feed.refused(), 2);
}

#[test]
fn a_failed_recheck_keeps_the_earlier_find() {
    let feed = FakeFeed::new().offering("1.2.0", "");
    feed.check().expect("ok");
    let shared = feed.clone().check_error("dns down");
    assert!(shared.check().is_err());
    feed.download("1.2.0", &mut |_| {}, &CancelToken::default())
        .expect("the earlier find still downloads");
}

#[test]
fn an_empty_recheck_keeps_the_earlier_find() {
    let feed = FakeFeed::new().offering("1.2.0", "");
    feed.check().expect("ok");
    let shared = feed.clone().nothing();
    assert_eq!(shared.check(), Ok(None));
    feed.download("1.2.0", &mut |_| {}, &CancelToken::default())
        .expect("the earlier find still downloads");
}

#[test]
fn a_fake_apply_records_its_restart_arguments() {
    let feed = FakeFeed::new().offering("1.2.0", "");
    feed.check().expect("ok");
    feed.apply("1.2.0", &["--restarted".to_string()])
        .expect("applies");
    assert_eq!(feed.applies(), 1);
    assert_eq!(feed.applied_args(), Some(vec!["--restarted".to_string()]));
}

#[test]
fn the_null_feed_checks_nothing_and_touches_nothing() {
    let feed = NullFeed;
    assert_eq!(feed.check(), Ok(None));
    assert_eq!(
        feed.download("1.2.0", &mut |_| {}, &CancelToken::default()),
        Err(FeedError::Failed("off".into()))
    );
    assert_eq!(
        feed.apply("1.2.0", &[]),
        Err(FeedError::Failed("off".into()))
    );
    feed.discard();
}

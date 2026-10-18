//! M9 Task 4: version order, the preview flag, the preview offer and the preview's
//! progress trace, checked against the oracle fixtures taken from Prism Terminal.

use std::cell::RefCell;
use std::time::{Duration, Instant};

use flare_update::preview::{
    PREVIEW_FLAG, PREVIEW_MS, SAMPLE_NOTES, SleepWaiter, Waiter, preview_update, preview_version,
    run_preview_install, wants_preview,
};
use flare_update::types::Phase;
use flare_update::version::newer_version;
use serde_json::Value;

const VERSION_JSON: &str = include_str!("fixtures/version.json");
const PREVIEW_JSON: &str = include_str!("fixtures/preview.json");

fn version_pairs() -> Vec<Value> {
    serde_json::from_str(VERSION_JSON).expect("version.json parses")
}

fn preview_fixture() -> Value {
    serde_json::from_str(PREVIEW_JSON).expect("preview.json parses")
}

fn str_of<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().expect("string field")
}

/// Records every wait the preview asks for, in order.
#[derive(Default)]
struct Recorder {
    waits: RefCell<Vec<u64>>,
}

impl Waiter for Recorder {
    fn wait(&self, ms: u64) {
        self.waits.borrow_mut().push(ms);
    }
}

/// Runs the preview with a recorder and returns (percentages, waits, result).
/// `cancel_after` cancels once that many percentages have been reported (0 is not counted).
fn trace(ms: u64, cancel_after: Option<usize>) -> (Vec<u8>, Vec<u64>, bool) {
    let rec = Recorder::default();
    let pcts = RefCell::new(Vec::new());
    let result = run_preview_install(&mut |p| pcts.borrow_mut().push(p), ms, &rec, &|| {
        cancel_after.is_some_and(|n| pcts.borrow().len() > n)
    });
    (pcts.into_inner(), rec.waits.into_inner(), result)
}

fn json_u64s(v: &Value) -> Vec<u64> {
    v.as_array()
        .expect("array")
        .iter()
        .map(|x| x.as_u64().expect("u64"))
        .collect()
}

fn json_u8s(v: &Value) -> Vec<u8> {
    v.as_array()
        .expect("array")
        .iter()
        .map(|x| x.as_u64().expect("u64") as u8)
        .collect()
}

#[test]
fn orders_plain_triples() {
    let pairs = version_pairs();
    for case in &pairs[..3] {
        let a = str_of(case, "a");
        let b = str_of(case, "b");
        assert_eq!(
            newer_version(a, b),
            case["newer"].as_bool().unwrap(),
            "{a} vs {b}"
        );
    }
}

#[test]
fn accepts_a_v_prefix() {
    assert!(newer_version("v0.7.1", "0.7.0"));
    assert!(newer_version("V1.0.0", "0.9.0"));
    assert!(!newer_version("v0.7.0", "v0.7.0"));
    assert!(!newer_version("v1.2.3", "1.2.3"));
}

#[test]
fn a_malformed_tag_never_claims_an_upgrade() {
    assert!(!newer_version("x.y.z", "0.0.0"));
    assert!(!newer_version("nightly", "0.7.0"));
    assert!(!newer_version("", "0.7.0"));
}

#[test]
fn the_oracle_agrees_on_every_pair() {
    let pairs = version_pairs();
    assert_eq!(pairs.len(), 60);
    let mut flagged = 0;
    for case in &pairs {
        let a = str_of(case, "a");
        let b = str_of(case, "b");
        let expect = match case.get("flare_newer") {
            Some(v) => {
                flagged += 1;
                v.as_bool().unwrap()
            }
            None => case["newer"].as_bool().unwrap(),
        };
        assert_eq!(newer_version(a, b), expect, "{a} vs {b}");
    }
    assert_eq!(
        flagged, 3,
        "the three pairs Prism Terminal's Number() reads differently"
    );
}

#[test]
fn wants_preview_is_the_flag_wherever_it_sits() {
    assert_eq!(PREVIEW_FLAG, "--preview-update");
    let fixture = preview_fixture();
    for case in fixture["wantsPreview"].as_array().unwrap() {
        let argv: Vec<String> = case["argv"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            wants_preview(&argv),
            case["out"].as_bool().unwrap(),
            "{argv:?}"
        );
    }
}

#[test]
fn wants_preview_is_nothing_else() {
    for other in [
        "--preview-updates",
        "--preview",
        "preview-update",
        " --preview-update",
    ] {
        assert!(!wants_preview(&[other.to_string()]), "{other}");
    }
}

#[test]
fn preview_version_is_the_next_minor() {
    assert_eq!(preview_version("0.55.6"), "0.56.0");
    assert_eq!(preview_version("1.9"), "1.10.0");
}

#[test]
fn preview_version_still_answers_for_a_non_version() {
    let fixture = preview_fixture();
    for case in fixture["previewVersion"].as_array().unwrap() {
        assert_eq!(
            preview_version(str_of(case, "in")),
            str_of(case, "out"),
            "{case}"
        );
    }
}

#[test]
fn the_preview_offer_is_a_mock_with_no_token() {
    let offer = preview_update("0.55.6");
    assert_eq!(offer.version, "0.56.0");
    assert!(offer.mock);
    assert_eq!(offer.token, 0);
    assert_eq!(offer.notes, SAMPLE_NOTES);
}

#[test]
fn the_sample_notes_are_a_generated_body() {
    assert!(SAMPLE_NOTES.contains("## What's Changed"));
    assert!(SAMPLE_NOTES.contains("## New Contributors"));
    assert!(SAMPLE_NOTES.contains("**Full Changelog**"));
    assert!(SAMPLE_NOTES.contains("Open in Flare"));
    assert!(SAMPLE_NOTES.contains("github.com/Maxaubert/Flare/"));
    assert!(!SAMPLE_NOTES.contains("PrismTerminal"));
}

#[test]
fn the_sample_has_no_em_dash() {
    assert!(!SAMPLE_NOTES.contains('\u{2014}'));
}

#[test]
fn the_preview_climbs_to_100_and_never_back() {
    let fixture = preview_fixture();
    let run = &fixture["runs"][0];
    assert_eq!(run["ms"].as_u64().unwrap(), PREVIEW_MS);
    let (pcts, _, _) = trace(PREVIEW_MS, None);
    assert_eq!(pcts, json_u8s(&run["onPct"]));
    assert!(pcts.windows(2).all(|w| w[0] <= w[1]));
    assert_eq!(*pcts.last().unwrap(), 100);
}

#[test]
fn the_preview_takes_three_seconds_the_last_part_installing() {
    let fixture = preview_fixture();
    let run = &fixture["runs"][0];
    let (_, waits, _) = trace(3000, None);
    assert_eq!(waits.iter().sum::<u64>(), 3000);
    assert_eq!(*waits.last().unwrap(), 600);
    assert_eq!(waits, json_u64s(&run["waits"]));
    // The other oracle traces: a 1 s preview and a zero-length one.
    for case in fixture["runs"].as_array().unwrap() {
        if case["cancelAfter"].is_null() {
            let (pcts, waits, _) = trace(case["ms"].as_u64().unwrap(), None);
            assert_eq!(waits, json_u64s(&case["waits"]), "ms {}", case["ms"]);
            assert_eq!(pcts, json_u8s(&case["onPct"]), "ms {}", case["ms"]);
        }
    }
}

#[test]
fn a_cancel_stops_the_preview_where_it_is_and_answers_false() {
    let fixture = preview_fixture();
    for case in fixture["runs"].as_array().unwrap() {
        if let Some(after) = case["cancelAfter"].as_u64() {
            let (pcts, waits, result) = trace(case["ms"].as_u64().unwrap(), Some(after as usize));
            assert_eq!(waits, json_u64s(&case["waits"]), "cancel after {after}");
            assert_eq!(pcts, json_u8s(&case["onPct"]), "cancel after {after}");
            assert!(!result);
        }
    }
}

#[test]
fn the_preview_resolves_false() {
    let (_, _, result) = trace(PREVIEW_MS, None);
    assert!(!result);
    assert!(!run_preview_install(
        &mut |_| {},
        0,
        &Recorder::default(),
        &|| false
    ));
}

#[test]
fn the_preview_runs_on_a_real_clock() {
    let start = Instant::now();
    let result = run_preview_install(&mut |_| {}, 50, &SleepWaiter, &|| false);
    assert!(!result);
    assert!(start.elapsed() < Duration::from_secs(1));
}

#[test]
fn phase_defaults_to_idle() {
    assert_eq!(Phase::default(), Phase::Idle);
}

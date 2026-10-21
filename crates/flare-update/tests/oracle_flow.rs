//! M9 Task 7: the update flow reducer against Prism Terminal's own answers (`flow.json`, 200 sequences).

use flare_update::flow::{Action, Flow, reduce};
use flare_update::types::{Offer, Phase};
use serde_json::Value;

/// The offers the fixture names by letter, in the order they were written, with the token each one gets.
fn offers(v: &Value) -> Vec<(Value, Offer)> {
    v["offers"]
        .as_object()
        .expect("offers")
        .iter()
        .enumerate()
        .map(|(i, (_, o))| {
            let offer = Offer {
                version: o["version"].as_str().unwrap().to_string(),
                notes: o["notes"].as_str().unwrap().to_string(),
                mock: o["mock"].as_bool().unwrap(),
                token: i as u64 + 1,
            };
            (o.clone(), offer)
        })
        .collect()
}

fn offer_named(offers_v: &Value, table: &[(Value, Offer)], letter: &str) -> Offer {
    let json = &offers_v[letter];
    table
        .iter()
        .find(|(j, _)| j == json)
        .map(|(_, o)| o.clone())
        .expect("a named offer")
}

fn phase(s: &str) -> Phase {
    match s {
        "idle" => Phase::Idle,
        "downloading" => Phase::Downloading,
        "installing" => Phase::Installing,
        other => panic!("unknown phase {other}"),
    }
}

/// A JSON number, or one of the strings `"NaN"`, `"Infinity"`, `"-Infinity"`.
fn number(v: &Value) -> f64 {
    match v {
        Value::String(s) => match s.as_str() {
            "NaN" => f64::NAN,
            "Infinity" => f64::INFINITY,
            "-Infinity" => f64::NEG_INFINITY,
            other => other.parse().expect("a number string"),
        },
        other => other.as_f64().expect("a number"),
    }
}

fn state(v: &Value, table: &[(Value, Offer)], offers_v: &Value) -> Flow {
    let info = match &v["info"] {
        Value::Null => None,
        json => {
            // The state's offer is one of the named ones: find it by its JSON.
            let letter = offers_v
                .as_object()
                .unwrap()
                .iter()
                .find(|(_, o)| *o == json)
                .map(|(k, _)| k.clone())
                .expect("the state names a known offer");
            Some(offer_named(offers_v, table, &letter))
        }
    };
    Flow {
        info,
        phase: phase(v["phase"].as_str().unwrap()),
        pct: v["pct"].as_u64().unwrap() as u8,
        open: v["open"].as_bool().unwrap(),
        notice: v["notice"].as_str().map(str::to_string),
        aborting: v["aborting"].as_bool().unwrap(),
    }
}

fn action(v: &Value, table: &[(Value, Offer)], doc: &Value) -> Action {
    match v["a"].as_str().unwrap() {
        "available" => Action::Available(offer_named(
            &doc["offers"],
            table,
            v["which"].as_str().unwrap(),
        )),
        "open" => Action::Open,
        "cancel" => Action::Cancel,
        "hide" => Action::Hide,
        "install" => Action::Install,
        "progress" => Action::Progress(number(&v["pct"])),
        "abort" => Action::Abort,
        "settled" => Action::Settled {
            ok: v["ok"].as_bool().unwrap(),
        },
        "dismiss" => Action::Dismiss,
        other => panic!("unknown action {other}"),
    }
}

#[test]
fn the_oracle_agrees_on_200_sequences() {
    let doc: Value = serde_json::from_str(include_str!("fixtures/flow.json")).expect("flow.json");
    let table = offers(&doc);
    let offers_v = &doc["offers"];
    let start = state(&doc["start"], &table, offers_v);
    assert_eq!(start, Flow::default(), "the fixture starts from nothing");

    let sequences = doc["sequences"].as_array().expect("sequences");
    assert_eq!(sequences.len(), 200);
    for (n, seq) in sequences.iter().enumerate() {
        let actions = seq["actions"].as_array().unwrap();
        let states = seq["states"].as_array().unwrap();
        assert_eq!(states.len(), actions.len() + 1, "sequence {n} lengths");
        let mut s = state(&states[0], &table, offers_v);
        assert_eq!(s, start, "sequence {n} starts at the start");
        for (i, a) in actions.iter().enumerate() {
            let act = action(a, &table, &doc);
            s = reduce(&s, &act);
            let want = state(&states[i + 1], &table, offers_v);
            assert_eq!(s, want, "sequence {n} step {i} ({act:?})");
        }
    }
}

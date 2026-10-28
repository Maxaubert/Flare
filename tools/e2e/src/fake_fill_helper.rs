//! The fake helper of the `pluginFill` scenario: the e2e exe itself, copied into the test plugin's folder and started
//! by Flare as that plugin's helper (`flare-e2e --fake-fill-helper <record file>`). It speaks the channel as a real
//! password manager's helper would: `hello` with `FLARE_PLUGIN_TOKEN`, then for each `tool` event it asks for the
//! tab's page address (`browser.fill.page`), fills a test login (`browser.fill`) and answers the agent with the
//! outcome only (`tools.respond`). It never touches a real vault: the login is [`USER`], [`PASS`] and [`CODE`].
//!
//! What it heard back is appended to the record file, one JSON line per call, so the scenario can check it; the
//! values it sent are never written there (the scenario scans that file for them too).

use std::io::Write;
use std::path::Path;
use std::time::Duration;

use flare_ctl::{connect, ClientError, HelloParams, Method};
use serde_json::{json, Value};

/// The test login's user name, password and one-time code. Fakes: no vault holds them.
pub const USER: &str = "e2e-user@flare-test.local";
pub const PASS: &str = "Flare-e2e-PASS-7Qx9";
pub const CODE: &str = "402719";
/// The tool the test plugin offers (`mod_filltest_fill_login`).
pub const TOOL: &str = "fill_login";

fn record(file: &Path, line: &Value) {
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)
    {
        let _ = writeln!(f, "{line}");
    }
}

fn outcome(r: Result<Value, ClientError>) -> Value {
    match r {
        Ok(v) => v,
        Err(ClientError::Remote(e)) => json!({"error": e.code, "message": e.message}),
        Err(e) => json!({"error": "client", "message": e.to_string()}),
    }
}

/// Runs the fake helper until Flare closes its connection. The exit code is 0, or 1 when it could not connect.
pub fn run(record_file: &Path) -> i32 {
    let (Ok(pipe), Ok(token)) = (
        std::env::var("FLARE_CTL_PIPE"),
        std::env::var("FLARE_PLUGIN_TOKEN"),
    ) else {
        record(record_file, &json!({"start": "no pipe or token"}));
        return 1;
    };
    let mut c = match connect(&pipe, Duration::from_secs(10)) {
        Ok(c) => c,
        Err(e) => {
            record(record_file, &json!({"start": format!("connect: {e}")}));
            return 1;
        }
    };
    let hello = c.hello(
        &HelloParams {
            client: "flare-e2e-fake-fill-helper".into(),
            version: "1".into(),
            token: Some(token),
            tab_token: None,
            no_events: false,
        },
        Duration::from_secs(10),
    );
    record(
        record_file,
        &json!({"hello": hello.as_ref().map(|h| h.permissions.clone()).map_err(|e| e.to_string())}),
    );
    if hello.is_err() {
        return 1;
    }
    while let Ok(next) = c.wait_event(Duration::from_secs(1)) {
        let Some(e) = next else { continue };
        if e.event != "tool" {
            continue;
        }
        let p = &e.params;
        let call_id = p["call_id"].as_str().unwrap_or_default().to_string();
        let tab = p["tab"].as_str().unwrap_or_default().to_string();
        let case = p["input"]["case"].as_str().unwrap_or("match").to_string();
        let page = outcome(c.call(
            Method::BrowserFillPage,
            json!({"tab": tab}),
            Duration::from_secs(10),
        ));
        let uris = if case == "mismatch" {
            json!(["https://example.com/login"])
        } else {
            json!(["https://flare-test.local/"])
        };
        let fill = outcome(c.call(
            Method::BrowserFill,
            json!({
                "tab": tab,
                "login_name": "E2E test login",
                "login_uris": uris,
                "username": USER,
                "password": PASS,
                "totp": CODE,
            }),
            Duration::from_secs(90),
        ));
        let answered = outcome(c.call(
            Method::ToolsRespond,
            json!({"call_id": call_id, "result": {"outcome": fill}}),
            Duration::from_secs(10),
        ));
        record(
            record_file,
            &json!({"event": e.event, "case": case, "tab": tab, "page": page, "fill": fill, "respond": answered}),
        );
    }
    0
}

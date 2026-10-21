//! L12 (audit 2026-10-20): the panic line carries the payload's length and a hash, never its text,
//! which can hold what the user pasted or said. One test, one process: the panic hook is global.

use flare_diag::crash::CrashHooks;
use flare_diag::writer::LineSink;
use serde_json::{Map, Value};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Cap(Mutex<Vec<(String, Value)>>);

impl LineSink for Cap {
    fn emit(&self, _src: &'static str, kind: &str, _at: u64, fields: Map<String, Value>) {
        self.0
            .lock()
            .unwrap()
            .push((kind.to_owned(), Value::Object(fields)));
    }
}

#[test]
fn a_panic_line_never_holds_the_payload_text() {
    let cap = Arc::new(Cap::default());
    let hooks = CrashHooks::install_with(cap.clone(), Arc::new(|| {}));
    let secret = "hunter2-the-pasted-password";
    let owned = format!("bad input: {secret}");
    let _ = catch_unwind(AssertUnwindSafe(|| panic!("{owned}")));
    let _ = catch_unwind(AssertUnwindSafe(|| std::panic::panic_any(secret)));
    hooks.uninstall();
    let lines = cap.0.lock().unwrap().clone();
    assert_eq!(lines.len(), 2, "both panics were written: {lines:?}");
    let text = format!("{lines:?}");
    assert!(!text.contains(secret), "the payload text leaked: {text}");
    let msg = lines[0].1["msg"].as_str().expect("a msg");
    assert!(
        msg.contains(&owned.len().to_string()),
        "the length is kept: {msg}"
    );
    assert!(msg.contains("hash"), "a hash is kept: {msg}");
    assert!(lines[0].1["loc"].is_string(), "the location stays");
}

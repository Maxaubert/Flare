//! The OSC 777 parser and the hook table (Task 2). Claude Code's hook signal arrives as
//! `OSC 777;flare-agent;state=<word>[;kind=<word>]` from the bundled `flare-status` plugin.
//! PT: `agentHookSignal.ts` (the parse and failed-label groups).

/// The first field of every payload this plugin writes. Flare accepts no other name.
pub const PREFIX: &str = "flare-agent";

/// Longest payload Flare reads (UTF-16 units). Longer is refused whole.
const MAX_LEN: usize = 200;

/// The state a hook reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HookState {
    Working,
    Question,
    Done,
    Failed,
}

impl HookState {
    /// The word written in the payload.
    pub fn as_str(self) -> &'static str {
        match self {
            HookState::Working => "working",
            HookState::Question => "question",
            HookState::Done => "done",
            HookState::Failed => "failed",
        }
    }
}

/// One parsed payload: the state, and for a failure the error kind when the plugin sent one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentSignal {
    pub state: HookState,
    pub kind: Option<String>,
}

/// Claude Code's StopFailure `error` values (2.1.289), one static hook each, in this order.
pub const STOP_FAILURE_KINDS: [&str; 12] = [
    "rate_limit",
    "overloaded",
    "authentication_failed",
    "oauth_org_not_allowed",
    "account_on_hold",
    "billing_error",
    "invalid_request",
    "model_not_found",
    "server_error",
    "max_output_tokens",
    "cloud_credential_error",
    "unknown",
];

/// One row of `hooks.json`: the event, its matcher and the args after the state word.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookRow {
    pub event: String,
    pub matcher: String,
    pub args: String,
}

fn row(event: &str, matcher: &str, args: &str) -> HookRow {
    HookRow {
        event: event.to_string(),
        matcher: matcher.to_string(),
        args: args.to_string(),
    }
}

/// Which hook says what. `hooks.json` is generated from this table once (Task 3) and a test
/// holds the file to it. 25 rows: six working events, two questions, three Notification
/// matchers, Stop, twelve StopFailure kinds and one catch-all StopFailure. SessionStart and
/// SessionEnd are NOT hooked (their bytes reached the pty in 1 of 3 and 0 of 3 runs).
pub fn agent_hook_events() -> Vec<HookRow> {
    let mut rows = Vec::new();
    for event in [
        "UserPromptSubmit",
        "PreToolUse",
        "PostToolUse",
        "PostToolUseFailure",
        "SubagentStart",
        "PreCompact",
    ] {
        rows.push(row(event, "*", "working"));
    }
    rows.push(row("PermissionRequest", "*", "question"));
    rows.push(row("Elicitation", "*", "question"));
    for matcher in [
        "permission_prompt",
        "elicitation_dialog",
        "elicitation_url_dialog",
    ] {
        rows.push(row("Notification", matcher, "question"));
    }
    rows.push(row("Stop", "*", "done"));
    for kind in STOP_FAILURE_KINDS {
        rows.push(row("StopFailure", kind, &format!("failed {kind}")));
    }
    rows.push(row("StopFailure", "*", "failed"));
    rows
}

/// A kind is a plain word: a lowercase letter, then lowercase letters, digits or underscores,
/// at most 40 characters in all.
fn is_plain_kind(kind: &str) -> bool {
    let bytes = kind.as_bytes();
    if bytes.is_empty() || bytes.len() > 40 {
        return false;
    }
    if !bytes[0].is_ascii_lowercase() {
        return false;
    }
    bytes[1..]
        .iter()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'_')
}

/// One OSC 777 payload (everything after `777;`), or None when it is not a state from our plugin.
pub fn parse_agent_signal(data: &str) -> Option<AgentSignal> {
    if data.encode_utf16().count() > MAX_LEN {
        return None;
    }
    let mut parts = data.split(';');
    if parts.next()? != PREFIX {
        return None;
    }
    let mut state: Option<&str> = None;
    let mut kind: Option<&str> = None;
    for part in parts {
        let eq = part.find('=')?;
        if eq == 0 {
            return None;
        }
        let (key, value) = (&part[..eq], &part[eq + 1..]);
        match key {
            "state" => state = Some(value),
            "kind" => kind = Some(value),
            _ => {}
        }
    }
    let state = match state? {
        "working" => HookState::Working,
        "question" => HookState::Question,
        "done" => HookState::Done,
        "failed" => HookState::Failed,
        _ => return None,
    };
    if state != HookState::Failed {
        return Some(AgentSignal { state, kind: None });
    }
    match kind {
        None => Some(AgentSignal { state, kind: None }),
        Some(k) if is_plain_kind(k) => Some(AgentSignal {
            state,
            kind: Some(k.to_string()),
        }),
        Some(_) => None,
    }
}

/// The words the tooltip shows for each known kind.
fn kind_words(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "rate_limit" => "rate limit",
        "overloaded" => "servers overloaded",
        "authentication_failed" => "sign-in failed",
        "oauth_org_not_allowed" => "organisation not allowed",
        "account_on_hold" => "account on hold",
        "billing_error" => "billing",
        "invalid_request" => "invalid request",
        "model_not_found" => "model not found",
        "server_error" => "server error",
        "max_output_tokens" => "reply too long",
        "cloud_credential_error" => "cloud credentials",
        "unknown" => "unknown error",
        _ => return None,
    })
}

/// "Failed: rate limit", or "Failed" when the kind is None. An unknown kind shows its words
/// with underscores as spaces ("Failed: brand new error").
pub fn failed_label(kind: Option<&str>) -> String {
    match kind {
        None => "Failed".to_string(),
        Some(k) => match kind_words(k) {
            Some(words) => format!("Failed: {words}"),
            None => format!("Failed: {}", k.replace('_', " ")),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sig(state: HookState, kind: Option<&str>) -> Option<AgentSignal> {
        Some(AgentSignal {
            state,
            kind: kind.map(str::to_string),
        })
    }

    #[test]
    fn reads_each_state_from_our_plugin() {
        assert_eq!(
            parse_agent_signal("flare-agent;state=working"),
            sig(HookState::Working, None)
        );
        assert_eq!(
            parse_agent_signal("flare-agent;state=question"),
            sig(HookState::Question, None)
        );
        assert_eq!(
            parse_agent_signal("flare-agent;state=done"),
            sig(HookState::Done, None)
        );
        assert_eq!(
            parse_agent_signal("flare-agent;state=failed"),
            sig(HookState::Failed, None)
        );
        assert_eq!(
            parse_agent_signal("flare-agent;state=failed;kind=rate_limit"),
            sig(HookState::Failed, Some("rate_limit"))
        );
    }

    #[test]
    fn refuses_an_unknown_state() {
        assert_eq!(parse_agent_signal("flare-agent;state=sleeping"), None);
        assert_eq!(parse_agent_signal("flare-agent;state="), None);
        assert_eq!(parse_agent_signal("flare-agent"), None);
    }

    #[test]
    fn leaves_other_osc_777_users_alone() {
        assert_eq!(parse_agent_signal("notify;Build;done"), None);
        assert_eq!(parse_agent_signal("flare-agentx;state=done"), None);
        assert_eq!(parse_agent_signal("FLARE-AGENT;state=done"), None);
        assert_eq!(parse_agent_signal(""), None);
        assert_eq!(parse_agent_signal("prism-agent;state=done"), None);
    }

    #[test]
    fn refuses_a_malformed_payload_or_a_bad_kind() {
        assert_eq!(parse_agent_signal("flare-agent;state"), None);
        assert_eq!(parse_agent_signal("flare-agent;=done"), None);
        assert_eq!(
            parse_agent_signal("flare-agent;state=failed;kind=<b>x</b>"),
            None
        );
        assert_eq!(
            parse_agent_signal("flare-agent;state=failed;kind=Rate Limit"),
            None
        );
        let long_kind = format!("a{}", "a".repeat(40));
        assert_eq!(
            parse_agent_signal(&format!("flare-agent;state=failed;kind={long_kind}")),
            None
        );
        let too_long = format!("flare-agent;state=done;{}", "x".repeat(300));
        assert_eq!(parse_agent_signal(&too_long), None);
        let exactly_40 = format!("a{}", "b".repeat(39));
        assert_eq!(
            parse_agent_signal(&format!("flare-agent;state=failed;kind={exactly_40}")),
            sig(HookState::Failed, Some(&exactly_40))
        );
    }

    #[test]
    fn keeps_the_state_when_a_newer_plugin_adds_a_field() {
        assert_eq!(
            parse_agent_signal("flare-agent;state=done;turn=3"),
            sig(HookState::Done, None)
        );
        assert_eq!(
            parse_agent_signal("flare-agent;state=done;kind=rate_limit"),
            sig(HookState::Done, None)
        );
    }

    #[test]
    fn failed_label_names_each_kind() {
        assert_eq!(failed_label(Some("rate_limit")), "Failed: rate limit");
        assert_eq!(
            failed_label(Some("model_not_found")),
            "Failed: model not found"
        );
        assert_eq!(
            failed_label(Some("brand_new_error")),
            "Failed: brand new error"
        );
        assert_eq!(failed_label(None), "Failed");
        for kind in STOP_FAILURE_KINDS {
            let label = failed_label(Some(kind));
            assert!(label.starts_with("Failed: "), "{label}");
            assert!(
                label[8..]
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == ' ' || c == '-'),
                "{label}"
            );
        }
    }

    #[test]
    fn the_table_has_25_rows() {
        let rows = agent_hook_events();
        assert_eq!(rows.len(), 25);
        let working: Vec<_> = rows.iter().filter(|r| r.args == "working").collect();
        assert_eq!(working.len(), 6);
        let failures: Vec<_> = rows
            .iter()
            .filter(|r| r.event == "StopFailure" && r.matcher != "*")
            .collect();
        assert_eq!(failures.len(), 12);
        for r in &failures {
            assert_eq!(r.args, format!("failed {}", r.matcher));
        }
        let catch_all: Vec<_> = rows
            .iter()
            .filter(|r| r.event == "StopFailure" && r.matcher == "*")
            .collect();
        assert_eq!(catch_all.len(), 1);
        assert_eq!(catch_all[0].args, "failed");
        assert!(
            rows.iter()
                .all(|r| r.event != "SessionStart" && r.event != "SessionEnd")
        );
    }
}

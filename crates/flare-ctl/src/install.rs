//! `install.staged`'s answer in words (rule 1326), one copy for the agent's tool (`flare-mcp`) and the
//! `flare --install-staged` command.
//!
//! The answer is `{state, why?, commit?, short?, built_at?, signed?, developer_id?, signer?}`; `state` is `armed`,
//! `installing`, `none`, `refused`, or `staged` for a `check`.

use serde_json::Value;

/// Where an agent's install stands, from the answer's `state`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallState {
    /// The install waits for every agent to be idle, then counts down 10 s, installs and restarts Flare.
    Armed,
    /// The countdown or the swap already runs.
    Installing,
    /// A build is staged and nothing is armed (a `check`).
    Staged,
    /// Nothing is staged (or the stage is not ready).
    None,
    /// Flare would not (a build folder run, a build not signed with Developer ID, ...).
    Refused,
}

impl InstallState {
    pub fn parse(word: &str) -> InstallState {
        match word {
            "armed" => InstallState::Armed,
            "installing" => InstallState::Installing,
            "staged" => InstallState::Staged,
            "none" => InstallState::None,
            _ => InstallState::Refused,
        }
    }

    /// The command's exit code: 0 armed, installing or staged; 2 nothing staged; 3 refused.
    pub fn exit_code(self) -> i32 {
        match self {
            InstallState::Armed | InstallState::Installing | InstallState::Staged => 0,
            InstallState::None => 2,
            InstallState::Refused => 3,
        }
    }
}

/// The answer as one line for a person or a model, and its state.
pub fn describe(answer: &Value) -> (InstallState, String) {
    let text = |k: &str| {
        answer
            .get(k)
            .and_then(Value::as_str)
            .map(|s| crate::text::clean_text(s, 300))
            .filter(|s| !s.is_empty())
    };
    let state = InstallState::parse(&text("state").unwrap_or_default());
    let build = match (text("short"), text("signer")) {
        (Some(c), Some(s)) => format!("build {c} ({s})"),
        (Some(c), None) => format!("build {c}"),
        _ => "the staged build".to_string(),
    };
    let why = text("why");
    let line = match state {
        InstallState::Armed => format!(
            "armed: {build} installs once every agent in Flare is idle (this one too, so end your turn), for a minute without a break; then a 10 s countdown the person can cancel, the install and a restart. Sessions resume after the restart."
        ),
        InstallState::Installing => {
            format!("installing: {build} is counting down or being installed; Flare restarts next.")
        }
        InstallState::Staged => {
            format!("staged: {build} is ready and nothing has asked to install it.")
        }
        InstallState::None => format!(
            "none: {}.",
            why.clone()
                .unwrap_or_else(|| "no build is staged".to_string())
        ),
        InstallState::Refused => format!(
            "refused: {}.",
            why.clone()
                .unwrap_or_else(|| "Flare gave no reason".to_string())
        ),
    };
    let line = match (state, why) {
        (InstallState::Armed | InstallState::Installing | InstallState::Staged, Some(w)) => {
            format!("{line} ({w})")
        }
        _ => line,
    };
    (state, line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn signed() -> Value {
        json!({"commit": "65273b3c9b14", "short": "65273b3", "signed": "developer-id", "developer_id": true,
               "signer": "Developer ID (WV7BY8CUM8)"})
    }

    #[test]
    fn each_state_reads_as_one_line_with_the_commit_and_the_signer() {
        let mut a = signed();
        a["state"] = json!("armed");
        let (s, line) = describe(&a);
        assert_eq!(s, InstallState::Armed);
        assert!(
            line.starts_with(
                "armed: build 65273b3 (Developer ID (WV7BY8CUM8)) installs once every agent"
            ),
            "{line}"
        );
        assert!(line.contains("this one too"), "{line}");
        assert_eq!(s.exit_code(), 0);
        a["state"] = json!("installing");
        assert!(describe(&a).1.starts_with("installing: build 65273b3"));
        a["state"] = json!("staged");
        assert_eq!(describe(&a).0, InstallState::Staged);
        let (s, line) = describe(&json!({"state": "none", "why": "no build is staged"}));
        assert_eq!(
            (s, line.as_str()),
            (InstallState::None, "none: no build is staged.")
        );
        assert_eq!(s.exit_code(), 2);
        let mut a = signed();
        a["state"] = json!("refused");
        a["why"] = json!("the staged build is ad hoc, not signed with Developer ID");
        let (s, line) = describe(&a);
        assert_eq!(s, InstallState::Refused);
        assert_eq!(
            line,
            "refused: the staged build is ad hoc, not signed with Developer ID."
        );
        assert_eq!(s.exit_code(), 3);
    }

    #[test]
    fn an_unknown_or_missing_state_is_a_refusal_and_control_characters_are_cleaned() {
        assert_eq!(describe(&json!({})).0, InstallState::Refused);
        assert_eq!(
            describe(&json!({"state": "maybe"})).0,
            InstallState::Refused
        );
        let (_, line) = describe(&json!({"state": "refused", "why": "a\u{1b}[31m\nb"}));
        assert!(!line.chars().any(char::is_control), "{line:?}");
    }
}

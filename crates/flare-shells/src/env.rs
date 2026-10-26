//! pty_env, plugin env, FLARE_TAB_ID and the control channel variables.

use crate::prompt::cmd_prompt;

/// Environment pairs in order; keys keep their spelling (Windows names are case-blind, see `env_get`).
pub type Env = Vec<(String, String)>;

/// The tab token every shell in a Flare tab sees.
pub const TAB_ID_VAR: &str = "FLARE_TAB_ID";
/// The secret of the tab (control channel `hello`). Unlike `FLARE_TAB_ID`, which names the tab in hook lines
/// and to Sonara, this one is never printed, logged or sent anywhere: a process that has it IS the tab.
pub const TAB_TOKEN_VAR: &str = "FLARE_TAB_TOKEN";
/// The control channel's pipe of the Flare instance that started the shell (per profile).
pub const CTL_PIPE_VAR: &str = "FLARE_CTL_PIPE";
/// The process id of the Flare that serves the pipe. A client takes the pipe's server to be this process
/// and no other, so a process that squatted the pipe name first is not trusted with the tab's secret.
pub const CTL_PID_VAR: &str = "FLARE_CTL_PID";
/// Where `flare-mcp.exe` is, for the bundled Claude Code plugin's `.mcp.json`.
pub const MCP_EXE_VAR: &str = "FLARE_MCP_EXE";
/// The Claude Code plugin folder list (`;`-separated).
pub const PLUGIN_DIRS_VAR: &str = "CLAUDE_CODE_PLUGIN_DIRS";
/// agy's switch for its OSC 7501 status reports. agy asks the terminal first (`OSC 7501 ; ?`); Flare
/// sets the switch instead of answering, so every shell gets it `on` unless the user set it already.
pub const AGY_STATUS_VAR: &str = "AGY_CLI_OSC7501";
/// Variables that name another session; a shell in a Flare tab never inherits them.
pub const SESSION_MARKERS: [&str; 12] = [
    "CLAUDECODE",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_PID",
    "CLAUDE_PLUGIN_DATA",
    "CLAUDE_EFFORT",
    "CODEX_COMPANION_SESSION_ID",
    "CODEX_COMPANION_TRANSCRIPT_PATH",
];

/// What a shell learns about the control channel (rule S1: the token is per tab and unguessable, the
/// pipe is per Flare instance). Variables the user already had under these names are replaced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CtlEnv {
    pub tab_token: String,
    pub pipe: String,
    /// The process id of the Flare that serves the pipe.
    pub pid: u32,
    /// The absolute path of `flare-mcp.exe`, when it was found beside `flare.exe`.
    pub mcp_exe: Option<String>,
}

/// Whether the Claude Code plugin folder is on for a tab, and where it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginEnv {
    pub dir: String,
    pub on: bool,
}

/// Case-insensitive lookup (Windows names are case-blind).
pub fn env_get<'a>(env: &'a Env, key: &str) -> Option<&'a str> {
    env.iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.as_str())
}

pub fn env_has(env: &Env, key: &str) -> bool {
    env_get(env, key).is_some()
}

/// "" for None; `"<1|0>|<dir>"` for Some. A warm shell is adopted only on an equal key.
pub fn plugin_key(p: Option<&PluginEnv>) -> String {
    match p {
        None => String::new(),
        Some(p) => format!("{}|{}", if p.on { 1 } else { 0 }, p.dir),
    }
}

/// The user's plugin folders (split on ';', blank entries and entries `is_ours` names dropped), with `dir`
/// appended when given and not already present (case-insensitive). Joined with ';'.
pub fn with_plugin_dir(
    dirs: Option<&str>,
    dir: Option<&str>,
    is_ours: &dyn Fn(&str) -> bool,
) -> String {
    let mut out: Vec<String> = dirs
        .unwrap_or("")
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty() && !is_ours(s))
        .map(str::to_string)
        .collect();
    if let Some(d) = dir.map(str::trim)
        && !out.iter().any(|x| x.eq_ignore_ascii_case(d))
    {
        out.push(d.to_string());
    }
    out.join(";")
}

/// The environment a shell in a Flare tab sees (see the plan, Task 6).
pub fn pty_env(
    from: &[(String, String)],
    shell_id: Option<&str>,
    plugin: Option<&PluginEnv>,
    is_ours: Option<&dyn Fn(&str) -> bool>,
    tab_token: Option<&str>,
    ctl: Option<&CtlEnv>,
) -> Env {
    let mut prompt: Option<String> = None;
    let mut out: Env = Vec::with_capacity(from.len() + 4);
    for (k, v) in from {
        let up = k.to_ascii_uppercase();
        if up == "PROMPT" {
            prompt = Some(v.clone());
        }
        let silences_colour = up == "NO_COLOR"
            || (up == "FORCE_COLOR"
                && matches!(v.to_ascii_lowercase().as_str(), "0" | "false" | "none"));
        let is_session = SESSION_MARKERS.iter().any(|m| *m == up);
        // The variables Flare sets itself are never inherited: a Flare started from a Flare tab must not
        // hand its shells the outer tab's secret.
        let ours = up == TAB_ID_VAR
            || up == TAB_TOKEN_VAR
            || up == CTL_PIPE_VAR
            || up == CTL_PID_VAR
            || up == MCP_EXE_VAR;
        if silences_colour || is_session || up == "TERM" || up == "COLORTERM" || ours {
            continue;
        }
        out.push((k.clone(), v.clone()));
    }
    out.push(("TERM".to_string(), "xterm-256color".to_string()));
    out.push(("COLORTERM".to_string(), "truecolor".to_string()));
    if !env_has(&out, AGY_STATUS_VAR) {
        out.push((AGY_STATUS_VAR.to_string(), "on".to_string()));
    }
    if let Some(token) = tab_token {
        out.push((TAB_ID_VAR.to_string(), token.to_string()));
    }
    if let Some(c) = ctl {
        out.push((TAB_TOKEN_VAR.to_string(), c.tab_token.clone()));
        out.push((CTL_PIPE_VAR.to_string(), c.pipe.clone()));
        out.push((CTL_PID_VAR.to_string(), c.pid.to_string()));
        if let Some(exe) = &c.mcp_exe {
            out.push((MCP_EXE_VAR.to_string(), exe.clone()));
        }
    }
    if shell_id == Some("cmd") {
        out.retain(|(k, _)| !k.eq_ignore_ascii_case("PROMPT"));
        out.push(("PROMPT".to_string(), cmd_prompt(prompt.as_deref())));
    }
    if let Some(p) = plugin {
        let never = |_: &str| false;
        let ours: &dyn Fn(&str) -> bool = is_ours.unwrap_or(&never);
        let existing = out
            .iter()
            .position(|(k, _)| k.eq_ignore_ascii_case(PLUGIN_DIRS_VAR));
        let dirs = with_plugin_dir(
            existing.map(|i| out[i].1.as_str()),
            if p.on { Some(p.dir.as_str()) } else { None },
            ours,
        );
        match existing {
            Some(i) if dirs.is_empty() => {
                out.remove(i);
            }
            Some(i) => out[i].1 = dirs,
            None if !dirs.is_empty() => out.push((PLUGIN_DIRS_VAR.to_string(), dirs)),
            None => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIR: &str = "C:\\PT\\resources\\claude-plugin";

    fn e(p: &[(&str, &str)]) -> Env {
        p.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn get<'a>(env: &'a Env, k: &str) -> Option<&'a str> {
        env_get(env, k)
    }

    fn on() -> PluginEnv {
        PluginEnv {
            dir: DIR.to_string(),
            on: true,
        }
    }

    fn off() -> PluginEnv {
        PluginEnv {
            dir: DIR.to_string(),
            on: false,
        }
    }

    fn count_upper(env: &Env, key: &str) -> usize {
        env.iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case(key))
            .count()
    }

    #[test]
    fn names_the_tab_and_never_keeps_an_inherited_one() {
        let r = pty_env(
            &e(&[("FLARE_TAB_ID", "old"), ("Flare_Tab_Id", "older")]),
            Some("pwsh"),
            None,
            None,
            Some("tok-1"),
            None,
        );
        assert_eq!(get(&r, TAB_ID_VAR), Some("tok-1"));
        assert_eq!(count_upper(&r, TAB_ID_VAR), 1);
        let r2 = pty_env(&e(&[("FLARE_TAB_ID", "old")]), None, None, None, None, None);
        assert!(!env_has(&r2, TAB_ID_VAR));
    }

    fn ctl() -> CtlEnv {
        CtlEnv {
            tab_token: "ab".repeat(32),
            pipe: "\\\\.\\pipe\\flare-ctl-1-2".to_string(),
            pid: 4242,
            mcp_exe: Some("C:\\Flare\\flare-mcp.exe".to_string()),
        }
    }

    #[test]
    fn the_control_channel_variables_reach_the_shell_and_replace_the_users_own() {
        let r = pty_env(
            &e(&[
                ("FLARE_TAB_TOKEN", "stolen"),
                ("Flare_Ctl_Pid", "1"),
                ("flare_ctl_pipe", "\\\\.\\pipe\\other"),
                ("Flare_Mcp_Exe", "C:\\evil.exe"),
            ]),
            Some("pwsh"),
            None,
            None,
            Some("tab-id"),
            Some(&ctl()),
        );
        assert_eq!(get(&r, TAB_TOKEN_VAR), Some("ab".repeat(32).as_str()));
        assert_eq!(get(&r, CTL_PIPE_VAR), Some("\\\\.\\pipe\\flare-ctl-1-2"));
        assert_eq!(get(&r, MCP_EXE_VAR), Some("C:\\Flare\\flare-mcp.exe"));
        assert_eq!(get(&r, CTL_PID_VAR), Some("4242"));
        for k in [TAB_TOKEN_VAR, CTL_PIPE_VAR, CTL_PID_VAR, MCP_EXE_VAR] {
            assert_eq!(count_upper(&r, k), 1, "{k}");
        }
        // The secret and the tab's public id are different variables with different values.
        assert_ne!(get(&r, TAB_TOKEN_VAR), get(&r, TAB_ID_VAR));
    }

    #[test]
    fn without_a_channel_an_inherited_token_never_passes() {
        let r = pty_env(
            &e(&[
                ("FLARE_TAB_TOKEN", "outer"),
                ("FLARE_CTL_PIPE", "p"),
                ("FLARE_CTL_PID", "7"),
                ("FLARE_MCP_EXE", "x"),
            ]),
            Some("pwsh"),
            None,
            None,
            None,
            None,
        );
        for k in [TAB_TOKEN_VAR, CTL_PIPE_VAR, CTL_PID_VAR, MCP_EXE_VAR] {
            assert!(!env_has(&r, k), "{k}");
        }
    }

    #[test]
    fn no_exe_means_no_exe_variable() {
        let mut c = ctl();
        c.mcp_exe = None;
        let r = pty_env(&[], Some("pwsh"), None, None, None, Some(&c));
        assert!(env_has(&r, TAB_TOKEN_VAR) && env_has(&r, CTL_PIPE_VAR));
        assert!(!env_has(&r, MCP_EXE_VAR));
    }

    #[test]
    fn turns_agys_status_reports_on_unless_the_user_chose() {
        let r = pty_env(&[], Some("pwsh"), None, None, None, None);
        assert_eq!(get(&r, AGY_STATUS_VAR), Some("on"));
        let r = pty_env(
            &e(&[("agy_cli_osc7501", "off")]),
            Some("cmd"),
            None,
            None,
            None,
            None,
        );
        assert_eq!(get(&r, AGY_STATUS_VAR), Some("off"));
        assert_eq!(count_upper(&r, AGY_STATUS_VAR), 1);
    }

    #[test]
    fn answers_for_what_the_panel_can_display_not_for_its_launcher() {
        let r = pty_env(
            &e(&[("TERM", "dumb"), ("PATH", "C:\\bin")]),
            Some("pwsh"),
            None,
            None,
            None,
            None,
        );
        assert_eq!(get(&r, "TERM"), Some("xterm-256color"));
        assert_eq!(get(&r, "COLORTERM"), Some("truecolor"));
        assert_eq!(get(&r, "PATH"), Some("C:\\bin"));
    }

    #[test]
    fn drops_the_variables_that_would_silence_colour() {
        let r = pty_env(
            &e(&[("NO_COLOR", "1"), ("FORCE_COLOR", "0")]),
            Some("pwsh"),
            None,
            None,
            None,
            None,
        );
        assert!(!env_has(&r, "NO_COLOR"));
        assert!(!env_has(&r, "FORCE_COLOR"));
    }

    #[test]
    fn keeps_force_color_that_asks_for_more() {
        let r = pty_env(
            &e(&[("FORCE_COLOR", "3")]),
            Some("pwsh"),
            None,
            None,
            None,
            None,
        );
        assert_eq!(get(&r, "FORCE_COLOR"), Some("3"));
        for v in ["false", "none", "FALSE"] {
            let r = pty_env(
                &e(&[("FORCE_COLOR", v)]),
                Some("pwsh"),
                None,
                None,
                None,
                None,
            );
            assert!(!env_has(&r, "FORCE_COLOR"), "{v} must be dropped");
        }
    }

    #[test]
    fn never_passes_on_another_sessions_identity() {
        let r = pty_env(
            &e(&[
                ("CLAUDECODE", "1"),
                ("CLAUDE_CODE_CHILD_SESSION", "1"),
                (
                    "CLAUDE_CODE_SESSION_ID",
                    "9fa24458-8fe5-4386-81d3-58cd302bee2d",
                ),
                ("CLAUDE_CODE_MESSAGING_SOCKET", "\\\\.\\pipe\\cc-msg-abc"),
                ("CLAUDE_CODE_MESSAGING_TOKEN", "secret"),
                ("CODEX_COMPANION_SESSION_ID", "x"),
                ("PATH", "C:\\bin"),
            ]),
            Some("pwsh"),
            None,
            None,
            None,
            None,
        );
        for k in [
            "CLAUDECODE",
            "CLAUDE_CODE_CHILD_SESSION",
            "CLAUDE_CODE_SESSION_ID",
            "CLAUDE_CODE_MESSAGING_SOCKET",
            "CLAUDE_CODE_MESSAGING_TOKEN",
            "CODEX_COMPANION_SESSION_ID",
        ] {
            assert!(!env_has(&r, k), "{k} must not pass");
        }
        assert_eq!(get(&r, "PATH"), Some("C:\\bin"));
    }

    #[test]
    fn keeps_the_claude_code_variables_that_are_real_configuration() {
        let r = pty_env(
            &e(&[
                ("CLAUDE_CODE_MAX_WEB_SEARCHES_PER_SESSION", "500"),
                ("CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS", "1"),
                ("ANTHROPIC_API_KEY", "k"),
            ]),
            Some("pwsh"),
            None,
            None,
            None,
            None,
        );
        assert_eq!(
            get(&r, "CLAUDE_CODE_MAX_WEB_SEARCHES_PER_SESSION"),
            Some("500")
        );
        assert_eq!(get(&r, "CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS"), Some("1"));
        assert_eq!(get(&r, "ANTHROPIC_API_KEY"), Some("k"));
    }

    #[test]
    fn never_forces_hyperlinks() {
        let r = pty_env(&[], Some("pwsh"), None, None, None, None);
        assert_eq!(count_upper(&r, "FORCE_HYPERLINK"), 0);
    }

    #[test]
    fn keeps_the_users_force_hyperlink() {
        let r = pty_env(
            &e(&[("FORCE_HYPERLINK", "0")]),
            Some("pwsh"),
            None,
            None,
            None,
            None,
        );
        assert_eq!(get(&r, "FORCE_HYPERLINK"), Some("0"));
    }

    #[test]
    fn force_hyperlink_has_one_spelling() {
        let r = pty_env(
            &e(&[("force_hyperlink", "0")]),
            Some("pwsh"),
            None,
            None,
            None,
            None,
        );
        assert_eq!(get(&r, "force_hyperlink"), Some("0"));
        assert_eq!(count_upper(&r, "FORCE_HYPERLINK"), 1);
        assert!(r.iter().any(|(k, _)| k == "force_hyperlink"));
    }

    #[test]
    fn passes_everything_else_through_untouched() {
        let r = pty_env(&e(&[("FOO", "bar")]), Some("pwsh"), None, None, None, None);
        assert_eq!(get(&r, "FOO"), Some("bar"));
        assert!(!env_has(&r, "GONE"));
    }

    #[test]
    fn plugin_dir_added_when_on() {
        let r = pty_env(&[], Some("pwsh"), Some(&on()), None, None, None);
        assert_eq!(get(&r, PLUGIN_DIRS_VAR), Some(DIR));
    }

    #[test]
    fn plugin_dir_appended_after_the_users() {
        let r = pty_env(
            &e(&[("CLAUDE_CODE_PLUGIN_DIRS", "D:\\mine;E:\\too")]),
            Some("pwsh"),
            Some(&on()),
            None,
            None,
            None,
        );
        assert_eq!(
            get(&r, PLUGIN_DIRS_VAR),
            Some("D:\\mine;E:\\too;C:\\PT\\resources\\claude-plugin")
        );
    }

    #[test]
    fn plugin_dir_keeps_the_existing_spelling() {
        let r = pty_env(
            &e(&[("Claude_Code_Plugin_Dirs", "D:\\mine")]),
            Some("pwsh"),
            Some(&on()),
            None,
            None,
            None,
        );
        assert_eq!(
            get(&r, "Claude_Code_Plugin_Dirs"),
            Some("D:\\mine;C:\\PT\\resources\\claude-plugin")
        );
        assert!(!r.iter().any(|(k, _)| k == PLUGIN_DIRS_VAR));
    }

    #[test]
    fn plugin_dir_not_added_twice() {
        let lower = DIR.to_lowercase();
        let r = pty_env(
            &e(&[("CLAUDE_CODE_PLUGIN_DIRS", &lower)]),
            Some("pwsh"),
            Some(&on()),
            None,
            None,
            None,
        );
        assert_eq!(get(&r, PLUGIN_DIRS_VAR), Some(lower.as_str()));
    }

    #[test]
    fn plugin_dir_left_out_when_off_and_the_users_value_kept() {
        let r = pty_env(&[], Some("pwsh"), Some(&off()), None, None, None);
        assert!(!env_has(&r, PLUGIN_DIRS_VAR));
        let r = pty_env(
            &e(&[("CLAUDE_CODE_PLUGIN_DIRS", "D:\\mine")]),
            Some("pwsh"),
            Some(&off()),
            None,
            None,
            None,
        );
        assert_eq!(get(&r, PLUGIN_DIRS_VAR), Some("D:\\mine"));
    }

    #[test]
    fn inherited_copy_of_our_plugin_is_dropped() {
        let ours = |d: &str| d.ends_with("stable\\claude-plugin");
        let input = e(&[(
            "CLAUDE_CODE_PLUGIN_DIRS",
            "D:\\mine;C:\\stable\\claude-plugin",
        )]);
        let r = pty_env(&input, Some("pwsh"), Some(&on()), Some(&ours), None, None);
        assert_eq!(
            get(&r, PLUGIN_DIRS_VAR),
            Some("D:\\mine;C:\\PT\\resources\\claude-plugin")
        );
        let r = pty_env(&input, Some("pwsh"), Some(&off()), Some(&ours), None, None);
        assert_eq!(get(&r, PLUGIN_DIRS_VAR), Some("D:\\mine"));
        let only = e(&[("CLAUDE_CODE_PLUGIN_DIRS", "C:\\stable\\claude-plugin")]);
        let r = pty_env(&only, Some("pwsh"), Some(&off()), Some(&ours), None, None);
        assert!(!env_has(&r, PLUGIN_DIRS_VAR));
    }

    #[test]
    fn no_plugin_means_the_variable_is_untouched() {
        let ours = |_: &str| true;
        let from = e(&[
            (
                "CLAUDE_CODE_PLUGIN_DIRS",
                "D:\\mine;;C:\\stable\\claude-plugin",
            ),
            ("FOO", "x"),
        ]);
        assert_eq!(
            pty_env(&from, Some("pwsh"), None, Some(&ours), None, None),
            pty_env(&from, Some("pwsh"), None, None, None, None)
        );
        let kept = pty_env(&from, Some("pwsh"), None, None, None, None);
        assert_eq!(
            get(&kept, PLUGIN_DIRS_VAR),
            Some("D:\\mine;;C:\\stable\\claude-plugin")
        );
        assert!(!env_has(
            &pty_env(&[], Some("pwsh"), None, Some(&ours), None, None),
            PLUGIN_DIRS_VAR
        ));
    }

    #[test]
    fn plugin_key_distinguishes_on_and_off() {
        assert_eq!(plugin_key(None), "");
        assert_ne!(plugin_key(Some(&on())), plugin_key(Some(&off())));
        assert_eq!(plugin_key(Some(&on())), plugin_key(Some(&on().clone())));
    }

    #[test]
    fn with_plugin_dir_drops_empty_entries() {
        assert_eq!(with_plugin_dir(Some(";a;;"), Some("b"), &|_| false), "a;b");
        assert_eq!(with_plugin_dir(None, None, &|_| false), "");
    }

    #[test]
    fn prompt_reaches_cmd_through_its_environment_only() {
        let r = pty_env(
            &e(&[("PROMPT", "$P$G")]),
            Some("cmd"),
            None,
            None,
            None,
            None,
        );
        assert_eq!(get(&r, "PROMPT"), Some("$e]9;9;$P$e\\$P$G"));
        let r = pty_env(&[], Some("cmd"), None, None, None, None);
        assert_eq!(get(&r, "PROMPT"), Some("$e]9;9;$P$e\\$P$G"));
        let r = pty_env(
            &e(&[("PROMPT", "$P$G")]),
            Some("pwsh"),
            None,
            None,
            None,
            None,
        );
        assert_eq!(get(&r, "PROMPT"), Some("$P$G"));
        let r = pty_env(&[], Some("pwsh"), None, None, None, None);
        assert_eq!(get(&r, "PROMPT"), None);
    }
}

//! What a Mac shell's environment gets on top of the shared `pty_env`: the terminal's name
//! (`TERM_PROGRAM`), a UTF-8 locale when none is set (a Flare started from Finder or launchd has
//! none), for zsh the `ZDOTDIR` shim of the integration, and for bash the switch that keeps
//! Apple's "the default interactive shell is now zsh" notice out of every new tab.

use crate::env::{Env, env_has};
use crate::macos::integration::{USER_ZDOTDIR_VAR, ZSH_DIR};
use std::path::Path;

/// What `TERM_PROGRAM` says (iTerm2 says `iTerm.app`, Terminal.app `Apple_Terminal`).
pub const TERM_PROGRAM: &str = "Flare";
/// Apple's bash prints a three-line notice at every start unless this is set; a value the user
/// set is kept.
pub const BASH_SILENCE_VAR: &str = "BASH_SILENCE_DEPRECATION_WARNING";
/// The locale a shell gets when none of `LANG`, `LC_ALL` and `LC_CTYPE` is set.
pub const DEFAULT_LANG: &str = "en_US.UTF-8";

/// Adds the Mac variables to `out` (already filtered by `pty_env`). `integration` is the active
/// integration folder, when its files are on disk.
pub fn finish(out: &mut Env, shell_id: Option<&str>, integration: Option<&Path>) {
    // The launcher's terminal (Flare started from iTerm) is never this one's.
    let mut user_zdotdir: Option<String> = None;
    out.retain(|(k, v)| match k.as_str() {
        "TERM_PROGRAM" | "TERM_PROGRAM_VERSION" | USER_ZDOTDIR_VAR => false,
        "ZDOTDIR" if shell_id == Some("zsh") && integration.is_some() => {
            user_zdotdir = Some(v.clone());
            false
        }
        _ => true,
    });
    out.push(("TERM_PROGRAM".to_string(), TERM_PROGRAM.to_string()));
    out.push((
        "TERM_PROGRAM_VERSION".to_string(),
        env!("CARGO_PKG_VERSION").to_string(),
    ));
    if !["LANG", "LC_ALL", "LC_CTYPE"]
        .iter()
        .any(|k| env_has(out, k))
    {
        out.push(("LANG".to_string(), DEFAULT_LANG.to_string()));
    }
    if shell_id == Some("bash") && !env_has(out, BASH_SILENCE_VAR) {
        out.push((BASH_SILENCE_VAR.to_string(), "1".to_string()));
    }
    if shell_id == Some("zsh")
        && let Some(dir) = integration
    {
        let shim = dir.join(ZSH_DIR).to_string_lossy().into_owned();
        // A ZDOTDIR that is already Flare's (a Flare started from a Flare tab mid-start) is not the user's.
        if let Some(z) = user_zdotdir.filter(|z| *z != shim) {
            out.push((USER_ZDOTDIR_VAR.to_string(), z));
        }
        out.push(("ZDOTDIR".to_string(), shim));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::env_get;

    fn e(p: &[(&str, &str)]) -> Env {
        p.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn count(env: &Env, key: &str) -> usize {
        env.iter().filter(|(k, _)| k == key).count()
    }

    #[test]
    fn names_flare_as_the_terminal_and_drops_the_launchers() {
        let mut r = e(&[
            ("TERM_PROGRAM", "iTerm.app"),
            ("TERM_PROGRAM_VERSION", "3.5"),
        ]);
        finish(&mut r, Some("bash"), None);
        assert_eq!(env_get(&r, "TERM_PROGRAM"), Some("Flare"));
        assert_eq!(
            env_get(&r, "TERM_PROGRAM_VERSION"),
            Some(env!("CARGO_PKG_VERSION"))
        );
        assert_eq!(count(&r, "TERM_PROGRAM"), 1);
    }

    #[test]
    fn bash_starts_without_apples_zsh_notice_and_a_users_choice_is_kept() {
        let mut r = e(&[]);
        finish(&mut r, Some("bash"), None);
        assert_eq!(env_get(&r, BASH_SILENCE_VAR), Some("1"));
        let mut r = e(&[(BASH_SILENCE_VAR, "0")]);
        finish(&mut r, Some("bash"), None);
        assert_eq!(env_get(&r, BASH_SILENCE_VAR), Some("0"));
        assert_eq!(count(&r, BASH_SILENCE_VAR), 1);
        for shell in [Some("zsh"), Some("fish"), None] {
            let mut r = e(&[]);
            finish(&mut r, shell, None);
            assert!(!env_has(&r, BASH_SILENCE_VAR), "{shell:?}");
        }
    }

    #[test]
    fn a_missing_locale_becomes_utf8_and_a_set_one_is_kept() {
        let mut r = e(&[("PATH", "/usr/bin:/bin")]);
        finish(&mut r, Some("zsh"), None);
        assert_eq!(env_get(&r, "LANG"), Some(DEFAULT_LANG));
        for set in ["LANG", "LC_ALL", "LC_CTYPE"] {
            let mut r = e(&[(set, "nb_NO.UTF-8")]);
            finish(&mut r, Some("zsh"), None);
            assert_eq!(env_get(&r, set), Some("nb_NO.UTF-8"));
            assert_eq!(count(&r, "LANG"), usize::from(set == "LANG"), "{set}");
        }
    }

    #[test]
    fn zsh_starts_from_the_shim_and_the_users_zdotdir_waits_beside_it() {
        let dir = Path::new("/Si");
        let mut r = e(&[("ZDOTDIR", "/Users/max/.config/zsh")]);
        finish(&mut r, Some("zsh"), Some(dir));
        assert_eq!(env_get(&r, "ZDOTDIR"), Some("/Si/zsh"));
        assert_eq!(
            env_get(&r, USER_ZDOTDIR_VAR),
            Some("/Users/max/.config/zsh")
        );
        assert_eq!(count(&r, "ZDOTDIR"), 1);

        let mut r = e(&[]);
        finish(&mut r, Some("zsh"), Some(dir));
        assert_eq!(env_get(&r, "ZDOTDIR"), Some("/Si/zsh"));
        assert!(
            !env_has(&r, USER_ZDOTDIR_VAR),
            "no ZDOTDIR of the user's to restore"
        );
    }

    #[test]
    fn an_inherited_shim_is_never_taken_for_the_users_zdotdir() {
        let mut r = e(&[("ZDOTDIR", "/Si/zsh"), (USER_ZDOTDIR_VAR, "/stale")]);
        finish(&mut r, Some("zsh"), Some(Path::new("/Si")));
        assert!(!env_has(&r, USER_ZDOTDIR_VAR));
        assert_eq!(count(&r, "ZDOTDIR"), 1);
    }

    #[test]
    fn other_shells_and_a_missing_integration_keep_the_users_zdotdir() {
        for (shell, dir) in [(Some("bash"), Some(Path::new("/Si"))), (Some("zsh"), None)] {
            let mut r = e(&[("ZDOTDIR", "/Users/max/.config/zsh")]);
            finish(&mut r, shell, dir);
            assert_eq!(env_get(&r, "ZDOTDIR"), Some("/Users/max/.config/zsh"));
            assert!(!env_has(&r, USER_ZDOTDIR_VAR));
        }
    }
}

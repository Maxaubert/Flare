//! Mod tokens: minted when a mod is enabled, held in memory only, dead when it is disabled (rule S5).
//!
//! A token is 256 bits from the system's random generator, written as 64 lower-case hex digits. It is
//! never written to a file, never logged and never shown; a restart mints new ones for the mods the user
//! still has on. [`Tokens::grant`] maps a presented token to the mod's [`Grant`]: exactly the permissions
//! its manifest declared. The lookup compares every live token in constant time and does not stop at the
//! first match, so the time it takes does not say which mod (or how close a guess) it was.

use flare_ctl::{Grant, Permission};

/// Bytes in a token (256 bits).
pub const TOKEN_BYTES: usize = 32;

/// Fills `buf` from the system's random number generator. False when Windows refuses.
pub fn random_bytes(buf: &mut [u8]) -> bool {
    use windows_sys::Win32::Security::Cryptography::{
        BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom,
    };
    // SAFETY: a null algorithm handle with the system-preferred-RNG flag is the documented form; `buf` is a
    // valid writable buffer of the stated length.
    let status = unsafe {
        BCryptGenRandom(
            std::ptr::null_mut(),
            buf.as_mut_ptr(),
            buf.len() as u32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    status >= 0
}

/// A fresh token: 64 lower-case hex digits. Err when Windows refuses to make random bytes.
pub fn new_token() -> Result<String, String> {
    let mut bytes = [0u8; TOKEN_BYTES];
    if !random_bytes(&mut bytes) {
        return Err("Windows refused to make a random token; the mod is not enabled".into());
    }
    Ok(to_hex(&bytes))
}

/// Lower-case hex.
pub fn to_hex(bytes: &[u8]) -> String {
    const D: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(D[(b >> 4) as usize] as char);
        s.push(D[(b & 0xf) as usize] as char);
    }
    s
}

/// Equal lengths and equal bytes, with no early exit on the first difference.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

/// The grant a mod's token holds: exactly its manifest permissions (S5). `eval` is never among them, even if
/// a caller passes it: it is the user's own switch for `browser.eval` (S4), and no mod gets it. `plugin` says
/// whether it is a protocol 2 plugin (only a plugin keeps a store).
pub fn mod_grant(id: &str, permissions: &[Permission], plugin: bool) -> Grant {
    Grant::Mod {
        id: id.to_string(),
        permissions: permissions
            .iter()
            .copied()
            .filter(|p| *p != Permission::Eval)
            .collect(),
        plugin,
    }
}

#[derive(Debug)]
struct Live {
    id: String,
    token: String,
    permissions: Vec<Permission>,
    plugin: bool,
}

/// The live tokens, one per enabled mod. Not `Clone`, not `Debug` for the secrets: the tokens are only
/// ever handed to the mod's own page.
#[derive(Default)]
pub struct Tokens {
    live: Vec<Live>,
}

impl std::fmt::Debug for Tokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let ids: Vec<&str> = self.live.iter().map(|l| l.id.as_str()).collect();
        write!(f, "Tokens({ids:?})")
    }
}

impl Tokens {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mints the mod's token, replacing (and so killing) any token it had. Err when no random bytes, or when the
    /// permissions include `eval` (a mod never holds it; a refused mint leaves the old token alive). `plugin` is
    /// the manifest's protocol: true for a protocol 2 plugin.
    pub fn mint(
        &mut self,
        id: &str,
        permissions: &[Permission],
        plugin: bool,
    ) -> Result<String, String> {
        if permissions.contains(&Permission::Eval) {
            return Err(
                "a mod can never hold eval: it is the user's own switch for browser.eval".into(),
            );
        }
        let token = new_token()?;
        self.revoke(id);
        self.live.push(Live {
            id: id.to_string(),
            token: token.clone(),
            permissions: permissions.to_vec(),
            plugin,
        });
        Ok(token)
    }

    /// Kills the mod's token. True when there was one.
    pub fn revoke(&mut self, id: &str) -> bool {
        let before = self.live.len();
        self.live.retain(|l| l.id != id);
        self.live.len() != before
    }

    /// The mod's live token, for the host to hand to its page.
    pub fn token_of(&self, id: &str) -> Option<&str> {
        self.live
            .iter()
            .find(|l| l.id == id)
            .map(|l| l.token.as_str())
    }

    /// Whether the mod's live token is a protocol 2 plugin's; None when it holds none.
    pub fn plugin_of(&self, id: &str) -> Option<bool> {
        self.live.iter().find(|l| l.id == id).map(|l| l.plugin)
    }

    /// The grant a presented token holds, or None for a token that is not live (a disabled mod's, or a
    /// guess).
    pub fn grant(&self, presented: &str) -> Option<Grant> {
        let mut found = None;
        for l in &self.live {
            if constant_time_eq(l.token.as_bytes(), presented.as_bytes()) {
                found = Some(mod_grant(&l.id, &l.permissions, l.plugin));
            }
        }
        found
    }

    /// The ids that hold a live token.
    pub fn ids(&self) -> Vec<String> {
        self.live.iter().map(|l| l.id.clone()).collect()
    }

    pub fn len(&self) -> usize {
        self.live.len()
    }

    pub fn is_empty(&self) -> bool {
        self.live.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Permission as P;

    #[test]
    fn a_token_is_256_bits_of_lower_case_hex() {
        let t = new_token().unwrap();
        assert_eq!(t.len(), 64);
        assert!(
            t.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        );
    }

    #[test]
    fn two_tokens_differ() {
        let a = new_token().unwrap();
        let b = new_token().unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn hex_is_two_digits_per_byte() {
        assert_eq!(to_hex(&[0x00, 0xff, 0x0a]), "00ff0a");
    }

    #[test]
    fn constant_time_eq_is_plain_equality() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(!constant_time_eq(b"", b"a"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn a_minted_token_maps_to_the_mod_with_exactly_its_permissions() {
        let mut t = Tokens::new();
        let tok = t.mint("notes", &[P::Panel, P::Tools], false).unwrap();
        assert_eq!(
            t.grant(&tok),
            Some(Grant::Mod {
                id: "notes".into(),
                permissions: vec![P::Panel, P::Tools],
                plugin: false,
            })
        );
        assert_eq!(t.token_of("notes"), Some(tok.as_str()));
    }

    #[test]
    fn a_grant_from_a_token_allows_its_permissions_and_no_others() {
        use flare_ctl::Method as M;
        let mut t = Tokens::new();
        let tok = t.mint("notes", &[P::Tools], false).unwrap();
        let g = t.grant(&tok).unwrap();
        assert!(g.may_call(M::ToolsRespond));
        assert!(!g.may_call(M::ViewImage));
        assert!(
            !g.may_call(M::BrowserNavigate),
            "no browser permission, no browser call"
        );
    }

    /// The browser cases: the eval check is tested on a grant that HAS browser, so it fails for the right reason.
    #[test]
    fn browser_needs_the_browser_permission_and_eval_never_comes_from_a_mod() {
        use flare_ctl::Method as M;
        let mut t = Tokens::new();
        let tok = t.mint("drive", &[P::Browser], false).unwrap();
        let g = t.grant(&tok).unwrap();
        assert!(g.may_call(M::BrowserNavigate));
        assert!(!g.may_call(M::BrowserEval), "browser alone is not eval");
        let view_only = t.mint("viewer", &[P::View], false).unwrap();
        let g = t.grant(&view_only).unwrap();
        assert!(!g.may_call(M::BrowserNavigate));
        assert!(!g.may_call(M::BrowserOpen));
        assert!(!g.may_call(M::ToolsRespond));
    }

    #[test]
    fn a_mint_with_eval_is_refused_and_keeps_the_old_token() {
        use flare_ctl::Method as M;
        let mut t = Tokens::new();
        let old = t.mint("drive", &[P::Browser], false).unwrap();
        let err = t.mint("drive", &[P::Browser, P::Eval], false).unwrap_err();
        assert!(err.contains("eval"), "{err}");
        assert_eq!(
            t.grant(&old).unwrap().kind(),
            "mod",
            "the old token still lives"
        );
        assert!(!t.grant(&old).unwrap().may_call(M::BrowserEval));
    }

    #[test]
    fn mod_grant_drops_eval_even_when_a_caller_passes_it() {
        use flare_ctl::Method as M;
        let g = mod_grant("drive", &[P::Browser, P::Eval], false);
        assert_eq!(g.permission_names(), vec!["browser"]);
        assert!(g.may_call(M::BrowserNavigate));
        assert!(!g.may_call(M::BrowserEval));
    }

    #[test]
    fn a_disabled_mods_token_is_dead() {
        let mut t = Tokens::new();
        let tok = t.mint("notes", &[P::View], false).unwrap();
        assert!(t.revoke("notes"));
        assert_eq!(t.grant(&tok), None);
        assert_eq!(t.token_of("notes"), None);
        assert!(!t.revoke("notes"), "a second revoke finds nothing");
        assert!(t.is_empty());
    }

    #[test]
    fn enabling_again_replaces_the_token() {
        let mut t = Tokens::new();
        let old = t.mint("notes", &[P::View], false).unwrap();
        let new = t.mint("notes", &[P::View], false).unwrap();
        assert_ne!(old, new);
        assert_eq!(t.grant(&old), None);
        assert!(t.grant(&new).is_some());
        assert_eq!(t.len(), 1);
    }

    #[test]
    fn each_mod_has_its_own_token_and_grant() {
        let mut t = Tokens::new();
        let a = t.mint("alpha", &[P::View], false).unwrap();
        let b = t.mint("beta", &[P::Convo], false).unwrap();
        assert_eq!(t.len(), 2);
        assert!(matches!(t.grant(&a), Some(Grant::Mod { ref id, .. }) if id == "alpha"));
        assert!(matches!(t.grant(&b), Some(Grant::Mod { ref id, .. }) if id == "beta"));
        t.revoke("alpha");
        assert_eq!(t.grant(&a), None);
        assert!(t.grant(&b).is_some());
    }

    #[test]
    fn guesses_and_prefixes_are_not_grants() {
        let mut t = Tokens::new();
        let tok = t.mint("notes", &[P::View], false).unwrap();
        assert_eq!(t.grant(""), None);
        assert_eq!(t.grant("0"), None);
        assert_eq!(t.grant(&tok[..63]), None);
        assert_eq!(t.grant(&format!("{tok}0")), None);
        assert_eq!(t.grant(&"0".repeat(64)), None);
    }

    #[test]
    fn a_token_is_not_in_the_debug_output() {
        let mut t = Tokens::new();
        let tok = t.mint("notes", &[P::View], false).unwrap();
        let shown = format!("{t:?}");
        assert!(shown.contains("notes"));
        assert!(!shown.contains(&tok));
    }

    #[test]
    fn an_empty_mod_gets_a_grant_with_no_permissions() {
        let mut t = Tokens::new();
        let tok = t.mint("empty-mod", &[], false).unwrap();
        let g = t.grant(&tok).unwrap();
        assert_eq!(g.permission_names(), Vec::<&str>::new());
    }
}

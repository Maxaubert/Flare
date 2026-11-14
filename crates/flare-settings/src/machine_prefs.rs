//! Machine pairing PR 2 (rules 1238 and 1241): the Computer sharing switch and what Flare changed to turn it on.
//!
//! Keys: `flare.machines.sharing` (a bool, default OFF: the switch, kept across restarts once the setup finished),
//! `flare.machines.serverByFlare` (a bool: Flare started or installed the SSH server when sharing turned on, so turning
//! sharing off offers to stop it again) and `flare.machines.adminKeys` (a bool: pairing, PR 3, put a key in Windows'
//! `administrators_authorized_keys`, so turning sharing off needs the elevated clean up).

use crate::kv::{PrefStore, bool_pref_get, bool_pref_set};

const SHARING: &str = "flare.machines.sharing";
const SERVER_BY_FLARE: &str = "flare.machines.serverByFlare";
const ADMIN_KEYS: &str = "flare.machines.adminKeys";

/// Computer sharing is on (the setup finished). Off by default.
pub fn sharing_on(s: &PrefStore) -> bool {
    bool_pref_get(s, SHARING, false)
}

pub fn set_sharing_on(s: &PrefStore, on: bool) {
    bool_pref_set(s, SHARING, on);
}

/// Flare turned the SSH server on (installed it, or started a stopped one) when sharing turned on.
pub fn server_by_flare(s: &PrefStore) -> bool {
    bool_pref_get(s, SERVER_BY_FLARE, false)
}

pub fn set_server_by_flare(s: &PrefStore, on: bool) {
    bool_pref_set(s, SERVER_BY_FLARE, on);
}

/// A paired computer's key went into Windows' `administrators_authorized_keys` (machine pairing PR 3 sets it).
pub fn admin_keys(s: &PrefStore) -> bool {
    bool_pref_get(s, ADMIN_KEYS, false)
}

pub fn set_admin_keys(s: &PrefStore, on: bool) {
    bool_pref_set(s, ADMIN_KEYS, on);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sharing_and_what_flare_changed_default_off_and_keep_what_is_set() {
        let s = PrefStore::memory();
        assert!(!sharing_on(&s) && !server_by_flare(&s) && !admin_keys(&s));
        set_sharing_on(&s, true);
        set_server_by_flare(&s, true);
        set_admin_keys(&s, true);
        assert!(sharing_on(&s) && server_by_flare(&s) && admin_keys(&s));
        assert_eq!(s.get("flare.machines.sharing").as_deref(), Some("1"));
        set_sharing_on(&s, false);
        assert!(!sharing_on(&s));
    }
}

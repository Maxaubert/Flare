//! The dictation probes (M8 Task 27): `dict.view`, `dict.pill`, `dict.mark`, `dict.rows`, `dict.ask`,
//! `dict.lang`, `dict.widget`, `dict.role` and `dict.engine`. Read-only. Their names are
//! `flare_e2e_proto::dict_probes`; Task 27 registers them.

use super::ProbeRegistry;

/// Adds this file's probes to `r` (none until Task 27).
pub fn register(_r: &mut ProbeRegistry) {}

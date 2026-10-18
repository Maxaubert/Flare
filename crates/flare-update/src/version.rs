//! M9 Task 4: versions and the version rules of the offers.
//!
//! Prism Terminal compares with `Number()`, which reads `1e2`, `0x10` and ` 5 ` as numbers. Flare reads
//! only plain digits: any other part counts as 0, so those three oracle pairs compare as Flare says in
//! `tests/fixtures/version.json`.

/// One leading `v` or `V` is dropped, then the version is split on `.`. Each part is a number when it is
/// all ASCII digits and 0 otherwise. A missing part is 0. Only the first three parts are compared.
fn parts(v: &str) -> [u64; 3] {
    let v = v.strip_prefix(['v', 'V']).unwrap_or(v);
    let mut out = [0u64; 3];
    for (slot, part) in out.iter_mut().zip(v.split('.')) {
        *slot = plain_number(part);
    }
    out
}

/// A part as a number: all ASCII digits, else 0 (also 0 when the digits do not fit in a `u64`).
fn plain_number(part: &str) -> u64 {
    if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
        return 0;
    }
    part.parse::<u64>().unwrap_or(0)
}

/// True when `a` is a newer version than `b`. A malformed version never claims an upgrade.
pub fn newer_version(a: &str, b: &str) -> bool {
    let (pa, pb) = (parts(a), parts(b));
    for i in 0..3 {
        if pa[i] != pb[i] {
            return pa[i] > pb[i];
        }
    }
    false
}

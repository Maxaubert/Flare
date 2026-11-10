//! HMAC-SHA256 for the link (Task 20), by hand over `sha2` (RFC 2104), plus the small helpers the link's
//! handshake needs: a constant-time compare, random nonces and hex.

use sha2::{Digest, Sha256};

const BLOCK: usize = 64;

/// `HMAC-SHA256(key, msg)` (RFC 2104). A key longer than the block is hashed first.
pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(k.map(|b| b ^ 0x36));
    inner.update(msg);
    let inner = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(k.map(|b| b ^ 0x5c));
    outer.update(inner);
    outer.finalize().into()
}

/// Whether two MACs are the same, without stopping at the first difference. Empty or unequal lengths are never
/// the same.
pub fn same_mac(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() || a.is_empty() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Fills `buf` from the system's random number generator. False when Windows refuses.
#[cfg(windows)]
pub fn random_bytes(buf: &mut [u8]) -> bool {
    use windows_sys::Win32::Security::Cryptography::{
        BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG,
    };
    // SAFETY: a null algorithm handle with the system-preferred-RNG flag is the documented form; `buf` is
    // a valid writable buffer of the stated length.
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

/// Fills `buf` from the kernel's random number generator (`/dev/urandom`). False when it cannot be read.
#[cfg(not(windows))]
pub fn random_bytes(buf: &mut [u8]) -> bool {
    use std::io::Read;
    std::fs::File::open("/dev/urandom") // sync-ok: a device read of a few bytes, returns at once
        .and_then(|mut f| f.read_exact(buf))
        .is_ok()
}

/// A fresh 16-byte nonce from the system's random number generator (all zeros only if Windows refuses, which
/// it does not for the system-preferred generator).
pub fn nonce() -> [u8; 16] {
    let mut n = [0u8; 16];
    random_bytes(&mut n);
    n
}

/// Lower-case hex.
pub fn to_hex(bytes: &[u8]) -> String {
    const D: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(D[(b >> 4) as usize] as char);
        s.push(D[(b & 15) as usize] as char);
    }
    s
}

/// Lower-case hex of exactly `len` digits to bytes; anything else is `None`.
pub fn from_hex(s: &str, len: usize) -> Option<Vec<u8>> {
    if s.len() != len || !len.is_multiple_of(2) {
        return None;
    }
    s.as_bytes()
        .chunks(2)
        .map(|p| {
            let hi = digit(p[0])?;
            let lo = digit(p[1])?;
            Some(hi << 4 | lo)
        })
        .collect()
}

fn digit(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(s: &str) -> Vec<u8> {
        from_hex(s, s.len()).unwrap()
    }

    // RFC 4231 test cases 1 to 4.
    #[test]
    fn rfc4231_case_1() {
        let key = [0x0bu8; 20];
        assert_eq!(
            to_hex(&hmac_sha256(&key, b"Hi There")),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
    }

    #[test]
    fn rfc4231_case_2() {
        assert_eq!(
            to_hex(&hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn rfc4231_case_3() {
        let key = [0xaau8; 20];
        let msg = [0xddu8; 50];
        assert_eq!(
            to_hex(&hmac_sha256(&key, &msg)),
            "773ea91e36800e46854db8ebd09181a72959098b3ef8c122d9635514ced565fe"
        );
    }

    #[test]
    fn rfc4231_case_4() {
        let key = h("0102030405060708090a0b0c0d0e0f10111213141516171819");
        let msg = [0xcdu8; 50];
        assert_eq!(
            to_hex(&hmac_sha256(&key, &msg)),
            "82558a389a443c0ea4cc819899f2083a85f0faa3e578f8077a2e3ff46729665b"
        );
    }

    #[test]
    fn rfc4231_case_6_a_key_longer_than_the_block() {
        let key = [0xaau8; 131];
        assert_eq!(
            to_hex(&hmac_sha256(
                &key,
                b"Test Using Larger Than Block-Size Key - Hash Key First"
            )),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn same_mac_is_exact() {
        assert!(same_mac(&[1, 2, 3], &[1, 2, 3]));
        assert!(!same_mac(&[1, 2, 3], &[1, 2, 4]));
        assert!(!same_mac(&[1, 2], &[1, 2, 3]));
        assert!(!same_mac(&[], &[]));
    }

    #[test]
    fn nonces_differ_and_are_not_zero() {
        let (a, b) = (nonce(), nonce());
        assert_ne!(a, b);
        assert_ne!(a, [0u8; 16]);
    }

    #[test]
    fn hex_round_trips_and_is_strict() {
        let b = [0u8, 15, 16, 255];
        assert_eq!(to_hex(&b), "000f10ff");
        assert_eq!(from_hex("000f10ff", 8).unwrap(), b);
        assert!(from_hex("000F10ff", 8).is_none());
        assert!(from_hex("000f10f", 7).is_none());
        assert!(from_hex("000f10ff", 6).is_none());
    }
}

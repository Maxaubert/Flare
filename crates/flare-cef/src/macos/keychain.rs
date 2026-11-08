//! Whether `codesign` can use a signing identity's private key without a prompt on the screen (port phase 8,
//! rule 267). Read here, in this process, with the keychain's user interface switched off
//! (`SecKeychainSetUserInteractionAllowed(false)`), so the check itself can never raise one: a call that would need
//! one fails instead. Nothing secret is read: the identity's certificate, and the key's access list (who may use
//! it), never the key.
//!
//! A Developer ID `codesign` is allowed only when all of this holds for every identity the name matches:
//!
//! - the keychain that holds the key is unlocked (a locked one asks for its password);
//! - an entry of the key's access list that allows signing names `/usr/bin/codesign` (or any program) and does not
//!   ask for the password;
//! - the key's partition list holds `apple:`, the partition `/usr/bin/codesign` is in (an OS binary). On Max's Mac
//!   it held only `apple-tool:` (what `security import` gives), and `codesign` put a SecurityAgent prompt on the
//!   screen (2026-11-08). An item with no partition list at all is not partitioned.

use std::ffi::{CString, c_char, c_void};
use std::path::Path;

/// The program the access list must name.
pub const CODESIGN: &str = "/usr/bin/codesign";

/// The partition `/usr/bin/codesign` belongs to.
pub const CODESIGN_PARTITION: &str = "apple:";

/// The access list's "ask for the keychain password" flag (`kSecKeychainPromptRequirePassphase`).
const PROMPT_REQUIRE_PASSPHRASE: u16 = 0x0001;

/// One entry of a key's access list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AclEntry {
    /// `ACLAuthorizationSign`, `ACLAuthorizationPartitionID`, ...
    pub auths: Vec<String>,
    /// The programs it lets in without asking; None: any program.
    pub apps: Option<Vec<String>>,
    /// The prompt selector bits.
    pub prompt: u16,
    /// The entry's description (for the partition entry: its plist, hex encoded).
    pub desc: String,
}

/// The partitions in a partition entry's description (a property list, hex encoded, or plain text).
pub fn partitions(desc: &str) -> Vec<String> {
    let bytes: Option<Vec<u8>> = desc
        .len()
        .is_multiple_of(2)
        .then(|| {
            (0..desc.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(desc.get(i..i + 2)?, 16).ok())
                .collect()
        })
        .flatten();
    let text = bytes
        .and_then(|b| String::from_utf8(b).ok())
        .unwrap_or_else(|| desc.to_string());
    text.split("<string>")
        .skip(1)
        .filter_map(|s| s.split_once("</string>").map(|(v, _)| v.trim().to_string()))
        .collect()
}

/// The verdict on one key: Ok when `codesign` can sign with it without asking; else why not and how to fix it.
pub fn verdict(entries: &[AclEntry], unlocked: bool) -> Result<(), String> {
    const FIX: &str = "To let codesign use the key without asking, run once in Terminal (it asks for the login \
        keychain password there, as text): security set-key-partition-list -S apple-tool:,apple: -s \
        ~/Library/Keychains/login.keychain-db";
    if !unlocked {
        return Err("the keychain that holds the signing key is locked (unlocking it would ask on the screen)".into());
    }
    let signs = |e: &&AclEntry| {
        e.auths
            .iter()
            .any(|a| a == "ACLAuthorizationSign" || a == "ACLAuthorizationAny")
    };
    let silent = entries.iter().filter(signs).any(|e| {
        e.prompt & PROMPT_REQUIRE_PASSPHRASE == 0
            && e.apps
                .as_ref()
                .is_none_or(|apps| apps.iter().any(|a| a.trim_end_matches('\0') == CODESIGN))
    });
    if !silent {
        return Err(format!(
            "the signing key's access list does not let {CODESIGN} sign without asking. {FIX}"
        ));
    }
    let parts: Vec<&AclEntry> = entries
        .iter()
        .filter(|e| e.auths.iter().any(|a| a == "ACLAuthorizationPartitionID"))
        .collect();
    if !parts.is_empty() {
        let all: Vec<String> = parts.iter().flat_map(|e| partitions(&e.desc)).collect();
        if !all.iter().any(|p| p == CODESIGN_PARTITION) {
            return Err(format!(
                "the signing key's partition list is {all:?}, without {CODESIGN_PARTITION:?} (the partition of \
                 {CODESIGN}), so codesign would put a keychain prompt on the screen. {FIX}"
            ));
        }
    }
    Ok(())
}

/// One identity the name matched: its certificate's name, its SHA-1, and its key's state.
#[derive(Clone, Debug)]
pub struct Found {
    pub name: String,
    pub sha1: String,
    pub unlocked: bool,
    pub acl: Vec<AclEntry>,
}

/// Whether `identity` (a certificate name, or part of one, as codesign takes it; or a SHA-1) names this identity.
pub fn matches(identity: &str, name: &str, sha1: &str) -> bool {
    let id = identity.trim();
    !id.is_empty() && (name.contains(id) || sha1.eq_ignore_ascii_case(id))
}

/// Ok when `codesign --sign <identity> [--keychain <keychain>]` can use the key without asking; else why not. Never
/// raises a prompt (see the module docs).
pub fn check(identity: &str, keychain: Option<&Path>) -> Result<(), String> {
    let found = find(identity, keychain)?;
    if found.is_empty() {
        return Err(format!(
            "no signing identity {identity:?} in {}",
            keychain.map_or("the keychain search list".to_string(), |k| k
                .display()
                .to_string())
        ));
    }
    for f in &found {
        verdict(&f.acl, f.unlocked).map_err(|e| format!("{} ({}): {e}", f.name, f.sha1))?;
    }
    Ok(())
}

type Ref = *const c_void;

#[repr(C)]
struct Opaque {
    _p: [u8; 0],
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFBooleanTrue: Ref;
    static kCFTypeDictionaryKeyCallBacks: Opaque;
    static kCFTypeDictionaryValueCallBacks: Opaque;
    static kCFTypeArrayCallBacks: Opaque;
    fn CFDictionaryCreate(
        alloc: Ref,
        keys: *const Ref,
        values: *const Ref,
        n: isize,
        kcb: *const Opaque,
        vcb: *const Opaque,
    ) -> Ref;
    fn CFArrayCreate(alloc: Ref, values: *const Ref, n: isize, cb: *const Opaque) -> Ref;
    fn CFArrayGetCount(a: Ref) -> isize;
    fn CFArrayGetValueAtIndex(a: Ref, i: isize) -> Ref;
    fn CFStringGetLength(s: Ref) -> isize;
    fn CFStringGetCString(s: Ref, buf: *mut c_char, size: isize, enc: u32) -> u8;
    fn CFDataGetLength(d: Ref) -> isize;
    fn CFDataGetBytePtr(d: Ref) -> *const u8;
    fn CFRelease(r: Ref);
}

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    static kSecClass: Ref;
    static kSecClassIdentity: Ref;
    static kSecMatchLimit: Ref;
    static kSecMatchLimitAll: Ref;
    static kSecReturnRef: Ref;
    static kSecMatchSearchList: Ref;
    fn SecKeychainGetUserInteractionAllowed(state: *mut u8) -> i32;
    fn SecKeychainSetUserInteractionAllowed(state: u8) -> i32;
    fn SecKeychainOpen(path: *const c_char, kc: *mut Ref) -> i32;
    fn SecKeychainGetStatus(kc: Ref, status: *mut u32) -> i32;
    fn SecItemCopyMatching(query: Ref, out: *mut Ref) -> i32;
    fn SecIdentityCopyCertificate(id: Ref, cert: *mut Ref) -> i32;
    fn SecIdentityCopyPrivateKey(id: Ref, key: *mut Ref) -> i32;
    fn SecCertificateCopySubjectSummary(cert: Ref) -> Ref;
    fn SecCertificateCopyData(cert: Ref) -> Ref;
    fn SecKeychainItemCopyKeychain(item: Ref, kc: *mut Ref) -> i32;
    fn SecKeychainItemCopyAccess(item: Ref, access: *mut Ref) -> i32;
    fn SecAccessCopyACLList(access: Ref, list: *mut Ref) -> i32;
    fn SecACLCopyAuthorizations(acl: Ref) -> Ref;
    fn SecACLCopyContents(acl: Ref, apps: *mut Ref, desc: *mut Ref, prompt: *mut u16) -> i32;
    fn SecTrustedApplicationCopyData(app: Ref, data: *mut Ref) -> i32;
}

unsafe extern "C" {
    // libSystem (CommonCrypto).
    fn CC_SHA1(data: *const c_void, len: u32, md: *mut u8) -> *mut u8;
}

/// A Core Foundation object this code owns (released on drop).
struct Owned(Ref);

impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the pointer came from a Create or Copy call (this code owns one reference) and is released once.
            unsafe { CFRelease(self.0) }
        }
    }
}

fn cf_string(s: Ref) -> String {
    if s.is_null() {
        return String::new();
    }
    // SAFETY: `s` is a live CFString; the buffer is sized for the longest UTF-8 form plus the NUL.
    unsafe {
        let size = CFStringGetLength(s) * 4 + 1;
        let mut buf = vec![0 as c_char; usize::try_from(size).unwrap_or(1)];
        if CFStringGetCString(s, buf.as_mut_ptr(), size, 0x0800_0100) == 0 {
            return String::new();
        }
        std::ffi::CStr::from_ptr(buf.as_ptr())
            .to_string_lossy()
            .into_owned()
    }
}

fn cf_bytes(d: Ref) -> Vec<u8> {
    if d.is_null() {
        return Vec::new();
    }
    // SAFETY: `d` is a live CFData; its bytes are read while it is alive.
    unsafe {
        let n = usize::try_from(CFDataGetLength(d)).unwrap_or(0);
        let p = CFDataGetBytePtr(d);
        if p.is_null() {
            Vec::new()
        } else {
            std::slice::from_raw_parts(p, n).to_vec()
        }
    }
}

fn cf_array(a: Ref) -> Vec<Ref> {
    if a.is_null() {
        return Vec::new();
    }
    // SAFETY: `a` is a live CFArray; the values are borrowed (not owned) while it is alive.
    unsafe {
        (0..CFArrayGetCount(a))
            .map(|i| CFArrayGetValueAtIndex(a, i))
            .collect()
    }
}

fn sha1_hex(data: &[u8]) -> String {
    let mut md = [0u8; 20];
    // SAFETY: CC_SHA1 reads `data` and writes 20 bytes into `md`.
    unsafe {
        CC_SHA1(
            data.as_ptr().cast(),
            u32::try_from(data.len()).unwrap_or(u32::MAX),
            md.as_mut_ptr(),
        );
    }
    md.iter().map(|b| format!("{b:02X}")).collect()
}

/// Switches the keychain's user interface off for this process, and back to what it was on drop.
struct NoKeychainUi(u8);

impl NoKeychainUi {
    fn new() -> NoKeychainUi {
        let mut was = 1u8;
        // SAFETY: plain calls with an out pointer to a local.
        unsafe {
            SecKeychainGetUserInteractionAllowed(&mut was);
            SecKeychainSetUserInteractionAllowed(0);
        }
        NoKeychainUi(was)
    }
}

impl Drop for NoKeychainUi {
    fn drop(&mut self) {
        // SAFETY: a plain call.
        unsafe {
            SecKeychainSetUserInteractionAllowed(self.0);
        }
    }
}

fn acl_of(key: Ref) -> Vec<AclEntry> {
    let mut access: Ref = std::ptr::null();
    // SAFETY: `key` is a live SecKey (a keychain item); the access object is owned and released by `Owned`.
    if unsafe { SecKeychainItemCopyAccess(key, &mut access) } != 0 {
        return Vec::new();
    }
    let access = Owned(access);
    let mut list: Ref = std::ptr::null();
    // SAFETY: as above.
    if unsafe { SecAccessCopyACLList(access.0, &mut list) } != 0 {
        return Vec::new();
    }
    let list = Owned(list);
    cf_array(list.0)
        .into_iter()
        .map(|acl| {
            // SAFETY: `acl` is a live SecACL borrowed from the list; every copied object is owned by `Owned`.
            unsafe {
                let auths = Owned(SecACLCopyAuthorizations(acl));
                let (mut apps, mut desc, mut prompt) = (std::ptr::null(), std::ptr::null(), 0u16);
                SecACLCopyContents(acl, &mut apps, &mut desc, &mut prompt);
                let (apps, desc) = (Owned(apps), Owned(desc));
                let app_paths = (!apps.0.is_null()).then(|| {
                    cf_array(apps.0)
                        .into_iter()
                        .map(|app| {
                            let mut data: Ref = std::ptr::null();
                            SecTrustedApplicationCopyData(app, &mut data);
                            let data = Owned(data);
                            String::from_utf8_lossy(&cf_bytes(data.0)).into_owned()
                        })
                        .collect()
                });
                AclEntry {
                    auths: cf_array(auths.0).into_iter().map(cf_string).collect(),
                    apps: app_paths,
                    prompt,
                    desc: cf_string(desc.0),
                }
            }
        })
        .collect()
}

/// Every signing identity `identity` names, in `keychain` or the search list, with its key's state.
pub fn find(identity: &str, keychain: Option<&Path>) -> Result<Vec<Found>, String> {
    let _ui = NoKeychainUi::new();
    let kc = match keychain {
        Some(p) => {
            let c = CString::new(p.as_os_str().as_encoded_bytes())
                .map_err(|_| format!("{}: a NUL in the path", p.display()))?;
            let mut kc: Ref = std::ptr::null();
            // SAFETY: a NUL-terminated path and an out pointer; the keychain is owned by `Owned`.
            let st = unsafe { SecKeychainOpen(c.as_ptr(), &mut kc) };
            if st != 0 {
                return Err(format!("{}: SecKeychainOpen {st}", p.display()));
            }
            Some(Owned(kc))
        }
        None => None,
    };
    // SAFETY: the dictionary and array are built from live constants and objects; both are owned by `Owned`.
    let (query, _search) = unsafe {
        let search = kc.as_ref().map(|k| {
            Owned(CFArrayCreate(
                std::ptr::null(),
                &k.0,
                1,
                &raw const kCFTypeArrayCallBacks,
            ))
        });
        let mut keys = vec![kSecClass, kSecMatchLimit, kSecReturnRef];
        let mut vals = vec![kSecClassIdentity, kSecMatchLimitAll, kCFBooleanTrue];
        if let Some(s) = &search {
            keys.push(kSecMatchSearchList);
            vals.push(s.0);
        }
        let q = Owned(CFDictionaryCreate(
            std::ptr::null(),
            keys.as_ptr(),
            vals.as_ptr(),
            isize::try_from(keys.len()).unwrap_or(0),
            &raw const kCFTypeDictionaryKeyCallBacks,
            &raw const kCFTypeDictionaryValueCallBacks,
        ));
        (q, search)
    };
    let mut out: Ref = std::ptr::null();
    // SAFETY: a live query and an out pointer; the result is owned by `Owned`.
    let st = unsafe { SecItemCopyMatching(query.0, &mut out) };
    let out = Owned(out);
    if st == -25300 {
        // errSecItemNotFound
        return Ok(Vec::new());
    }
    if st != 0 {
        return Err(format!("SecItemCopyMatching {st}"));
    }
    let mut found = Vec::new();
    for id in cf_array(out.0) {
        // SAFETY: `id` is a live SecIdentity borrowed from the result; copies are owned by `Owned`.
        unsafe {
            let mut cert: Ref = std::ptr::null();
            if SecIdentityCopyCertificate(id, &mut cert) != 0 {
                continue;
            }
            let cert = Owned(cert);
            let summary = Owned(SecCertificateCopySubjectSummary(cert.0));
            let name = cf_string(summary.0);
            let data = Owned(SecCertificateCopyData(cert.0));
            let sha1 = sha1_hex(&cf_bytes(data.0));
            if !matches(identity, &name, &sha1) {
                continue;
            }
            let mut key: Ref = std::ptr::null();
            if SecIdentityCopyPrivateKey(id, &mut key) != 0 {
                continue;
            }
            let key = Owned(key);
            let mut key_kc: Ref = std::ptr::null();
            let mut status = 0u32;
            let unlocked = SecKeychainItemCopyKeychain(key.0, &mut key_kc) == 0 && {
                let key_kc = Owned(key_kc);
                SecKeychainGetStatus(key_kc.0, &mut status) == 0 && status & 1 != 0
            };
            found.push(Found {
                name,
                sha1,
                unlocked,
                acl: acl_of(key.0),
            });
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> String {
        s.bytes().map(|b| format!("{b:02x}")).collect()
    }

    fn mac_like(partitions: &[&str], apps: Option<Vec<String>>, prompt: u16) -> Vec<AclEntry> {
        let plist: String = partitions
            .iter()
            .map(|p| format!("<string>{p}</string>"))
            .collect();
        vec![
            AclEntry {
                auths: vec!["ACLAuthorizationEncrypt".into()],
                apps: None,
                prompt: 0,
                desc: "devid.key".into(),
            },
            AclEntry {
                auths: vec![
                    "ACLAuthorizationDecrypt".into(),
                    "ACLAuthorizationSign".into(),
                ],
                apps,
                prompt,
                desc: "devid.key".into(),
            },
            AclEntry {
                auths: vec!["ACLAuthorizationPartitionID".into()],
                apps: None,
                prompt: 0,
                desc: hex(&format!(
                    "<plist><dict><key>Partitions</key><array>{plist}</array></dict></plist>"
                )),
            },
        ]
    }

    fn codesign_and_security() -> Option<Vec<String>> {
        Some(vec![
            "/usr/bin/codesign\0".into(),
            "/usr/bin/security\0".into(),
        ])
    }

    #[test]
    fn a_key_codesign_would_have_to_ask_for_is_refused_before_codesign_runs() {
        // Max's key on 2026-11-08: codesign in the list, but the partitions only `apple-tool:`. codesign prompted.
        let e = verdict(
            &mac_like(&["apple-tool:"], codesign_and_security(), 0),
            true,
        )
        .expect_err("refused");
        assert!(
            e.contains("apple-tool:") && e.contains("set-key-partition-list"),
            "{e}"
        );
        // What `set-key-partition-list -S apple-tool:,apple:` makes of it.
        verdict(
            &mac_like(&["apple-tool:", "apple:"], codesign_and_security(), 0),
            true,
        )
        .expect("codesign can sign without asking");
        // Any program, no partition list: not partitioned.
        let mut any = mac_like(&[], None, 0);
        any.pop();
        verdict(&any, true).expect("open key");
        // codesign not in the list, the password asked for, or the keychain locked: refused.
        assert!(
            verdict(
                &mac_like(&["apple:"], Some(vec!["/usr/bin/security\0".into()]), 0),
                true
            )
            .is_err()
        );
        assert!(verdict(&mac_like(&["apple:"], codesign_and_security(), 1), true).is_err());
        assert!(
            verdict(&mac_like(&["apple:"], codesign_and_security(), 0), false)
                .expect_err("locked")
                .contains("locked")
        );
    }

    #[test]
    fn partitions_are_read_from_the_hex_plist_and_a_name_matches_like_codesign() {
        assert_eq!(
            partitions(&hex(
                "<array><string>apple-tool:</string><string>teamid:X</string></array>"
            )),
            ["apple-tool:", "teamid:X"]
        );
        assert_eq!(partitions("<string>apple:</string>"), ["apple:"]);
        assert!(partitions("devid.key").is_empty());
        let name = "Developer ID Application: MAX AUBERT (WV7BY8CUM8)";
        let sha = "0123456789ABCDEF0123456789ABCDEF01234567";
        assert!(matches(name, name, sha));
        assert!(matches("Developer ID Application", name, sha));
        assert!(matches(&sha.to_lowercase(), name, sha));
        assert!(!matches("Apple Development", name, sha));
        assert!(!matches(" ", name, sha));
    }

    #[test]
    fn the_check_runs_with_the_keychain_ui_off_and_finds_no_identity_that_is_not_there() {
        // Reads the real keychain search list, with the user interface off: nothing can be asked on the screen.
        let e = check("Flare test identity that no keychain holds 7f3a", None).expect_err("none");
        assert!(e.contains("no signing identity"), "{e}");
        let mut was = 9u8;
        // SAFETY: a plain call with an out pointer to a local.
        unsafe { SecKeychainGetUserInteractionAllowed(&mut was) };
        assert_eq!(was, 1, "the keychain UI setting is put back");
    }
}

//! Machine pairing PR 3 (rules 1242 and 1244): Flare's own SSH key, the `authorized_keys` line a host adds for a paired
//! computer, and the host key a viewer pins in Flare's own known-hosts file.
//!
//! - **Flare's key** (`id_flare_ed25519` and its `.pub`, in Flare's profile, never in `~/.ssh`): an ed25519 key made the
//!   first time pairing needs it, written in OpenSSH's own format (no passphrase: ssh runs with `BatchMode`). The
//!   private half is only ever written to its file (mode 600 off Windows), never logged, never sent.
//! - **The host's line**: `ssh-ed25519 <key> flare-pair:<name>`. Only an ed25519 key is taken, with no options in front
//!   of it; the same key again replaces its old line.
//! - **The pin**: `<address> <type> <key>` in `flare_known_hosts` in the profile, which a paired machine's ssh reads
//!   with `StrictHostKeyChecking=yes` (rule 1235): ssh never asks, and never trusts another key for that address.
//!
//! The text work is pure and tested; the file writes are BLOCKING (a worker's).

use base64::Engine as _;
use std::path::{Path, PathBuf};

use crate::sharing::KEY_TAG;

/// Flare's own key file in the profile; its public half is this plus `.pub`.
pub const KEY_FILE: &str = "id_flare_ed25519";
/// Flare's own known-hosts file in the profile.
pub const KNOWN_HOSTS_FILE: &str = "flare_known_hosts";
/// The only key type a host takes from a viewer.
pub const ED25519: &str = "ssh-ed25519";
/// The host key types a viewer pins.
pub const HOST_KEY_TYPES: [&str; 4] = [
    "ssh-ed25519",
    "ecdsa-sha2-nistp256",
    "ecdsa-sha2-nistp384",
    "rsa-sha2-512",
];

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// SSH's `string`: a big-endian length, then the bytes.
fn put_string(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(bytes);
}

/// The public key blob of an ed25519 key: `string "ssh-ed25519"`, `string key`.
pub fn ed25519_blob(public: &[u8; 32]) -> Vec<u8> {
    let mut b = Vec::with_capacity(51);
    put_string(&mut b, ED25519.as_bytes());
    put_string(&mut b, public);
    b
}

/// A key's comment: printable ASCII (Windows writes the admin key file in ASCII), a dash for any space, 60 characters
/// at most; `computer` when nothing is left.
pub fn comment_word(name: &str) -> String {
    let w: String = name
        .chars()
        .filter(|c| c.is_ascii() && !c.is_ascii_control())
        .map(|c| if c.is_whitespace() { '-' } else { c })
        .take(60)
        .collect();
    let w = w.trim_matches('-').to_string();
    if w.is_empty() { "computer".into() } else { w }
}

/// Pure: the OpenSSH private key file for the ed25519 key `seed` (no passphrase), its public line, and `check` (the
/// two check words the format repeats, random in a real key).
pub fn openssh_key_files(seed: &[u8; 32], comment: &str, check: u32) -> (String, String) {
    let signing = ed25519_dalek::SigningKey::from_bytes(seed);
    let public = signing.verifying_key().to_bytes();
    let blob = ed25519_blob(&public);
    let mut private = Vec::new();
    private.extend_from_slice(&check.to_be_bytes());
    private.extend_from_slice(&check.to_be_bytes());
    put_string(&mut private, ED25519.as_bytes());
    put_string(&mut private, &public);
    let mut both = [0u8; 64];
    both[..32].copy_from_slice(seed);
    both[32..].copy_from_slice(&public);
    put_string(&mut private, &both);
    put_string(&mut private, comment.as_bytes());
    let mut pad = 1u8;
    while private.len() % 8 != 0 {
        private.push(pad);
        pad += 1;
    }
    let mut body = b"openssh-key-v1\0".to_vec();
    put_string(&mut body, b"none");
    put_string(&mut body, b"none");
    put_string(&mut body, b"");
    body.extend_from_slice(&1u32.to_be_bytes());
    put_string(&mut body, &blob);
    put_string(&mut body, &private);
    let text = B64.encode(&body);
    let mut file = String::from("-----BEGIN OPENSSH PRIVATE KEY-----\n");
    for chunk in text.as_bytes().chunks(70) {
        file.push_str(std::str::from_utf8(chunk).unwrap_or_default());
        file.push('\n');
    }
    file.push_str("-----END OPENSSH PRIVATE KEY-----\n");
    let line = format!("{ED25519} {} {comment}", B64.encode(&blob));
    (file, line)
}

/// A public key line read apart: its type and its base64 key, both checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicKey {
    pub kind: String,
    pub key: String,
}

impl PublicKey {
    /// `<type> <key>`.
    pub fn line(&self) -> String {
        format!("{} {}", self.kind, self.key)
    }
}

/// Pure: reads `<type> <base64> [comment]`. The type must be one of `types`, the base64 must decode, and the blob must
/// name the same type (and, for ed25519, hold 32 bytes). No option may come before the type.
pub fn parse_public_key(line: &str, types: &[&str]) -> Option<PublicKey> {
    if line.contains(['\n', '\r', '\0']) {
        return None;
    }
    let mut words = line.split_ascii_whitespace();
    let kind = words.next()?;
    let key = words.next()?;
    if !types.contains(&kind) || key.len() > 2048 {
        return None;
    }
    let blob = B64.decode(key).ok()?;
    let n = u32::from_be_bytes(blob.get(..4)?.try_into().ok()?) as usize;
    let named = blob.get(4..4 + n)?;
    let named = if kind.starts_with("rsa-sha2") {
        // An RSA key's blob names `ssh-rsa` whatever signature it is used with.
        (named == b"ssh-rsa").then_some(kind)?
    } else {
        std::str::from_utf8(named).ok()?
    };
    if named != kind {
        return None;
    }
    if kind == ED25519 {
        let rest = blob.get(4 + n..)?;
        if rest.len() != 36 || rest[..4] != 32u32.to_be_bytes() {
            return None;
        }
    }
    Some(PublicKey {
        kind: kind.to_string(),
        key: key.to_string(),
    })
}

/// Pure: the line a host adds to `authorized_keys` for a paired computer: its ed25519 key and `flare-pair:<name>`.
/// None when the key is not one plain ed25519 key.
pub fn authorized_line(public: &str, name: &str) -> Option<String> {
    let k = parse_public_key(public, &[ED25519])?;
    Some(format!("{} {KEY_TAG}{}", k.line(), comment_word(name)))
}

/// The key field of an `authorized_keys` line (the word after the type, skipping no options: Flare's lines have none).
fn key_field(line: &str) -> Option<&str> {
    let mut w = line.split_ascii_whitespace();
    let first = w.next()?;
    if first.starts_with('#') {
        return None;
    }
    w.next()
}

/// Pure: `authorized_keys` text with `line` added at the end; a line with the same key goes first (pairing again
/// replaces it). Every other line stays as it was.
pub fn add_authorized_line(text: &str, line: &str) -> String {
    let key = key_field(line).unwrap_or_default();
    let mut out = String::with_capacity(text.len() + line.len() + 1);
    for l in text.split_inclusive('\n') {
        if key_field(l) == Some(key) {
            continue;
        }
        out.push_str(l);
    }
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(line);
    out.push('\n');
    out
}

/// Pure: `authorized_keys` text without the line holding `key` (a paired computer removed), and whether one went.
pub fn remove_authorized_key(text: &str, key: &str) -> (String, bool) {
    let mut out = String::with_capacity(text.len());
    let mut gone = false;
    for l in text.split_inclusive('\n') {
        if key_field(l) == Some(key) {
            gone = true;
        } else {
            out.push_str(l);
        }
    }
    (out, gone)
}

/// The host pattern of a known-hosts line: the address, in brackets with the port when it is not 22.
pub fn host_pattern(address: &str, port: u16) -> String {
    if port == 22 {
        address.to_string()
    } else {
        format!("[{address}]:{port}")
    }
}

/// Pure: known-hosts text with `pattern` pinned to `key` only: every older line for that pattern goes.
pub fn pin_host(text: &str, pattern: &str, key: &PublicKey) -> String {
    let mut out = String::with_capacity(text.len() + 128);
    for l in text.split_inclusive('\n') {
        let first = l.split_ascii_whitespace().next().unwrap_or_default();
        if first.split(',').any(|p| p == pattern) {
            continue;
        }
        out.push_str(l);
    }
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&format!("{pattern} {}\n", key.line()));
    out
}

/// A random u32 from the OS.
fn random_u32() -> Result<u32, String> {
    let mut b = [0u8; 4];
    getrandom::getrandom(&mut b).map_err(|e| e.to_string())?;
    Ok(u32::from_be_bytes(b))
}

/// Flare's pairing files in its profile.
#[derive(Clone, Debug)]
pub struct PairFiles {
    profile: PathBuf,
}

impl PairFiles {
    pub fn new(profile: &Path) -> Self {
        Self {
            profile: profile.to_path_buf(),
        }
    }

    pub fn key_path(&self) -> PathBuf {
        self.profile.join(KEY_FILE)
    }

    pub fn known_hosts_path(&self) -> PathBuf {
        self.profile.join(KNOWN_HOSTS_FILE)
    }

    /// BLOCKING: Flare's public key line, the key made first when there is none (rule 1242). A key file whose public
    /// half is missing or broken is made again.
    pub fn ensure_key(&self, comment: &str) -> Result<String, String> {
        let key = self.key_path();
        let public = key.with_extension("pub");
        if key.exists()
            && let Ok(line) = std::fs::read_to_string(&public) // sync-ok: a pairing worker
            && let Some(k) = parse_public_key(line.trim(), &[ED25519])
        {
            return Ok(k.line());
        }
        let mut seed = [0u8; 32];
        getrandom::getrandom(&mut seed).map_err(|e| format!("no random bytes: {e}"))?;
        let (file, line) = openssh_key_files(&seed, &comment_word(comment), random_u32()?);
        seed.fill(0);
        std::fs::create_dir_all(&self.profile).map_err(|e| e.to_string())?; // sync-ok: a pairing worker
        write_private(&key, &file).map_err(|e| format!("Flare's key could not be saved: {e}"))?;
        std::fs::write(&public, format!("{line}\n")) // sync-ok: a pairing worker
            .map_err(|e| format!("Flare's key could not be saved: {e}"))?;
        log::info!("pairing: made Flare's own SSH key ({})", key.display());
        parse_public_key(&line, &[ED25519])
            .map(|k| k.line())
            .ok_or_else(|| "Flare's key could not be read back".to_string())
    }

    /// BLOCKING: pins `key` for `address:port` in Flare's known-hosts file (written whole, rule 36). The file's path.
    pub fn pin(&self, address: &str, port: u16, key: &PublicKey) -> Result<PathBuf, String> {
        let file = self.known_hosts_path();
        let old = std::fs::read_to_string(&file).unwrap_or_default(); // sync-ok: a pairing worker
        let text = pin_host(&old, &host_pattern(address, port), key);
        flare_settings::atomic::write_atomic(&file, &text)
            .map_err(|e| format!("Flare's known hosts could not be saved: {e}"))?;
        Ok(file)
    }
}

/// Writes a private key readable by its owner only: mode 600 off Windows (ssh refuses a key others can read); on
/// Windows the profile folder's own access (the user, SYSTEM, Administrators) is what ssh accepts.
fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        let _ = std::fs::remove_file(path); // sync-ok: a pairing worker
        let mut f = std::fs::OpenOptions::new() // sync-ok: a pairing worker
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?; // sync-ok: a pairing worker
        f.write_all(text.as_bytes())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, text) // sync-ok: a pairing worker
    }
}

/// BLOCKING: adds `line` to the user's own `authorized_keys` under `home` (rule 1244): `~/.ssh` made with mode 700 and
/// the file with mode 600 off Windows (sshd's StrictModes), the same key's older line replaced.
pub fn add_user_key(home: &Path, line: &str) -> Result<(), String> {
    let dir = home.join(".ssh");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?; // sync-ok: a pairing worker
    let file = dir.join("authorized_keys");
    let old = match std::fs::read_to_string(&file) {
        // sync-ok: a pairing worker
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.to_string()),
    };
    std::fs::write(&file, add_authorized_line(&old, line)).map_err(|e| e.to_string())?; // sync-ok: a pairing worker
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let set =
            |p: &Path, m: u32| std::fs::set_permissions(p, std::fs::Permissions::from_mode(m)); // sync-ok: a pairing worker
        set(&dir, 0o700).map_err(|e| e.to_string())?;
        set(&file, 0o600).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// BLOCKING: removes the line holding `key` from the user's own `authorized_keys` under `home`; whether one went.
pub fn remove_user_key(home: &Path, key: &str) -> Result<bool, String> {
    let file = home.join(".ssh").join("authorized_keys");
    let old = match std::fs::read_to_string(&file) {
        // sync-ok: a pairing worker
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.to_string()),
    };
    let (text, gone) = remove_authorized_key(&old, key);
    if gone {
        // Written in place: the file keeps its owner and mode.
        std::fs::write(&file, text).map_err(|e| e.to_string())?; // sync-ok: a pairing worker
    }
    Ok(gone)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rule 1242: Flare's key is an OpenSSH ed25519 key with no passphrase, in the format ssh reads, and its public
    /// line names the same key.
    #[test]
    fn flares_key_is_an_openssh_ed25519_key_whose_public_line_matches() {
        let seed = [7u8; 32];
        let (file, line) = openssh_key_files(&seed, "flare", 0x0102_0304);
        assert!(file.starts_with("-----BEGIN OPENSSH PRIVATE KEY-----\n"));
        assert!(file.ends_with("-----END OPENSSH PRIVATE KEY-----\n"));
        let body: String = file
            .lines()
            .filter(|l| !l.starts_with("-----"))
            .inspect(|l| assert!(l.len() <= 70))
            .collect();
        let raw = B64.decode(body).unwrap();
        assert!(raw.starts_with(b"openssh-key-v1\0\0\0\0\x04none\0\0\0\x04none\0\0\0\0\0\0\0\x01"));
        let k = parse_public_key(&line, &[ED25519]).expect("a plain ed25519 line");
        assert!(line.ends_with(" flare"));
        let public = ed25519_dalek::SigningKey::from_bytes(&seed)
            .verifying_key()
            .to_bytes();
        assert_eq!(B64.decode(&k.key).unwrap(), ed25519_blob(&public));
        // The private section repeats its check word and is padded to a multiple of 8.
        let at = raw
            .windows(8)
            .position(|w| w == [1, 2, 3, 4, 1, 2, 3, 4])
            .expect("the check words");
        let len = u32::from_be_bytes(raw[at - 4..at].try_into().unwrap()) as usize;
        assert_eq!(len % 8, 0);
        assert_eq!(at + len, raw.len());
        // The seed is the private key; it never shows in the public line.
        assert!(!line.contains(&B64.encode(seed)));
    }

    #[test]
    fn the_key_is_made_once_and_read_back_after() {
        let dir = tempfile::tempdir().unwrap();
        let files = PairFiles::new(dir.path());
        let first = files.ensure_key("Max's PC").unwrap();
        assert!(first.starts_with("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI"));
        assert!(files.key_path().exists());
        assert_eq!(files.ensure_key("other").unwrap(), first, "made once");
        let pubtext = std::fs::read_to_string(files.key_path().with_extension("pub")).unwrap();
        assert!(pubtext.trim_end().ends_with(" Max's-PC"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(files.key_path())
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    /// Rule 1244: the host takes one plain ed25519 key with no options, tagged `flare-pair:<name>`.
    #[test]
    fn the_authorized_keys_line_is_one_plain_ed25519_key_tagged_with_the_name() {
        let (_, line) = openssh_key_files(&[9u8; 32], "x", 1);
        let key = parse_public_key(&line, &[ED25519]).unwrap();
        assert_eq!(
            authorized_line(&line, "Max's Mac mini").unwrap(),
            format!("{} flare-pair:Max's-Mac-mini", key.line())
        );
        assert_eq!(
            authorized_line(&line, " \t").unwrap(),
            format!("{} flare-pair:computer", key.line())
        );
        // Options, other types, broken base64, a short key or a second line are refused.
        for bad in [
            format!("command=\"x\" {line}"),
            format!("{line}\nssh-ed25519 AAAA evil"),
            "ssh-rsa AAAAB3NzaC1yc2EAAAADAQAB x".to_string(),
            "ssh-ed25519 not-base64!! x".to_string(),
            format!(
                "ssh-ed25519 {} x",
                B64.encode(b"\0\0\0\x0bssh-ed25519\0\0\0\x02ab")
            ),
            format!("ssh-ed25519 {} x", B64.encode(b"\0\0\0\x07ssh-rsa")),
        ] {
            assert_eq!(authorized_line(&bad, "n"), None, "{bad}");
        }
    }

    #[test]
    fn pairing_again_replaces_the_same_keys_line_and_keeps_every_other() {
        let (_, a) = openssh_key_files(&[1u8; 32], "a", 1);
        let (_, b) = openssh_key_files(&[2u8; 32], "b", 1);
        let la = authorized_line(&a, "A").unwrap();
        let lb = authorized_line(&b, "B").unwrap();
        let mine = "# my keys\nssh-rsa AAAAB3Nza mine@laptop";
        let one = add_authorized_line(mine, &la);
        assert_eq!(one, format!("{mine}\n{la}\n"));
        let two = add_authorized_line(&one, &lb);
        let again = add_authorized_line(&two, &authorized_line(&a, "A renamed").unwrap());
        assert_eq!(again.matches(&la[..60]).count(), 1, "one line per key");
        assert!(again.contains("flare-pair:A-renamed") && !again.contains("flare-pair:A\n"));
        assert!(again.starts_with(mine));
        assert_eq!(add_authorized_line("", &la), format!("{la}\n"));
        let key = parse_public_key(&a, &[ED25519]).unwrap().key;
        let (left, gone) = remove_authorized_key(&again, &key);
        assert!(gone && !left.contains(&key) && left.contains(&lb) && left.starts_with(mine));
        assert!(!remove_authorized_key(&left, &key).1);
    }

    #[test]
    fn a_user_key_file_is_written_with_its_folder_and_the_key_removed_again() {
        let home = tempfile::tempdir().unwrap();
        let (_, a) = openssh_key_files(&[1u8; 32], "a", 1);
        let line = authorized_line(&a, "A").unwrap();
        add_user_key(home.path(), &line).unwrap();
        add_user_key(home.path(), &line).unwrap();
        let file = home.path().join(".ssh").join("authorized_keys");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), format!("{line}\n"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let m = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(m(&home.path().join(".ssh")), 0o700);
            assert_eq!(m(&file), 0o600);
        }
        let key = parse_public_key(&a, &[ED25519]).unwrap().key;
        assert!(remove_user_key(home.path(), &key).unwrap());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "");
    }

    /// Rule 1245: the viewer pins the host's key for the address it reached, in Flare's own file; pairing again
    /// replaces the pin, and no other address's line is touched.
    #[test]
    fn the_host_key_is_pinned_for_its_address_alone() {
        let (_, a) = openssh_key_files(&[1u8; 32], "a", 1);
        let (_, b) = openssh_key_files(&[2u8; 32], "b", 1);
        let ka = parse_public_key(&a, &HOST_KEY_TYPES).unwrap();
        let kb = parse_public_key(&b, &HOST_KEY_TYPES).unwrap();
        let one = pin_host("", "192.168.0.139", &ka);
        assert_eq!(one, format!("192.168.0.139 {}\n", ka.line()));
        let two = pin_host(&one, &host_pattern("192.168.0.120", 2222), &kb);
        assert!(two.contains(&format!("[192.168.0.120]:2222 {}", kb.line())));
        let again = pin_host(&two, "192.168.0.139", &kb);
        assert_eq!(again.matches("192.168.0.139 ").count(), 1);
        assert!(again.contains(&format!("192.168.0.139 {}", kb.line())));
        assert!(!again.contains(&ka.key));
        let dir = tempfile::tempdir().unwrap();
        let files = PairFiles::new(dir.path());
        let path = files.pin("10.0.0.2", 22, &ka).unwrap();
        assert_eq!(path, dir.path().join(KNOWN_HOSTS_FILE));
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            format!("10.0.0.2 {}\n", ka.line())
        );
        assert!(crate::machines::path_word(
            &files.known_hosts_path().to_string_lossy()
        ));
    }
}

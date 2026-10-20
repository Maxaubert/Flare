//! The e2e fixture cache for the dictation scenarios (M8 Task 28): ggml-tiny.bin, jfk.wav and the
//! Parakeet q4 file, filled only by --fetch-fixtures, never by a scenario.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use flare_dictation::catalog;
use flare_dictation::fetch_ureq::UreqFetch;
use flare_dictation::store::Fetch;
use sha2::{Digest, Sha256};

/// whisper.cpp's sample clip of a known sentence (PT's `JFK`), pinned by commit.
pub const JFK_URL: &str = "https://raw.githubusercontent.com/ggml-org/whisper.cpp/b0a11594aec50892a02cd8d129eee2dfe93a8bb8/samples/jfk.wav";
pub const JFK_SHA256: &str = "59dfb9a4acb36fe2a2affc14bacbee2920ff435cb13cc314a08c13f66ba7860e";

/// One file the dictation scenarios need: its cache name, where it comes from and its SHA-256.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fixture {
    pub name: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
}

/// The three fixtures, read from the catalogue where the catalogue has them.
pub fn fixtures() -> Vec<Fixture> {
    // The file name is the last segment of the pinned url, as PT's cache names them.
    let file_name = |url: &'static str| url.rsplit('/').next().unwrap_or(url);
    let tiny = catalog::model("tiny").expect("the catalogue has the tiny model");
    let parakeet =
        catalog::model("parakeet-v3-q4").expect("the catalogue has the e2e Parakeet file");
    vec![
        Fixture {
            name: file_name(tiny.url),
            url: tiny.url,
            sha256: tiny.sha256,
        },
        Fixture {
            name: "jfk.wav",
            url: JFK_URL,
            sha256: JFK_SHA256,
        },
        Fixture {
            name: file_name(parakeet.url),
            url: parakeet.url,
            sha256: parakeet.sha256,
        },
    ]
}

/// The fixture cache: `tools/e2e/.e2e-cache` (gitignored).
pub fn cache_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(".e2e-cache")
}

/// The SHA-256 of a file, read in pieces (the Parakeet file is 356 MB).
fn sha256_of(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    io::copy(&mut file, &mut hasher).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(format!("{:x}", hasher.finalize()))
}

/// The cached file for `name`, if it is there AND its SHA-256 is the pinned one. Never downloads.
pub fn require_in(dir: &Path, name: &str) -> Result<PathBuf, String> {
    let fixture = fixtures()
        .into_iter()
        .find(|f| f.name == name)
        .ok_or_else(|| format!("{name} is not a dictation fixture"))?;
    let file = dir.join(fixture.name);
    if !file.is_file() {
        return Err(format!(
            "{name} is not in the fixture cache: run `cargo e2e -- --fetch-fixtures` first"
        ));
    }
    if sha256_of(&file)? != fixture.sha256 {
        return Err(format!(
            "{name} fails its SHA-256 check: run `cargo e2e -- --fetch-fixtures` again"
        ));
    }
    Ok(file)
}

/// `require_in` on the real cache.
pub fn require(name: &str) -> Result<PathBuf, String> {
    require_in(&cache_dir(), name)
}

/// Downloads every fixture that is not yet in `dir` with the pinned hash. Each file is written to
/// `<name>.part`, checked, then renamed; a file that fails its check is deleted.
pub fn fetch_all_into(dir: &Path, fetch: &dyn Fetch) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for f in fixtures() {
        let file = dir.join(f.name);
        if file.is_file() && sha256_of(&file)? == f.sha256 {
            println!("  {} is in the cache", f.name);
            continue;
        }
        println!("  fetching {} into the cache", f.name);
        let part = dir.join(format!("{}.part", f.name));
        let mut out = fs::File::create(&part).map_err(|e| format!("{}: {e}", part.display()))?;
        let mut hasher = Sha256::new();
        let mut write_failed = false;
        let fetched = {
            let mut sink = |chunk: &[u8]| {
                if chunk.is_empty() {
                    return true; // a cancel check from the fetch, nothing to write
                }
                hasher.update(chunk);
                if out.write_all(chunk).is_err() {
                    write_failed = true;
                    return false;
                }
                true
            };
            fetch.get(f.url, &mut sink)
        };
        drop(out);
        if write_failed {
            let _ = fs::remove_file(&part);
            return Err(format!("{}: the cache could not be written", f.name));
        }
        if let Err(e) = fetched {
            let _ = fs::remove_file(&part);
            return Err(format!("{}: {}", f.name, e.text()));
        }
        let got = format!("{:x}", hasher.finalize());
        if got != f.sha256 {
            // A file that fails its pin is deleted, never kept under the real name (rule 92).
            let _ = fs::remove_file(&part);
            return Err(format!("{}: SHA-256 mismatch ({got})", f.name));
        }
        fs::rename(&part, &file).map_err(|e| format!("{}: {e}", file.display()))?;
        println!("  {} is in the cache", f.name);
    }
    Ok(())
}

/// `fetch_all_into` on the real cache with the real HTTPS fetch. Behind `--fetch-fixtures` only.
pub fn fetch_all() -> Result<(), String> {
    fetch_all_into(&cache_dir(), &UreqFetch::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn require_never_downloads() {
        let dir = tempfile::tempdir().expect("tempdir");
        let err = require_in(dir.path(), "ggml-tiny.bin").expect_err("nothing is cached");
        assert!(err.contains("cargo e2e -- --fetch-fixtures"), "{err}");
        // Nothing was fetched or written: the cache folder is still empty.
        assert!(fs::read_dir(dir.path()).expect("read").next().is_none());
    }

    #[test]
    fn a_cached_file_that_fails_its_pin_is_refused_and_left_alone() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("jfk.wav"), b"not the clip").expect("write");
        let err = require_in(dir.path(), "jfk.wav").expect_err("wrong bytes");
        assert!(err.contains("SHA-256"), "{err}");
        assert!(dir.path().join("jfk.wav").is_file());
    }

    #[test]
    fn the_fixtures_are_the_tiny_model_the_clip_and_the_parakeet_file_with_their_pins() {
        let all = fixtures();
        let names: Vec<&str> = all.iter().map(|f| f.name).collect();
        assert_eq!(
            names,
            vec![
                "ggml-tiny.bin",
                "jfk.wav",
                "ggml-parakeet-tdt-0.6b-v3-q4_0.bin"
            ]
        );
        assert_eq!(all[1].url, JFK_URL);
        assert_eq!(all[1].sha256, JFK_SHA256);
        for f in &all {
            assert!(f.url.starts_with("https://"), "{}", f.name);
            assert_eq!(f.sha256.len(), 64, "{}", f.name);
        }
    }

    #[test]
    fn fetch_refuses_bytes_that_fail_the_pin_and_leaves_no_part_file() {
        struct Wrong;
        impl Fetch for Wrong {
            fn get(
                &self,
                _url: &str,
                sink: &mut dyn FnMut(&[u8]) -> bool,
            ) -> Result<(), flare_dictation::store::Failure> {
                sink(b"not a model");
                Ok(())
            }
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let err = fetch_all_into(dir.path(), &Wrong).expect_err("the pin refuses it");
        assert!(err.contains("SHA-256"), "{err}");
        assert!(fs::read_dir(dir.path()).expect("read").all(|e| !e
            .expect("entry")
            .file_name()
            .to_string_lossy()
            .ends_with(".part")));
        assert!(!dir.path().join("ggml-tiny.bin").exists());
    }
}

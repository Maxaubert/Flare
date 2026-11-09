//! Dictation preferences and the import (Task 7). Ported from Prism Terminal's
//! `core/renderer/lib/dictationPrefs.ts` at e5848cb, with Flare's keys and Flare's default key.
//!
//! Values are per app. OFF BY DEFAULT: every reader treats a missing key as off, so nothing listens,
//! downloads or runs until the user says so. Each reader takes the stored text (or nothing) and
//! returns a value that is safe to use; a damaged value reads as its default, never as an error.

use serde_json::Value;

use crate::catalog::valid_language;
use crate::hotkey::{default_hotkey, usable_hotkey_here, DictMode, Hotkey};

/// Flare's stored dictation keys, in the order of the Dictation page (the import writes them too).
pub const KEYS: [&str; 9] = [
    "flare.dictation.enabled",
    "flare.dictation.mode",
    "flare.dictation.hotkey",
    "flare.dictation.mic",
    "flare.dictation.language",
    "flare.dictation.pauseMedia",
    "flare.dictation.sounds",
    "flare.dictation.model",
    "flare.dictation.anywhere",
];

/// The dictation preferences as this copy reads them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DictPrefs {
    pub enabled: bool,
    pub mode: DictMode,
    pub hotkey: Hotkey,
    /// '' is the system default input.
    pub mic: String,
    /// `auto`, or a language code Whisper knows.
    pub language: String,
    pub pause_media: bool,
    /// On unless switched off: the cue is how you know the microphone opened.
    pub sounds: bool,
    /// The active model's catalogue id, '' when none was chosen.
    pub model: String,
    /// On unless switched off (N12).
    pub anywhere: bool,
}

/// Reads every dictation key through `kv` (the stored text, or None). `app_keys` are the keys this
/// app reserves for itself, which can never be the dictation key.
pub fn read(kv: &dyn Fn(&str) -> Option<String>, app_keys: &[Hotkey]) -> DictPrefs {
    let get = |key: &str| kv(key);
    DictPrefs {
        enabled: get(KEYS[0]).as_deref() == Some("1"),
        mode: if get(KEYS[1]).as_deref() == Some("toggle") {
            DictMode::Toggle
        } else {
            DictMode::Hold
        },
        hotkey: read_hotkey(get(KEYS[2]).as_deref(), app_keys),
        mic: get(KEYS[3]).unwrap_or_default(),
        language: valid_language(get(KEYS[4]).as_deref().unwrap_or("")).to_string(),
        pause_media: get(KEYS[5]).as_deref() == Some("1"),
        sounds: get(KEYS[6]).as_deref() != Some("0"),
        model: get(KEYS[7]).unwrap_or_default(),
        anywhere: get(KEYS[8]).as_deref() != Some("0"),
    }
}

/// The stored hotkey JSON, or the default when it is missing, damaged, has no key, or is a key
/// that types (it would be swallowed or typed away from the user).
fn read_hotkey(raw: Option<&str>, app_keys: &[Hotkey]) -> Hotkey {
    let Some(raw) = raw.filter(|r| !r.is_empty()) else {
        return default_hotkey();
    };
    let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(raw) else {
        return default_hotkey();
    };
    let Some(code) = obj
        .get("code")
        .and_then(Value::as_str)
        .filter(|c| !c.is_empty())
    else {
        return default_hotkey();
    };
    let flag = |k: &str| obj.get(k).and_then(Value::as_bool) == Some(true);
    let hk = Hotkey {
        code: code.to_string(),
        ctrl: flag("ctrl"),
        alt: flag("alt"),
        shift: flag("shift"),
        meta: flag("meta"),
    };
    if usable_hotkey_here(&hk, app_keys) {
        hk
    } else {
        default_hotkey()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hotkey::{default_hotkey, DictMode, Hotkey};

    /// A stored-value lookup that knows one key.
    fn only(k: &'static str, v: &'static str) -> impl Fn(&str) -> Option<String> {
        move |key: &str| (key == k).then(|| v.to_string())
    }

    fn none(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn defaults_are_off_hold_ctrl_win_auto_sounds_on_anywhere_on() {
        let p = read(&none, &[]);
        assert_eq!(
            p,
            DictPrefs {
                enabled: false,
                mode: DictMode::Hold,
                hotkey: default_hotkey(),
                mic: String::new(),
                language: "auto".into(),
                pause_media: false,
                sounds: true,
                model: String::new(),
                anywhere: true,
            }
        );
    }

    #[test]
    fn a_damaged_or_unusable_hotkey_reads_the_default() {
        let bad = [
            "{nope",
            "",
            r#"{"code":""}"#,
            r#"{"code":5}"#,
            r#"{"code":"KeyR"}"#,
            r#"{"code":"Digit1","ctrl":true}"#,
            r#"{"code":"KeyC","ctrl":true}"#,
        ];
        for raw in bad {
            let kv = only("flare.dictation.hotkey", raw);
            assert_eq!(read(&kv, &[]).hotkey, default_hotkey(), "{raw}");
        }
    }

    #[test]
    fn a_usable_stored_hotkey_is_read() {
        let kv = only(
            "flare.dictation.hotkey",
            "{\"code\":\"F9\",\"ctrl\":false,\"alt\":false,\"shift\":false,\"meta\":false}",
        );
        assert_eq!(
            read(&kv, &[]).hotkey,
            Hotkey {
                code: "F9".into(),
                ctrl: false,
                alt: false,
                shift: false,
                meta: false,
            }
        );
    }

    #[test]
    fn a_stored_right_alt_is_kept() {
        let raw = r#"{"code":"AltRight","ctrl":false,"alt":false,"shift":false,"meta":false}"#;
        let kv = only("flare.dictation.hotkey", raw);
        assert_eq!(
            read(&kv, &[]).hotkey,
            Hotkey {
                code: "AltRight".into(),
                ctrl: false,
                alt: false,
                shift: false,
                meta: false,
            }
        );
    }

    #[test]
    fn language_outside_the_list_reads_auto() {
        assert_eq!(
            read(&only("flare.dictation.language", "no"), &[]).language,
            "no"
        );
        assert_eq!(
            read(&only("flare.dictation.language", "xx"), &[]).language,
            "auto"
        );
        assert_eq!(
            read(&only("flare.dictation.language", "NO"), &[]).language,
            "auto"
        );
        assert_eq!(
            read(&only("flare.dictation.language", "en-US"), &[]).language,
            "auto"
        );
        assert_eq!(
            read(&only("flare.dictation.language", ""), &[]).language,
            "auto"
        );
    }

    #[test]
    fn the_flags_mode_and_strings_read_as_stored() {
        let on = |k: &'static str, v: &'static str| read(&only(k, v), &[]);
        assert!(on("flare.dictation.enabled", "1").enabled);
        assert!(!on("flare.dictation.enabled", "0").enabled);
        assert!(!on("flare.dictation.enabled", "yes").enabled);
        assert_eq!(on("flare.dictation.mode", "toggle").mode, DictMode::Toggle);
        assert_eq!(on("flare.dictation.mode", "push").mode, DictMode::Hold);
        assert!(on("flare.dictation.pauseMedia", "1").pause_media);
        assert!(!on("flare.dictation.pauseMedia", "0").pause_media);
        assert!(!on("flare.dictation.sounds", "0").sounds);
        assert!(on("flare.dictation.sounds", "1").sounds);
        assert!(!on("flare.dictation.anywhere", "0").anywhere);
        assert!(on("flare.dictation.anywhere", "1").anywhere);
        assert_eq!(on("flare.dictation.mic", "device-1").mic, "device-1");
        assert_eq!(
            on("flare.dictation.model", "parakeet-v3").model,
            "parakeet-v3"
        );
    }

    #[test]
    fn every_dictation_key_has_a_reader() {
        let cases: [(&str, &str); 9] = [
            ("flare.dictation.enabled", "1"),
            ("flare.dictation.mode", "toggle"),
            (
                "flare.dictation.hotkey",
                r#"{"code":"F9","ctrl":false,"alt":false,"shift":false,"meta":false}"#,
            ),
            ("flare.dictation.mic", "device-1"),
            ("flare.dictation.language", "no"),
            ("flare.dictation.pauseMedia", "1"),
            ("flare.dictation.sounds", "0"),
            ("flare.dictation.model", "parakeet-v3"),
            ("flare.dictation.anywhere", "0"),
        ];
        assert_eq!(KEYS.len(), cases.len());
        let base = read(&none, &[]);
        for key in KEYS {
            assert!(key.starts_with("flare.dictation."), "{key}");
            let value: &'static str = cases
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| *v)
                .unwrap_or_else(|| panic!("{key} has no case"));
            let got = read(&only(key, value), &[]);
            assert_ne!(got, base, "{key} changes nothing when set");
        }
    }
}

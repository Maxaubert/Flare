//! The model catalogue: ids, sizes, pinned hashes (Task 3). Ported from Prism Terminal's
//! `core/shared/dictationCatalog.ts` at e5848cb. Nothing is run or loaded unless its bytes match a
//! SHA-256 written here, so every url names bytes that cannot change under its checksum: a GitHub
//! release asset by tag, and a Hugging Face file by commit, never `resolve/main`.

/// What runs a model: `whisper-server` for Whisper, the resident host or `parakeet-cli` for Parakeet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runner {
    Whisper,
    Parakeet,
}

/// One downloadable model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelEntry {
    pub id: &'static str,
    pub name: &'static str,
    pub url: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
    pub runner: Runner,
    pub needs_gpu: bool,
    pub limited_languages: bool,
    pub note: &'static str,
    pub test_only: bool,
}

/// The NVIDIA GPU pack, unzipped beside the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackEntry {
    pub id: &'static str,
    pub name: &'static str,
    pub url: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
    pub note: &'static str,
}

/// The CPU engine Flare bundles, fetched at build time from one pinned release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnginePin {
    pub tag: &'static str,
    pub asset: &'static str,
    pub url: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
    pub files: &'static [&'static str],
}

/// The engine every copy bundles: whisper.cpp b5130, with Parakeet's runner from the same zip.
pub const ENGINE: EnginePin = EnginePin {
    tag: "b5130",
    asset: "whisper-bin-x64.zip",
    url: "https://github.com/ggml-org/whisper.cpp/releases/download/b5130/whisper-bin-x64.zip",
    bytes: 8_573_270,
    sha256: "f9ec6c52a2e949b62ab51fa21d0d497958f9e41c3010c157c4e42932d5316f3c",
    files: &[
        "whisper-server.exe",
        "whisper.dll",
        "ggml.dll",
        "ggml-base.dll",
        "ggml-cpu-alderlake.dll",
        "ggml-cpu-cannonlake.dll",
        "ggml-cpu-cascadelake.dll",
        "ggml-cpu-haswell.dll",
        "ggml-cpu-icelake.dll",
        "ggml-cpu-sandybridge.dll",
        "ggml-cpu-skylakex.dll",
        "ggml-cpu-sse42.dll",
        "ggml-cpu-x64.dll",
        "parakeet-cli.exe",
        "parakeet.dll",
    ],
};

/// The engine a Mac bundles (port phase 7, rule 1084): built from whisper.cpp's source at one commit, because the
/// release Windows takes its zip from carries no macOS program. Pinned by the commit and its tree hash
/// (`tools/build-whisper-macos.sh` refuses any other source); Metal is built in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourcePin {
    pub repo: &'static str,
    /// The release tag the Windows engine is from: the same sources on both systems.
    pub tag: &'static str,
    pub commit: &'static str,
    pub tree: &'static str,
    /// The programs the engine folder holds (the libraries they link to come beside them).
    pub programs: &'static [&'static str],
}

/// The macOS engine: the commit tag b5130 names, the tag the Windows zip is from.
pub const ENGINE_MACOS: SourcePin = SourcePin {
    repo: "https://github.com/ggml-org/whisper.cpp.git",
    tag: "b5130",
    commit: "927cfce34f31707e17f2bff35c349632fb9e2c3a",
    tree: "db312954f67ca741569ac3a889a9ff329e51d952",
    programs: &["whisper-server", "parakeet-cli"],
};

/// Whether the GPU pack is offered at all: on Windows only. A Mac's bundled engine is built with Metal (rule 1082).
pub const PACK_OFFERED: bool = cfg!(windows);

/// The GPU pack for every NVIDIA card (one pack, from the same release as the engine).
pub const GPU_PACK: PackEntry = PackEntry {
    id: "gpu-pack",
    name: "NVIDIA GPU acceleration",
    url: "https://github.com/ggml-org/whisper.cpp/releases/download/b5130/whisper-cublas-12.4.0-bin-x64.zip",
    bytes: 674_539_285,
    sha256: "af520ddd034d985b55dfeea3e465ed93653ba2aee1a55e865033edc548c272a7",
    note: "Large models answer in under a second.",
};

/// Every model, in Prism Terminal's order: the five the manager shows, then the two test-only ones.
pub const MODELS: &[ModelEntry] = &[
    ModelEntry {
        id: "base",
        name: "Whisper Base",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-base.bin",
        bytes: 147_951_465,
        sha256: "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe",
        runner: Runner::Whisper,
        needs_gpu: false,
        limited_languages: false,
        note: "Fast live text on any PC.",
        test_only: false,
    },
    ModelEntry {
        id: "small",
        name: "Whisper Small",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-small.bin",
        bytes: 487_601_967,
        sha256: "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
        runner: Runner::Whisper,
        needs_gpu: false,
        limited_languages: false,
        note: "More accurate, about three times slower.",
        test_only: false,
    },
    ModelEntry {
        id: "parakeet-v3",
        name: "Parakeet v3",
        url: "https://huggingface.co/ggml-org/parakeet-GGUF/resolve/35156454d1a39de06863303dd209fd2bed6ee079/ggml-parakeet-tdt-0.6b-v3-q8_0.bin",
        bytes: 668_757_119,
        sha256: "4d64e9e96c2792186d072fde0034df0ad670cf680a2f53069052ead827fd600e",
        runner: Runner::Parakeet,
        needs_gpu: false,
        limited_languages: true,
        note: "Fast and accurate, in fewer languages.",
        test_only: false,
    },
    ModelEntry {
        id: "large-v3-turbo",
        name: "Whisper Large v3 Turbo",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-large-v3-turbo.bin",
        bytes: 1_624_555_275,
        sha256: "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69",
        runner: Runner::Whisper,
        needs_gpu: true,
        limited_languages: false,
        note: "Close to Large v3 at half the download.",
        test_only: false,
    },
    ModelEntry {
        id: "large-v3",
        name: "Whisper Large v3",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-large-v3.bin",
        bytes: 3_095_033_483,
        sha256: "64d182b440b98d5203c4f9bd541544d84c605196c4f7b845dfa11fb23594d1e2",
        runner: Runner::Whisper,
        needs_gpu: true,
        limited_languages: false,
        note: "The most accurate.",
        test_only: false,
    },
    ModelEntry {
        id: "tiny",
        name: "Whisper Tiny",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-tiny.bin",
        bytes: 77_691_713,
        sha256: "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21",
        runner: Runner::Whisper,
        needs_gpu: false,
        limited_languages: false,
        note: "For the automated tests only.",
        test_only: true,
    },
    ModelEntry {
        id: "parakeet-v3-q4",
        name: "Parakeet v3 (4 bit)",
        url: "https://huggingface.co/ggml-org/parakeet-GGUF/resolve/35156454d1a39de06863303dd209fd2bed6ee079/ggml-parakeet-tdt-0.6b-v3-q4_0.bin",
        bytes: 355_615_679,
        sha256: "aa7fe2f5fb47d863ca23e8b1d490632d63a2599f515268b6d6bd656158dad45e",
        runner: Runner::Parakeet,
        needs_gpu: false,
        limited_languages: true,
        note: "For the automated tests only.",
        test_only: true,
    },
];

/// The language picker: `auto` first, then Prism Terminal's order. Codes are whisper.cpp's `-l` values.
pub const LANGUAGES: &[(&str, &str)] = &[
    ("auto", "Auto-detect"),
    ("en", "English"),
    ("no", "Norwegian"),
    ("sv", "Swedish"),
    ("da", "Danish"),
    ("fi", "Finnish"),
    ("ar", "Arabic"),
    ("zh", "Chinese"),
    ("cs", "Czech"),
    ("nl", "Dutch"),
    ("fr", "French"),
    ("de", "German"),
    ("el", "Greek"),
    ("he", "Hebrew"),
    ("hi", "Hindi"),
    ("hu", "Hungarian"),
    ("id", "Indonesian"),
    ("it", "Italian"),
    ("ja", "Japanese"),
    ("ko", "Korean"),
    ("pl", "Polish"),
    ("pt", "Portuguese"),
    ("ro", "Romanian"),
    ("ru", "Russian"),
    ("es", "Spanish"),
    ("tr", "Turkish"),
    ("uk", "Ukrainian"),
    ("vi", "Vietnamese"),
];

/// Said where a model that does not support every language is picked or in use.
pub const LIMITED_LANGUAGES_TEXT: &str = "This model doesn't support all languages.";

/// The entry with that exact id (ids are lowercase), or `None`.
pub fn model(id: &str) -> Option<&'static ModelEntry> {
    MODELS.iter().find(|m| m.id == id)
}

/// What the model manager lists: models a user picks from, smallest first (no test-only models).
pub fn visible_models() -> Vec<&'static ModelEntry> {
    MODELS.iter().filter(|m| !m.test_only).collect()
}

/// The model id to offer and to mark Recommended: `base` on a CPU, `large-v3-turbo` once a GPU engine is
/// there: the pack INSTALLED on Windows (not merely an NVIDIA card present), Metal on a Mac
/// (`gpu::gpu_ready`, rule 1082).
pub fn recommended_model(gpu_ready: bool) -> &'static str {
    if gpu_ready {
        "large-v3-turbo"
    } else {
        "base"
    }
}

/// Whether the model picks its own language and does not support every one.
pub fn limited_languages(model_id: &str) -> bool {
    model(model_id).is_some_and(|m| m.limited_languages)
}

/// The language a pass is asked for: `auto` for a limited model, else the stored choice (validated).
/// The stored choice is never touched, so going back to a Whisper model brings it back.
pub fn language_for(model_id: &str, stored: &str) -> &'static str {
    if limited_languages(model_id) {
        "auto"
    } else {
        valid_language(stored)
    }
}

/// A stored language code if it is `auto` or one of `LANGUAGES`, else `auto`.
pub fn valid_language(v: &str) -> &'static str {
    LANGUAGES
        .iter()
        .find(|(code, _)| *code == v)
        .map_or("auto", |(code, _)| *code)
}

/// "1.6 GB" from 1e9 bytes (one decimal), else "148 MB".
pub fn size_text(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!("{:.1} GB", bytes as f64 / 1e9)
    } else {
        format!("{} MB", (bytes as f64 / 1e6).round() as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RELEASE: &str = "https://github.com/ggml-org/whisper.cpp/releases/download/";

    fn all_entries() -> Vec<(
        &'static str,
        &'static str,
        &'static str,
        u64,
        &'static str,
        &'static str,
    )> {
        let mut v: Vec<_> = MODELS
            .iter()
            .map(|m| (m.id, m.url, m.sha256, m.bytes, m.name, m.note))
            .collect();
        v.push((
            GPU_PACK.id,
            GPU_PACK.url,
            GPU_PACK.sha256,
            GPU_PACK.bytes,
            GPU_PACK.name,
            GPU_PACK.note,
        ));
        v
    }

    fn is_hex(s: &str, n: usize) -> bool {
        s.len() == n
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }

    #[test]
    fn gives_every_entry_an_https_url_an_exact_size_and_a_lowercase_checksum() {
        for (id, url, sha, bytes, name, note) in all_entries() {
            assert!(url.starts_with("https://"), "{id}");
            assert!(bytes > 0, "{id}");
            assert!(is_hex(sha, 64), "{id}");
            assert!(!name.is_empty(), "{id}");
            // One short line: the model manager has a row, not a paragraph.
            assert!(
                !note.is_empty() && note.len() <= 80 && !note.contains('\n'),
                "{id}"
            );
            assert!(
                note.split_whitespace().count() <= 8,
                "{id}: at most eight words"
            );
            for badge in ["Recommended", "Active", "Enabled", "Needs"] {
                assert!(
                    !note.contains(badge),
                    "{id}: a note never says a badge word"
                );
            }
        }
    }

    #[test]
    fn has_unique_ids_and_no_two_entries_share_a_url_or_a_checksum() {
        let all = all_entries();
        let ids: Vec<&str> = all.iter().map(|e| e.0).collect();
        let urls: Vec<&str> = all.iter().map(|e| e.1).collect();
        let shas: Vec<&str> = all.iter().map(|e| e.2).collect();
        for list in [ids, urls, shas] {
            let mut sorted = list.clone();
            sorted.sort_unstable();
            sorted.dedup();
            assert_eq!(sorted.len(), list.len());
        }
    }

    #[test]
    fn holds_the_seven_models_and_the_one_gpu_pack_the_design_names() {
        let mut ids: Vec<&str> = MODELS.iter().map(|m| m.id).collect();
        ids.sort_unstable();
        assert_eq!(
            ids,
            vec![
                "base",
                "large-v3",
                "large-v3-turbo",
                "parakeet-v3",
                "parakeet-v3-q4",
                "small",
                "tiny"
            ]
        );
        assert_eq!(GPU_PACK.id, "gpu-pack");
        let label = |id: &str| model(id).map(|m| m.name);
        assert_eq!(label("tiny"), Some("Whisper Tiny"));
        assert_eq!(label("base"), Some("Whisper Base"));
        assert_eq!(label("small"), Some("Whisper Small"));
        assert_eq!(label("large-v3-turbo"), Some("Whisper Large v3 Turbo"));
        assert_eq!(label("large-v3"), Some("Whisper Large v3"));
        assert_eq!(label("parakeet-v3"), Some("Parakeet v3"));
    }

    #[test]
    fn pins_every_whisper_model_to_a_commit_of_the_hugging_face_repo_never_main() {
        let whisper: Vec<&ModelEntry> = MODELS
            .iter()
            .filter(|m| m.runner == Runner::Whisper)
            .collect();
        assert_eq!(whisper.len(), 5);
        for m in whisper {
            let rest = m
                .url
                .strip_prefix("https://huggingface.co/ggerganov/whisper.cpp/resolve/")
                .unwrap_or_else(|| panic!("{}: wrong repo", m.id));
            let (commit, file) = rest
                .split_once('/')
                .unwrap_or_else(|| panic!("{}: no file", m.id));
            assert!(is_hex(commit, 40), "{}: commit, not a branch", m.id);
            assert_eq!(file, format!("ggml-{}.bin", m.id));
        }
    }

    #[test]
    fn pins_parakeet_v3_to_a_commit_of_ggml_org_parakeet_gguf_with_the_files_read_from_its_api() {
        let commit = "https://huggingface.co/ggml-org/parakeet-GGUF/resolve/35156454d1a39de06863303dd209fd2bed6ee079/";
        let app = model("parakeet-v3").expect("parakeet-v3");
        assert_eq!(
            app.url,
            format!("{commit}ggml-parakeet-tdt-0.6b-v3-q8_0.bin")
        );
        assert_eq!(app.bytes, 668_757_119);
        assert_eq!(
            app.sha256,
            "4d64e9e96c2792186d072fde0034df0ad670cf680a2f53069052ead827fd600e"
        );
        // The e2e's copy is the smallest official file of the same commit.
        let e2e = model("parakeet-v3-q4").expect("parakeet-v3-q4");
        assert_eq!(
            e2e.url,
            format!("{commit}ggml-parakeet-tdt-0.6b-v3-q4_0.bin")
        );
        assert_eq!(e2e.bytes, 355_615_679);
        assert_eq!(
            e2e.sha256,
            "aa7fe2f5fb47d863ca23e8b1d490632d63a2599f515268b6d6bd656158dad45e"
        );
        for m in [app, e2e] {
            assert_eq!(m.runner, Runner::Parakeet);
            assert!(m.limited_languages);
            // Fast on a CPU (measured under a second a pass): never marked as needing the pack.
            assert!(!m.needs_gpu);
        }
        assert!(e2e.test_only);
        assert!(!app.test_only);
    }

    #[test]
    fn flags_limited_languages_on_the_parakeet_files_only_and_runs_them_with_parakeet() {
        let limited: Vec<&str> = MODELS
            .iter()
            .filter(|m| m.limited_languages)
            .map(|m| m.id)
            .collect();
        assert_eq!(limited, vec!["parakeet-v3", "parakeet-v3-q4"]);
        let parakeet: Vec<&str> = MODELS
            .iter()
            .filter(|m| m.runner == Runner::Parakeet)
            .map(|m| m.id)
            .collect();
        assert_eq!(parakeet, vec!["parakeet-v3", "parakeet-v3-q4"]);
        // An id nobody knows has no entry: the controller treats it as Whisper's, the engine that was there first.
        assert!(model("nope").is_none());
    }

    #[test]
    fn takes_the_gpu_pack_from_the_same_pinned_release_as_the_engine() {
        assert_eq!(
            GPU_PACK.url,
            format!("{RELEASE}{}/whisper-cublas-12.4.0-bin-x64.zip", ENGINE.tag)
        );
        assert_eq!(
            GPU_PACK.sha256,
            "af520ddd034d985b55dfeea3e465ed93653ba2aee1a55e865033edc548c272a7"
        );
        assert_eq!(GPU_PACK.bytes, 674_539_285);
        // The pack is not a model: it is never listed as one.
        assert!(visible_models().iter().all(|m| m.id != GPU_PACK.id));
    }

    #[test]
    fn flags_the_models_a_cpu_cannot_keep_up_with_and_only_those() {
        let mut needs: Vec<&str> = MODELS
            .iter()
            .filter(|m| m.needs_gpu)
            .map(|m| m.id)
            .collect();
        needs.sort_unstable();
        assert_eq!(needs, vec!["large-v3", "large-v3-turbo"]);
    }

    #[test]
    fn visible_models_is_the_manager_list_models_only_the_test_ones_left_out_smallest_first() {
        let visible = visible_models();
        let ids: Vec<&str> = visible.iter().map(|m| m.id).collect();
        assert_eq!(
            ids,
            vec!["base", "small", "parakeet-v3", "large-v3-turbo", "large-v3"]
        );
        for pair in visible.windows(2) {
            assert!(
                pair[1].bytes > pair[0].bytes,
                "{} then {}",
                pair[0].id,
                pair[1].id
            );
        }
        assert!(model("tiny").is_some_and(|m| m.test_only));
        assert!(!ids.contains(&"tiny"));
        assert!(!ids.contains(&"parakeet-v3-q4"));
    }

    #[test]
    fn marks_exactly_one_pick_for_a_cpu_and_one_for_the_gpu_pack() {
        // The CPU pick is Base; the GPU pick is Turbo, and only once the pack is installed.
        assert_eq!(recommended_model(false), "base");
        assert_eq!(recommended_model(true), "large-v3-turbo");
        let visible = visible_models();
        assert!(visible.iter().any(|m| m.id == "base" && !m.needs_gpu));
        assert!(visible
            .iter()
            .any(|m| m.id == "large-v3-turbo" && m.needs_gpu));
    }

    #[test]
    fn finds_an_entry_by_id_and_answers_none_for_anything_else() {
        assert_eq!(model("base").map(|m| m.bytes), Some(147_951_465));
        assert!(model("").is_none());
        assert!(model("BASE").is_none());
        assert!(model("parakeet").is_none());
    }

    #[test]
    fn recommends_by_what_the_machine_can_run() {
        assert_eq!(recommended_model(false), "base");
        assert_eq!(recommended_model(true), "large-v3-turbo");
    }

    #[test]
    fn asks_a_model_that_picks_its_own_language_for_auto_and_leaves_the_stored_choice_to_the_rest()
    {
        assert!(limited_languages("parakeet-v3"));
        assert!(!limited_languages("base"));
        assert!(!limited_languages(""));
        assert!(!limited_languages("nope"));
        assert_eq!(language_for("parakeet-v3", "no"), "auto");
        assert_eq!(language_for("parakeet-v3", "auto"), "auto");
        // Back on a Whisper model, the user's language is what it always was.
        assert_eq!(language_for("base", "no"), "no");
        assert_eq!(language_for("large-v3-turbo", "de"), "de");
        assert_eq!(language_for("", "sv"), "sv");
    }

    #[test]
    fn says_the_one_line_the_owner_wrote_and_nothing_about_why() {
        assert_eq!(
            LIMITED_LANGUAGES_TEXT,
            "This model doesn't support all languages."
        );
        let lower = LIMITED_LANGUAGES_TEXT.to_ascii_lowercase();
        for word in ["nvidia", "parakeet", "whisper", "europe"] {
            assert!(!lower.contains(word), "{word}");
        }
    }

    #[test]
    fn starts_with_auto_detect_and_lists_each_whisper_code_once() {
        assert_eq!(LANGUAGES[0], ("auto", "Auto-detect"));
        let codes: Vec<&str> = LANGUAGES.iter().map(|l| l.0).collect();
        let mut unique = codes.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), codes.len());
        for &(code, name) in &LANGUAGES[1..] {
            assert!(
                code.len() == 2 && code.bytes().all(|b| b.is_ascii_lowercase()),
                "{code}"
            );
            assert!(!name.is_empty(), "{code}");
        }
        for must in "en no sv da de fr es it pt nl pl fi uk ru tr ar hi ja ko zh".split(' ') {
            assert!(codes.contains(&must), "{must}");
        }
        assert!((21..=35).contains(&LANGUAGES.len()));
    }

    #[test]
    fn language_names_are_unique_too_so_the_picker_never_shows_one_language_twice() {
        let mut names: Vec<&str> = LANGUAGES.iter().map(|l| l.1).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(names.len(), before);
    }

    #[test]
    fn validates_a_stored_language_to_auto_or_a_listed_code() {
        assert_eq!(valid_language("auto"), "auto");
        assert_eq!(valid_language("no"), "no");
        assert_eq!(valid_language("zz"), "auto");
        assert_eq!(valid_language("NO"), "auto");
        assert_eq!(valid_language("english"), "auto");
        assert_eq!(valid_language(""), "auto");
    }

    #[test]
    fn engine_is_the_pinned_official_cpu_build() {
        assert_eq!(ENGINE.tag, "b5130");
        assert_eq!(ENGINE.asset, "whisper-bin-x64.zip");
        assert_eq!(ENGINE.url, format!("{RELEASE}b5130/whisper-bin-x64.zip"));
        assert_eq!(
            ENGINE.sha256,
            "f9ec6c52a2e949b62ab51fa21d0d497958f9e41c3010c157c4e42932d5316f3c"
        );
        assert_eq!(ENGINE.bytes, 8_573_270);
    }

    #[test]
    fn engine_files_are_flat_unique_and_include_parakeets_runner() {
        assert!(ENGINE.files.contains(&"whisper-server.exe"));
        // Parakeet's runner comes out of the same zip.
        assert!(ENGINE.files.contains(&"parakeet-cli.exe"));
        assert!(ENGINE.files.contains(&"parakeet.dll"));
        assert_eq!(ENGINE.files.len(), 15);
        let mut sorted = ENGINE.files.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), ENGINE.files.len());
        // Flattened names: what the engine folder holds, not where the zip kept them.
        for f in ENGINE.files {
            assert!(!f.contains('\\') && !f.contains('/'), "{f}");
        }
    }

    #[test]
    fn the_mac_engine_is_the_same_tag_built_from_a_pinned_commit_and_tree() {
        assert_eq!(ENGINE_MACOS.tag, ENGINE.tag);
        assert!(is_hex(ENGINE_MACOS.commit, 40));
        assert!(is_hex(ENGINE_MACOS.tree, 40));
        assert_ne!(ENGINE_MACOS.commit, ENGINE_MACOS.tree);
        assert!(ENGINE_MACOS
            .repo
            .starts_with("https://github.com/ggml-org/whisper.cpp"));
        // The programs the engine runs by name (engine::WHISPER_SERVER and PARAKEET_CLI on a Mac).
        assert_eq!(ENGINE_MACOS.programs, &["whisper-server", "parakeet-cli"]);
        assert_eq!(PACK_OFFERED, cfg!(windows));
    }

    #[test]
    fn size_text_is_gb_with_one_decimal_from_1e9() {
        assert_eq!(size_text(1_624_555_275), "1.6 GB");
        assert_eq!(size_text(1_000_000_000), "1.0 GB");
        assert_eq!(size_text(147_951_465), "148 MB");
        assert_eq!(size_text(487_601_967), "488 MB");
    }
}

//! The Dictation page provider (M8 Task 24). It turns the catalogue, the store and the stored
//! preferences into the `DictationData` the page model draws. Nothing here starts a download or
//! opens a microphone; the page only reads.

use flare_dictation::catalog::{self, LANGUAGES, ModelEntry, Runner};
use flare_dictation::hotkey::DictMode;
use flare_dictation::hotkey::format_hotkey_here;
use flare_settings::page::dictation::{
    self as page, Ask, DictationData, GPU_PACK_ID, ItemInfo, ItemState, ModelInfo, Prefs, Vendor,
};
use flare_settings::page::{DictationPage, PageEnv, SectionModel};

/// The DictationPage that M7 left a seam for (M8 Task 24 fills it).
pub struct DictationPageProvider;

impl DictationPage for DictationPageProvider {
    fn sections(&self, env: &PageEnv<'_>) -> Vec<SectionModel> {
        let data = page_data(env);
        page::sections(env, &data)
    }
}

/// The recommended model: `base` on a CPU, `large-v3-turbo` once the pack is installed.
pub fn recommended(pack_installed: bool) -> &'static str {
    catalog::recommended_model(pack_installed)
}

/// Whether the stored model picks its own language, by the catalogue (PT's `limitedLanguages`): an e2e-only model
/// limits the picker too, which the page's visible list would not say.
pub fn limits_language(model: &str) -> bool {
    catalog::limited_languages(model)
}

/// The model the page's row describes, from its catalogue entry.
fn model_info(m: &ModelEntry) -> ModelInfo {
    ModelInfo {
        id: m.id,
        name: m.name,
        size_text: catalog::size_text(m.bytes),
        vendor: match m.runner {
            Runner::Whisper => Vendor::Openai,
            Runner::Parakeet => Vendor::Nvidia,
        },
        needs_gpu: m.needs_gpu,
        limited: m.limited_languages,
        note: m.note,
    }
}

/// Every model the manager lists, in the catalogue's order.
pub fn model_infos() -> Vec<ModelInfo> {
    catalog::visible_models()
        .into_iter()
        .map(model_info)
        .collect()
}

/// The page's data for this environment. Reads the stored preferences, the page's own state (a key being
/// captured, the Test, the question), the microphones a worker found and, while dictation is on, the store (every
/// model and the pack, installed or not, or downloading). It opens no microphone and starts no download.
pub fn page_data(env: &PageEnv<'_>) -> DictationData {
    let kv = |k: &str| env.prefs.get(k);
    let dp = crate::dictation_ctl::stored_prefs(&kv);
    let store = crate::dictation_ctl::store();
    let snap = crate::dictation_ctl::page_snap();
    let mut items: Vec<(String, ItemInfo)> = Vec::new();
    let item = |id: &str, bytes: u64, yes: bool| {
        let (pct, failure) = crate::dictation_ctl::run_view(id, bytes);
        ItemInfo {
            state: match pct {
                Some(pct) => ItemState::Downloading { pct },
                None if yes => ItemState::Installed,
                None => ItemState::Absent,
            },
            failure: failure.map(str::to_string),
        }
    };
    for m in catalog::visible_models() {
        // sync-ok: a file check per model, answered from the store's 30 s memo while the page is open
        let yes = crate::dictation_ctl::scene_installed(m.id)
            || crate::dictation_ctl::installed_for_page(m.id, || {
                store.as_ref().is_some_and(|s| s.model_path(m.id).is_some())
            });
        items.push((m.id.to_string(), item(m.id, m.bytes, yes)));
    }
    // sync-ok: the pack's marker file, checked once per page build
    let pack = crate::dictation_ctl::scene_installed(GPU_PACK_ID)
        || crate::dictation_ctl::installed_for_page(GPU_PACK_ID, || {
            store.as_ref().is_some_and(|s| s.gpu_engine_dir().is_some())
        });
    items.push((
        GPU_PACK_ID.to_string(),
        item(GPU_PACK_ID, catalog::GPU_PACK.bytes, pack),
    ));
    let stored_limited = limits_language(&dp.model);
    DictationData {
        models: model_infos(),
        // Metal on a Mac is the GPU the pack is on Windows: Turbo is the pick there without one (rule 1082).
        recommended: recommended(flare_dictation::gpu::gpu_ready(pack)),
        items,
        mics: crate::dictation_ctl::page_mics(),
        languages: LANGUAGES.to_vec(),
        prefs: Prefs {
            enabled: dp.enabled,
            toggle: dp.mode == DictMode::Toggle,
            key_text: format_hotkey_here(&dp.hotkey),
            mic: dp.mic,
            language: dp.language,
            pause_media: dp.pause_media,
            sounds: dp.sounds,
            model: dp.model,
            anywhere: dp.anywhere,
        },
        stored_limited,
        capturing: snap.capturing,
        mic_testing: snap.mic_testing,
        mic_level: snap.mic_level,
        mic_problem: snap.mic_problem,
        gpu_fell_back: crate::dictation_ctl::gpu_fell_back(),
        ask: snap.ask.map(|model_id| Ask {
            model_id,
            text: catalog::LIMITED_LANGUAGES_TEXT,
        }),
        pause_media_unavailable: flare_dictation::media::unavailable().map(str::to_string),
        // Read without asking (macOS: CGPreflightListenEventAccess), so a grant shows at the next page build.
        anywhere_problem: flare_dictation::keyhook::anywhere_problem(&dp.hotkey)
            .map(str::to_string),
        gpu_built_in: flare_dictation::gpu::BUILT_IN_GPU,
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turbo_is_recommended_with_the_pack_else_base() {
        assert_eq!(recommended(true), "large-v3-turbo");
        assert_eq!(recommended(false), "base");
        let models = model_infos();
        let turbo = models
            .iter()
            .find(|m| m.id == "large-v3-turbo")
            .expect("the turbo model is listed");
        assert!(turbo.needs_gpu);
        assert!(models.iter().any(|m| m.id == "base"));
    }

    #[test]
    fn the_models_carry_their_vendor_and_their_size_words() {
        let models = model_infos();
        assert!(models.iter().any(|m| m.vendor == Vendor::Openai));
        assert!(models.iter().any(|m| m.vendor == Vendor::Nvidia));
        let base = models.iter().find(|m| m.id == "base").expect("base");
        assert_eq!(base.size_text, "148 MB");
    }

    #[test]
    fn a_stored_model_limits_the_language_as_the_catalogue_says_test_only_ones_included() {
        // PT's language row asks the catalogue (limitedLanguages), e2e-only models included, so the
        // e2e's Parakeet file limits the picker like the real one (M8 Task 29).
        assert!(limits_language("parakeet-v3-q4"));
        assert!(limits_language("parakeet-v3"));
        assert!(!limits_language("base"));
        assert!(!limits_language(""));
    }

    #[test]
    fn the_pack_id_is_the_catalogue_pack_id() {
        assert_eq!(flare_dictation::catalog::GPU_PACK.id, GPU_PACK_ID);
    }
}

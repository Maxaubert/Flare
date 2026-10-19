//! The Dictation page provider (M8 Task 24). It turns the catalogue, the store and the stored
//! preferences into the `DictationData` the page model draws. Nothing here starts a download or
//! opens a microphone; the page only reads.

use flare_dictation::catalog::{self, LANGUAGES, ModelEntry, Runner};
use flare_dictation::hotkey::DictMode;
use flare_dictation::hotkey::format_hotkey;
use flare_dictation::mic;
use flare_settings::page::dictation::{
    self as page, DictationData, GPU_PACK_ID, ItemInfo, ItemState, ModelInfo, Prefs, Vendor,
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

/// The page's data for this environment. Reads the stored preferences, the microphones and, while
/// dictation is on, the store (every model and the pack, installed or not).
pub fn page_data(env: &PageEnv<'_>) -> DictationData {
    let kv = |k: &str| env.prefs.get(k);
    let dp = crate::dictation_ctl::stored_prefs(&kv);
    let store = crate::dictation_ctl::store();
    let mut items: Vec<(String, ItemInfo)> = Vec::new();
    let installed = |yes: bool| ItemInfo {
        state: if yes {
            ItemState::Installed
        } else {
            ItemState::Absent
        },
        failure: None,
    };
    for m in catalog::visible_models() {
        // sync-ok: a file check per model, answered from the store's 30 s memo while the page is open
        let yes = store.as_ref().is_some_and(|s| s.model_path(m.id).is_some());
        items.push((m.id.to_string(), installed(yes)));
    }
    // sync-ok: the pack's marker file, checked once per page build
    let pack = store.as_ref().is_some_and(|s| s.gpu_engine_dir().is_some());
    items.push((GPU_PACK_ID.to_string(), installed(pack)));
    let mics = mic::list_mics()
        .into_iter() // sync-ok: device enumeration when the Dictation page is built
        .map(|m| (m.id, m.name))
        .collect();
    DictationData {
        models: model_infos(),
        recommended: recommended(pack),
        items,
        mics,
        languages: LANGUAGES.to_vec(),
        prefs: Prefs {
            enabled: dp.enabled,
            toggle: dp.mode == DictMode::Toggle,
            key_text: format_hotkey(&dp.hotkey),
            mic: dp.mic,
            language: dp.language,
            pause_media: dp.pause_media,
            sounds: dp.sounds,
            model: dp.model,
            anywhere: dp.anywhere,
        },
        capturing: false,
        mic_testing: false,
        mic_level: 0.0,
        mic_problem: None,
        gpu_fell_back: false,
        ask: None,
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
    fn the_pack_id_is_the_catalogue_pack_id() {
        assert_eq!(flare_dictation::catalog::GPU_PACK.id, GPU_PACK_ID);
    }
}

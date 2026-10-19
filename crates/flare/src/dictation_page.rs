//! The Dictation page provider (M8 Task 24). Until then it answers what M7's placeholder did.

use flare_settings::page::{DictationPage, PageEnv, PlaceholderDictation, SectionModel};

/// The DictationPage that M7 left a seam for.
pub struct DictationPageProvider;

impl DictationPage for DictationPageProvider {
    fn sections(&self, env: &PageEnv<'_>) -> Vec<SectionModel> {
        PlaceholderDictation.sections(env)
    }
}

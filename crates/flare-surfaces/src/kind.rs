//! The kinds of panel. `image`, `text`, `web` and `changes` are built; the rest are named now so the
//! store can keep them and later stages only add a viewer.

/// The longest mod id (a folder name under the plugins folder).
const MOD_ID_MAX: usize = 64;

/// What a panel shows.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum PanelKind {
    Image,
    Text,
    Web,
    Changes,
    /// A panel a mod provides: `mod:<id>`.
    Mod(String),
}

impl PanelKind {
    /// The name stored in `panels.json` and used on the control channel.
    pub fn name(&self) -> String {
        match self {
            PanelKind::Image => "image".into(),
            PanelKind::Text => "text".into(),
            PanelKind::Web => "web".into(),
            PanelKind::Changes => "changes".into(),
            PanelKind::Mod(id) => format!("mod:{id}"),
        }
    }

    /// The kind a stored name means, or None for a name this build does not know (rule 40: the
    /// panel is dropped on load, never guessed at).
    pub fn parse(s: &str) -> Option<PanelKind> {
        Some(match s {
            "image" => PanelKind::Image,
            "text" => PanelKind::Text,
            "web" => PanelKind::Web,
            "changes" => PanelKind::Changes,
            other => {
                let id = other.strip_prefix("mod:")?;
                if !valid_mod_id(id) {
                    return None;
                }
                PanelKind::Mod(id.to_string())
            }
        })
    }

    /// True for the kinds drawn by Slint itself (the others are web content).
    pub fn is_native(&self) -> bool {
        !matches!(self, PanelKind::Web | PanelKind::Mod(_))
    }

    /// True for the kinds this build can draw. A stored panel of another kind is kept in the file
    /// but not shown until a later stage builds its viewer.
    pub fn is_built(&self) -> bool {
        matches!(
            self,
            PanelKind::Image | PanelKind::Text | PanelKind::Web | PanelKind::Changes
        )
    }

    /// True for the panel that shows the agent's conversation (stage 6, Changes): it reads no file, its content
    /// comes from the transcript tracker, and its target is only the kind's own name.
    pub fn is_convo(&self) -> bool {
        matches!(self, PanelKind::Changes)
    }

    /// The target a conversation panel stores (a target is never empty): its own name, otherwise None.
    pub fn convo_target(&self) -> Option<&'static str> {
        match self {
            PanelKind::Changes => Some("changes"),
            _ => None,
        }
    }

    /// The target a panel that shows no file stores: the conversation panel's own name.
    pub fn fixed_target(&self) -> Option<&'static str> {
        self.convo_target()
    }
}

/// A mod id: 1 to 64 of a-z, 0-9, `-` and `_`.
pub fn valid_mod_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MOD_ID_MAX
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_round_trips_through_its_name() {
        let all = [
            PanelKind::Image,
            PanelKind::Text,
            PanelKind::Web,
            PanelKind::Changes,
            PanelKind::Mod("notes".into()),
        ];
        for k in all {
            assert_eq!(PanelKind::parse(&k.name()), Some(k));
        }
    }

    #[test]
    fn an_unknown_kind_is_none() {
        for s in [
            "",
            "Image",
            "video",
            // The agent board was removed (2026-10-22): a stored one reads as unknown and is dropped.
            "board",
            // The Tasks panel was removed (2026-10-26): a stored one reads as unknown and is dropped.
            "tasks",
            "mod",
            "mod:",
            "mod:Bad Id",
            "mod:a/b",
            "mod:\u{e9}",
        ] {
            assert_eq!(PanelKind::parse(s), None, "{s:?}");
        }
        assert_eq!(PanelKind::parse(&format!("mod:{}", "a".repeat(65))), None);
        assert!(PanelKind::parse(&format!("mod:{}", "a".repeat(64))).is_some());
    }

    #[test]
    fn the_built_kinds_are_known_and_web_and_mods_are_not_native() {
        assert!(PanelKind::Image.is_built() && PanelKind::Text.is_built());
        assert!(PanelKind::Web.is_built());
        assert!(PanelKind::Changes.is_built());
        assert!(!PanelKind::Mod("x".into()).is_built());
        assert_eq!(PanelKind::Changes.fixed_target(), Some("changes"));
        assert_eq!(PanelKind::Web.fixed_target(), None);
        assert!(PanelKind::Changes.is_native() && !PanelKind::Web.is_native());
        assert!(PanelKind::Changes.is_convo());
        assert!(!PanelKind::Text.is_convo());
        assert_eq!(PanelKind::Changes.convo_target(), Some("changes"));
        assert_eq!(PanelKind::Web.convo_target(), None);
        assert!(!PanelKind::Mod("x".into()).is_native());
    }
}

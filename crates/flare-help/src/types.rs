//! Command help types (M7 Task 5). The shape of one catalogue entry, as the generated data files
//! hold it (`PT/core/shared/help/types.ts`).

use serde::Deserialize;
use std::collections::BTreeMap;

/// Which command language an entry is written in. `Any` reads the same in every shell; `Windows`
/// is the same text in PowerShell and Command Prompt and never shows in Bash.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HelpShell {
    Powershell,
    Cmd,
    Bash,
    Any,
    Windows,
}

/// A shell somebody can be in (`Any` and `Windows` are not one of them).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HelpShellChoice {
    Powershell,
    Cmd,
    Bash,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HelpCategory {
    Files,
    Folders,
    Text,
    Processes,
    Network,
    System,
    Packages,
    Git,
    Shell,
    Agents,
    Wsl,
}

/// A further ready-to-copy command, a row of its own in the panel.
#[derive(Clone, Debug, Deserialize)]
pub struct Variant {
    pub label: String,
    pub command: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub shells: Option<Vec<HelpShellChoice>>,
}

/// One catalogue entry, written task first.
#[derive(Clone, Debug, Deserialize)]
pub struct HelpEntry {
    pub id: String,
    pub shell: HelpShell,
    pub category: HelpCategory,
    pub task: String,
    pub summary: String,
    pub command: String,
    #[serde(default)]
    pub variants: Vec<Variant>,
    #[serde(default)]
    pub placeholders: BTreeMap<String, String>,
    pub keywords: Vec<String>,
    #[serde(default)]
    pub danger: Option<String>,
}

//! The Machines page (remote sessions PR 7): the machines this Flare opens remote tabs on, and a form to link one more.
//!
//! The owner's picks (2026-11-12 mockups, `docs/superpowers/specs/remote-sessions-mockups/pick.html`): no subtext
//! anywhere on the page; the linked machines are a table (Machine with its system's logo and name, SSH host, Flare
//! version, Status, and a menu with Test and Remove); adding is a "+ Add a machine" row that opens an inline form in
//! its place (an SSH host drop-down, a Name field, Cancel and Add), and a failed check is one red line under the form.
//!
//! This is data only. The app's `machines_page` reads `machines.json` and `~/.ssh/config` and checks a link on a
//! worker, and hands the page what it found in a [`MachinesInfo`].

use super::{BlockModel, Control, PageEnv, PageModel, RowModel, plain_row, section};
use crate::rows::row;

/// The add row's id (and the add form's, which takes its place while it is open).
pub const ADD_ROW: &str = "machines-add";
/// A machine row's id is this prefix and the machine's id (its ⋯ menu answers to it).
pub const MACHINE_PREFIX: &str = "machine:";
/// The form's buttons and its name field, as `row-button` and `row-text` name them.
pub const ADD_BUTTON: &str = "add";
pub const CANCEL_BUTTON: &str = "cancel";
pub const OPEN_BUTTON: &str = "tap";
/// The ⋯ menu's two choices.
pub const TEST_CHOICE: &str = "test";
pub const REMOVE_CHOICE: &str = "remove";

/// The system a machine runs: its logo on the page, on its tabs and in the + menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MachineOs {
    Mac,
    Pc,
}

/// What the last check of a machine's link found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum LinkStatus {
    /// Not checked in this run yet.
    #[default]
    Unknown,
    /// A check is running.
    Checking,
    /// `remote.hello` answered: the protocols agree.
    Linked,
    /// The check failed: the one line that says why.
    Failed(String),
}

impl LinkStatus {
    /// The word the Slint row draws by (`unknown`, `checking`, `linked`, `failed`).
    pub fn word(&self) -> &'static str {
        match self {
            LinkStatus::Unknown => "unknown",
            LinkStatus::Checking => "checking",
            LinkStatus::Linked => "linked",
            LinkStatus::Failed(_) => "failed",
        }
    }

    /// The words the Status column shows.
    pub fn text(&self) -> &'static str {
        match self {
            LinkStatus::Unknown => "",
            LinkStatus::Checking => "Checking",
            LinkStatus::Linked => "Linked",
            LinkStatus::Failed(_) => "Not linked",
        }
    }
}

/// One linked machine, as its table row shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MachineRow {
    pub id: String,
    pub name: String,
    pub os: Option<MachineOs>,
    pub ssh: String,
    /// The Flare version the host said at its last check ("" until one answered).
    pub version: String,
    pub status: LinkStatus,
}

/// The add form while it is open.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AddForm {
    pub open: bool,
    /// The SSH host picked ("" before a pick).
    pub host: String,
    /// The name typed.
    pub name: String,
    /// The link is being checked: Add waits.
    pub busy: bool,
    /// The one red line of the last failed check.
    pub error: Option<String>,
}

/// What the page shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MachinesInfo {
    pub machines: Vec<MachineRow>,
    /// The hosts `~/.ssh/config` names (no wildcard, no `Match`), for the form's drop-down.
    pub ssh_hosts: Vec<String>,
    pub add: AddForm,
}

/// The add form's block: the form and the hosts its drop-down offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddBlock {
    pub form: AddForm,
    pub hosts: Vec<String>,
}

/// The "+ Add a machine" row: its plus, its label and no subtext (owner pick); a press anywhere opens the form.
fn add_row() -> RowModel {
    let def = row(ADD_ROW).expect("machines-add is a row");
    RowModel {
        sub: String::new(),
        tap: true,
        ..plain_row(def, Control::None)
    }
}

/// The Machines page: the table of linked machines (its heading row first, none while there is no machine), then the
/// add row, or the add form in its place while it is open.
pub fn build(env: &PageEnv<'_>) -> PageModel {
    let empty = MachinesInfo::default();
    let info = env.machines.unwrap_or(&empty);
    let mut blocks = Vec::new();
    if !info.machines.is_empty() {
        blocks.push(BlockModel::MachineHead);
        blocks.extend(info.machines.iter().cloned().map(BlockModel::Machine));
    }
    if info.add.open {
        blocks.push(BlockModel::MachineAdd(AddBlock {
            form: info.add.clone(),
            hosts: info.ssh_hosts.clone(),
        }));
    } else {
        blocks.push(BlockModel::Row(add_row()));
    }
    PageModel {
        id: "machines",
        label: "Machines",
        sections: vec![section("machines", blocks)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page::test_env::with_env_machines;

    fn mac() -> MachineRow {
        MachineRow {
            id: "mac".into(),
            name: "Mac Mini".into(),
            os: Some(MachineOs::Mac),
            ssh: "mac-mini".into(),
            version: "0.1.0".into(),
            status: LinkStatus::Linked,
        }
    }

    fn kinds(page: &PageModel) -> Vec<&'static str> {
        page.sections[0]
            .blocks
            .iter()
            .map(|b| match b {
                BlockModel::MachineHead => "head",
                BlockModel::Machine(_) => "machine",
                BlockModel::MachineAdd(_) => "form",
                BlockModel::Row(r) if r.id == ADD_ROW => "add",
                _ => "other",
            })
            .collect()
    }

    #[test]
    fn with_no_machine_the_page_is_the_add_row_alone_with_no_subtext() {
        with_env_machines(MachinesInfo::default(), |env| {
            let page = build(env);
            assert_eq!(page.id, "machines");
            assert_eq!(page.sections.len(), 1);
            assert_eq!(page.sections[0].title, "Linked machines");
            assert_eq!(kinds(&page), ["add"]);
            let BlockModel::Row(r) = &page.sections[0].blocks[0] else {
                panic!("the add row");
            };
            assert_eq!(r.label, "Add a machine");
            assert_eq!(r.icon, "plus");
            assert!(r.sub.is_empty() && r.tap);
        });
    }

    #[test]
    fn linked_machines_are_a_table_under_its_heading_row_then_the_add_row() {
        let pc = MachineRow {
            id: "pc".into(),
            name: "Desk".into(),
            os: Some(MachineOs::Pc),
            ssh: "desk".into(),
            version: String::new(),
            status: LinkStatus::Failed("No Flare answered over SSH on desk.".into()),
        };
        let info = MachinesInfo {
            machines: vec![mac(), pc.clone()],
            ..MachinesInfo::default()
        };
        with_env_machines(info, |env| {
            let page = build(env);
            assert_eq!(kinds(&page), ["head", "machine", "machine", "add"]);
            assert_eq!(page.sections[0].blocks[1], BlockModel::Machine(mac()));
            assert_eq!(page.sections[0].blocks[2], BlockModel::Machine(pc));
        });
    }

    #[test]
    fn an_open_form_takes_the_add_rows_place_with_the_hosts_and_its_one_line() {
        let info = MachinesInfo {
            machines: vec![mac()],
            ssh_hosts: vec!["studio".into(), "nas".into()],
            add: AddForm {
                open: true,
                host: "studio".into(),
                name: "Studio".into(),
                busy: false,
                error: Some(
                    "Flare there speaks remote protocol 0, here 1. Update it there.".into(),
                ),
            },
        };
        with_env_machines(info.clone(), |env| {
            let page = build(env);
            assert_eq!(kinds(&page), ["head", "machine", "form"]);
            let BlockModel::MachineAdd(b) = &page.sections[0].blocks[2] else {
                panic!("the form");
            };
            assert_eq!(b.form, info.add);
            assert_eq!(b.hosts, ["studio", "nas"]);
        });
    }

    #[test]
    fn a_status_has_one_word_to_draw_by_and_the_words_it_shows() {
        let all = [
            LinkStatus::Unknown,
            LinkStatus::Checking,
            LinkStatus::Linked,
            LinkStatus::Failed("x".into()),
        ];
        let words: Vec<&str> = all.iter().map(LinkStatus::word).collect();
        assert_eq!(words, ["unknown", "checking", "linked", "failed"]);
        let texts: Vec<&str> = all.iter().map(LinkStatus::text).collect();
        assert_eq!(texts, ["", "Checking", "Linked", "Not linked"]);
    }
}

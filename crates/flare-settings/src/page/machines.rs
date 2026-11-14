//! The Machines page (remote sessions PR 7): the machines this Flare opens remote tabs on, and a way to link one more.
//!
//! The owner's picks (2026-11-12 mockups, `docs/superpowers/specs/remote-sessions-mockups/pick.html`): no subtext
//! anywhere on the page; the linked machines are a table (Machine with its system's logo and name, Address, Flare
//! version, Status, and a menu with Test and Remove). A machine that is Not linked shows why as one small grey line
//! under its row, not as a tooltip (2026-11-13).
//!
//! Machine pairing PR 1 (approved mockup `docs/superpowers/specs/2026-11-14-machine-pairing-mockup.html`, rule 1251):
//! "+ Add a machine" opens a Flare dialog over the page, no longer a form in the row's place. Its Machine drop-down
//! lists the computers found nearby (PR 3; none yet), then the SSH config hosts, then "Type an address", which shows an
//! Address field and an optional User field. A failed check is one red line in the dialog.
//!
//! This is data only. The app's `machines_page` reads `machines.json` and `~/.ssh/config` and checks a link on a
//! worker, and hands the page what it found in a [`MachinesInfo`].

use super::{BlockModel, Control, PageEnv, PageModel, RowModel, plain_row, section};
use crate::rows::row;

/// The add row's id (the dialog's Machine drop-down and its fields answer to it too).
pub const ADD_ROW: &str = "machines-add";
/// A machine row's id is this prefix and the machine's id (its ⋯ menu answers to it).
pub const MACHINE_PREFIX: &str = "machine:";
/// The dialog's buttons, as `row-button` names them.
pub const ADD_BUTTON: &str = "add";
pub const CANCEL_BUTTON: &str = "cancel";
pub const OPEN_BUTTON: &str = "tap";
/// The dialog's two text fields, as `row-text` names them (the row is [`ADD_ROW`] and the text follows the word).
pub const ADDRESS_FIELD: &str = "address";
pub const USER_FIELD: &str = "user";
/// The ⋯ menu's two choices.
pub const TEST_CHOICE: &str = "test";
pub const REMOVE_CHOICE: &str = "remove";
/// The Machine drop-down's last choice. It starts with a space, so no SSH host (a plain argv word) can be it.
pub const TYPE_ADDRESS: &str = " address";
/// A computer found nearby is this prefix and its id (machine pairing PR 3 fills them; none in PR 1).
pub const NEARBY_PREFIX: &str = " nearby:";

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
    /// Where ssh connects: the machine's address, or its SSH host when it has none.
    pub address: String,
    /// The Flare version the host said at its last check ("" until one answered).
    pub version: String,
    pub status: LinkStatus,
}

impl MachineRow {
    /// The small grey line under the row: why the last check failed, "" for any other status. It is a line of its own,
    /// never the status's tooltip (owner rule 2026-11-13: a control with words has no tooltip).
    pub fn sub_line(&self) -> &str {
        match &self.status {
            LinkStatus::Failed(reason) => reason,
            _ => "",
        }
    }
}

/// The Add a machine dialog while it is open.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AddForm {
    pub open: bool,
    /// The Machine drop-down's pick: an SSH host, [`TYPE_ADDRESS`], or "" before a pick ("Pick a machine").
    pub pick: String,
    /// What is typed in the Address and User fields (shown while [`TYPE_ADDRESS`] is picked).
    pub address: String,
    pub user: String,
    /// The link is being checked: Add waits.
    pub busy: bool,
    /// The one red line of the last failed check.
    pub error: Option<String>,
}

impl AddForm {
    /// "Type an address" is picked: the Address and User fields show.
    pub fn typing(&self) -> bool {
        self.pick == TYPE_ADDRESS
    }

    /// Add can be pressed: a machine is picked (with an address typed, for "Type an address"), and no check runs.
    pub fn can_add(&self) -> bool {
        !self.busy
            && match self.pick.as_str() {
                "" => false,
                TYPE_ADDRESS => !self.address.trim().is_empty(),
                _ => true,
            }
    }
}

/// One choice of the Machine drop-down: its id, its words, and whether they are drawn in the code font (an SSH host).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MachineChoice {
    pub id: String,
    pub label: String,
    pub mono: bool,
}

/// The Machine drop-down in the mockup's order: the computers found nearby (`(id, name)`, machine pairing PR 3), then
/// the SSH config hosts, then "Type an address".
pub fn machine_choices(nearby: &[(String, String)], hosts: &[String]) -> Vec<MachineChoice> {
    let mut out: Vec<MachineChoice> = nearby
        .iter()
        .map(|(id, name)| MachineChoice {
            id: format!("{NEARBY_PREFIX}{id}"),
            label: name.clone(),
            mono: false,
        })
        .collect();
    out.extend(hosts.iter().map(|h| MachineChoice {
        id: h.clone(),
        label: h.clone(),
        mono: true,
    }));
    out.push(MachineChoice {
        id: TYPE_ADDRESS.to_string(),
        label: "Type an address".to_string(),
        mono: false,
    });
    out
}

/// What the page shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MachinesInfo {
    pub machines: Vec<MachineRow>,
    /// The hosts `~/.ssh/config` names (no wildcard, no `Match`), for the dialog's drop-down.
    pub ssh_hosts: Vec<String>,
    pub add: AddForm,
}

/// The "+ Add a machine" row: its plus, its label and no subtext (owner pick); a press anywhere opens the dialog.
fn add_row() -> RowModel {
    let def = row(ADD_ROW).expect("machines-add is a row");
    RowModel {
        sub: String::new(),
        tap: true,
        ..plain_row(def, Control::None)
    }
}

/// The Machines page: the table of linked machines (its heading row first, none while there is no machine), then the
/// add row. The Add a machine dialog is drawn over the page from [`MachinesInfo::add`], not as a block.
pub fn build(env: &PageEnv<'_>) -> PageModel {
    let empty = MachinesInfo::default();
    let info = env.machines.unwrap_or(&empty);
    let mut blocks = Vec::new();
    if !info.machines.is_empty() {
        blocks.push(BlockModel::MachineHead);
        blocks.extend(info.machines.iter().cloned().map(BlockModel::Machine));
    }
    blocks.push(BlockModel::Row(add_row()));
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
            address: "mac-mini".into(),
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
            address: "desk".into(),
            version: String::new(),
            status: LinkStatus::Failed("Not answering".into()),
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

    /// Rule 1251: the dialog is drawn over the page; an open one leaves the add row in its place.
    #[test]
    fn an_open_dialog_leaves_the_page_as_it_is() {
        let info = MachinesInfo {
            machines: vec![mac()],
            ssh_hosts: vec!["studio".into()],
            add: AddForm {
                open: true,
                pick: "studio".into(),
                error: Some("Not answering".into()),
                ..AddForm::default()
            },
        };
        with_env_machines(info, |env| {
            assert_eq!(kinds(&build(env)), ["head", "machine", "add"]);
        });
    }

    /// Rule 1251: the drop-down lists the nearby computers, then the SSH hosts in the code font, then "Type an
    /// address"; no host can take the last one's id.
    #[test]
    fn the_machine_drop_down_lists_nearby_then_ssh_hosts_then_type_an_address() {
        let hosts = vec!["studio".to_string(), "nas".to_string()];
        let words = |c: &[MachineChoice]| -> Vec<(String, bool)> {
            c.iter().map(|c| (c.label.clone(), c.mono)).collect()
        };
        let plain = machine_choices(&[], &hosts);
        assert_eq!(
            words(&plain),
            [
                ("studio".to_string(), true),
                ("nas".to_string(), true),
                ("Type an address".to_string(), false)
            ]
        );
        assert_eq!(plain[2].id, TYPE_ADDRESS);
        let near = machine_choices(&[("a1".into(), "Max's PC".into())], &hosts);
        assert_eq!(near[0].label, "Max's PC");
        assert_eq!(near[0].id, format!("{NEARBY_PREFIX}a1"));
        assert!(!near[0].mono);
        assert_eq!(near.len(), 4);
        // An id that starts with a space is never a plain host name.
        assert!(TYPE_ADDRESS.starts_with(' ') && NEARBY_PREFIX.starts_with(' '));
        assert_eq!(
            words(&machine_choices(&[], &[])),
            [("Type an address".to_string(), false)]
        );
    }

    /// Rule 1251: Add waits for a pick, for an address when "Type an address" is picked, and while a check runs.
    #[test]
    fn add_can_be_pressed_once_a_machine_is_picked_or_an_address_typed() {
        let form = |pick: &str, address: &str, busy: bool| AddForm {
            open: true,
            pick: pick.into(),
            address: address.into(),
            busy,
            ..AddForm::default()
        };
        assert!(!form("", "", false).can_add(), "nothing picked");
        assert!(form("studio", "", false).can_add());
        assert!(!form("studio", "", true).can_add(), "a check runs");
        assert!(!form(TYPE_ADDRESS, "  ", false).can_add(), "no address");
        assert!(form(TYPE_ADDRESS, "192.168.0.139", false).can_add());
        assert!(form(TYPE_ADDRESS, "", false).typing());
        assert!(!form("studio", "", false).typing());
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

    #[test]
    fn a_failed_machine_carries_its_reason_as_its_sub_line_and_the_others_have_none() {
        let with = |status: LinkStatus| MachineRow { status, ..mac() };
        let failed = with(LinkStatus::Failed("Not answering".into()));
        assert_eq!(failed.sub_line(), "Not answering");
        assert_eq!(
            failed.status.text(),
            "Not linked",
            "the status keeps its words"
        );
        for status in [
            LinkStatus::Unknown,
            LinkStatus::Checking,
            LinkStatus::Linked,
        ] {
            assert_eq!(with(status.clone()).sub_line(), "", "{status:?}: no line");
        }
    }
}

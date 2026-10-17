//! The help panel model (M7 Task 28). Ported from Prism Terminal's `HelpPanel.tsx`: the rows, the
//! keys, the copy rule, the copied state, the paging and the count text.

use flare_help::entries::{HELP_CATEGORIES, all_help, is_key_press};
use flare_help::panel::{
    COPIED_MS, DRAG_PX, EMPTY_TEXT, FIRST_PAGE, MAX_RESULTS, NEXT_PAGE, PLACEHOLDER, PanelKey,
    PanelOut, PanelState, SEARCH_LABEL, front_shell,
};
use flare_help::types::HelpShellChoice;
use regex::Regex;
use std::collections::HashSet;

const PANEL_SRC: &str = include_str!("../src/panel.rs");

fn ps() -> HelpShellChoice {
    HelpShellChoice::Powershell
}

/// The index of the first drawn row whose command is a key press, if the chip shows one.
fn key_row_index(p: &PanelState) -> Option<usize> {
    p.rows().iter().position(|r| is_key_press(&r.command))
}

/// The index of the first row whose command ends in a blank (`Set-Location `).
fn blank_end_index(p: &PanelState) -> Option<usize> {
    p.rows()
        .iter()
        .position(|r| r.command.ends_with(' ') && !is_key_press(&r.command))
}

#[test]
fn nothing_is_marked_until_the_first_down_which_marks_the_first_row() {
    let mut p = PanelState::new(ps());
    assert!(!p.marked());
    assert_eq!(p.active_id(), None);
    assert_eq!(p.key(PanelKey::Down, false), PanelOut::None);
    assert!(p.marked());
    assert_eq!(p.active(), 0);
    let first = &p.rows()[0];
    assert_eq!(
        p.active_id(),
        Some(format!("help-entry-{}-{}", first.entry.id, first.n))
    );
}

#[test]
fn up_down_page_up_page_down_move_by_one_and_five_and_clamp() {
    let mut p = PanelState::new(ps());
    let last = p.rows().len() - 1;
    assert!(last > 12, "the PowerShell list is long enough to page");
    // The first press marks where the cursor is (row 0), it does not step off it.
    p.key(PanelKey::Down, false);
    assert_eq!(p.active(), 0);
    p.key(PanelKey::Down, false);
    assert_eq!(p.active(), 1);
    p.key(PanelKey::PageDown, false);
    assert_eq!(p.active(), 6);
    p.key(PanelKey::PageUp, false);
    assert_eq!(p.active(), 1);
    p.key(PanelKey::Up, false);
    assert_eq!(p.active(), 0);
    p.key(PanelKey::Up, false);
    assert_eq!(p.active(), 0, "clamped at the top");
    p.key(PanelKey::PageUp, false);
    assert_eq!(p.active(), 0, "clamped at the top");
    for _ in 0..(last + 10) {
        p.key(PanelKey::PageDown, false);
    }
    assert_eq!(p.active(), last, "clamped at the end");
}

#[test]
fn the_pointer_over_the_list_clears_the_mark() {
    let mut p = PanelState::new(ps());
    p.key(PanelKey::Down, false);
    p.key(PanelKey::Down, false);
    assert!(p.marked());
    p.pointer_moved_over_list();
    assert!(!p.marked());
    assert_eq!(p.active_id(), None);
    // The next key press marks where the cursor is, rather than stepping off it.
    p.key(PanelKey::Down, false);
    assert!(p.marked());
    assert_eq!(p.active(), 1);
}

#[test]
fn enter_copies_the_marked_row_text_exactly() {
    let mut p = PanelState::new(ps());
    let i = blank_end_index(&p).expect("a PowerShell row ends in a blank");
    p.pointer_down_on(i);
    let row = p.rows()[i].clone();
    match p.key(PanelKey::Enter, false) {
        PanelOut::Copy { key, text } => {
            assert_eq!(key, format!("{}#{}", row.entry.id, row.n));
            assert_eq!(text, row.command);
            assert!(text.ends_with(' '), "the trailing space is kept: {text:?}");
        }
        other => panic!("expected a copy, got {other:?}"),
    }
}

#[test]
fn a_click_copies_and_a_drag_over_four_pixels_does_not() {
    let mut p = PanelState::new(ps());
    p.pointer_down_on(2);
    let row = p.rows()[2].clone();
    assert_eq!(
        p.click_row(2, 0.0),
        PanelOut::Copy {
            key: format!("{}#{}", row.entry.id, row.n),
            text: row.command.clone(),
        }
    );
    assert_eq!(p.click_row(2, DRAG_PX + 1.0), PanelOut::None);
    assert_eq!(DRAG_PX, 4.0);
    assert!(matches!(p.click_row(2, DRAG_PX), PanelOut::Copy { .. }));
}

#[test]
fn a_key_row_is_never_copied_and_enter_says_so() {
    let (mut p, i) = [ps(), HelpShellChoice::Cmd, HelpShellChoice::Bash]
        .into_iter()
        .find_map(|shell| {
            let p = PanelState::new(shell);
            key_row_index(&p).map(|i| (p, i))
        })
        .expect("the catalogue has a key row");
    let command = p.rows()[i].command.clone();
    p.pointer_down_on(i);
    assert_eq!(p.click_row(i, 0.0), PanelOut::None);
    assert_eq!(p.key(PanelKey::Enter, false), PanelOut::None);
    assert_eq!(
        p.said(),
        format!("{command} is a key to press, not text to copy")
    );
}

#[test]
fn copied_wears_the_check_for_1400_ms_and_a_second_copy_restarts_the_clock() {
    assert_eq!(COPIED_MS, 1400);
    let mut p = PanelState::new(ps());
    p.copy_result("k#0", "Get-ChildItem ", true, 1000);
    assert_eq!(p.copied(), Some(("k#0", true)));
    assert_eq!(p.said(), "Copied Get-ChildItem ");
    p.tick(2399);
    assert_eq!(p.copied(), Some(("k#0", true)));
    assert!(!p.said().is_empty());
    p.tick(2400);
    assert_eq!(p.copied(), None);
    assert_eq!(p.said(), "");
    // Copying the same row again restarts the clock; the first copy's time is not inherited.
    p.copy_result("k#0", "Get-ChildItem ", true, 3000);
    p.tick(3500);
    assert_eq!(p.copied(), Some(("k#0", true)));
    p.copy_result("k#0", "Get-ChildItem ", true, 3600);
    p.tick(4400);
    assert_eq!(
        p.copied(),
        Some(("k#0", true)),
        "the second copy's clock runs from 3600"
    );
    p.tick(5000);
    assert_eq!(p.copied(), None);
}

#[test]
fn a_failed_copy_says_so() {
    let mut p = PanelState::new(ps());
    p.copy_result("k#0", "dir ", false, 0);
    assert_eq!(p.said(), "Could not copy to the clipboard");
    assert_eq!(p.copied(), Some(("k#0", false)));
}

#[test]
fn first_page_40_then_60_more_near_the_end() {
    assert_eq!((FIRST_PAGE, NEXT_PAGE), (40, 60));
    let mut p = PanelState::new(ps());
    assert!(p.rows().len() > FIRST_PAGE);
    assert_eq!(p.drawn().len(), FIRST_PAGE);
    assert_eq!(p.limit(), FIRST_PAGE);
    p.near_end();
    assert_eq!(p.limit(), FIRST_PAGE + NEXT_PAGE);
    assert_eq!(
        p.drawn().len(),
        (FIRST_PAGE + NEXT_PAGE).min(p.rows().len())
    );
    // A key move past the drawn page makes the page grow to reach it.
    let mut q = PanelState::new(ps());
    for _ in 0..(FIRST_PAGE + 2) {
        q.key(PanelKey::Down, false);
    }
    assert!(
        q.limit() > FIRST_PAGE,
        "moving past the page draws the next one"
    );
    assert!(q.drawn().len() > q.active());
}

#[test]
fn a_search_answers_at_most_fifty() {
    let mut p = PanelState::new(ps());
    p.set_query("file");
    assert!(p.searching());
    let entries: HashSet<&str> = p.rows().iter().map(|r| r.entry.id.as_str()).collect();
    assert!(!entries.is_empty());
    assert!(entries.len() <= MAX_RESULTS, "{} entries", entries.len());
}

#[test]
fn the_count_says_commands_results_no_results_and_fifty_plus() {
    let mut p = PanelState::new(ps());
    assert_eq!(p.count_text(), format!("{} commands", p.rows().len()));
    p.set_query("zzqxnomatchzzq");
    assert_eq!(p.count_text(), "No results");
    p.set_query("file");
    let n = p.rows().len();
    let expected = format!(
        "{}{} {}",
        n,
        if n == MAX_RESULTS { "+" } else { "" },
        if n == 1 { "result" } else { "results" }
    );
    assert_eq!(p.count_text(), expected);
    assert_eq!(MAX_RESULTS, 50);
}

#[test]
fn browsing_groups_by_category_in_the_panels_order_with_headings() {
    let p = PanelState::new(ps());
    let order: Vec<&str> = HELP_CATEGORIES.iter().map(|(_, n)| *n).collect();
    let mut last_rank = 0usize;
    for (i, row) in p.rows().iter().enumerate() {
        let name = HELP_CATEGORIES
            .iter()
            .find(|(c, _)| *c == row.entry.category)
            .map(|(_, n)| *n)
            .expect("every category is in the panel's list");
        let rank = order.iter().position(|n| *n == name).unwrap();
        assert!(rank >= last_rank, "rows follow the panel's category order");
        last_rank = rank;
        let changed = i == 0 || p.rows()[i - 1].entry.category != row.entry.category;
        assert_eq!(p.heading_for(i).is_some(), changed, "heading at row {i}");
        if changed {
            assert_eq!(p.heading_for(i), Some(name));
        }
    }
    assert_eq!(p.heading_for(p.rows().len()), None);
    let mut s = PanelState::new(ps());
    s.set_query("file");
    assert_eq!(s.heading_for(0), None, "a search has no headings");
}

#[test]
fn variants_are_rows_of_their_own_filtered_by_shell() {
    // An entry with a variant that names its shells, shown in one of them.
    let (entry, index, shells) = all_help()
        .iter()
        .find_map(|e| {
            e.variants
                .iter()
                .enumerate()
                .find_map(|(i, v)| v.shells.as_ref().map(|s| (e, i, s.clone())))
        })
        .expect("the catalogue has a shell-limited variant");
    for shell in [ps(), HelpShellChoice::Cmd, HelpShellChoice::Bash] {
        let mut p = PanelState::new(shell);
        p.key(PanelKey::Down, false);
        let want_row = shells.contains(&shell);
        let shown = p
            .rows()
            .iter()
            .any(|r| r.entry.id == entry.id && r.n == index + 1);
        if want_row {
            // Only when the entry itself is shown in this shell.
            let entry_shown = p.rows().iter().any(|r| r.entry.id == entry.id);
            if entry_shown {
                assert!(shown, "{shell:?} shows its variant row");
            }
        } else {
            assert!(
                !shown,
                "{shell:?} does not show a variant limited to {shells:?}"
            );
        }
    }
    // The variant's `n` counts every variant, not just the ones of this shell.
    let p = PanelState::new(shells[0]);
    if let Some(r) = p.rows().iter().find(|r| r.entry.id == entry.id && r.n > 0) {
        assert_eq!(r.n, index + 1);
    }
}

#[test]
fn each_opening_starts_on_the_front_shell_with_an_empty_question() {
    let mut p = PanelState::new(ps());
    p.set_query("copy");
    p.key(PanelKey::Down, false);
    p.key(PanelKey::Down, false);
    p.pick_chip(HelpShellChoice::Bash);
    p.open(HelpShellChoice::Cmd);
    assert_eq!(p.chip(), HelpShellChoice::Cmd);
    assert_eq!(p.query(), "");
    assert!(!p.searching());
    assert_eq!(p.active(), 0);
    assert!(!p.marked());
    assert_eq!(p.limit(), FIRST_PAGE);
}

#[test]
fn a_hand_picked_chip_is_remembered_by_the_host_and_resets_the_list() {
    let mut p = PanelState::new(ps());
    p.set_query("copy");
    p.key(PanelKey::Down, false);
    p.pick_chip(HelpShellChoice::Cmd);
    assert_eq!(p.chip(), HelpShellChoice::Cmd);
    assert_eq!(
        p.query(),
        "copy",
        "the question stays, as in Prism Terminal"
    );
    assert_eq!(p.active(), 0);
    assert!(!p.marked());
    let mut fresh = PanelState::new(HelpShellChoice::Cmd);
    fresh.set_query("copy");
    assert_eq!(p.rows().len(), fresh.rows().len());
}

#[test]
fn escape_closes() {
    let mut p = PanelState::new(ps());
    assert_eq!(p.key(PanelKey::Escape, false), PanelOut::Close);
}

#[test]
fn ctrl_alt_meta_chords_are_left_to_the_host() {
    let mut p = PanelState::new(ps());
    for k in [
        PanelKey::Escape,
        PanelKey::Down,
        PanelKey::Up,
        PanelKey::PageDown,
        PanelKey::PageUp,
        PanelKey::Enter,
    ] {
        assert_eq!(p.key(k, true), PanelOut::None, "{k:?} with a chord");
    }
    assert!(!p.marked(), "a chord moves nothing");
    assert_eq!(p.active(), 0);
}

#[test]
fn the_panel_has_no_way_to_reach_a_shell() {
    let banned = Regex::new(r"\b(write|pty|Session)\b").unwrap();
    assert!(
        !banned.is_match(PANEL_SRC),
        "panel.rs names a way to reach a shell"
    );
}

#[test]
fn an_empty_result_says_how_to_ask() {
    assert_eq!(
        EMPTY_TEXT,
        "Nothing matches. Say it another way (what you want to DO, such as \"delete a folder\"), or try another shell."
    );
    assert_eq!(PLACEHOLDER, "Try: find big files, what is using port 3000");
    assert_eq!(
        SEARCH_LABEL,
        "Search commands. Up and Down move through the results, Enter copies the highlighted command."
    );
    let mut p = PanelState::new(ps());
    p.set_query("zzqxnomatchzzq");
    assert!(p.rows().is_empty());
}

#[test]
fn the_front_shell_decides_the_chip_else_the_picked_chip_else_powershell() {
    assert_eq!(front_shell(Some("cmd"), Some("bash")), HelpShellChoice::Cmd);
    assert_eq!(
        front_shell(Some("wsl-Ubuntu"), Some("cmd")),
        HelpShellChoice::Bash
    );
    assert_eq!(front_shell(None, Some("bash")), HelpShellChoice::Bash);
    assert_eq!(front_shell(None, Some("cmd")), HelpShellChoice::Cmd);
    assert_eq!(
        front_shell(None, Some("nonsense")),
        HelpShellChoice::Powershell
    );
    assert_eq!(front_shell(None, None), HelpShellChoice::Powershell);
}

#[test]
fn the_stripe_follows_the_drawn_order() {
    assert!(!PanelState::stripe(0));
    assert!(PanelState::stripe(1));
    assert!(!PanelState::stripe(2));
    assert!(PanelState::stripe(7));
}

#[test]
fn the_copy_text_is_the_row_command_exactly() {
    let p = PanelState::new(ps());
    let i = blank_end_index(&p).unwrap();
    let mut q = p.clone();
    q.pointer_down_on(i);
    let PanelOut::Copy { text, .. } = q.key(PanelKey::Enter, false) else {
        panic!("a copy")
    };
    assert_eq!(text, p.rows()[i].command);
}

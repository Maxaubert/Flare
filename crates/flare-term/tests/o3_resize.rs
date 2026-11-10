//! Decision O3: the prompt line survives a resize under the real bundled ConPTY.
//! Windows-only until macOS phase 2 (the real bundled ConPTY and the Windows shells).
#![cfg(windows)]

mod common;

use common::{NamedDir, Rig, temp_dir_named};

/// A folder name that makes cmd's prompt (`<temp>\.tmpXXXXXX\<name>>`) 73 characters: one row at 80
/// columns, two when narrowed. That is the case ConPTY and a plain resize disagree on.
fn long_name() -> String {
    let temp = std::env::temp_dir()
        .to_string_lossy()
        .trim_end_matches('\\')
        .len();
    let room = 60usize.saturating_sub(temp).max(9);
    let mut name = String::from("flare-o3-");
    while name.len() < room {
        name.push('x');
    }
    name
}

fn prompt_up(rig: &Rig) {
    let seen = rig.wait_for(15_000, |e| {
        (0..e.rows()).any(|r| e.row_text(i32::try_from(r).unwrap_or(0)).ends_with('>'))
    });
    assert!(seen, "no prompt appeared; screen: {:?}", rig.screen());
    rig.wait_quiet(500);
}

/// Types `s` and waits until the shell has echoed it. `wait_quiet` alone returns at once when
/// the last byte is old, which would let the next resize race the echo.
fn type_and_see(rig: &Rig, s: &str) {
    rig.type_str(s);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    // A wrapped line splits the text over two rows (PowerShell wraps without a WRAPLINE flag), so join the rows.
    while !rig.screen().concat().trim_end().ends_with(s) {
        assert!(
            std::time::Instant::now() < deadline,
            "the shell never echoed {s:?}\n{}",
            dump(rig)
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    rig.wait_quiet(400);
}

fn dump(rig: &Rig) -> String {
    format!(
        "screen: {:#?}\ncursor: {:?} history: {}",
        rig.screen(),
        rig.cursor(),
        rig.history()
    )
}

fn resize_steps(rig: &mut Rig, steps: &[(u16, u16)]) {
    for (c, r) in steps {
        rig.resize(*c, *r);
        rig.wait_quiet(700);
    }
}

fn typed_line_checks(rig: &Rig, folder: &str, cols: usize) {
    let lines = rig.logical_lines();
    let hits: Vec<&String> = lines.iter().filter(|l| l.contains("abcdefXYZ")).collect();
    assert_eq!(hits.len(), 1, "want one abcdefXYZ line\n{}", dump(rig));
    let all = lines.join("\n");
    assert_eq!(
        all.matches("abcdef").count(),
        1,
        "stray duplicate of the typed text\n{}",
        dump(rig)
    );
    assert!(
        hits[0].starts_with(folder),
        "line must start with the prompt folder\n{}",
        dump(rig)
    );
    let (line, col) = rig.cursor();
    let screen = rig.screen();
    let at = usize::try_from(line).unwrap_or(0);
    assert!(
        screen.get(at + 1).is_none_or(String::is_empty),
        "the cursor is not on the last row of the line\n{}",
        dump(rig)
    );
    assert_eq!(
        col,
        hits[0].chars().count() % cols,
        "cursor column\n{}",
        dump(rig)
    );
}

fn cmd_in_long_dir() -> (NamedDir, Rig, String) {
    let dir = temp_dir_named(&long_name());
    let folder = dir.path().to_string_lossy().into_owned();
    let rig = Rig::spawn("cmd.exe", &[], dir.path(), None, 80, 24);
    (dir, rig, folder)
}

#[test]
fn o3_cmd_typed_text_survives_narrow_and_wide() {
    let (_dir, mut rig, folder) = cmd_in_long_dir();
    prompt_up(&rig);
    type_and_see(&rig, "abcdef");
    resize_steps(&mut rig, &[(60, 24), (40, 24), (100, 24), (80, 24)]);
    type_and_see(&rig, "XYZ");
    typed_line_checks(&rig, &folder, 80);
}

#[test]
fn o3_cmd_banner_stays_on_screen_when_narrowing() {
    let (_dir, mut rig, _folder) = cmd_in_long_dir();
    prompt_up(&rig);
    resize_steps(&mut rig, &[(60, 24)]);
    assert_eq!(rig.history(), 0, "{}", dump(&rig));
    assert!(
        rig.screen()[0].starts_with("Microsoft Windows"),
        "{}",
        dump(&rig)
    );
}

/// The theme nudge (one column narrower, then back, 80 ms apart) must not cost the prompt line.
#[test]
fn a_resize_nudge_never_loses_the_prompt_line() {
    let (_dir, mut rig, folder) = cmd_in_long_dir();
    prompt_up(&rig);
    type_and_see(&rig, "abcdef");
    resize_steps(&mut rig, &[(79, 24), (80, 24)]);
    type_and_see(&rig, "XYZ");
    typed_line_checks(&rig, &folder, 80);
    assert_eq!(rig.history(), 0, "{}", dump(&rig));
}

fn fill_screen(rig: &Rig) {
    rig.type_str("for /L %i in (1,1,60) do @echo line%i\r");
    let ok = rig.wait_for(15_000, |e| {
        (0..e.rows()).any(|r| e.row_text(i32::try_from(r).unwrap_or(0)) == "line60")
    });
    assert!(ok, "line60 never appeared\n{}", dump(rig));
    prompt_up(rig);
}

#[test]
fn o3_cmd_full_screen_widening() {
    let (_dir, mut rig, folder) = cmd_in_long_dir();
    prompt_up(&rig);
    fill_screen(&rig);
    type_and_see(&rig, "abcdef");
    resize_steps(&mut rig, &[(60, 24), (100, 24)]);
    type_and_see(&rig, "XYZ");
    typed_line_checks(&rig, &folder, 100);
    let (line, _) = rig.cursor();
    let next = usize::try_from(line).unwrap_or(0) + 1;
    if let Some(row) = rig.screen().get(next) {
        assert!(
            row.is_empty(),
            "row after the prompt is not blank\n{}",
            dump(&rig)
        );
    }
}

#[test]
fn o3_cmd_row_changes() {
    let (_dir, mut rig, folder) = cmd_in_long_dir();
    prompt_up(&rig);
    fill_screen(&rig);
    type_and_see(&rig, "abcdef");
    resize_steps(&mut rig, &[(80, 30), (80, 18), (80, 24)]);
    type_and_see(&rig, "XYZ");
    typed_line_checks(&rig, &folder, 80);
    assert!(
        rig.screen().iter().any(|r| r == "line60"),
        "line60 left the screen\n{}",
        dump(&rig)
    );
}

fn powershell_scenario(exe: &str) {
    let dir = temp_dir_named(&long_name());
    let mut rig = Rig::spawn(exe, &["-NoLogo", "-NoProfile"], dir.path(), None, 80, 24);
    prompt_up(&rig);
    type_and_see(&rig, "abcdef");
    resize_steps(&mut rig, &[(60, 24), (40, 24), (100, 24), (80, 24)]);
    type_and_see(&rig, "XYZ");
    let all = rig.screen().concat();
    assert_eq!(all.matches("abcdefXYZ").count(), 1, "{}", dump(&rig));
    assert_eq!(
        all.matches("abcdef").count(),
        1,
        "stray copy
{}",
        dump(&rig)
    );
}

#[test]
fn o3_powershell_typed_text_survives() {
    powershell_scenario("powershell.exe");
}

#[test]
fn o3_pwsh_typed_text_survives() {
    let candidates = [r"C:\Program Files\PowerShell\7\pwsh.exe"];
    match candidates.iter().find(|p| std::path::Path::new(p).exists()) {
        Some(p) => powershell_scenario(p),
        None => println!("skipped: no pwsh"),
    }
}

//! Real-shell smoke tests through the rig (extended in Task 24).

mod common;

use common::{Rig, process_alive, temp_dir_named};
use std::time::{Duration, Instant};

#[test]
fn rig_runs_cmd_and_sees_the_prompt() {
    let dir = temp_dir_named("rig-prompt");
    let rig = Rig::spawn("cmd.exe", &[], dir.path(), None, 80, 24);
    let seen = rig.wait_for(10_000, |e| {
        (0..e.rows()).any(|r| e.row_text(i32::try_from(r).unwrap_or(0)).ends_with('>'))
    });
    assert!(
        seen,
        "cmd never printed a prompt; screen: {:?}",
        rig.screen()
    );
}

#[test]
fn rig_drop_kills_the_shell() {
    let dir = temp_dir_named("rig-drop");
    let rig = Rig::spawn("cmd.exe", &[], dir.path(), None, 80, 24);
    let pid = rig.pid();
    assert!(pid != 0, "the shell reported no process id");
    assert!(process_alive(pid), "cmd was not running before the drop");
    drop(rig);
    let deadline = Instant::now() + Duration::from_secs(5);
    while process_alive(pid) {
        assert!(
            Instant::now() < deadline,
            "cmd (pid {pid}) still ran 5 s after the rig was dropped"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

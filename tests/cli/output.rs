//! The palette: color marks structure only.

use crate::support::*;

#[test]
fn color_marks_structure_only_and_can_be_turned_off() {
    let f = workspace("color");
    f.ok(&["harvest"]);
    let plain = f.ok(&["status"]);
    assert!(!plain.contains('\x1b'), "a pipe gets no color: {plain}");

    let forced = f.run_env_in("app", &["status"], &[("CFGSYNC_COLOR", "always")]);
    let painted = String::from_utf8_lossy(&forced.stdout).into_owned();
    assert!(painted.contains("\x1b[1;36m[status]\x1b[0m"), "{painted}");
    assert!(painted.contains("\x1b[32mclean\x1b[0m"), "{painted}");

    // NO_COLOR turns it off; CFGSYNC_COLOR is the explicit answer and wins
    let off = f.run_env_in("app", &["status"], &[("NO_COLOR", "1")]);
    assert!(String::from_utf8_lossy(&off.stdout).contains("[status] app"));
    let off = f.run_env_in("app", &["status"], &[("CFGSYNC_COLOR", "never"), ("CLICOLOR_FORCE", "1")]);
    assert!(!String::from_utf8_lossy(&off.stdout).contains('\x1b'));
    let forced = f.run_env_in("app", &["status"], &[("CFGSYNC_COLOR", "always"), ("NO_COLOR", "1")]);
    assert!(String::from_utf8_lossy(&forced.stdout).contains("\x1b[1;36m[status]\x1b[0m"));
}

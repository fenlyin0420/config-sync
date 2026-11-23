//! `harvest`: adoption, the base, the overlay, the commit.

use crate::support::*;

#[test]
fn harvest_adopts_and_routes_owned_keys_to_the_overlay() {
    let f = workspace("harvest");
    let os = f.read("central/.os").trim().to_string();
    let out = f.ok(&["harvest"]);
    assert!(out.contains("config.ini") && out.contains("adopted -> base"), "{out}");
    let repo = f.repo("app");

    // the base is the file byte for byte, so comments and order survive
    assert_eq!(
        f.read(&format!("{repo}/base/config.base.ini")),
        "[core]\neditor = vim\ntheme = dark\n\n[limits]\ntimeout = 45\n"
    );
    let overlay = f.read(&format!("{repo}/platform/{os}/config.overlay.ini"));
    assert!(overlay.contains("editor = vim"), "{overlay}");
    assert!(overlay.contains("theme = dark"), "{overlay}");
    assert!(!overlay.contains("timeout"), "{overlay}");
    assert!(f.read(&format!("{repo}/.hash")).len() >= 16);
    assert_eq!(f.last_commit("app"), "cfgsync: harvest");
}

#[test]
fn harvest_pins_an_owned_key_that_still_equals_base() {
    let f = workspace("pin");
    f.ok(&["harvest"]);
    let os = f.read("central/.os").trim().to_string();
    let repo = f.repo("app");
    // editor is not in sync.rules yet, so it lives in the base only
    assert!(!f
        .read(&format!("{repo}/platform/{os}/config.overlay.ini"))
        .contains("timeout"));
    f.write(
        &format!("{repo}/sync.rules"),
        "core.*\nlimits.timeout\n",
    );
    f.ok(&["harvest"]);
    let overlay = f.read(&format!("{repo}/platform/{os}/config.overlay.ini"));
    assert!(
        overlay.contains("timeout = 45"),
        "adding the rule pins the machine value: {overlay}"
    );
}

#[test]
fn harvest_is_idempotent() {
    let f = workspace("harvest-twice");
    f.ok(&["harvest"]);
    let before = f.read(&format!("{}/base/config.base.ini", f.repo("app")));
    let os = f.read("central/.os").trim().to_string();
    let overlay_before = f.read(&format!("{}/platform/{os}/config.overlay.ini", f.repo("app")));
    let commits = f.commit_count("app");
    let out = f.ok(&["harvest"]);
    assert!(out.contains("no changes"), "{out}");
    assert_eq!(f.read(&format!("{}/base/config.base.ini", f.repo("app"))), before);
    assert_eq!(
        f.read(&format!("{}/platform/{os}/config.overlay.ini", f.repo("app"))),
        overlay_before
    );
    assert_eq!(f.commit_count("app"), commits, "nothing new to commit");
}

#[test]
fn harvest_refuses_to_run_outside_a_managed_repo() {
    let f = workspace("harvest-outside");
    let out = f.run_in("", &["harvest"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("is not a managed source repo"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn harvest_all_covers_every_source_repo() {
    let f = workspace("harvest-all");
    f.write("tool/config.ini", "[core]\neditor = vim\n");
    f.ok_in("", &["add", "tool"]);
    f.write(&format!("{}/adopt.rules", f.repo("tool")), "config.ini\n");
    let out = f.ok(&["harvest", "-a"]);
    assert!(out.matches("cfgsync: harvest").count() >= 1 || out.contains("harvest"), "{out}");
    assert!(f.exists(&format!("{}/base/config.base.ini", f.repo("app"))));
    assert!(f.exists(&format!("{}/base/config.base.ini", f.repo("tool"))));
}

#[test]
fn harvest_warns_about_config_nobody_claims() {
    let f = workspace("unmanaged");
    f.write("app/extra.json", "{}\n");
    let out = f.run(&["harvest", "-v"]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("extra.json"), "{err}");
    assert!(err.contains("adopt.rules"), "{err}");
}

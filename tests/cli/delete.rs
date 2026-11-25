//! A deletion propagates, and only on the record's word.

use crate::support::*;

#[test]
fn harvest_drops_the_central_copy_when_the_file_is_deleted() {
    let f = workspace("delete-harvest");
    let repo = f.repo("app");
    let os = f.read("central/.os").trim().to_string();
    f.ok(&["harvest"]);
    std::fs::remove_file(f.root.join("app/settings.json")).unwrap();
    let out = f.ok(&["harvest"]);
    assert!(out.contains("1 deleted"), "{out}");
    assert!(!f.exists(&format!("{repo}/base/settings.base.json")), "the base file is gone");
    assert!(!f.exists(&format!("{repo}/platform/{os}/settings.overlay.json")));
    assert!(
        !f.read(&format!("{repo}/.hash")).contains("settings.json"),
        "the record drops it"
    );
    assert_eq!(f.git("app", &["status", "--porcelain"]).trim(), "", "the deletion is committed");
    // and it does not come back
    f.ok(&["render"]);
    assert!(!f.exists("app/settings.json"));
    assert!(f.exists("app/config.ini"), "the other file is untouched");
}

#[test]
fn a_dry_run_deletes_nothing() {
    let f = workspace("delete-dry");
    let repo = f.repo("app");
    f.ok(&["harvest"]);
    std::fs::remove_file(f.root.join("app/settings.json")).unwrap();
    let out = f.ok(&["harvest", "--dry-run"]);
    assert!(out.contains("1 to delete"), "{out}");
    assert!(f.exists(&format!("{repo}/base/settings.base.json")));
}

#[test]
fn render_deletes_the_file_the_central_repo_dropped() {
    let f = workspace("delete-render");
    let repo = f.repo("app");
    let os = f.read("central/.os").trim().to_string();
    f.ok(&["harvest"]);
    // another machine dropped the file; a pull brings the deletion in
    for name in [
        format!("{repo}/base/settings.base.json"),
        format!("{repo}/platform/{os}/settings.overlay.json"),
    ] {
        std::fs::remove_file(f.root.join(name)).unwrap();
    }
    let out = f.ok(&["render"]);
    assert!(out.contains("1 deleted"), "{out}");
    assert!(!f.exists("app/settings.json"));
    assert!(f.exists("app/config.ini"), "what the central repo still has stays");
    assert_eq!(f.git("app", &["status", "--porcelain"]).trim(), "");
}

#[test]
fn a_fresh_checkout_deletes_nothing() {
    let f = workspace("delete-fresh");
    let repo = f.repo("app");
    f.ok(&["harvest"]);
    // a fresh machine: the record is machine-local and never travels
    std::fs::remove_file(f.root.join(format!("{repo}/.hash"))).unwrap();
    for name in ["config.ini", "settings.json"] {
        std::fs::remove_file(f.root.join(format!("app/{name}"))).unwrap();
    }
    let out = f.ok(&["harvest"]);
    assert!(!out.contains("deleted"), "{out}");
    assert!(
        f.exists(&format!("{repo}/base/config.base.ini")),
        "without a record nothing is pruned"
    );
    // and render puts them back
    f.ok(&["render"]);
    assert!(f.exists("app/config.ini"));
}

#[test]
fn a_removed_line_leaves_the_base_and_then_the_file() {
    let f = workspace("delete-line");
    let repo = f.repo("app");
    f.ok(&["harvest"]);
    // a shared key this machine drops: the base drops the line with it
    f.write(
        "app/config.ini",
        r#"[core]
editor = vim
theme = dark

[limits]
"#,
    );
    f.ok(&["harvest"]);
    let base = f.read(&format!("{repo}/base/config.base.ini"));
    assert!(!base.contains("timeout"), "{base}");

    // a shared line deleted in the central repo leaves the file
    f.write(
        &format!("{repo}/base/config.base.ini"),
        r#"[core]
editor = vim
theme = dark

[limits]
retries = 3
"#,
    );
    f.ok(&["render"]);
    assert!(f.read("app/config.ini").contains("retries = 3"));
    f.write(
        &format!("{repo}/base/config.base.ini"),
        r#"[core]
editor = vim
theme = dark
"#,
    );
    f.ok(&["render"]);
    assert!(
        !f.read("app/config.ini").contains("retries"),
        "the line is gone from the file too"
    );
}

//! `edit`: handing a rules file to the editor.

use crate::support::*;

#[test]
fn edit_opens_the_rules_in_the_editor() {
    let f = workspace("edit");
    let out = f.ok_in("app", &["edit", "-s"]);
    assert!(out.contains("sync.rules"), "{out}");
    assert!(out.contains("true"), "{out}");
    let out = f.ok_in("app", &["edit", "-a"]);
    assert!(out.contains("adopt.rules"), "{out}");
}

#[test]
fn edit_creates_a_missing_rules_file_and_needs_a_flag() {
    let f = workspace("edit-create");
    let repo = f.repo("app");
    std::fs::remove_file(f.root.join(format!("{repo}/sync.rules"))).unwrap();
    f.run_env_in("app", &["edit", "-s"], &[("EDITOR", "true")]);
    assert!(f.exists(&format!("{repo}/sync.rules")));
    let out = f.run_in("app", &["edit"]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn edit_works_from_a_subdirectory_of_the_source_repo() {
    let f = workspace("edit-sub");
    f.write("app/ui/.keep", "");
    let out = f.run_in("app/ui", &["edit", "-a"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("adopt.rules"));
}

#[test]
fn edit_opens_the_rules_of_the_directory_it_was_started_in() {
    // `-a` means adopt.rules here, never "every source repo"
    let f = Fixture::new("edit-cwd");
    f.write(
        "aaa/config.ini",
        r#"[core]
editor = vim
"#,
    )
    .write(
        "zzz/config.ini",
        r#"[core]
editor = vim
"#,
    );
    f.ok(&["init"]);
    f.ok(&["add", "aaa"]);
    f.ok(&["add", "zzz"]);
    let out = f.ok_in("zzz", &["edit", "-a"]);
    assert!(
        slashes(&out).contains(&format!("{}/adopt.rules", f.repo("zzz"))),
        "edit opened another repo's rules: {out}"
    );
}

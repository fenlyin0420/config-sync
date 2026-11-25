//! `add` / `rm`: registering a source directory and letting go of it.

use crate::support::*;

#[test]
fn add_registers_a_directory_and_git_inits_it() {
    let f = Fixture::new("add");
    f.write(
        "app/config.ini",
        r#"[core]
editor = vim
"#,
    );
    f.ok(&["init"]);
    let out = f.ok(&["add", "app"]);
    let repo = f.repo("app");
    assert!(out.contains(&repo), "{out}");
    assert!(f.exists(&format!("{repo}/adopt.rules")));
    assert!(f.exists(&format!("{repo}/sync.rules")));
    assert!(f.exists(&format!("{repo}/base")));
    let os = f.read("central/.os").trim().to_string();
    assert!(f.exists(&format!("{repo}/platform/{os}")));
    assert!(
        f.git("app", &["rev-parse", "--git-dir"]).contains(".git"),
        "the source repo owns a git repo now"
    );
}

#[test]
fn add_refuses_a_file_and_a_missing_directory() {
    let f = Fixture::new("add-refuse");
    f.write(
        "app/config.ini",
        r#"[core]
editor = vim
"#,
    );
    f.ok(&["init"]);
    let out = f.run(&["add", "app/config.ini"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("only directories"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(f.run(&["add", "nope"]).status.code() == Some(2));
}

#[test]
fn add_needs_a_central_repo() {
    let f = Fixture::new("add-no-init");
    f.write(
        "app/config.ini",
        r#"[core]
editor = vim
"#,
    );
    let out = f.run(&["add", "app"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("cfgsync init"));
}

#[test]
fn add_takes_a_custom_name_and_suffixes_a_taken_one() {
    let f = Fixture::new("add-name");
    f.write(
        "one/app/config.ini",
        r#"[core]
editor = vim
"#,
    )
    .write(
        "two/app/config.ini",
        r#"[core]
editor = vi
"#,
    )
    .write(
        "three/other/config.ini",
        r#"[core]
editor = nano
"#,
    );
    f.ok(&["init"]);
    f.ok_in("", &["add", "one/app", "nvim"]);
    assert!(f.exists("central/nvim/path.rules"));
    let out = f.ok_in("", &["add", "two/app", "nvim"]);
    assert!(out.contains("nvim-2"), "{out}");
    assert!(f.exists("central/nvim-2/path.rules"));
    // no name at all: the directory's own name
    f.ok_in("", &["add", "three/other"]);
    assert!(f.exists("central/other/path.rules"));
}

#[test]
fn adding_the_same_directory_twice_keeps_one_central_repo() {
    let f = Fixture::new("add-twice");
    f.write(
        "app/config.ini",
        r#"[core]
editor = vim
"#,
    );
    f.ok(&["init"]);
    f.ok(&["add", "app"]);
    let out = f.ok_in("", &["add", "app", "other"]);
    assert!(out.contains("already managed as app"), "{out}");
    assert!(!f.exists("central/other"));
}

#[test]
fn rm_drops_the_central_copy_and_leaves_the_source_alone() {
    let f = workspace("rm");
    f.ok(&["harvest"]);
    let repo = f.repo("app");
    assert!(f.exists(&format!("{repo}/adopt.rules")));
    let out = f.ok_in("", &["rm", "app"]);
    assert!(out.contains("removed from the central repo"), "{out}");
    assert!(!f.exists(&repo));
    assert!(f.exists("app/config.ini"), "the source directory survives");
    assert!(f.exists("app/.git"));
}

#[test]
fn rm_accepts_the_central_name_after_a_double_dash() {
    let f = workspace("rm-name");
    f.ok(&["harvest"]);
    let name = f.name_of("app");
    let out = f.ok_in("", &["rm", "--", &name]);
    assert!(out.contains("removed from the central repo"), "{out}");
    assert!(!f.exists(&format!("central/{name}")));
}

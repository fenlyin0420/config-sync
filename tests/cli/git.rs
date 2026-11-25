//! `push` / `pull` against a bare remote.

use std::process::Command;

use crate::support::*;

fn bare_remote(f: &Fixture, name: &str) -> String {
    let path = f.root.join(format!("remote-{name}.git"));
    Command::new("git")
        .args(["init", "--bare", "-q"])
        .arg(&path)
        .output()
        .unwrap();
    path.to_string_lossy().into_owned()
}

#[test]
fn push_commits_the_central_repo_and_pushes_it() {
    let f = workspace("push");
    f.ok(&["harvest"]);
    let remote = bare_remote(&f, "push");
    f.ok(&["init", "--remote", &remote]);
    let out = f.ok(&["push"]);
    assert!(out.contains("pushed"), "{out}");
    assert_ne!(f.git("central", &["log", "-1", "--pretty=%s"]).trim(), "");

    // a fresh clone of the cloud repo carries the base file
    let clone = f.root.join("clone");
    Command::new("git")
        .args(["clone", "-q", &remote])
        .arg(&clone)
        .output()
        .unwrap();
    let repo_dir = clone.join(f.name_of("app"));
    assert!(
        repo_dir.join("base/config.base.ini").is_file(),
        "the named repo directory is in the cloud copy"
    );
    assert!(
        repo_dir.join("path.rules").is_file(),
        "and so is the platform path"
    );
}

#[test]
fn pull_brings_a_remote_change_into_the_central_repo() {
    let f = workspace("pull");
    f.ok(&["harvest"]);
    let remote = bare_remote(&f, "pull");
    f.ok(&["init", "--remote", &remote]);
    f.ok(&["push"]);

    // another machine pushes a new base
    let other = f.root.join("other");
    Command::new("git")
        .args(["clone", "-q", &remote])
        .arg(&other)
        .output()
        .unwrap();
    let repo_dir = std::fs::read_dir(&other)
        .unwrap()
        .filter_map(|e| e.ok())
        .find(|e| e.path().is_dir() && !e.file_name().to_string_lossy().starts_with('.'))
        .unwrap();
    std::fs::write(
        repo_dir.path().join("base/settings.base.json"),
        r#"{
  "theme": "light"
}
"#,
    )
    .unwrap();
    for args in [
        vec!["add", "-A"],
        vec![
            "-c",
            "user.name=other",
            "-c",
            "user.email=o@example.com",
            "commit",
            "-qm",
            "other machine",
        ],
        vec!["push", "-q"],
    ] {
        Command::new("git")
            .arg("-C")
            .arg(&other)
            .args(&args)
            .output()
            .unwrap();
    }
    let out = f.ok(&["pull"]);
    assert!(out.contains("updated"), "{out}");
    assert!(f
        .read(&format!("{}/base/settings.base.json", f.repo("app")))
        .contains("light"));
}

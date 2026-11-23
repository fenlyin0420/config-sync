//! `cd`: the central repo path, and nothing else, on stdout.

use crate::support::*;

#[test]
fn cd_prints_the_central_repo_path() {
    let f = workspace("cd");
    let out = f.ok_in("app", &["cd"]);
    let central = std::fs::canonicalize(f.central()).unwrap();
    assert_eq!(out.trim(), central.to_string_lossy().replace('\\', "/"));
}

#[test]
fn cd_needs_a_central_repo_and_takes_no_arguments() {
    let f = Fixture::new("cd-empty");
    let out = f.run_in("", &["cd"]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("no central repo"), "{err}");

    f.ok(&["init"]);
    let out = f.run_in("", &["cd", "app"]);
    assert_eq!(out.status.code(), Some(2));
}

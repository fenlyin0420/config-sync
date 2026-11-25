//! `init`: the central repo comes into being exactly once.

use crate::support::*;

#[test]
fn init_creates_the_central_repo_once() {
    let f = Fixture::new("init");
    let out = f.ok(&["init"]);
    assert!(out.contains("[init] central repo"), "{out}");
    assert!(f.git("central", &["rev-parse", "--git-dir"]).contains(".git"));
    assert_eq!(
        f.read("central/.gitignore"),
        r#".hash
.os
"#
    );
    assert!(!f.read("central/.os").trim().is_empty());

    // second run is a no-op, not an error
    let again = f.ok(&["init"]);
    assert!(again.contains("central repo"), "{again}");
    assert_eq!(
        f.read("central/.gitignore"),
        r#".hash
.os
"#,
        "idempotent"
    );
}

#[test]
fn init_dry_run_writes_nothing() {
    let f = Fixture::new("init-dry");
    let out = f.ok(&["init", "--dry-run"]);
    assert!(out.contains("would"), "{out}");
    assert!(!f.exists("central/.git"));
}

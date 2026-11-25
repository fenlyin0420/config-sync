//! `status`: what would move, in one line per file.

use crate::support::*;

#[test]
fn status_reports_changed_files_then_goes_clean() {
    let f = workspace("status");
    f.ok(&["harvest"]);
    f.ok(&["render"]);
    let clean = f.ok(&["status"]);
    assert!(clean.contains("[status] clean"), "{clean}");

    f.write(
        "app/config.ini",
        r#"[core]
editor = nano
theme = dark

[limits]
timeout = 45
"#,
    );
    f.write(
        "app/settings.json",
        r#"{
  "theme": "light",
  "packages": ["npm:a"],
  "tools": ["+codemode"]
}
"#,
    );
    let dirty = f.ok(&["status"]);
    assert!(dirty.contains("M  config.ini"), "{dirty}");
    assert!(dirty.contains("M  settings.json"), "{dirty}");
    assert!(dirty.contains("render would refuse"), "{dirty}");
    assert!(dirty.contains("2 file(s) differ"), "{dirty}");
}

#[test]
fn status_reports_a_file_that_is_not_on_disk() {
    let f = workspace("status-missing");
    let repo = f.repo("app");
    f.write(
        &format!("{repo}/base/gone.base.ini"),
        r#"[core]
editor = vim
"#,
    );
    f.write(
        &format!("{repo}/adopt.rules"),
        r#"config.ini
settings.json
gone.ini
"#,
    );
    let out = f.ok(&["status"]);
    assert!(out.contains("+  gone.ini"), "{out}");
}

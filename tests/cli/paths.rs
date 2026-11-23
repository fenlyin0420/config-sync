//! `path.rules` is the only mapping: resolve, create, skip, report.

use crate::support::*;

#[test]
fn add_records_the_platform_path_in_path_rules() {
    let f = Fixture::new("add-path");
    f.write("app/config.ini", "[core]\neditor = vim\n");
    f.ok(&["init"]);
    let out = f.ok(&["add", "app"]);
    let repo = f.repo("app");
    let os = f.read("central/.os").trim().to_string();
    let rules = f.read(&format!("{repo}/path.rules"));
    assert!(
        rules.contains(&format!("{os} = {}", source_path(&f, "app"))),
        "path.rules should hold this platform's path:\n{rules}"
    );
    assert!(out.contains("path.rules"), "{out}");
}

#[test]
fn a_second_machine_adds_its_own_path_to_the_same_repo() {
    // the same app lives somewhere else on windows: the second machine extends
    // path.rules instead of creating a second central repo
    let f = Fixture::new("second-machine");
    f.write("linux/nvim/init.lua", "vim.o.number = true\n")
        .write("win/nvim/init.lua", "vim.o.number = true\n");
    f.ok(&["init"]);
    f.ok_in("", &["add", "linux/nvim", "nvim"]);
    f.ok_in("", &["add", "win/nvim", "nvim", "--os", "win"]);
    let rules = f.read("central/nvim/path.rules");
    assert!(rules.contains("linux = "), "{rules}");
    assert!(rules.contains("win = "), "{rules}");
    assert!(!f.exists("central/nvim-2"), "one repo, two platform paths");
    // and each platform renders its own copy
    f.ok_in("win/nvim", &["render", "--os", "win"]);
    assert!(f.read("win/nvim/init.lua").contains("number"));
}

#[test]
fn render_creates_the_directory_path_rules_spells_out() {
    // a fresh machine: the app never ran, path.rules already says where it
    // goes -> render builds the directory instead of refusing
    let f = workspace("gone");
    f.ok(&["harvest"]);
    std::fs::remove_dir_all(f.root.join("app")).unwrap();
    let out = f.ok_in("", &["render", "app"]);
    assert!(out.contains("not there yet -> created it"), "{out}");
    assert!(f.read("app/config.ini").contains("editor = vim"));
    assert!(f.read("app/settings.json").contains("packages"));
}

#[test]
fn a_dry_run_says_it_would_create_the_directory_and_creates_nothing() {
    let f = workspace("gone-dry");
    f.ok(&["harvest"]);
    std::fs::remove_dir_all(f.root.join("app")).unwrap();
    let out = f.ok_in("", &["render", "app", "--dry-run"]);
    assert!(out.contains("would create it"), "{out}");
    assert!(!f.exists("app"), "the dry run wrote the directory");
}

#[test]
fn harvest_and_status_still_refuse_a_directory_that_is_not_there() {
    let f = workspace("gone-others");
    f.ok(&["harvest"]);
    std::fs::remove_dir_all(f.root.join("app")).unwrap();
    for cmd in ["harvest", "status"] {
        let out = f.run_in("", &[cmd, "app"]);
        assert_eq!(out.status.code(), Some(1), "{cmd}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("no such directory"), "{cmd}: {err}");
        assert!(err.contains("path.rules"), "{cmd}: {err}");
    }
}

#[test]
fn all_skips_a_repo_that_has_no_path_for_this_platform() {
    let f = workspace("all-skip");
    let app = source_path(&f, "app");
    f.write(
        &format!("{}/path.rules", f.repo("app")),
        &format!("plan9 = {app}\n"),
    );
    let out = f.run_in("", &["render", "-a", "-v"]);
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("no path for"), "{err}");
    assert!(err.contains("skipped"), "{err}");
}

#[test]
fn a_directory_without_path_rules_is_reported_not_used() {
    let f = workspace("old-layout");
    f.write(
        "central/legacy/base/config.base.ini",
        "[core]\neditor = vim\n",
    );
    let out = f.run_in("", &["status", "-a", "-v"]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("legacy has no legacy/path.rules"), "{err}");
    assert!(err.contains("cfgsync rm legacy"), "{err}");
}

#[test]
fn a_command_takes_the_central_name_or_the_path() {
    let f = workspace("name-arg");
    f.ok(&["harvest"]);
    let by_name = f.ok_in("", &["status", "app"]);
    let by_path = f.ok_in("", &["status", &source_path(&f, "app")]);
    assert!(by_name.contains("[status]"), "{by_name}");
    assert!(by_path.contains("[status]"), "{by_path}");
}

//! `render`: base + overlay into the source repo, behind the hash guard.

use crate::support::*;

#[test]
fn render_writes_base_plus_overlay_into_an_empty_source_repo() {
    let f = Fixture::new("render-fresh");
    f.ok(&["init"]);
    f.write("app/.gitkeep", "");
    f.ok(&["add", "app"]);
    let repo = f.repo("app");
    let os = f.read("central/.os").trim().to_string();
    f.write(&format!("{repo}/adopt.rules"), "config.ini\n")
        .write(
            &format!("{repo}/base/config.base.ini"),
            "[core]\neditor = vim\ntheme = dark\n",
        )
        .write(
            &format!("{repo}/platform/{os}/config.overlay.ini"),
            "[core]\ntheme = solarized\n",
        );
    let out = f.ok_in("app", &["render"]);
    assert!(out.contains("1 written"), "{out}");
    assert_eq!(
        f.read("app/config.ini"),
        "[core]\neditor = vim\ntheme = solarized\n"
    );
    assert_eq!(f.last_commit("app"), "cfgsync: render");
}

#[test]
fn render_is_idempotent_and_commits_nothing_twice() {
    let f = workspace("render-twice");
    f.ok(&["harvest"]);
    f.ok(&["render"]);
    let commits = f.commit_count("app");
    let out = f.ok(&["render"]);
    assert!(out.contains("0 written"), "{out}");
    assert_eq!(f.commit_count("app"), commits);
}

#[test]
fn render_refuses_after_an_unharvested_edit() {
    let f = workspace("render-guard");
    f.ok(&["harvest"]);
    f.write("app/config.ini", "[core]\neditor = nano\ntheme = dark\n\n[limits]\ntimeout = 45\n");
    let out = f.run(&["render"]);
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("changed since the last harvest"), "{err}");
    assert!(
        f.read("app/config.ini").contains("nano"),
        "the unharvested edit is still there"
    );
}

#[test]
fn render_force_overwrites_the_unharvested_edit() {
    let f = workspace("render-force");
    f.ok(&["harvest"]);
    f.write("app/config.ini", "[core]\neditor = nano\ntheme = dark\n\n[limits]\ntimeout = 45\n");
    f.ok(&["render", "--force"]);
    assert!(f.read("app/config.ini").contains("vim"), "forced back to base + overlay");
}

#[test]
fn render_leaves_a_file_it_never_wrote_alone() {
    let f = Fixture::new("render-no-record");
    f.ok(&["init"]);
    f.write("app/config.ini", "[core]\neditor = nano\n");
    f.ok(&["add", "app"]);
    let repo = f.repo("app");
    f.write(&format!("{repo}/adopt.rules"), "config.ini\n").write(
        &format!("{repo}/base/config.base.ini"),
        "[core]\neditor = vim\n",
    );
    let out = f.run_in("app", &["render", "-v"]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("never synced it"), "{err}");
    assert!(f.read("app/config.ini").contains("nano"));
    assert!(!f.exists(&format!("{repo}/.hash")), "no record is written");
}

#[test]
fn render_writes_a_pulled_base_for_a_file_this_machine_never_had() {
    let f = workspace("render-pulled");
    f.ok(&["harvest"]);
    let repo = f.repo("app");
    // a base arrives from another machine for a file that does not exist here
    f.write(
        &format!("{repo}/base/extra.base.json"),
        "{\n  \"theme\": \"dark\"\n}\n",
    );
    f.write(&format!("{repo}/adopt.rules"), "config.ini\nsettings.json\nextra.json\n");
    let out = f.ok(&["render"]);
    assert!(out.contains("extra.json"), "{out}");
    assert_eq!(f.read("app/extra.json"), "{\n  \"theme\": \"dark\"\n}\n");
}

#[test]
fn the_platform_overlay_is_selected_by_os() {
    let f = workspace("os");
    let repo = f.repo("app");
    // every platform the test asks for needs its own line; a real second
    // machine would have added it itself
    let app = source_path(&f, "app");
    let os = f.read("central/.os").trim().to_string();
    f.write(
        &format!("{repo}/path.rules"),
        &format!("{os} = {app}\nwin = {app}\nplan9 = {app}\n"),
    );
    f.write(
        &format!("{repo}/base/config.base.ini"),
        "[core]\neditor = vim\n",
    )
    .write(
        &format!("{repo}/platform/win/config.overlay.ini"),
        "[core]\neditor = code\n",
    );
    // a fresh machine: nothing harvested here yet, so the files are written
    std::fs::remove_file(f.root.join("app/config.ini")).unwrap();
    std::fs::remove_file(f.root.join("app/settings.json")).unwrap();
    f.ok(&["render", "--os", "win"]);
    assert!(f.read("app/config.ini").contains("code"));
    f.ok(&["render", "--os", "plan9"]);
    assert!(
        f.read("app/config.ini").contains("vim"),
        "an unknown platform degrades to base only"
    );
}

#[test]
fn render_accepts_a_pulled_base_while_the_source_is_untouched() {
    let f = workspace("render-pulled-base");
    f.ok(&["harvest"]);
    f.ok(&["render"]);
    let repo = f.repo("app");
    // a base edit arrives from another machine; nothing here was edited
    f.write(
        &format!("{repo}/base/config.base.ini"),
        "[core]\neditor = vim\ntheme = dark\n\n[limits]\ntimeout = 90\n",
    );
    let out = f.ok(&["render"]);
    assert!(out.contains("config.ini"), "{out}");
    assert!(f.read("app/config.ini").contains("timeout = 90"));
}

#[test]
fn dry_run_prints_the_plan_and_writes_nothing() {
    let f = workspace("dry");
    let out = f.ok(&["harvest", "--dry-run"]);
    assert!(out.contains("config.ini"), "{out}");
    assert!(!f.exists(&format!("{}/base/config.base.ini", f.repo("app"))));
    assert!(!f.exists(&format!("{}/.hash", f.repo("app"))));
    assert_eq!(f.commit_count("app"), 0, "dry run commits nothing");

    f.ok(&["harvest"]);
    let commits = f.commit_count("app");
    let out = f.ok(&["render", "--dry-run"]);
    assert!(out.contains("to write"), "{out}");
    assert_eq!(f.commit_count("app"), commits);
}

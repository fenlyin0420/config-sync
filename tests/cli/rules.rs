//! `sync.rules` is authoritative in both directions, vetoes included.

use crate::support::*;

#[test]
fn rules_are_authoritative_in_both_directions() {
    let f = workspace("drift");
    f.ok(&["harvest"]);
    let os = f.read("central/.os").trim().to_string();
    let repo = f.repo("app");
    // an overlay entry sync.rules does not own is drift: it goes back to base
    f.write(
        &format!("{repo}/platform/{os}/config.overlay.ini"),
        "[core]\neditor = code\n\n[limits]\nnotowned = 1\n",
    );
    f.write(
        "app/config.ini",
        "[core]\neditor = code\n\n[limits]\nnotowned = 1\ntimeout = 45\n",
    );
    f.ok(&["harvest"]);
    assert!(
        f.read(&format!("{repo}/platform/{os}/config.overlay.ini"))
            .contains("editor = code"),
        "the owned key stays in the overlay"
    );
    assert!(
        !f.read(&format!("{repo}/platform/{os}/config.overlay.ini"))
            .contains("notowned"),
        "the unowned key is pruned from the overlay"
    );
    assert!(
        f.read(&format!("{repo}/base/config.base.ini"))
            .contains("notowned = 1"),
        "and promoted to the base: {}",
        f.read(&format!("{repo}/base/config.base.ini"))
    );
}

#[test]
fn a_rule_can_scope_itself_to_one_nested_file() {
    let f = Fixture::new("rule-path");
    f.write(
        "app/settings.json",
        "{\n  \"theme\": \"dark\",\n  \"defaultTools\": [\"a\"]\n}\n",
    )
    .write(
        "app/agent/settings.json",
        "{\n  \"theme\": \"dark\",\n  \"defaultTools\": [\"a\"]\n}\n",
    );
    f.ok(&["init"]);
    f.ok(&["add", "app"]);
    let repo = f.repo("app");
    let os = f.read("central/.os").trim().to_string();
    f.write(&format!("{repo}/adopt.rules"), "settings.json\n")
        .write(
            &format!("{repo}/sync.rules"),
            "/agent/settings.json:defaultTools\n",
        );
    f.ok_in("app", &["harvest"]);

    let nested = f.read(&format!("{repo}/platform/{os}/agent/settings.overlay.json"));
    assert!(nested.contains("defaultTools"), "{nested}");
    assert!(
        f.read(&format!("{repo}/base/agent/settings.base.json"))
            .contains("theme"),
        "the untouched key stays in the nested base"
    );
    assert!(
        f.read(&format!("{repo}/base/settings.base.json"))
            .contains("defaultTools"),
        "the rule does not reach the root file"
    );
    assert!(
        !f.exists(&format!("{repo}/platform/{os}/settings.overlay.json")),
        "the root file owns nothing -> no overlay file"
    );
    // both directions still round-trip
    let out = f.ok_in("app", &["render"]);
    assert!(out.contains("0 written"), "{out}");
}

#[test]
fn a_bang_line_keeps_a_key_out_of_the_central_repo() {
    let f = Fixture::new("veto");
    let secret = "\"apiKey\": \"sk-local-123\",\n";
    f.write(
        "app/agent/settings.json",
        &format!("{{\n  \"theme\": \"solarized\",\n  {secret}  \"packages\": [\"npm:a\"]\n}}\n"),
    );
    f.ok(&["init"]);
    f.ok(&["add", "app"]);
    let repo = f.repo("app");
    let os = f.read("central/.os").trim().to_string();
    f.write(&format!("{repo}/adopt.rules"), "agent/settings.json\n")
        .write(
            &format!("{repo}/sync.rules"),
            "/agent/settings.json:theme\n!/agent/settings.json:apiKey\n",
        );
    f.ok_in("app", &["harvest"]);

    let base = f.read(&format!("{repo}/base/agent/settings.base.json"));
    assert!(!base.contains("apiKey"), "adoption must not commit the secret: {base}");
    assert!(!base.contains("sk-local-123"), "adoption must not commit the secret: {base}");
    assert!(base.contains("packages"), "the rest of the file is still the base: {base}");
    let overlay = f.read(&format!("{repo}/platform/{os}/agent/settings.overlay.json"));
    assert!(overlay.contains("solarized"), "{overlay}");
    assert!(!overlay.contains("apiKey"), "the veto is not machine-owned either: {overlay}");
    assert!(
        f.read("app/agent/settings.json").contains("sk-local-123"),
        "the app's own file keeps its value"
    );

    // render must not strip it from the source repo, and nothing moves
    let out = f.ok_in("app", &["render"]);
    assert!(out.contains("0 written"), "{out}");
    assert!(f.read("app/agent/settings.json").contains("sk-local-123"));
    let out = f.ok_in("app", &["harvest"]);
    assert!(out.contains("no changes"), "{out}");

    // a central repo that already committed the key loses it on the next harvest
    f.write(
        &format!("{repo}/base/agent/settings.base.json"),
        &format!("{{\n  \"theme\": \"solarized\",\n  {secret}  \"packages\": []\n}}\n"),
    );
    let out = f.ok_in("app", &["harvest"]);
    assert!(out.contains("keep : apiKey"), "{out}");
    let base = f.read(&format!("{repo}/base/agent/settings.base.json"));
    assert!(!base.contains("apiKey"), "the committed secret is pruned: {base}");
    assert!(base.contains("packages"), "everything else survives: {base}");
}

#[test]
fn no_overlay_file_is_created_while_nothing_is_machine_owned() {
    let f = Fixture::new("no-overlay");
    f.write("app/config.ini", "[core]\neditor = vim\ntheme = dark\n");
    f.ok(&["init"]);
    f.ok(&["add", "app"]);
    let repo = f.repo("app");
    let os = f.read("central/.os").trim().to_string();
    f.write(&format!("{repo}/adopt.rules"), "config.ini\n")
        .write(&format!("{repo}/sync.rules"), "# nothing is machine-owned yet\n");
    f.ok_in("app", &["harvest"]);

    assert!(
        f.read(&format!("{repo}/base/config.base.ini")).contains("editor"),
        "everything goes to the base"
    );
    assert!(
        !f.exists(&format!("{repo}/platform/{os}/config.overlay.ini")),
        "no delta routed here -> no overlay file"
    );
    let out = f.ok_in("app", &["harvest"]);
    assert!(out.contains("no changes"), "{out}");

    // the first owned key is what brings the overlay file into being
    f.write(&format!("{repo}/sync.rules"), "core.theme\n");
    f.ok_in("app", &["harvest"]);
    assert!(f
        .read(&format!("{repo}/platform/{os}/config.overlay.ini"))
        .contains("theme = dark"));
}

#[test]
fn the_last_machine_owned_key_takes_the_overlay_with_it() {
    let f = workspace("delete-overlay");
    let repo = f.repo("app");
    let os = f.read("central/.os").trim().to_string();
    f.ok(&["harvest"]);
    assert!(f.exists(&format!("{repo}/platform/{os}/settings.overlay.json")));
    // nothing is machine-owned any more, so every overlay entry is drift
    f.write(&format!("{repo}/sync.rules"), "# nothing is machine-owned\n");
    f.ok(&["harvest"]);
    assert!(
        !f.exists(&format!("{repo}/platform/{os}")),
        "an overlay with no key left is not kept"
    );
}

#[test]
fn a_second_harvest_keeps_an_owned_list_in_the_overlay() {
    let f = workspace("pin-list");
    let repo = f.repo("app");
    let os = f.read("central/.os").trim().to_string();
    f.ok(&["harvest"]);
    let overlay = format!("{repo}/platform/{os}/settings.overlay.json");
    let first = f.read(&overlay);
    assert!(first.contains("+codemode"), "the owned list is pinned: {first}");

    let out = f.ok(&["harvest"]);
    assert!(out.contains("no changes"), "{out}");
    assert_eq!(f.read(&overlay), first, "a second harvest rewrites nothing");
}

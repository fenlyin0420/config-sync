//! One adapter per file style; the registry `adaptor` prints.

use crate::support::*;

#[test]
fn adaptor_lists_every_supported_file_style() {
    let f = Fixture::new("adaptor");
    let out = f.run(&["adaptor"]);
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    for needle in [
        "ini", "json", "jsonl", "plain", "toml", "yaml", "*.json", "*.ini", "*.jsonl", "*.toml",
        "*.yaml", "config (file name)", "share",
    ] {
        assert!(text.contains(needle), "{needle} missing from:\n{text}");
    }
    // the old spelling keeps working
    assert!(f.run(&["formats"]).status.success());
}

#[test]
fn a_yaml_config_is_split_by_key() {
    let f = Fixture::new("yaml");
    let doc = r#"# app
ui:
  theme: dark
  padding: 2

db:
  host: local
"#;
    f.write("app/config.yaml", doc);
    f.ok(&["init"]);
    f.ok(&["add", "app"]);
    let repo = f.repo("app");
    let os = f.read("central/.os").trim().to_string();
    f.write(&format!("{repo}/adopt.rules"), "config.yaml\n")
        .write(&format!("{repo}/sync.rules"), "db.*\n");
    f.ok_in("app", &["harvest"]);

    // adoption keeps the file byte for byte, comments and blank lines included
    assert_eq!(f.read(&format!("{repo}/base/config.base.yaml")), doc);
    let overlay = f.read(&format!("{repo}/platform/{os}/config.overlay.yaml"));
    assert!(overlay.contains("db.host: local"), "{overlay}");
    assert!(!overlay.contains("theme"), "{overlay}");

    // base + overlay reproduce the file exactly
    f.ok_in("app", &["render"]);
    assert_eq!(f.read("app/config.yaml"), doc);

    // a shared edit reaches the base, an owned one stays in the overlay
    f.write(
        "app/config.yaml",
        &doc.replace("theme: dark", "theme: light")
            .replace("host: local", "host: remote"),
    );
    f.ok_in("app", &["harvest"]);
    assert!(f
        .read(&format!("{repo}/base/config.base.yaml"))
        .contains("theme: light"));
    assert!(f
        .read(&format!("{repo}/platform/{os}/config.overlay.yaml"))
        .contains("db.host: remote"));
    let out = f.ok_in("app", &["status"]);
    assert!(out.contains("clean"), "{out}");
}

#[test]
fn a_toml_config_is_split_by_key() {
    let f = Fixture::new("toml");
    let doc = r#"# app
title = "app"

[window]
size = 2
opacity = 1.0
"#;
    f.write("app/config.toml", doc);
    f.ok(&["init"]);
    f.ok(&["add", "app"]);
    let repo = f.repo("app");
    let os = f.read("central/.os").trim().to_string();
    f.write(&format!("{repo}/adopt.rules"), "config.toml\n")
        .write(&format!("{repo}/sync.rules"), "window.size\n");
    f.ok_in("app", &["harvest"]);
    assert_eq!(f.read(&format!("{repo}/base/config.base.toml")), doc);
    let overlay = f.read(&format!("{repo}/platform/{os}/config.overlay.toml"));
    assert!(overlay.contains("[window]"), "{overlay}");
    assert!(overlay.contains("size = 2"), "{overlay}");
    assert!(!overlay.contains("opacity"), "{overlay}");

    f.ok_in("app", &["render"]);
    assert_eq!(f.read("app/config.toml"), doc);

    f.write(
        "app/config.toml",
        &doc.replace("size = 2", "size = 3")
            .replace("opacity = 1.0", "opacity = 0.9"),
    );
    f.ok_in("app", &["harvest"]);
    assert!(f
        .read(&format!("{repo}/base/config.base.toml"))
        .contains("opacity = 0.9"));
    let out = f.ok_in("app", &["status"]);
    assert!(out.contains("clean"), "{out}");
}

#[test]
fn a_jsonl_config_is_split_by_record() {
    let f = Fixture::new("jsonl");
    let doc = r#"{"model": "a", "temp": 1}
{"model": "b", "temp": 2}
"#;
    f.write("app/app.jsonl", doc);
    f.ok(&["init"]);
    f.ok(&["add", "app"]);
    let repo = f.repo("app");
    let os = f.read("central/.os").trim().to_string();
    f.write(&format!("{repo}/adopt.rules"), "app.jsonl\n")
        .write(&format!("{repo}/sync.rules"), "line2\n");
    f.ok_in("app", &["harvest"]);
    assert_eq!(f.read(&format!("{repo}/base/app.base.jsonl")), doc);
    let overlay = f.read(&format!("{repo}/platform/{os}/app.overlay.jsonl"));
    assert!(overlay.contains("\"line2\""), "{overlay}");
    assert!(overlay.contains("\"model\":\"b\""), "{overlay}");

    // the first record is shared, the second machine-owned
    f.write(
        "app/app.jsonl",
        r#"{"model": "z", "temp": 1}
{"model": "b", "temp": 2}
"#,
    );
    f.ok_in("app", &["harvest"]);
    let base = f.read(&format!("{repo}/base/app.base.jsonl"));
    assert!(base.contains("\"model\":\"z\""), "{base}");
    assert!(base.contains("{\"model\": \"b\", \"temp\": 2}"), "{base}");
    assert!(f
        .read(&format!("{repo}/platform/{os}/app.overlay.jsonl"))
        .contains("\"model\":\"b\""));
    let out = f.ok_in("app", &["status"]);
    assert!(out.contains("clean"), "{out}");
}

#[test]
fn an_extensionless_config_is_split_by_key() {
    let f = Fixture::new("plain");
    let ghostty = r#"# ghostty
theme = catppuccin-mocha
keybind = ctrl+a=text:a
keybind = ctrl+b=text:b
"#;
    f.write("app/config", ghostty);
    f.ok(&["init"]);
    f.ok(&["add", "app"]);
    let repo = f.repo("app");
    let os = f.read("central/.os").trim().to_string();
    // `keybind` owns every occurrence, including `keybind#2`
    f.write(&format!("{repo}/adopt.rules"), "config\n")
        .write(&format!("{repo}/sync.rules"), "keybind\n");
    f.ok_in("app", &["harvest"]);

    // adoption keeps the file verbatim as the base, exactly like the other
    // adapters: the owned keys are pinned into the overlay on top of it
    assert_eq!(f.read(&format!("{repo}/base/config.base")), ghostty);
    let overlay = f.read(&format!("{repo}/platform/{os}/config.overlay"));
    assert!(overlay.contains("keybind = ctrl+a=text:a"), "{overlay}");
    assert!(overlay.contains("keybind#2 = ctrl+b=text:b"), "{overlay}");

    // a shared key moves to the base, the owned ones stay put
    f.ok_in("app", &["render"]);
    assert_eq!(f.read("app/config"), ghostty);
    f.write(
        "app/config",
        r#"# ghostty
theme = dracula
keybind = ctrl+a=text:a
keybind = ctrl+b=text:b
"#,
    );
    f.ok_in("app", &["harvest"]);
    assert!(
        f.read(&format!("{repo}/base/config.base")).contains("theme = dracula"),
        "the shared theme reaches the base"
    );
    let out = f.ok_in("app", &["status"]);
    assert!(out.contains("clean"), "{out}");
}

#[test]
fn a_file_the_adapter_cannot_read_is_carried_whole() {
    // rime's `*.dict.yaml` is `.yaml` by name and tab separated in practice:
    // the adapter must not freeze it, it has to reach every machine anyway
    let f = Fixture::new("carried-whole");
    let dict = r#"---
name: demo
version: "1"
...

你	ni	1
"#;
    f.write("app/demo.dict.yaml", dict);
    f.ok(&["init"]);
    f.ok(&["add", "app"]);
    let repo = f.repo("app");
    f.write(&format!("{repo}/adopt.rules"), "demo.dict.yaml\n");
    f.ok_in("app", &["harvest"]);
    assert_eq!(
        f.read(&format!("{repo}/base/demo.dict.base.yaml")),
        dict,
        "adopted byte for byte"
    );

    // a fresh machine has no such file: render writes the bytes back
    std::fs::remove_file(f.root.join("app/demo.dict.yaml")).unwrap();
    let out = f.ok_in("app", &["render"]);
    assert!(out.contains("carried whole"), "{out}");
    assert_eq!(f.read("app/demo.dict.yaml"), dict);

    // and an edit on this machine goes back into the base whole
    f.write(
        "app/demo.dict.yaml",
        r#"---
name: demo
...

你	ni	2
"#,
    );
    let out = f.ok_in("app", &["harvest"]);
    assert!(out.contains("carried whole"), "{out}");
    assert!(f
        .read(&format!("{repo}/base/demo.dict.base.yaml"))
        .contains("\t2"));
}

#[test]
fn a_file_with_no_adapter_is_carried_whole_in_share() {
    let f = Fixture::new("share");
    let lua = r#"vim.opt.number = true
vim.g.mapleader = ' '
"#;
    f.write("app/lua/keymaps.lua", lua);
    f.ok(&["init"]);
    f.ok(&["add", "app"]);
    let repo = f.repo("app");
    f.write(&format!("{repo}/adopt.rules"), "lua/\n");
    let out = f.ok_in("app", &["harvest"]);
    assert!(out.contains("share/"), "{out}");
    assert_eq!(f.read(&format!("{repo}/share/lua/keymaps.lua")), lua);
    // no key splitting for it: nothing lands in base/
    assert!(!f.exists(&format!("{repo}/base/lua")));

    // verbatim in both directions, and the hash guard still applies
    f.write("app/lua/keymaps.lua", "vim.opt.number = false\n");
    assert_eq!(f.run_in("app", &["render"]).status.code(), Some(1));
    f.ok_in("app", &["harvest"]);
    assert_eq!(
        f.read(&format!("{repo}/share/lua/keymaps.lua")),
        "vim.opt.number = false\n"
    );
    std::fs::remove_file(f.root.join("app/lua/keymaps.lua")).unwrap();
    // the file is gone, so the record cannot match any more: that is what
    // --force is for
    let out = f.ok_in("app", &["render", "--force"]);
    assert!(out.contains("<= share/lua/keymaps.lua"), "{out}");
    assert_eq!(f.read("app/lua/keymaps.lua"), "vim.opt.number = false\n");
}

//! End-to-end tests: the binary is driven exactly like a user would.
//!
//! Every test builds a throwaway `central/` + `src/` pair under
//! `std::env::temp_dir()`, points `CFGSYNC_CENTRAL` at it and runs from inside
//! a source repo. Nothing outside the temp directory is touched, no network.

use std::path::PathBuf;
use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_cfgsync");

struct Fixture {
    root: PathBuf,
    /// where `run` / `ok` start, relative to the fixture root
    cwd: String,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("cfgsync-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Fixture {
            root,
            cwd: String::new(),
        }
    }

    fn central(&self) -> PathBuf {
        self.root.join("central")
    }

    /// the directory name the central repo uses for `rel`
    fn name_of(&self, rel: &str) -> String {
        let abs = std::fs::canonicalize(self.root.join(rel)).unwrap();
        abs.to_string_lossy().replace('/', "--")
    }

    /// `<central>/<name>` as a path relative to the fixture root
    fn repo(&self, rel: &str) -> String {
        format!("central/{}", self.name_of(rel))
    }

    fn write(&self, rel: &str, text: &str) -> &Self {
        let p = self.root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, text).unwrap();
        self
    }

    fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.root.join(rel)).unwrap_or_default()
    }

    fn exists(&self, rel: &str) -> bool {
        self.root.join(rel).exists()
    }

    fn run_in(&self, dir: &str, args: &[&str]) -> Output {
        self.run_env_in(dir, args, &[])
    }

    /// run from the fixture's default directory (the source repo, if any)
    fn run(&self, args: &[&str]) -> Output {
        self.run_in(&self.cwd.clone(), args)
    }

    fn run_env_in(&self, dir: &str, args: &[&str], env: &[(&str, &str)]) -> Output {
        let mut cmd = Command::new(BIN);
        cmd.args(args)
            .current_dir(self.root.join(dir))
            .env("CFGSYNC_CENTRAL", self.central())
            // never let a test fall through to the real interactive editor
            .env("EDITOR", "true")
            .env_remove("VISUAL")
            .env_remove("CFGSYNC_OS");
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd.output().unwrap()
    }

    fn ok_in(&self, dir: &str, args: &[&str]) -> String {
        let out = self.run_in(dir, args);
        assert!(
            out.status.success(),
            "(cd {dir}) {args:?} failed: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn ok(&self, args: &[&str]) -> String {
        let cwd = self.cwd.clone();
        self.ok_in(&cwd, args)
    }

    fn git(&self, dir: &str, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(self.root.join(dir))
            .args(args)
            .env("GIT_AUTHOR_NAME", "cfgsync test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "cfgsync test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// subject line of the newest commit in `dir`
    fn last_commit(&self, dir: &str) -> String {
        self.git(dir, &["log", "-1", "--pretty=%s"]).trim().to_string()
    }

    fn commit_count(&self, dir: &str) -> usize {
        self.git(dir, &["rev-list", "--count", "HEAD"])
            .trim()
            .parse()
            .unwrap_or(0)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// an initialized central repo plus one source repo (`app`) that is added and
/// carries two managed files and a routing table
fn workspace(name: &str) -> Fixture {
    let mut f = Fixture::new(name);
    f.write(
        "app/config.ini",
        "[core]\neditor = vim\ntheme = dark\n\n[limits]\ntimeout = 45\n",
    )
    .write(
        "app/settings.json",
        "{\n  \"theme\": \"dark\",\n  \"packages\": [\"npm:a\"],\n  \"tools\": [\"+codemode\"]\n}\n",
    );
    f.ok(&["init"]);
    f.ok(&["add", "app"]);
    f.write(
        &format!("{}/adopt.rules", f.repo("app")),
        "config.ini\nsettings.json\n",
    )
    .write(
        &format!("{}/sync.rules", f.repo("app")),
        "# machine owned\n[platform]\ncore.*\ntheme\ntools\n",
    );
    f.cwd = "app".to_string();
    f
}

// ---------------------------------------------------------------- init

#[test]
fn init_creates_the_central_repo_once() {
    let f = Fixture::new("init");
    let out = f.ok(&["init"]);
    assert!(out.contains("[init] central repo"), "{out}");
    assert!(f.git("central", &["rev-parse", "--git-dir"]).contains(".git"));
    assert_eq!(f.read("central/.gitignore"), ".hash\n.os\n");
    assert!(!f.read("central/.os").trim().is_empty());

    // second run is a no-op, not an error
    let again = f.ok(&["init"]);
    assert!(again.contains("central repo"), "{again}");
    assert_eq!(f.read("central/.gitignore"), ".hash\n.os\n", "idempotent");
}

#[test]
fn init_dry_run_writes_nothing() {
    let f = Fixture::new("init-dry");
    let out = f.ok(&["init", "--dry-run"]);
    assert!(out.contains("would"), "{out}");
    assert!(!f.exists("central/.git"));
}

// ---------------------------------------------------------------- add / rm

#[test]
fn add_registers_a_directory_and_git_inits_it() {
    let f = Fixture::new("add");
    f.write("app/config.ini", "[core]\neditor = vim\n");
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
    f.write("app/config.ini", "[core]\neditor = vim\n");
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
    f.write("app/config.ini", "[core]\neditor = vim\n");
    let out = f.run(&["add", "app"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("cfgsync init"));
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
fn rm_accepts_the_mangled_name_after_a_double_dash() {
    let f = workspace("rm-name");
    f.ok(&["harvest"]);
    let name = f.name_of("app");
    assert!(name.starts_with("--"), "{name}");
    let out = f.ok_in("", &["rm", "--", &name]);
    assert!(out.contains("removed from the central repo"), "{out}");
    assert!(!f.exists(&format!("central/{name}")));
}

// ---------------------------------------------------------------- harvest

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
fn harvest_adopts_and_routes_owned_keys_to_the_overlay() {
    let f = workspace("harvest");
    let os = f.read("central/.os").trim().to_string();
    let out = f.ok(&["harvest"]);
    assert!(out.contains("config.ini: adopted"), "{out}");
    let repo = f.repo("app");

    // the base is the file byte for byte, so comments and order survive
    assert_eq!(
        f.read(&format!("{repo}/base/config.base.ini")),
        "[core]\neditor = vim\ntheme = dark\n\n[limits]\ntimeout = 45\n"
    );
    let overlay = f.read(&format!("{repo}/platform/{os}/config.overlay.ini"));
    assert!(overlay.contains("editor = vim"), "{overlay}");
    assert!(overlay.contains("theme = dark"), "{overlay}");
    assert!(!overlay.contains("timeout"), "{overlay}");
    assert!(f.read(&format!("{repo}/.hash")).len() >= 16);
    assert_eq!(f.last_commit("app"), "cfgsync: harvest");
}

#[test]
fn harvest_pins_an_owned_key_that_still_equals_base() {
    let f = workspace("pin");
    f.ok(&["harvest"]);
    let os = f.read("central/.os").trim().to_string();
    let repo = f.repo("app");
    // editor is not in sync.rules yet, so it lives in the base only
    assert!(!f
        .read(&format!("{repo}/platform/{os}/config.overlay.ini"))
        .contains("timeout"));
    f.write(
        &format!("{repo}/sync.rules"),
        "core.*\nlimits.timeout\n",
    );
    f.ok(&["harvest"]);
    let overlay = f.read(&format!("{repo}/platform/{os}/config.overlay.ini"));
    assert!(
        overlay.contains("timeout = 45"),
        "adding the rule pins the machine value: {overlay}"
    );
}

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
fn harvest_is_idempotent() {
    let f = workspace("harvest-twice");
    f.ok(&["harvest"]);
    let before = f.read(&format!("{}/base/config.base.ini", f.repo("app")));
    let os = f.read("central/.os").trim().to_string();
    let overlay_before = f.read(&format!("{}/platform/{os}/config.overlay.ini", f.repo("app")));
    let commits = f.commit_count("app");
    let out = f.ok(&["harvest"]);
    assert!(out.contains("no changes"), "{out}");
    assert_eq!(f.read(&format!("{}/base/config.base.ini", f.repo("app"))), before);
    assert_eq!(
        f.read(&format!("{}/platform/{os}/config.overlay.ini", f.repo("app"))),
        overlay_before
    );
    assert_eq!(f.commit_count("app"), commits, "nothing new to commit");
}

#[test]
fn harvest_refuses_to_run_outside_a_managed_repo() {
    let f = workspace("harvest-outside");
    let out = f.run_in("", &["harvest"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("not inside a managed source repo"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn harvest_all_covers_every_source_repo() {
    let f = workspace("harvest-all");
    f.write("tool/config.ini", "[core]\neditor = vim\n");
    f.ok_in("", &["add", "tool"]);
    f.write(&format!("{}/adopt.rules", f.repo("tool")), "config.ini\n");
    let out = f.ok(&["harvest", "-a"]);
    assert!(out.matches("cfgsync: harvest").count() >= 1 || out.contains("harvest"), "{out}");
    assert!(f.exists(&format!("{}/base/config.base.ini", f.repo("app"))));
    assert!(f.exists(&format!("{}/base/config.base.ini", f.repo("tool"))));
}

#[test]
fn harvest_warns_about_config_nobody_claims() {
    let f = workspace("unmanaged");
    f.write("app/extra.json", "{}\n");
    let out = f.run(&["harvest"]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("extra.json"), "{err}");
    assert!(err.contains("adopt.rules"), "{err}");
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
fn an_extensionless_config_is_split_by_key() {
    let f = Fixture::new("plain");
    let ghostty = "# ghostty\ntheme = catppuccin-mocha\nkeybind = ctrl+a=text:a\nkeybind = ctrl+b=text:b\n";
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
        "# ghostty\ntheme = dracula\nkeybind = ctrl+a=text:a\nkeybind = ctrl+b=text:b\n",
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
fn a_file_with_no_adapter_is_carried_whole_in_share() {
    let f = Fixture::new("share");
    let lua = "vim.opt.number = true\nvim.g.mapleader = ' '\n";
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

// ---------------------------------------------------------------- render

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
    let out = f.run_in("app", &["render"]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("no harvest record"), "{err}");
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

// ---------------------------------------------------------------- status

#[test]
fn status_reports_changed_files_then_goes_clean() {
    let f = workspace("status");
    f.ok(&["harvest"]);
    f.ok(&["render"]);
    let clean = f.ok(&["status"]);
    assert!(clean.contains("[status] clean"), "{clean}");

    f.write("app/config.ini", "[core]\neditor = nano\ntheme = dark\n\n[limits]\ntimeout = 45\n");
    f.write("app/settings.json", "{\n  \"theme\": \"light\",\n  \"packages\": [\"npm:a\"],\n  \"tools\": [\"+codemode\"]\n}\n");
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
    f.write(&format!("{repo}/base/gone.base.ini"), "[core]\neditor = vim\n");
    f.write(&format!("{repo}/adopt.rules"), "config.ini\nsettings.json\ngone.ini\n");
    let out = f.ok(&["status"]);
    assert!(out.contains("+  gone.ini"), "{out}");
}

// ---------------------------------------------------------------- edit

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
    f.write("aaa/config.ini", "[core]\neditor = vim\n")
        .write("zzz/config.ini", "[core]\neditor = vim\n");
    f.ok(&["init"]);
    f.ok(&["add", "aaa"]);
    f.ok(&["add", "zzz"]);
    let out = f.ok_in("zzz", &["edit", "-a"]);
    assert!(
        out.contains(&format!("{}/adopt.rules", f.repo("zzz"))),
        "edit opened another repo's rules: {out}"
    );
}

#[test]
fn a_yaml_config_is_split_by_key() {
    let f = Fixture::new("yaml");
    let doc = "# app\nui:\n  theme: dark\n  padding: 2\n\ndb:\n  host: local\n";
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
    let doc = "# app\ntitle = \"app\"\n\n[window]\nsize = 2\nopacity = 1.0\n";
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
    let doc = "{\"model\": \"a\", \"temp\": 1}\n{\"model\": \"b\", \"temp\": 2}\n";
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
        "{\"model\": \"z\", \"temp\": 1}\n{\"model\": \"b\", \"temp\": 2}\n",
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

// ---------------------------------------------------------------- push / pull

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
    let repo_dir = std::fs::read_dir(&clone)
        .unwrap()
        .filter_map(|e| e.ok())
        .find(|e| e.file_name().to_string_lossy().starts_with("--"))
        .expect("the mangled repo directory is in the cloud copy");
    assert!(repo_dir.path().join("base/config.base.ini").is_file());
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
        "{\n  \"theme\": \"light\"\n}\n",
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

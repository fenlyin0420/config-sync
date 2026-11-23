//! The shared fixture: a throwaway `central/` + `src/` pair per test.
//!
//! Every test builds one under `std::env::temp_dir()`, points
//! `CFGSYNC_CENTRAL` at it and runs the real binary from inside a source repo.
//! Nothing outside the temp directory is touched, no network.

use std::path::PathBuf;
use std::process::{Command, Output};

pub const BIN: &str = env!("CARGO_BIN_EXE_cfgsync");

pub struct Fixture {
    pub root: PathBuf,
    /// where `run` / `ok` start, relative to the fixture root
    pub cwd: String,
}

impl Fixture {
    pub fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("cfgsync-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Fixture {
            root,
            cwd: String::new(),
        }
    }

    pub fn central(&self) -> PathBuf {
        self.root.join("central")
    }

    /// the directory name the central repo uses for `rel`: the source
    /// directory's own name, unless `add` was given another one
    pub fn name_of(&self, rel: &str) -> String {
        rel.rsplit('/').next().unwrap().to_string()
    }

    /// `<central>/<name>` as a path relative to the fixture root
    pub fn repo(&self, rel: &str) -> String {
        format!("central/{}", self.name_of(rel))
    }

    pub fn write(&self, rel: &str, text: &str) -> &Self {
        let p = self.root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, text).unwrap();
        self
    }

    pub fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.root.join(rel)).unwrap_or_default()
    }

    pub fn exists(&self, rel: &str) -> bool {
        self.root.join(rel).exists()
    }

    pub fn run_in(&self, dir: &str, args: &[&str]) -> Output {
        self.run_env_in(dir, args, &[])
    }

    /// run from the fixture's default directory (the source repo, if any)
    pub fn run(&self, args: &[&str]) -> Output {
        self.run_in(&self.cwd.clone(), args)
    }

    pub fn run_env_in(&self, dir: &str, args: &[&str], env: &[(&str, &str)]) -> Output {
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

    pub fn ok_in(&self, dir: &str, args: &[&str]) -> String {
        let out = self.run_in(dir, args);
        assert!(
            out.status.success(),
            "(cd {dir}) {args:?} failed: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    pub fn ok(&self, args: &[&str]) -> String {
        let cwd = self.cwd.clone();
        self.ok_in(&cwd, args)
    }

    pub fn git(&self, dir: &str, args: &[&str]) -> String {
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
    pub fn last_commit(&self, dir: &str) -> String {
        self.git(dir, &["log", "-1", "--pretty=%s"]).trim().to_string()
    }

    pub fn commit_count(&self, dir: &str) -> usize {
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

/// the absolute path of `rel`, forward slashes, as path.rules stores it
pub fn source_path(f: &Fixture, rel: &str) -> String {
    let abs = std::fs::canonicalize(f.root.join(rel)).unwrap();
    abs.to_string_lossy().replace('\\', "/")
}

/// an initialized central repo plus one source repo (`app`) that is added and
/// carries two managed files and a routing table
pub fn workspace(name: &str) -> Fixture {
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

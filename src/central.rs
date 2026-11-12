//! The central repo (`~/.local/share/cfgsync` on linux): one git repo holding a
//! directory per managed source repo, named after the source's absolute path
//! with `/` replaced by `--`:
//!
//!   /home/fenlyin/.config  ->  --home--fenlyin--.config
//!
//! The directory name *is* the mapping, so no registry file has to be kept in
//! step with it. Per source repo:
//!
//!   <central>/<name>/adopt.rules                   which files are managed
//!   <central>/<name>/sync.rules                    which keys are machine-owned
//!   <central>/<name>/base/<stem>.base.<ext>        shared baseline
//!   <central>/<name>/platform/<os>/<stem>.overlay.<ext>
//!                                                  this machine's delta
//!   <central>/<name>/.hash                         content hash, last harvest/render
//!
//! `.hash` and `.os` are the only machine-local files and both are gitignored:
//! a foreign hash would make every `render` refuse to write.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use crate::fsx::{read, write};

pub const ENV: &str = "CFGSYNC_CENTRAL";
pub const HASH_FILE: &str = ".hash";
pub const OS_FILE: &str = ".os";
const GITIGNORE: &str = ".gitignore";
const GITIGNORE_BLOCK: [&str; 2] = [HASH_FILE, OS_FILE];

/// One managed source repo: its directory inside the central repo, and the
/// directory on disk it stands for.
#[derive(Clone, Debug)]
pub struct Repo {
    pub name: String,
    pub source: PathBuf,
    pub dir: PathBuf,
}

impl Repo {
    pub fn new(root: &Path, name: &str) -> Repo {
        Repo {
            name: name.to_string(),
            source: source_of(name),
            dir: root.join(name),
        }
    }

    pub fn adopt_rules(&self) -> PathBuf {
        self.dir.join(crate::adopt::NAME)
    }

    pub fn sync_rules(&self) -> PathBuf {
        self.dir.join(crate::rules::NAME)
    }
}

// ---------------------------------------------------------------- locations

pub fn default_root() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = home() {
            return home.join("Library").join("Application Support").join("cfgsync");
        }
    }
    #[cfg(windows)]
    {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            if !appdata.is_empty() {
                return PathBuf::from(appdata).join("cfgsync");
            }
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
            if !xdg.is_empty() {
                return PathBuf::from(xdg).join("cfgsync");
            }
        }
        if let Some(home) = home() {
            return home.join(".local").join("share").join("cfgsync");
        }
    }
    PathBuf::from("cfgsync")
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// `--central DIR` > `$CFGSYNC_CENTRAL` > the platform data directory
pub fn resolve(explicit: Option<&Path>) -> PathBuf {
    if let Some(p) = explicit {
        return absolute(p);
    }
    if let Some(v) = std::env::var_os(ENV) {
        if !v.is_empty() {
            return absolute(Path::new(&v));
        }
    }
    default_root()
}

fn absolute(p: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|c| c.join(p))
            .unwrap_or_else(|_| p.to_path_buf())
    }
}

pub fn canonical(p: &Path) -> Option<PathBuf> {
    let abs = absolute(p);
    abs.canonicalize().ok().filter(|p| p.is_dir())
}

// ---------------------------------------------------------------- naming

/// An absolute path with every `/` turned into `--`: the directory name that
/// stands for this source repo inside the central repo.
pub fn name_for(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let text = text.trim_end_matches('/');
    let text = if text.is_empty() { "/" } else { text };
    text.replace('/', "--")
}

/// The inverse, for display only -- `--` never round-trips a path that already
/// contains one, and nothing here depends on it doing so.
pub fn source_of(name: &str) -> PathBuf {
    PathBuf::from(name.replace("--", "/"))
}

// ---------------------------------------------------------------- lookup

/// The deepest ancestor of `start` (inclusive) that is a managed source repo.
pub fn locate(root: &Path, start: &Path) -> Option<Repo> {
    let mut cur = Some(canonical(start).unwrap_or_else(|| absolute(start)));
    while let Some(dir) = cur {
        let name = name_for(&dir);
        if root.join(&name).is_dir() {
            return Some(Repo::new(root, &name));
        }
        cur = dir.parent().map(Path::to_path_buf);
    }
    None
}

/// Every source repo the central repo knows about, sorted by name.
pub fn all(root: &Path) -> Vec<Repo> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.'))
        .collect();
    names.sort();
    names.iter().map(|n| Repo::new(root, n)).collect()
}

// ---------------------------------------------------------------- state

pub fn read_hash(dir: &Path) -> Option<u64> {
    let text = std::fs::read_to_string(dir.join(HASH_FILE)).ok()?;
    u64::from_str_radix(text.trim(), 16).ok()
}

pub fn write_hash(dir: &Path, hash: u64) {
    write(&dir.join(HASH_FILE), &format!("{hash:016x}\n"));
}

pub fn read_os(root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(root.join(OS_FILE)).ok()?;
    match text.trim() {
        "" => None,
        v => Some(normalize_os(v)),
    }
}

pub fn write_os(root: &Path, os: &str) {
    write(&root.join(OS_FILE), &format!("{os}\n"));
}

// ---------------------------------------------------------------- platform

pub fn normalize_os(name: &str) -> String {
    match name.trim().to_ascii_lowercase().as_str() {
        "win" | "windows" | "win32" | "win64" | "msys" | "mingw" | "cygwin" => "win",
        "linux" => "linux",
        "macos" | "darwin" | "osx" => "macos",
        other => other,
    }
    .to_string()
}

pub fn host_os() -> String {
    match std::env::consts::OS {
        "windows" => "win",
        "macos" => "macos",
        os => os,
    }
    .to_string()
}

/// `--os` > `$CFGSYNC_OS` > `<central>/.os` (written by `init`) > the host.
/// An unknown name is used verbatim: a typo means "base only", never a guess at
/// another machine's settings.
pub fn resolve_os(root: &Path, cli: Option<&str>) -> String {
    if let Some(v) = cli.filter(|v| !v.trim().is_empty()) {
        return normalize_os(v);
    }
    if let Ok(v) = std::env::var("CFGSYNC_OS") {
        if !v.trim().is_empty() {
            return normalize_os(&v);
        }
    }
    if let Some(v) = read_os(root) {
        return v;
    }
    host_os()
}

// ---------------------------------------------------------------- git

pub fn git(dir: &Path, args: &[&str]) -> Option<Output> {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()
}

pub fn git_ok(dir: &Path, args: &[&str]) -> bool {
    git(dir, args).map(|o| o.status.success()).unwrap_or(false)
}

pub fn git_out(dir: &Path, args: &[&str]) -> String {
    git(dir, args)
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

pub fn git_err(out: &Output) -> String {
    let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let line = if err.is_empty() { text } else { err };
    line.lines().last().unwrap_or("").to_string()
}

/// unmerged paths in the work tree -- the only honest "needs a human" signal
pub fn has_conflict(dir: &Path) -> bool {
    git_out(dir, &["status", "--porcelain"])
        .lines()
        .any(|line| {
            let code: String = line.chars().take(2).collect();
            code.contains('U') || code == "AA" || code == "DD"
        })
}

pub fn is_repo(dir: &Path) -> bool {
    git_ok(dir, &["rev-parse", "--git-dir"])
}

pub fn dirty_count(dir: &Path) -> usize {
    git_out(dir, &["status", "--porcelain"])
        .lines()
        .filter(|l| !l.trim().is_empty())
        .count()
}

/// `git add -A && git commit`; `Ok(false)` when there was nothing to commit.
pub fn commit_all(dir: &Path, message: &str) -> Result<bool, String> {
    if dirty_count(dir) == 0 {
        return Ok(false);
    }
    match git(dir, &["add", "-A"]) {
        Some(o) if o.status.success() => {}
        Some(o) => return Err(git_err(&o)),
        None => return Err("git is not on PATH".to_string()),
    }
    match git(dir, &["commit", "-m", message]) {
        Some(o) if o.status.success() => Ok(true),
        Some(o) => Err(git_err(&o)),
        None => return Err("git is not on PATH".to_string()),
    }
}

/// UTC stamp without a time crate: `2026-11-09T21:58:00Z`
pub fn utc_stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Howard Hinnant's days -> civil, valid for any modern date
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

// ---------------------------------------------------------------- commands

pub fn init(root: &Path, remote: Option<&str>, dry: bool) -> i32 {
    let created = !root.is_dir();
    println!(
        "[init] central repo {}{}",
        root.display(),
        if created { "  (created)" } else { "" }
    );
    if dry {
        println!("[init] would create base dirs, .gitignore, .os and git init");
        if let Some(url) = remote {
            println!("[init] would set origin -> {url}");
        }
        return 0;
    }
    if let Err(e) = std::fs::create_dir_all(root) {
        eprintln!("[init] cannot create {}: {e}", root.display());
        return 2;
    }
    for line in ensure_gitignore(root) {
        println!("[init] {GITIGNORE}: gitignored {line}");
    }
    if !is_repo(root) {
        let out = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["init", "-q"])
            .output();
        match out {
            Ok(o) if o.status.success() => println!("[init] git init"),
            Ok(o) => {
                eprintln!("[init] git init failed: {}", git_err(&o));
                return 1;
            }
            Err(e) => {
                eprintln!("[init] cannot run git: {e}");
                return 1;
            }
        }
    }
    let os = host_os();
    write_os(root, &os);
    println!("[init] platform {os} recorded in {OS_FILE}");
    if let Some(url) = remote {
        let verb = if git_ok(root, &["remote", "get-url", "origin"]) {
            "set-url"
        } else {
            "add"
        };
        match git(root, &["remote", verb, "origin", url]) {
            Some(o) if o.status.success() => println!("[init] origin -> {url}"),
            Some(o) => {
                eprintln!("[init] git remote {verb} failed: {}", git_err(&o));
                return 1;
            }
            None => return 1,
        }
    } else if !git_ok(root, &["remote", "get-url", "origin"]) {
        println!("[init] no origin yet -- `cfgsync init --remote <url>` adds one");
    }
    if !git_ok(root, &["rev-parse", "--verify", "--quiet", "HEAD"])
        && git_ok(root, &["remote", "get-url", "origin"])
    {
        println!("[init] this machine has no commits yet -- `cfgsync pull` brings the cloud copy");
        return 0;
    }
    println!("[init] next: cfgsync add DIR");
    0
}

/// Make sure `.hash` and `.os` are gitignored; returns the lines it added.
fn ensure_gitignore(root: &Path) -> Vec<&'static str> {
    let ignore = root.join(GITIGNORE);
    let mut text = read(&ignore);
    let mut added = Vec::new();
    for line in GITIGNORE_BLOCK {
        if !text.lines().any(|l| l.trim() == line) {
            text.push_str(line);
            text.push('\n');
            added.push(line);
        }
    }
    if !added.is_empty() {
        write(&ignore, &text);
    }
    added
}

pub fn remove(root: &Path, target: &str, dry: bool) -> i32 {
    let name = match root.join(target) {
        p if target != "." && target != ".." && p.is_dir() && !target.contains('/') => {
            target.to_string()
        }
        _ => match canonical(Path::new(target)) {
            Some(abs) => name_for(&abs),
            None => {
                eprintln!("[rm] no such directory: {target}");
                return 2;
            }
        },
    };
    if name.starts_with('.') || name == name_for(root) {
        eprintln!("[rm] refusing to remove {name} from the central repo");
        return 2;
    }
    let dir = root.join(&name);
    if !dir.is_dir() {
        eprintln!(
            "[rm] not managed: {target} (no {} in the central repo)",
            name
        );
        return 2;
    }
    println!(
        "[rm] {} ({}): removed from the central repo",
        source_of(&name).display(),
        name
    );
    println!("[rm] the directory itself and its git history are untouched");
    if dry {
        return 0;
    }
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => {
            println!("[rm] next: cfgsync push");
            0
        }
        Err(e) => {
            eprintln!("[rm] cannot remove {}: {e}", dir.display());
            2
        }
    }
}

pub fn push(root: &Path, message: Option<&str>, dry: bool) -> i32 {
    if !root.is_dir() {
        eprintln!("[push] no central repo at {} -- run `cfgsync init`", root.display());
        return 2;
    }
    if !is_repo(root) {
        eprintln!("[push] {} is not a git repo -- run `cfgsync init`", root.display());
        return 2;
    }
    let message = match message {
        Some(m) => m.to_string(),
        None => format!("chore: sync {}", utc_stamp()),
    };
    let mut committed = false;
    let dirty = dirty_count(root);
    if dirty > 0 {
        if dry {
            println!("[push] would commit {dirty} change(s): {message}");
        } else {
            match commit_all(root, &message) {
                Ok(committed_now) => {
                    committed = committed_now;
                    if committed {
                        println!("[push] committed {dirty} change(s): {message}");
                    }
                }
                Err(e) => {
                    eprintln!("[push] commit failed: {e}");
                    return 1;
                }
            }
        }
    }
    let upstream = git_ok(root, &["rev-parse", "--abbrev-ref", "@{u}"]);
    if !git_ok(root, &["remote", "get-url", "origin"]) {
        eprintln!(
            "[sync] warning: no `origin` remote -- `cfgsync init --remote <url>` adds one, \
             work is committed locally"
        );
        return if committed { 0 } else { 1 };
    }
    if dry {
        println!(
            "[push] would push{}",
            if upstream { "" } else { " (-u origin HEAD)" }
        );
        return 0;
    }
    let out = if upstream {
        git(root, &["push"])
    } else {
        git(root, &["push", "-u", "origin", "HEAD"])
    };
    match out {
        Some(o) if o.status.success() => {
            println!("[push] pushed");
            0
        }
        Some(o) => {
            eprintln!("[push] FAILED: {}", git_err(&o));
            1
        }
        None => {
            eprintln!("[push] git is not on PATH");
            1
        }
    }
}

/// a central repo that has no commit yet: the state `init` leaves a fresh machine
/// in, where the cloud copy is the only history there is
fn unborn(root: &Path) -> bool {
    !git_ok(root, &["rev-parse", "--verify", "--quiet", "HEAD"])
}

fn has_origin(root: &Path) -> bool {
    git_ok(root, &["remote", "get-url", "origin"])
}

/// `origin/main` from the remote's HEAD, or the only branch it has
fn remote_branch(root: &Path) -> Result<String, String> {
    let _ = git(root, &["remote", "set-head", "origin", "--auto"]);
    let head = git_out(root, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"]);
    if head.starts_with("origin/") {
        return Ok(head);
    }
    let refs: Vec<String> = git_out(
        root,
        &["for-each-ref", "--format=%(refname:short)", "refs/remotes/origin"],
    )
    .lines()
    .map(str::to_string)
    .filter(|l| l.starts_with("origin/") && !l.ends_with("/HEAD"))
    .collect();
    match refs.len() {
        1 => Ok(refs[0].clone()),
        0 => Err("origin has no branches yet".to_string()),
        _ => Err(format!(
            "origin has several branches ({}), cannot tell which one to follow",
            refs.join(", ")
        )),
    }
}

/// `.gitignore` is ours, not the user's: drop it before checking out a cloud
/// copy that carries its own. Anything else in the file and it stays put.
fn drop_generated_gitignore(root: &Path) {
    let path = root.join(GITIGNORE);
    let text = read(&path);
    if text
        .lines()
        .all(|l| l.trim().is_empty() || GITIGNORE_BLOCK.contains(&l.trim()))
    {
        let _ = std::fs::remove_file(&path);
    }
}

pub fn pull(root: &Path, dry: bool) -> i32 {
    if !root.is_dir() {
        eprintln!("[pull] no central repo at {} -- run `cfgsync init`", root.display());
        return 2;
    }
    if !is_repo(root) {
        eprintln!("[pull] {} is not a git repo -- run `cfgsync init`", root.display());
        return 2;
    }
    if unborn(root) {
        if !has_origin(root) {
            println!("[pull] no origin remote and no commits yet, nothing to pull");
            return 0;
        }
        if dry {
            println!("[pull] would fetch origin and check out its branch");
            return 0;
        }
        match git(root, &["fetch", "origin"]) {
            Some(o) if o.status.success() => {}
            Some(o) => {
                eprintln!("[pull] fetch failed: {}", git_err(&o));
                return 1;
            }
            None => {
                eprintln!("[pull] git is not on PATH");
                return 1;
            }
        }
        let remote = match remote_branch(root) {
            Ok(name) => name,
            Err(e) => {
                eprintln!("[pull] {e}");
                return 1;
            }
        };
        let branch = remote.trim_start_matches("origin/").to_string();
        drop_generated_gitignore(root);
        match git(root, &["checkout", "-q", "-B", &branch, "--track", &remote]) {
            Some(o) if o.status.success() => {}
            Some(o) => {
                eprintln!("[pull] cannot check out {remote}: {}", git_err(&o));
                return 1;
            }
            None => return 1,
        }
        ensure_gitignore(root);
        println!("[pull] checked out {remote} -- the cloud copy is here now");
        println!("[pull] next: cfgsync add DIR, then cfgsync render");
        return 0;
    }
    let dirty = dirty_count(root);
    if dirty > 0 {
        let message = format!("chore: sync {}", utc_stamp());
        if dry {
            println!("[pull] would commit {dirty} change(s) first: {message}");
        } else {
            match commit_all(root, &message) {
                Ok(true) => println!("[pull] committed {dirty} local change(s) first"),
                Ok(false) => {}
                Err(e) => {
                    eprintln!("[pull] commit failed: {e}");
                    return 1;
                }
            }
        }
    }
    if !git_ok(root, &["rev-parse", "--abbrev-ref", "@{u}"]) {
        if !has_origin(root) {
            println!("[pull] no upstream, nothing to pull");
            return 0;
        }
        if dry {
            println!("[pull] would fetch origin and follow its branch");
            return 0;
        }
        match git(root, &["fetch", "origin"]) {
            Some(o) if o.status.success() => {}
            Some(o) => {
                eprintln!("[pull] fetch failed: {}", git_err(&o));
                return 1;
            }
            None => return 1,
        }
        let remote = match remote_branch(root) {
            Ok(name) => name,
            Err(e) => {
                eprintln!("[pull] {e}");
                return 1;
            }
        };
        match git(root, &["branch", &format!("--set-upstream-to={remote}")]) {
            Some(o) if o.status.success() => println!("[pull] following {remote}"),
            Some(o) => {
                eprintln!("[pull] cannot follow {remote}: {}", git_err(&o));
                return 1;
            }
            None => return 1,
        }
    }
    if dry {
        println!("[pull] would pull --rebase --autostash");
        return 0;
    }
    let before = git_out(root, &["rev-parse", "HEAD"]);
    let out = git(root, &["pull", "--rebase", "--autostash"]);
    if has_conflict(root) {
        eprintln!(
            "[pull] conflict: resolve it by hand, then `git -C {} rebase --continue`",
            root.display()
        );
        return 1;
    }
    match out {
        Some(o) if o.status.success() => {
            let after = git_out(root, &["rev-parse", "HEAD"]);
            println!(
                "[pull] {}",
                if before == after { "up to date" } else { "updated" }
            );
            println!("[pull] next: cfgsync render to write the new base into a source repo");
            0
        }
        Some(o) => {
            eprintln!("[pull] FAILED: {}", git_err(&o));
            1
        }
        None => {
            eprintln!("[pull] git is not on PATH");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_names_round_trip() {
        let p = Path::new("/home/fenlyin/.config");
        assert_eq!(name_for(p), "--home--fenlyin--.config");
        assert_eq!(source_of(&name_for(p)), p.to_path_buf());
    }

    #[test]
    fn a_name_is_never_a_dot_entry() {
        assert!(!name_for(Path::new("/home/x")).starts_with('.'));
    }

    #[test]
    fn os_names_are_normalized_once() {
        for raw in ["windows", "WIN", "win32", "msys", "cygwin"] {
            assert_eq!(normalize_os(raw), "win");
        }
        assert_eq!(normalize_os("darwin"), "macos");
        assert_eq!(normalize_os(" freebsd "), "freebsd");
    }

    #[test]
    fn utc_stamp_shape() {
        let s = utc_stamp();
        assert_eq!(s.len(), 20, "{s}");
        assert!(s.ends_with('Z') && s.contains('T'), "{s}");
    }

    #[test]
    fn civil_roundtrip() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_000), (2024, 10, 4));
    }
}

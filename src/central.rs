//! The central repo (`~/.local/share/cfgsync` on linux): one git repo holding a
//! directory per managed source repo. The directory name is a label the user
//! picks (`nvim`, `ghostty`), because the same app does not live at the same
//! path everywhere -- `~/.config/nvim` on linux, `%LOCALAPPDATA%\nvim` on
//! windows -- and a path mangled into a name cannot express that. Per source
//! repo:
//!
//!   <central>/<name>/path.rules                    where it lives per platform
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
use crate::paths;
use crate::ui;

pub const ENV: &str = "CFGSYNC_CENTRAL";
pub const HASH_FILE: &str = ".hash";
pub const OS_FILE: &str = ".os";
const GITIGNORE: &str = ".gitignore";
const GITIGNORE_BLOCK: [&str; 2] = [HASH_FILE, OS_FILE];

/// One managed source repo: its directory inside the central repo, and the
/// directory on disk it stands for on the platform in use.
#[derive(Clone, Debug)]
pub struct Repo {
    pub name: String,
    pub source: PathBuf,
    pub dir: PathBuf,
}

impl Repo {
    pub fn new(root: &Path, name: &str, source: PathBuf) -> Repo {
        Repo {
            name: name.to_string(),
            source,
            dir: root.join(name),
        }
    }

    pub fn path_rules(&self) -> PathBuf {
        self.dir.join(paths::NAME)
    }

    pub fn adopt_rules(&self) -> PathBuf {
        self.dir.join(crate::adopt::NAME)
    }

    pub fn sync_rules(&self) -> PathBuf {
        self.dir.join(crate::rules::NAME)
    }
}

/// A directory of the central repo together with the source path `path.rules`
/// gives for the platform in use. `source` is `None` when this platform has no
/// line yet -- the repo is known but cannot be reached from here.
#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub dir: PathBuf,
    pub source: Option<PathBuf>,
}

impl Entry {
    /// `Some(dir)` when this platform has a path, with no promise it exists.
    pub fn source(&self) -> Option<&Path> {
        self.source.as_deref()
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
    abs.canonicalize()
        .ok()
        .filter(|p| p.is_dir())
        .map(unverbatim)
}

/// Windows `canonicalize` answers `\\?\C:\...`, which no other tool, no
/// `path.rules` line and no comparison here wants to see.
fn unverbatim(p: PathBuf) -> PathBuf {
    let text = p.to_string_lossy().into_owned();
    match text.strip_prefix(r"\\?\UNC\") {
        Some(rest) => PathBuf::from(format!(r"\\{rest}")),
        None => match text.strip_prefix(r"\\?\") {
            Some(rest) => PathBuf::from(rest),
            None => p,
        },
    }
}

// ---------------------------------------------------------------- naming

/// The default directory name for a source repo: the directory's own name,
/// cleaned up enough to be typed. `~/.claude` -> `claude`,
/// `Application Support` -> `Application-Support`.
pub fn suggest_name(source: &Path) -> String {
    let raw = source
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let cleaned: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = cleaned
        .trim_start_matches(['.', '-', '_'])
        .trim_end_matches('-')
        .to_string();
    if trimmed.is_empty() {
        "repo".to_string()
    } else {
        trimmed
    }
}

/// A name `add` accepts: one directory name, never a dot entry, never a path.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.starts_with('.')
        && !name.contains(['/', '\\', ' ', '\t'])
}

/// The directory `add` may use for `source` on this platform.
///
/// `wanted` wins when it is free, or when it exists but has no line for this
/// platform -- that case is how a second machine declares where the same app
/// lives on its side. A name that already stands for another directory here is
/// taken, so the next candidate is `wanted-2`, `wanted-3`, ...
pub fn claim(root: &Path, source: &Path, os: &str, wanted: &str) -> Result<String, String> {
    for n in 1..=50 {
        let name = if n == 1 {
            wanted.to_string()
        } else {
            format!("{wanted}-{n}")
        };
        let dir = root.join(&name);
        if !dir.exists() {
            return Ok(name);
        }
        if !dir.is_dir() {
            return Err(format!("{} exists and is not a directory", dir.display()));
        }
        match paths::path_for(&dir, os) {
            Some(path) if same_dir(&path, source) => return Ok(name),
            Some(_) => continue,
            None => return Ok(name),
        }
    }
    Err(format!("no free name left for {wanted}"))
}

/// Two paths name the same directory, whatever their spelling.
pub fn same_dir(a: &Path, b: &Path) -> bool {
    match (canonical(a), canonical(b)) {
        (Some(a), Some(b)) => a == b,
        _ => absolute(a) == absolute(b),
    }
}

// ---------------------------------------------------------------- lookup

/// The central directory `name`, with this platform's path filled in.
pub fn entry(root: &Path, name: &str, os: &str) -> Entry {
    let dir = root.join(name);
    Entry {
        name: name.to_string(),
        source: paths::path_for(&dir, os),
        dir,
    }
}

/// Every source repo the central repo knows about, sorted by name. A directory
/// without `path.rules` is not one (old layout, or junk).
pub fn entries(root: &Path, os: &str) -> Vec<Entry> {
    let Ok(read) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut names: Vec<String> = read
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.'))
        .filter(|n| root.join(n).join(paths::NAME).is_file())
        .collect();
    names.sort();
    names.iter().map(|n| entry(root, n, os)).collect()
}

/// Directories in the central repo that carry no `path.rules` -- the layout
/// before names were labels, or junk. They are not source repos any more, but
/// they are still committed history, so they are reported and never deleted.
pub fn unconfigured(root: &Path) -> Vec<String> {
    let Ok(read) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut names: Vec<String> = read
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.'))
        .filter(|n| !root.join(n).join(paths::NAME).is_file())
        .collect();
    names.sort();
    names
}

/// The deepest managed source repo that contains `start` (inclusive), looking
/// only at the path this platform has recorded.
pub fn entry_for_path(root: &Path, start: &Path, os: &str) -> Option<Entry> {
    let start = canonical(start).unwrap_or_else(|| absolute(start));
    let mut best: Option<(usize, Entry)> = None;
    for e in entries(root, os) {
        let Some(src) = e.source.clone() else {
            continue;
        };
        let src = canonical(&src).unwrap_or(src);
        if start != src && !start.starts_with(&src) {
            continue;
        }
        let depth = src.components().count();
        if best.as_ref().map(|(d, _)| depth > *d).unwrap_or(true) {
            best = Some((depth, e));
        }
    }
    best.map(|(_, e)| e)
}

/// A directory in the central repo by name, whether or not it has a path here.
pub fn by_name(root: &Path, name: &str) -> Option<Entry> {
    if !valid_name(name) || !root.join(name).is_dir() {
        return None;
    }
    Some(entry(root, name, ""))
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
        None => Err("git is not on PATH".to_string()),
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
    say!(
        "init",
        "central repo {}{}",
        root.display(),
        match created {
            true => ui::ok("  (created)"),
            false => String::new(),
        }
    );
    if dry {
        say!("init", "{}", ui::warn("would create base dirs, .gitignore, .os and git init"));
        if let Some(url) = remote {
            say!("init", "would set origin -> {url}");
        }
        return 0;
    }
    if let Err(e) = std::fs::create_dir_all(root) {
        refuse!("init", "cannot create {}: {e}", root.display());
        return 2;
    }
    for line in ensure_gitignore(root) {
        say!("init", "{}: gitignored {line}", ui::dim(GITIGNORE));
    }
    if !is_repo(root) {
        let out = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["init", "-q"])
            .output();
        match out {
            Ok(o) if o.status.success() => say!("init", "{}", ui::ok("git init")),
            Ok(o) => {
                refuse!("init", "git init failed: {}", git_err(&o));
                return 1;
            }
            Err(e) => {
                refuse!("init", "cannot run git: {e}");
                return 1;
            }
        }
    }
    let os = host_os();
    write_os(root, &os);
    say!("init", "platform {} recorded in {}", ui::bold(&os), ui::dim(OS_FILE));
    say!(
        "init",
        "{}",
        ui::dim(&format!(
            "each source repo records its own per-platform path in {}",
            paths::NAME
        ))
    );
    if let Some(url) = remote {
        let verb = if git_ok(root, &["remote", "get-url", "origin"]) {
            "set-url"
        } else {
            "add"
        };
        match git(root, &["remote", verb, "origin", url]) {
            Some(o) if o.status.success() => say!("init", "origin -> {url}"),
            Some(o) => {
                refuse!("init", "git remote {verb} failed: {}", git_err(&o));
                return 1;
            }
            None => return 1,
        }
    } else if !git_ok(root, &["remote", "get-url", "origin"]) {
        say!("init", "{}", ui::warn("no origin yet -- `cfgsync init --remote <url>` adds one"));
    }
    if !git_ok(root, &["rev-parse", "--verify", "--quiet", "HEAD"])
        && git_ok(root, &["remote", "get-url", "origin"])
    {
        say!("init", "this machine has no commits yet -- `cfgsync pull` brings the cloud copy");
        return 0;
    }
    say!("init", "{}", ui::dim("next: cfgsync add DIR"));
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

pub fn remove(root: &Path, target: &str, os: &str, dry: bool) -> i32 {
    let entry = match by_name(root, target) {
        Some(e) => entry(root, &e.name, os),
        None => {
            let Some(abs) = canonical(Path::new(target)) else {
                refuse!("rm", "no such directory: {target}");
                return 2;
            };
            match entry_for_path(root, &abs, os) {
                Some(e) => e,
                None => {
                    refuse!("rm", "not managed: {target} (no source repo in {} has that path)", root.display());
                    return 2;
                }
            }
        }
    };
    let name = entry.name.clone();
    let dir = entry.dir.clone();
    let shown = match entry.source() {
        Some(path) => path.display().to_string(),
        None => format!("{name} (no path for {os})"),
    };
    say!("rm", "{shown} ({name}): {}", ui::ok("removed from the central repo"));
    say!("rm", "{}", ui::dim("the directory itself and its git history are untouched"));
    if dry {
        return 0;
    }
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => {
            say!("rm", "{}", ui::dim("next: cfgsync push"));
            0
        }
        Err(e) => {
            refuse!("rm", "cannot remove {}: {e}", dir.display());
            2
        }
    }
}

pub fn push(root: &Path, message: Option<&str>, dry: bool) -> i32 {
    if !root.is_dir() {
        refuse!("push", "no central repo at {} -- run `cfgsync init`", root.display());
        return 2;
    }
    if !is_repo(root) {
        refuse!("push", "{} is not a git repo -- run `cfgsync init`", root.display());
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
            say!("push", "{}", ui::warn(&format!("would commit {dirty} change(s): {message}")));
        } else {
            match commit_all(root, &message) {
                Ok(committed_now) => {
                    committed = committed_now;
                    if committed {
                        say!("push", "committed {dirty} change(s): {message}");
                    }
                }
                Err(e) => {
                    refuse!("push", "commit failed: {e}");
                    return 1;
                }
            }
        }
    }
    let upstream = git_ok(root, &["rev-parse", "--abbrev-ref", "@{u}"]);
    if !git_ok(root, &["remote", "get-url", "origin"]) {
        warn!("no `origin` remote -- `cfgsync init --remote <url>` adds one, work is committed locally");
        return if committed { 0 } else { 1 };
    }
    if dry {
        say!(
            "push",
            "{}",
            ui::warn(&format!(
                "would push{}",
                if upstream { "" } else { " (-u origin HEAD)" }
            ))
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
            say!("push", "{}", ui::ok("pushed"));
            0
        }
        Some(o) => {
            refuse!("push", "FAILED: {}", git_err(&o));
            1
        }
        None => {
            refuse!("push", "git is not on PATH");
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
        refuse!("pull", "no central repo at {} -- run `cfgsync init`", root.display());
        return 2;
    }
    if !is_repo(root) {
        refuse!("pull", "{} is not a git repo -- run `cfgsync init`", root.display());
        return 2;
    }
    if unborn(root) {
        if !has_origin(root) {
            say!("pull", "{}", ui::ok("no origin remote and no commits yet, nothing to pull"));
            return 0;
        }
        if dry {
            say!("pull", "{}", ui::warn("would fetch origin and check out its branch"));
            return 0;
        }
        match git(root, &["fetch", "origin"]) {
            Some(o) if o.status.success() => {}
            Some(o) => {
                refuse!("pull", "fetch failed: {}", git_err(&o));
                return 1;
            }
            None => {
                refuse!("pull", "git is not on PATH");
                return 1;
            }
        }
        let remote = match remote_branch(root) {
            Ok(name) => name,
            Err(e) => {
                refuse!("pull", "{e}");
                return 1;
            }
        };
        let branch = remote.trim_start_matches("origin/").to_string();
        drop_generated_gitignore(root);
        match git(root, &["checkout", "-q", "-B", &branch, "--track", &remote]) {
            Some(o) if o.status.success() => {}
            Some(o) => {
                refuse!("pull", "cannot check out {remote}: {}", git_err(&o));
                return 1;
            }
            None => return 1,
        }
        ensure_gitignore(root);
        say!("pull", "checked out {remote} -- {}", ui::ok("the cloud copy is here now"));
        say!("pull", "{}", ui::dim("next: cfgsync add DIR, then cfgsync render"));
        return 0;
    }
    let dirty = dirty_count(root);
    if dirty > 0 {
        let message = format!("chore: sync {}", utc_stamp());
        if dry {
            say!("pull", "{}", ui::warn(&format!("would commit {dirty} change(s) first: {message}")));
        } else {
            match commit_all(root, &message) {
                Ok(true) => say!("pull", "committed {dirty} local change(s) first"),
                Ok(false) => {}
                Err(e) => {
                    refuse!("pull", "commit failed: {e}");
                    return 1;
                }
            }
        }
    }
    if !git_ok(root, &["rev-parse", "--abbrev-ref", "@{u}"]) {
        if !has_origin(root) {
            say!("pull", "{}", ui::ok("no upstream, nothing to pull"));
            return 0;
        }
        if dry {
            say!("pull", "{}", ui::warn("would fetch origin and follow its branch"));
            return 0;
        }
        match git(root, &["fetch", "origin"]) {
            Some(o) if o.status.success() => {}
            Some(o) => {
                refuse!("pull", "fetch failed: {}", git_err(&o));
                return 1;
            }
            None => return 1,
        }
        let remote = match remote_branch(root) {
            Ok(name) => name,
            Err(e) => {
                refuse!("pull", "{e}");
                return 1;
            }
        };
        match git(root, &["branch", &format!("--set-upstream-to={remote}")]) {
            Some(o) if o.status.success() => say!("pull", "following {remote}"),
            Some(o) => {
                refuse!("pull", "cannot follow {remote}: {}", git_err(&o));
                return 1;
            }
            None => return 1,
        }
    }
    if dry {
        say!("pull", "{}", ui::warn("would pull --rebase --autostash"));
        return 0;
    }
    let before = git_out(root, &["rev-parse", "HEAD"]);
    let out = git(root, &["pull", "--rebase", "--autostash"]);
    if has_conflict(root) {
        refuse!(
            "pull",
            "conflict: resolve it by hand, then `git -C {} rebase --continue`",
            root.display()
        );
        return 1;
    }
    match out {
        Some(o) if o.status.success() => {
            let after = git_out(root, &["rev-parse", "HEAD"]);
            say!(
                "pull",
                "{}",
                match before == after {
                    true => ui::ok("up to date"),
                    false => ui::ok("updated"),
                }
            );
            say!("pull", "{}", ui::dim("next: cfgsync render to write the new base into a source repo"));
            0
        }
        Some(o) => {
            refuse!("pull", "FAILED: {}", git_err(&o));
            1
        }
        None => {
            refuse!("pull", "git is not on PATH");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_suggested_name_is_the_directory_name_cleaned_up() {
        assert_eq!(suggest_name(Path::new("/home/x/.config/nvim")), "nvim");
        assert_eq!(suggest_name(Path::new("/home/x/.claude")), "claude");
        assert_eq!(suggest_name(Path::new("/home/x/Application Support")), "Application-Support");
        assert_eq!(suggest_name(Path::new("/")), "repo");
    }

    #[test]
    fn a_name_is_never_a_dot_entry_or_a_path() {
        assert!(!suggest_name(Path::new("/home/x/.hidden")).starts_with('.'));
        for bad in ["", ".", "..", ".git", "a/b", "a\\b", "a b"] {
            assert!(!valid_name(bad), "{bad} should be rejected");
        }
        for good in ["nvim", "ghostty-config", "a.b", "x_1"] {
            assert!(valid_name(good), "{good} should be accepted");
        }
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

//! cfgsync -- keep one app's config files in sync across machines through a
//! single central repo.
//!
//! Three pieces, and every command moves between them:
//!
//!   central repo   `~/.local/share/cfgsync`, a git repo: one directory per
//!                  source repo, holding path.rules (where it lives on each
//!                  platform), base/, platform/<os>/ and the other rules
//!   source repo    a real config directory of an app, git-init'd by `add` so
//!                  every harvest and render shows up in its own history
//!   cloud repo     the central repo's git remote
//!
//! `harvest` reads a source repo into the central one, `render` writes it back,
//! `push` / `pull` move the central repo to and from the cloud.

mod adopt;
mod central;
mod engine;
mod formats;
mod fsx;
mod paths;
mod rules;

use std::path::{Path, PathBuf};

const USAGE: &str = "\
cfgsync -- keep app config files in sync across machines through one central repo

three pieces:
  central repo  ~/.local/share/cfgsync -- a git repo with one directory per
                source repo, named by you (nvim, ghostty), holding path.rules
                (where it lives on each platform), adopt.rules, sync.rules,
                base/ and platform/<os>/
  source repo   a real config directory of an app; `add` git-inits it so every
                harvest and render lands in its own history
  cloud repo    the central repo's git remote

usage:
  cfgsync init    [--central DIR] [--remote URL] [--dry-run]
  cfgsync add     DIR [NAME] [--dry-run]
  cfgsync rm      NAME|DIR [--dry-run]
  cfgsync harvest [NAME|DIR] [-a] [--os OS] [--dry-run]
  cfgsync render  [NAME|DIR] [-a] [--os OS] [--dry-run] [--force]
  cfgsync status  [NAME|DIR] [-a] [--os OS]
  cfgsync edit    -a|-s|-p [NAME|DIR]
  cfgsync push    [-m MESSAGE] [--dry-run]
  cfgsync pull    [--dry-run]
  cfgsync adaptor            list the file types that are split key by key

harvest / render / status work on the source repo the current directory is in,
or on NAME|DIR; -a does it for every source repo this platform has a path for.
edit -a opens adopt.rules (which files), -s sync.rules (which keys), -p
path.rules (where the app lives on each platform).

add records this platform's path in <name>/path.rules. The name defaults to the
directory's own name; a name already standing for another directory on this
platform gets a -2/-3 suffix. Passing an existing NAME adds this machine's path
to that repo, which is how a second machine joins.

layout of one source repo inside the central repo:
  <name>/path.rules                      where it lives, per platform
  <name>/adopt.rules                     which files are managed
  <name>/sync.rules                      which keys are machine-owned
  <name>/base/<stem>.base.<ext>          shared baseline
  <name>/platform/<os>/<stem>.overlay.<ext>   this machine's delta
  <name>/.hash                           content hash at the last harvest/render

platform resolution (first hit wins):
  --os NAME > CFGSYNC_OS > <central>/.os (written by init) > host

exit codes: 0 ok, 1 refused or a git command failed, 2 usage or I/O
";

struct Cli {
    cmd: String,
    central: Option<PathBuf>,
    os: Option<String>,
    remote: Option<String>,
    message: Option<String>,
    positional: Option<String>,
    second: Option<String>,
    dry: bool,
    force: bool,
    all: bool,
    sync_rules: bool,
    path_rules: bool,
}

/// `--flag=value` -> (`--flag`, Some("value")); the value may still be the next
/// argv entry
fn split_flag(arg: &str) -> (&str, Option<&str>) {
    match arg.split_once('=') {
        Some((flag, value)) => (flag, Some(value)),
        None => (arg, None),
    }
}

fn take(
    it: &mut std::slice::Iter<'_, String>,
    inline: Option<&str>,
    flag: &str,
) -> Result<String, (String, i32)> {
    match inline {
        Some(v) => Ok(v.to_string()),
        None => it
            .next()
            .cloned()
            .ok_or((format!("{flag} needs a value"), 2)),
    }
}

/// The command word, then at most two arguments (`add DIR [NAME]`).
fn take_positional(
    cli: &mut Cli,
    command: &mut Option<String>,
    arg: &str,
) -> Result<(), (String, i32)> {
    if command.is_none() {
        *command = Some(arg.to_string());
    } else if cli.positional.is_none() {
        cli.positional = Some(arg.to_string());
    } else if cli.second.is_none() {
        cli.second = Some(arg.to_string());
    } else {
        return Err((format!("unexpected argument: {arg}"), 2));
    }
    Ok(())
}

fn parse(argv: &[String]) -> Result<Cli, (String, i32)> {
    let mut cli = Cli {
        cmd: String::new(),
        central: None,
        os: None,
        remote: None,
        message: None,
        positional: None,
        second: None,
        dry: false,
        force: false,
        all: false,
        sync_rules: false,
        path_rules: false,
    };
    let mut command: Option<String> = None;
    let mut it = argv.iter();
    while let Some(arg) = it.next() {
        let arg = arg.as_str();
        let (flag, inline) = split_flag(arg);
        match flag {
            "-h" | "--help" => return Err((USAGE.to_string(), 0)),
            "-V" | "--version" => {
                return Err((format!("cfgsync {}", env!("CARGO_PKG_VERSION")), 0))
            }
            "--dry-run" => cli.dry = true,
            "--force" => cli.force = true,
            "-a" | "--all" => cli.all = true,
            "-s" | "--sync" => cli.sync_rules = true,
            "-p" | "--path" => cli.path_rules = true,
            "--os" => cli.os = Some(take(&mut it, inline, "--os")?),
            "--central" => cli.central = Some(PathBuf::from(take(&mut it, inline, "--central")?)),
            "--remote" => cli.remote = Some(take(&mut it, inline, "--remote")?),
            "-m" | "--message" => cli.message = Some(take(&mut it, inline, "-m")?),
            // POSIX end-of-flags, for a directory or name that starts with '-'
            "--" => {
                for rest in it.by_ref() {
                    take_positional(&mut cli, &mut command, rest)?;
                }
            }
            _ if arg.starts_with('-') => {
                return Err((format!("unknown flag: {arg}\n\n{USAGE}"), 2))
            }
            _ => take_positional(&mut cli, &mut command, arg)?,
        }
    }
    cli.cmd = command.ok_or_else(|| (USAGE.to_string(), 0))?;
    Ok(cli)
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let cli = match parse(&argv) {
        Ok(cli) => cli,
        Err((message, code)) => {
            if code == 0 {
                println!("{message}");
            } else {
                eprintln!("{message}");
            }
            std::process::exit(code);
        }
    };
    std::process::exit(run(cli));
}

fn run(cli: Cli) -> i32 {
    let root = central::resolve(cli.central.as_deref());
    let os = central::resolve_os(&root, cli.os.as_deref());
    match cli.cmd.as_str() {
        "init" => central::init(&root, cli.remote.as_deref(), cli.dry),
        "add" => engine::add(
            &root,
            cli.positional.as_deref(),
            cli.second.as_deref(),
            &os,
            cli.dry,
        ),
        "rm" => match cli.positional.as_deref() {
            Some(target) => central::remove(&root, target, &os, cli.dry),
            None => {
                eprintln!("usage: cfgsync rm NAME|DIR");
                2
            }
        },
        "harvest" | "render" | "status" => {
            let entries = match pick(&root, &cli, &os) {
                Ok(entries) => entries,
                Err(code) => return code,
            };
            let create = match (cli.cmd.as_str(), cli.dry) {
                ("render", true) => Create::Plan,
                ("render", false) => Create::Now,
                _ => Create::Never,
            };
            let mut code = 0;
            for entry in &entries {
                let Some(source) = reachable(entry, &os, cli.all, create) else {
                    code = 1;
                    continue;
                };
                let repo = central::Repo::new(&root, &entry.name, source);
                let result = match cli.cmd.as_str() {
                    "harvest" => engine::harvest(&repo, &os, cli.dry),
                    "render" => engine::render(&repo, &os, cli.dry, cli.force),
                    _ => engine::status(&repo, &os),
                };
                if result != 0 {
                    code = result;
                }
            }
            code
        }
        "edit" => {
            let which = match (cli.all, cli.sync_rules, cli.path_rules) {
                (true, false, false) => engine::RulesFile::Adopt,
                (false, true, false) => engine::RulesFile::Sync,
                (false, false, true) => engine::RulesFile::Path,
                _ => {
                    eprintln!(
                        "usage: cfgsync edit -a|-s|-p [NAME|DIR]\n  -a  adopt.rules\n  \
                         -s  sync.rules\n  -p  path.rules"
                    );
                    return 2;
                }
            };
            // `-a` means adopt.rules here, never "every source repo": edit opens
            // the rules of the directory the command was started in
            let entry = match pick_one(&root, cli.positional.as_deref(), &os) {
                Ok(entry) => entry,
                Err(code) => return code,
            };
            let source = entry.source().map(Path::to_path_buf).unwrap_or_default();
            if source.as_os_str().is_empty() && which != engine::RulesFile::Path {
                eprintln!(
                    "[sync] error: {} has no path for {os} -- `cfgsync add DIR {}` here, \
                     or edit -p",
                    entry.name, entry.name
                );
                return 1;
            }
            engine::edit(&central::Repo::new(&root, &entry.name, source), which)
        }
        "push" => central::push(&root, cli.message.as_deref(), cli.dry),
        "pull" => central::pull(&root, cli.dry),
        "adaptor" | "formats" => {
            let rows = formats::inventory();
            let width = rows.iter().map(|(name, _)| name.len()).max().unwrap_or(0);
            println!("[adaptor] extension or file name -> key-level sync:");
            for (name, keys) in rows {
                println!("[adaptor]   {name:<width$}  {}", keys.join("  "));
            }
            println!(
                "[adaptor]   {:<width$}  any other file adopt.rules claims: carried whole in \
                 share/, the same on every platform",
                "share"
            );
            0
        }
        other => {
            eprintln!("unknown command: {other}\n\n{USAGE}");
            2
        }
    }
}

/// The repos a command should act on, still unresolved: `-a` means every one
/// the central repo knows, NAME|DIR means that one, nothing means the current
/// directory.
fn pick(root: &Path, cli: &Cli, os: &str) -> Result<Vec<central::Entry>, i32> {
    if cli.all {
        for name in central::unconfigured(root) {
            eprintln!(
                "[sync] warning: {name} has no {}/{} (old layout?) -> `cfgsync rm {name}` \
                 and `cfgsync add DIR {name}` again",
                name,
                paths::NAME
            );
        }
        let entries = central::entries(root, os);
        if entries.is_empty() {
            eprintln!(
                "[sync] no source repos in {} -- `cfgsync add DIR` first",
                root.display()
            );
            return Err(2);
        }
        return Ok(entries);
    }
    Ok(vec![pick_one(root, cli.positional.as_deref(), os)?])
}

/// One repo, spelled as its central name, as a source path, or as nothing at
/// all (the directory the command was started in).
fn pick_one(root: &Path, arg: Option<&str>, os: &str) -> Result<central::Entry, i32> {
    if let Some(name) = arg {
        if let Some(entry) = central::by_name(root, name) {
            return Ok(central::entry(root, &entry.name, os));
        }
    }
    let start = match arg {
        Some(dir) => central::canonical(Path::new(dir)).unwrap_or_else(|| PathBuf::from(dir)),
        None => std::env::current_dir().unwrap_or_default(),
    };
    match central::entry_for_path(root, &start, os) {
        Some(entry) => Ok(entry),
        None => {
            eprintln!(
                "[sync] {} is not a managed source repo -- `cfgsync add {}`, or pass -a for \
                 all of them",
                start.display(),
                start.display()
            );
            Err(2)
        }
    }
}

/// Whether a missing source directory stops the command or gets created:
/// `render` writes the config a fresh machine pulled, so a path that
/// `path.rules` spells out is enough to create it -- `harvest` and `status`
/// have nothing to read there and keep refusing.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Create {
    Never,
    Now,
    /// `--dry-run`: say what would happen, create nothing
    Plan,
}

/// The directory `entry` stands for on this platform. A missing line or a
/// directory that is not there is fatal for a command that named the repo, and
/// a skip for `-a`: a linux-only repo must not fail `render -a` on windows.
fn reachable(
    entry: &central::Entry,
    os: &str,
    skip: bool,
    create: Create,
) -> Option<PathBuf> {
    let Some(path) = entry.source().map(Path::to_path_buf) else {
        report(
            skip,
            &format!(
                "{}: no path for {os} in {}/{}, so this machine cannot reach it",
                entry.name,
                entry.name,
                paths::NAME
            ),
        );
        return None;
    };
    if path.is_dir() {
        return Some(path);
    }
    match create {
        Create::Never => {
            report(
                skip,
                &format!(
                    "{}: no such directory: {} -- fix {}/{}",
                    entry.name,
                    path.display(),
                    entry.name,
                    paths::NAME
                ),
            );
            None
        }
        Create::Plan => {
            println!(
                "[render] {}: not there yet -> would create it (from {}/{})",
                path.display(),
                entry.name,
                paths::NAME
            );
            Some(path)
        }
        Create::Now => match std::fs::create_dir_all(&path) {
            Ok(()) => {
                println!(
                    "[render] {}: not there yet -> created it (from {}/{})",
                    path.display(),
                    entry.name,
                    paths::NAME
                );
                Some(path)
            }
            Err(e) => {
                eprintln!("[sync] error: {}: cannot create {}: {e}", entry.name, path.display());
                None
            }
        },
    }
}

fn report(skip: bool, problem: &str) {
    if skip {
        eprintln!("[sync] warning: {problem} -> skipped");
    } else {
        eprintln!("[sync] error: {problem}");
    }
}

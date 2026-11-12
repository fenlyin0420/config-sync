//! cfgsync -- keep one app's config files in sync across machines through a
//! single central repo.
//!
//! Three pieces, and every command moves between them:
//!
//!   central repo   `~/.local/share/cfgsync`, a git repo: one directory per
//!                  source repo, holding base/, platform/<os>/ and the rules
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
mod rules;

use std::path::{Path, PathBuf};

const USAGE: &str = "\
cfgsync -- keep app config files in sync across machines through one central repo

three pieces:
  central repo  ~/.local/share/cfgsync -- a git repo with one directory per
                source repo (its absolute path with '/' -> '--'), holding
                base/, platform/<os>/, adopt.rules, sync.rules
  source repo   a real config directory of an app; `add` git-inits it so every
                harvest and render lands in its own history
  cloud repo    the central repo's git remote

usage:
  cfgsync init    [--central DIR] [--remote URL] [--dry-run]
  cfgsync add     DIR [--dry-run]
  cfgsync rm      DIR|-- NAME [--dry-run]
  cfgsync harvest [DIR] [-a] [--os OS] [--dry-run]
  cfgsync render  [DIR] [-a] [--os OS] [--dry-run] [--force]
  cfgsync status  [DIR] [-a] [--os OS]
  cfgsync edit    -a|-s [DIR]
  cfgsync push    [-m MESSAGE] [--dry-run]
  cfgsync pull    [--dry-run]
  cfgsync adaptor            list the file types that are split key by key

harvest / render / status work on the source repo the current directory is in,
or on DIR; -a does it for every source repo the central repo knows.
edit -a opens adopt.rules (which files), edit -s sync.rules (which keys).

layout of one source repo inside the central repo:
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
    dry: bool,
    force: bool,
    all: bool,
    sync_rules: bool,
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

fn parse(argv: &[String]) -> Result<Cli, (String, i32)> {
    let mut cli = Cli {
        cmd: String::new(),
        central: None,
        os: None,
        remote: None,
        message: None,
        positional: None,
        dry: false,
        force: false,
        all: false,
        sync_rules: false,
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
            "--os" => cli.os = Some(take(&mut it, inline, "--os")?),
            "--central" => cli.central = Some(PathBuf::from(take(&mut it, inline, "--central")?)),
            "--remote" => cli.remote = Some(take(&mut it, inline, "--remote")?),
            "-m" | "--message" => cli.message = Some(take(&mut it, inline, "-m")?),
            // POSIX end-of-flags: a repo directory is named `--home--user--...`,
            // so `rm -- --home--user--.config` has to be spellable
            "--" => {
                for rest in it.by_ref() {
                    if command.is_none() {
                        command = Some(rest.clone());
                    } else if cli.positional.is_none() {
                        cli.positional = Some(rest.clone());
                    } else {
                        return Err((format!("unexpected argument: {rest}"), 2));
                    }
                }
            }
            _ if arg.starts_with('-') => {
                return Err((format!("unknown flag: {arg}\n\n{USAGE}"), 2))
            }
            _ => {
                if command.is_none() {
                    command = Some(arg.to_string());
                } else if cli.positional.is_none() {
                    cli.positional = Some(arg.to_string());
                } else {
                    return Err((format!("unexpected argument: {arg}"), 2));
                }
            }
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
        "add" => engine::add(&root, cli.positional.as_deref(), &os, cli.dry),
        "rm" => match cli.positional.as_deref() {
            Some(target) => central::remove(&root, target, cli.dry),
            None => {
                eprintln!("usage: cfgsync rm NAME|DIR");
                2
            }
        },
        "harvest" | "render" | "status" => {
            let repos = match select(&root, &cli) {
                Ok(repos) => repos,
                Err(code) => return code,
            };
            let mut code = 0;
            for repo in &repos {
                if !repo.source.is_dir() {
                    eprintln!(
                        "[sync] warning: {} ({}): no such directory -> skipped",
                        repo.source.display(),
                        repo.name
                    );
                    code = 1;
                    continue;
                }
                let result = match cli.cmd.as_str() {
                    "harvest" => engine::harvest(repo, &os, cli.dry),
                    "render" => engine::render(repo, &os, cli.dry, cli.force),
                    _ => engine::status(repo, &os),
                };
                if result != 0 {
                    code = result;
                }
            }
            code
        }
        "edit" => {
            let which = match (cli.all, cli.sync_rules) {
                (true, false) => engine::RulesFile::Adopt,
                (false, true) => engine::RulesFile::Sync,
                _ => {
                    eprintln!("usage: cfgsync edit -a|--sync [DIR]\n  -a  adopt.rules\n  -s  sync.rules");
                    return 2;
                }
            };
            // `-a` means adopt.rules here, never "every source repo": edit opens
            // the rules of the directory the command was started in
            let repo = match locate_one(&root, &cli) {
                Ok(repo) => repo,
                Err(code) => return code,
            };
            engine::edit(&repo, which)
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

/// The source repos a command should act on: `-a` means every one the central
/// repo knows, DIR means that directory, nothing means the current directory.
fn select(root: &Path, cli: &Cli) -> Result<Vec<central::Repo>, i32> {
    if cli.all {
        let repos = central::all(root);
        if repos.is_empty() {
            eprintln!(
                "[sync] no source repos in {} -- `cfgsync add DIR` first",
                root.display()
            );
            return Err(2);
        }
        return Ok(repos);
    }
    Ok(vec![locate_one(root, cli)?])
}

/// The one source repo a command reaches without `-a`: DIR, or the directory the
/// command was started in, resolved to the deepest managed ancestor.
fn locate_one(root: &Path, cli: &Cli) -> Result<central::Repo, i32> {
    let start = match cli.positional.as_deref() {
        Some(dir) => central::canonical(Path::new(dir)).unwrap_or_else(|| PathBuf::from(dir)),
        None => std::env::current_dir().unwrap_or_default(),
    };
    match central::locate(root, &start) {
        Some(repo) => Ok(repo),
        None => {
            eprintln!(
                "[sync] {} is not inside a managed source repo -- `cfgsync add {}`, \
                 or pass -a for all of them",
                start.display(),
                start.display()
            );
            Err(2)
        }
    }
}

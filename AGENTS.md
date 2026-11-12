# AGENTS.md

`cfgsync` is a single-binary Rust CLI that keeps the config files of many apps
in sync across machines through **one central git repo**. Per app there is a
committed shared **base** file, a per-platform **overlay** holding only this
platform's keys, and the app's real **source repo** that `render` writes and
`harvest` reads. The central directory is named by the user (`nvim`), not by a
path: `path.rules` inside it maps each platform to that platform's real
directory, because the same app lives at different paths on different systems. README.md is the user manual; this file is for changing the
code.

## Commands

```sh
cargo build            # binary: target/debug/cfgsync
cargo test             # unit tests + tests/cli.rs (spawns the real binary)
```

A release build (`cargo build --release`, `strip` + LTO) is what gets installed.

`tests/cli.rs` needs `git` on `PATH` (it creates local bare remotes and real
commits in `std::env::temp_dir()`). No network, no test touches the user's real
config, `$HOME` or a real central repo. Every test points `CFGSYNC_CENTRAL` at a
temp directory.

## Layout

| file | responsibility |
| --- | --- |
| `src/main.rs` | argv parsing (`parse`), `USAGE` text, dispatch, repo selection, exit codes |
| `src/central.rs` | the central repo: location, naming, lookup, `.hash` / `.os`, platform resolution, git plumbing, `init` / `rm` / `push` / `pull` |
| `src/paths.rs` | `path.rules` parsing: per-platform real path, `~` / `$VAR` / `%VAR%` expansion, in-place rewrite |
| `src/engine.rs` | discovery, the harvest/render plan, `harvest` / `render` / `status`, `add` / `edit` |
| `src/fsx.rs` | `read` (BOM-trimmed), `write` (LF), `rel`, `walk` |
| `src/adopt.rs` | `adopt.rules` parsing: gitignore-style allowlist of the managed files |
| `src/rules.rs` | `sync.rules` parsing and fnmatch-style glob matching |
| `src/formats/mod.rs` | `Adapter` trait, `Op`/`OpData`/`Kind`, the extension + file-name registry |
| `src/formats/ini.rs` | line-preserving ini-family adapter |
| `src/formats/json.rs` | JSON deep-merge adapter with list patch ops |
| `src/formats/jsonl.rs` | JSONL adapter: one record per line, addressed `line3.model`; reuses the json merge and list patches |
| `src/formats/toml.rs` | line-preserving TOML adapter: `[table]` + `key = value` as text, `[[table]]` repeats as `bin#2.name` |
| `src/formats/yaml.rs` | line-preserving block YAML adapter: indentation paths, sequence items as `servers#2.host` |
| `src/formats/plain.rs` | extensionless `key = value` adapter (ghostty's `config`), duplicate keys addressed as `key#N` |

The central repo (`~/.local/share/cfgsync`, overridable by `--central` /
`CFGSYNC_CENTRAL`) holds one directory per source repo, named by the user (the
directory's own name by default, `add DIR NAME` for anything else):

```text
<central>/<name>/path.rules                         where it lives, per platform
<central>/<name>/adopt.rules                        which files are managed
<central>/<name>/sync.rules                         which keys are machine-owned
<central>/<name>/base/<stem>.base.<ext>             shared baseline
<central>/<name>/platform/<os>/<stem>.overlay.<ext> this platform's delta
<central>/<name>/share/<rel>                        no adapter: verbatim, all platforms
<central>/<name>/.hash                              content hash, last harvest/render
<central>/.os                                       this machine's platform
```

A file with no extension keeps its name and grows the tag: `agent/config` maps
to `base/agent/config.base` / `platform/<os>/agent/config.overlay`.

`path.rules` holds one `os = path` line per platform (`linux = ~/.config/nvim`,
`win = %LOCALAPPDATA%\nvim`), with `~`, `$VAR` and `%VAR%` expanded on read.
`add` writes the line of the machine it runs on; another machine extends the
same file by running `add <its path> <name>`. `render` / `harvest` / `status`
resolve the source directory through it and refuse (exit 1) when this platform
has no line or the directory is not there; `-a` skips such a repo with a
warning, so a linux-only repo does not fail `render -a` on windows.

Discovery: a source file is managed when `adopt.rules` claims it -- the union of
the files on disk, the committed `base/` files and the `share/` copies, because
a fresh machine has no source file yet while the central copies are there. Which
adapter handles a file is decided by extension, and for an extensionless file by
its name (`formats::for_file`); no adapter means `share/`, never a skip.

Data flow, both directions:

- `render`: `base` + `overlay_ops(overlay)` → `apply` → source file. Guarded by
  the recorded hash (below).
- `harvest`: `diff(base, current)` → split by `sync.rules` → shared ops into
  base, owned ops into the overlay, plus `pin_ops` (owned keys copied out of
  base) and pruning of overlay entries the table does not own. A claimed file
  with no base file is **adopted**: its current content becomes the base
  verbatim (comments and ordering survive, no `apply` round trip) and owned keys
  are pinned from it.
- `share`: a file no adapter can read never goes through the plan. `harvest`
  copies it to `share/<rel>`, `render` copies it back, `status` compares bytes;
  `sync.rules` has no paths to match on there, and the hash covers it.

## Invariants

- **The three pieces never blur.** The central repo holds every sync-side file;
  a source repo holds app config and its own git history; nothing is generated
  behind the app's back, nothing gets gitignored, and the source repo is never
  modified except by `render` / `harvest` / `add` (git init).
- **`path.rules` is the mapping.** The directory name is a label the user picks,
  so the source path may only come from `<name>/path.rules` -- never from the
  name, never from a guess, never from an environment default. A platform with
  no line there is an error, not a fallback to another platform's path.
- **`add` is the only writer of a `path.rules` line**, and it writes only the
  current platform's. It never overwrites a different path recorded for the same
  platform (that is what `edit -p` is for), never creates a second central
  directory for a source directory that is already managed, and suffixes a name
  that is taken (`nvim` -> `nvim-2`) rather than reusing it for another path.
- **`.hash` and `.os` are machine-local and gitignored.** A committed hash would
  make every other machine's `render` refuse. `init` is the only writer of
  `.gitignore`; keep it idempotent.
- **`adopt.rules` is authoritative for which files are managed.** No base file
  is read for a source file its rules do not claim, and the base walk may not
  bypass the check. Removing a rule leaves the committed base behind: warn about
  it, never delete it.
- **`sync.rules` is authoritative for overlay content, in both directions.**
  Matched keys belong in the overlay even while they still equal base (pinning),
  unmatched overlay entries are drift and get promoted to base. Do not add a
  path that silently bypasses this. A line's `file-glob` is matched against the
  file's repo-relative path (`rules::path_match`): no `/` means the file name at
  any depth, a `/` anchors to the source repo root, `**` spans directories. A key
  path also covers the keys below it, so a rule naming a parent owns (or vetoes)
  the whole subtree.
- **A `!` line takes a key out of the central repo entirely.** `rules::route`
  returns `Excluded`, which vetoes ownership: the key goes into neither base nor
  overlay, `harvest` prunes it from a base/overlay that already has it (adoption
  stripped it too), and `render` / `harvest` / `status` leave the app's own value
  alone via `keep_local`, which starts from the file on disk so the key keeps its
  place. That is how a secret stays out of the pushed git history; never route an
  excluded key into base "because the base is the document".
- **No overlay file until there is something to put in it.** `harvest` writes
  `platform/<os>/<stem>.overlay.<ext>` only when a key was routed there (or the
  file already exists and has to be rewritten / emptied). An empty overlay is
  noise in the central repo, and a missing one reads exactly like an empty one.
- **A file the adapter cannot read is never rewritten.** `parses` gates `render`
  and `harvest`: adoption still copies the bytes into the base verbatim, but
  nothing overwrites the source file until it is readable, because a failed
  parse would otherwise be written back as an empty document.
- **The hash guard is the only proof that a source file is ours to overwrite.**
  `render` refuses (exit 1) when the recorded `.hash` no longer matches; with no
  record at all it leaves an existing file that differs from base + overlay
  alone and writes no record; `--force` is the documented escape hatch. Never
  weaken this into "render always wins".
- **Idempotency.** Rendering the same base + overlay twice must produce
  identical bytes, and a second harvest must write nothing new (no second commit
  either). The json adapter returning the base text when a merge changes nothing
  is what makes an adopted base a fixed point -- keep it.
- **`harvest` and `render` commit the source repo** with `git add -A` and the
  messages `cfgsync: harvest` / `cfgsync: render`; that history is the whole
  point of `add` git-initializing the directory. Nothing else may commit there.
- **`stale_overlay` / `unmanaged` warnings are the safety net.** A config-looking
  file no `adopt.rules` line claims must keep the "add it to adopt.rules"
  warning, a claimed file with no base must warn instead of being overwritten,
  and an overlay entry outside `sync.rules` must warn rather than be deleted
  silently.
- **Conflicts and destructive git are the user's call.** `push` / `pull` may
  commit the central repo and rebase, but never resolve conflicts, never
  force-push, never discard work. `pull` reports conflicts and stops. On a
  central repo that has no commit yet it may fetch and check out the remote's
  default branch -- that is the only way a fresh machine gets the cloud copy,
  and the only file it may remove first is the `.gitignore` `init` itself wrote.
- **Files are written LF, forward slashes.** Reads tolerate a UTF-8 BOM but never
  write one. Don't sprinkle `\r` handling in adapters.
- **Dependencies stay tiny.** Only `serde_json` (with `preserve_order`) so far;
  timestamps, hashing (FNV-1a), path normalization and glob matching are
  hand-rolled. Prefer a few lines over a crate.
- **Platform names are normalized once** (`central::normalize_os`); everything
  downstream compares the short form `win` / `linux` / `macos`. An unknown
  platform must degrade to "base only + warning", never to a guess.
- **`add` only takes directories** and never overwrites rules a pull brought in;
  `rm` only removes the central copy and never touches the source directory.
  Both take a central name or a source path: a name has to keep working when the
  path is not reachable from this machine.

## Adding a format

1. Implement `formats::Adapter` in `src/formats/<name>.rs`: `name()` (what
   `cfgsync adaptor` prints), `overlay_ops` (text → ops), `diff` (base, current →
   ops, `Kind::Delete` for removals), `apply` (base, ops → text, returning the
   base text unchanged when the ops change nothing), `dump_overlay` (ops →
   overlay file text), `pin_ops` (base, owned-predicate → set ops for owned
   keys), `drop_ops` (base, veto-predicate → delete ops, so a `!` key leaves the
   base), and optionally `parses` (text → can you read this at all; the default
   says yes).
2. Register it in `formats::registry()` (extensions) or `formats::by_name()`
   (whole file names, for extensionless files like ghostty's `config`).
3. The op `path` is what `sync.rules` matches: whatever is natural for the
   format, dot-joined (e.g. `section.key`, `a.b.c`). A repeated key gets a
   `#N` suffix for the Nth occurrence; `rules::route` matches `keybind` against
   `keybind#2` so one line still covers the key.
4. Add unit tests in the adapter module and at least one end-to-end case in
   `tests/cli.rs` using the `workspace`-style fixture.
5. Update the format table in `README.md` and the `cfgsync adaptor` output. The
   extension alone makes files adoptable; `adopt.rules` decides which ones are,
   and a file with no adapter is carried whole in `share/` instead of skipped.

## Conventions

- `sync.rules` and `adopt.rules` templates are written by `add`; their comment
  text is Chinese because that is the file users actually read, while code and
  docs stay English.
- Output style: `[command] ...` on stdout for what happened, `[sync] warning:`
  on stderr for what needs attention. Keep messages stable — tests assert on
  them.
- Exit codes: `0` success, `1` the command refused or a git command failed, `2`
  usage or I/O errors. `--help` / `--version` print to stdout and exit `0`.
- `--dry-run` must print the same plan without writing anything, including the
  `.hash` file and any commit.
- Replies and docs follow the language of the surrounding code (English);
  `sync.rules`' generated template comments are Chinese, matching the file users
  actually read.

# config-sync

`cfgsync` keeps the config files of many apps in sync across machines through
**one central git repo**, reconciling them **key by key** rather than file by
file:

- the **central repo** holds, per app, a shared **base** file plus a per-OS
  **overlay** holding only the keys that platform owns;
- the app's **source repo** — the real directory the app reads — is written by
  `render` and read by `harvest`;
- the central repo's git remote is the **cloud repo** that carries everything
  between machines.

Because the overlay only carries deltas, two machines never fight over the same
file, and a base change reaches everyone without touching anyone's local
settings. One static binary, no Python, nothing installed into the app's
directory.

```text
~/.config/app/config.ini     <=     base/config.base.ini  +  platform/linux/config.overlay.ini
 (the app reads this)               (shared, committed)      (this platform only, committed)
```

## Install

Prebuilt binaries for linux (x86_64 / aarch64), macOS (x86_64 / aarch64) and
Windows x86_64 are attached to every [tagged release][releases].

[releases]: https://github.com/fenlyin0420/config-sync/releases

```sh
cargo build --release        # target/release/cfgsync
cargo install --path .       # or put it on PATH
```

## Quick start

First machine:

```sh
cfgsync init                              # central repo + git + platform pin
cfgsync add ~/.config/nvim                # register a source repo, git-init it
cfgsync edit -a                           # uncomment the files to manage
cfgsync harvest                           # split them into base + overlay
cfgsync init --remote git@github.com:me/cfgsync.git   # once, the cloud repo
cfgsync push                              # commit the central repo and push
```

Another machine:

```sh
cfgsync init --remote git@github.com:me/cfgsync.git
cfgsync pull                              # fetch + check out the cloud branch
cfgsync add %LOCALAPPDATA%\nvim nvim      # record this machine's path for `nvim`
cfgsync render                            # write base + overlay into it
```

The second argument is the name the central repo already uses for that app: the
same app lives at a different path on every platform, so `add` writes this
machine's path into the repo's `path.rules` instead of creating a second one.
`add` keeps the `adopt.rules` / `sync.rules` that came with the pull; it only
creates them when the central repo has never seen this directory.

Day to day, from inside a source repo:

```sh
cfgsync status     # what differs from the central copy
cfgsync cd         # jump to the central repo (~/.local/share/cfgsync)
cfgsync harvest    # after editing the app's file by hand
cfgsync render     # write the central copy back (refuses if unharvested edits exist)
cfgsync pull       # other machines' changes
cfgsync push       # this machine's changes
```

## Commands

| command | what it does |
| --- | --- |
| `init [--central DIR] [--remote URL]` | create the central repo (`git init`, `.gitignore`, this platform), optionally set `origin` |
| `add DIR [NAME]` | register a directory as a source repo: create `NAME` (default: the directory's own name) in the central repo, record this platform's path in its `path.rules`, write `adopt.rules` / `sync.rules`, `git init` it if needed |
| `rm NAME` / `rm DIR` | drop a source repo from the central repo; the directory on disk and its history stay |
| `harvest [NAME\|DIR] [-a]` | split the source repo into base + overlay, record the content hash, commit the source repo |
| `render [NAME\|DIR] [-a] [--force]` | write base + overlay into the source repo, record the hash, commit the source repo |
| `status [NAME\|DIR] [-a]` | compare the source repo with what `render` would write |
| `cd` | go to the central repo itself, the one directory holding every app's base and overlay |
| `edit -a` / `-s` / `-p` | open `adopt.rules` / `sync.rules` / `path.rules` in `$VISUAL` / `$EDITOR` |
| `push [-m MESSAGE]` | commit the central repo if dirty, then push |
| `pull` | commit the central repo if dirty, then `git pull --rebase --autostash` |
| `adaptor` | list the file types that are split key by key (alias: `formats`) |

`harvest`, `render` and `status` work on the source repo the current directory
is in — including any subdirectory of it — or on a repo named by its central
`NAME` or by its `DIR`; `-a` does it for every source repo this platform has a
path for. `--dry-run` prints the same plan and writes
nothing, not even the hash.

`cd` takes no arguments: it goes to the central repo, the directory every app's
`base/` and `platform/<os>/` lives in. A terminal gets the path and an
interactive shell in it, so `exit` brings you back where you were; redirected
stdout gets the bare path instead, for a shell function to `cd` into:

```sh
ccd() { cd "$(cfgsync cd)"; }   # ccd = where the configs are kept
```

Exit codes: `0` ok, `1` refused (the hash guard, a git failure), `2` usage or
I/O. `--help` / `--version` print to stdout and exit `0`.

`--central DIR` (or `$CFGSYNC_CENTRAL`) points the whole tool at a different
central repo; the default is `~/.local/share/cfgsync` on linux,
`~/Library/Application Support/cfgsync` on macOS, `%APPDATA%\cfgsync` on
Windows.

## Output

One line per fact, `[command]` in front, and the files of a repo printed as an
indented block with one column for the path:

```text
[harvest] nvim  ~/.config/nvim
  init.lua          2 shared -> base, 1 machine-owned -> platform/linux
  base : runtimepath  = ~/.config/nvim
  local : theme       = catppuccin
  keymaps.lua       no changes
[harvest] ~/.config/nvim: 1 harvested (0 adopted), 1 unchanged
```

`status` codes a file with one letter: `M` differs from what `render` would
write, `+` is missing on disk, `?` is claimed by `adopt.rules` but has no base
or share copy yet. `base` / `local` / `pin` / `prune` / `keep` label where a key
went: shared baseline, this machine's overlay, copied out of the base, dropped
from the overlay, or left to the app by a `!` line.

Colour marks the structure only — the command tag, a status letter, a result
word — never a path or a value, and it is off unless the stream is a terminal.
`NO_COLOR=1` turns it off, `CLICOLOR` / `CLICOLOR_FORCE` are honored, and
`CFGSYNC_COLOR=always|never` is the explicit answer, so it wins over all of
them. `--dry-run` prints the same
lines with `would` in place of what it did.

## Central repo layout

One directory per source repo, named whatever you like — by default the
directory's own name (`nvim`, `ghostty`), and `add DIR NAME` picks another. The
name is only a label: `path.rules` inside it holds the real path for each
platform, because the same app does not live in the same place everywhere.

```text
~/.local/share/cfgsync/
├── .gitignore                              # .hash and .os: machine-local
├── .os                                     # this machine's platform
├── nvim/
│   ├── path.rules                          # linux = ~/.config/nvim, win = ...
│   ├── adopt.rules                         # which files of it are managed
│   ├── sync.rules                          # which keys are this platform's alone
│   ├── base/init.base.lua                  # shared baseline
│   ├── platform/linux/init.overlay.lua     # this platform's delta
│   ├── share/lua/keymaps.lua               # no adapter: verbatim, every platform
│   └── .hash                               # content hash at the last harvest/render
└── ghostty/
    └── ...
```

`.hash` and `.os` are the only machine-local files and both are gitignored — a
foreign hash would make every `render` refuse to write.

Base files may sit in subdirectories: `ui/key.json` of the source repo maps to
`base/ui/key.base.json` and `platform/<os>/ui/key.overlay.json`. A file with no
extension keeps its name and grows the tag: `agent/config` becomes
`base/agent/config.base` and `platform/<os>/agent/config.overlay`.

## `path.rules`

Where the app's directory is on each platform, one `os = path` line each. `add`
writes the line for the machine it runs on; another machine adds its own line by
running `cfgsync add <its path> <name>` — an existing name that has no line for
this platform is extended, never duplicated into `name-2`.

```text
# '~' is the home directory; $VAR and %VAR% are read from the environment
linux = ~/.config/nvim
win   = %LOCALAPPDATA%\nvim
macos = ~/.config/nvim
```

- `render`, `harvest` and `status` resolve the source directory through this
  file. A platform with no line is an error (`1`) rather than a guess, and `-a`
  skips such a repo with a warning, so a linux-only repo does not fail
  `render -a` on windows.
- A directory that is not there yet does not stop `render`: the path is already
  written down, so a first render on a fresh machine creates the directory and
  puts the pulled base + overlay into it. `harvest` and `status` still refuse,
  since they have nothing to read there.
- `cfgsync edit -p` opens it. Editing a path by hand is fine — this file is the
  only place that mapping lives.
- `add` names a repo after its directory; a name already standing for another
  directory on this platform becomes `name-2`, `name-3`, … A directory that is
  already managed keeps the name it has, whatever `add` was given.

## `adopt.rules`

Which files of the source repo cfgsync manages, one pattern per line in
`.gitignore` syntax. `add` writes the file with the syntax and the candidates it
found commented out; uncommenting a line is what adopts a file.

```text
# no '/' -> that name at any depth
config.json

# any ini-family file
*.ini

# trailing '/' -> everything under ui/
ui/

# leading '/' -> only the copy at the repo root
/config.json

# '**' spans any number of directories
a/**/key.json

# '!': never adopt this one
!ui/secret.json
```

- **The last matching line wins and an unmatched path is not adopted**: this is
  an allowlist, the opposite default of a `.gitignore`.
- A directory line covers everything below it, exactly like `git check-ignore`.
- `.git` and nested git checkouts are never walked.
- A file no `adopt.rules` line claims is not synced; `harvest` names the
  config-looking ones it found so a file never silently stays behind. Removing a
  line stops managing the file but leaves its committed base file behind —
  deleting it is your call.

## `sync.rules`

Which keys are this platform's alone, one glob per line, `#` for comments,
`[section]` lines ignored. A line matches a key path — `section.key` for ini-ish
files, `[table].key` for TOML, `a.b.c` for JSON and block YAML, `line3.model` for
JSONL — prefix it with `file-glob:` to scope it to one file.

```text
# machine owned
platform.*                     # ini section
core.theme                     # ini key
settings.json:defaultTools     # in any file named settings.json
/agent/settings.json:tools     # only the one at the repo root
**/settings.json:tools         # settings.json at any depth
*.json:editor.fontSize         # any json file

# never synced (secrets, this machine's only values)
!/agent/settings.json:apiKey
!limits.*
```

A `file-glob` reads a path the same way `adopt.rules` does: no `/` matches the
file name at any depth, a `/` in it anchors the glob to the source repo root,
and `**` spans directories. A key path also covers the keys below it, so
`!secrets` drops a whole JSON object (or ini section) rather than one leaf.

- **matched** differences → `platform/<os>/<stem>.overlay.<ext>`
- **`!`-matched** keys → nowhere: never in the base, never in the overlay, never
  committed to the central repo. The app's own file keeps its value untouched,
  and harvest also prunes the key from a base or overlay that already has it
- **unmatched** differences → `base/<stem>.base.<ext>`, i.e. they propagate to
  every machine
- a file with nothing machine-owned gets **no overlay file** at all; the
  overlay appears the first time harvest routes a key to it

A `!` line is a veto, so it wins over any positive line that matches the same
key. It is not a rejection like `adopt.rules`' `!`: the file stays managed, one
key of it stays local.

Globs are fnmatch-compatible: `*`, `?`, `[seq]`, `[!seq]`. A key an adapter
repeats in one file — `keybind` in a ghostty `config` — is addressed by
occurrence: `keybind` is the first one, `keybind#2` the second, and a `keybind`
rule covers all of them. Files carried whole in `share/` have no keys to match,
so `sync.rules` says nothing about them.

The table is authoritative for overlay content in both directions: a key it
lists is copied into the overlay at harvest time even while it still equals the
base value (so a later base edit cannot leak in), and an overlay entry it does
not own is reported as drift and promoted to the base on the next harvest. JSON
arrays are the one exception: they are never pinned, because an overlay entry
would freeze the list against later base additions.

## Platforms

Resolution order, first hit wins:

1. `--os NAME`
2. `CFGSYNC_OS`
3. `<central>/.os`, written by `init`
4. the host platform

Names are normalized: `windows`/`win32`/`msys`/`mingw`/`cygwin` → `win`,
`darwin`/`osx` → `macos`. Overlays are only ever read from `platform/<os>/`, and
a repo reaches its directory only through that platform's `path.rules` line, so
a typo in either place stops the command instead of guessing at another
machine's settings.

## The harvest hash guard

`harvest` and `render` both record a fingerprint of every managed file of the
source repo in `<central>/<repo>/.hash`. Before writing, `render` recomputes it:

- **it matches** → write base + overlay into the source repo;
- **it differs** → the source repo holds content nobody harvested yet, so
  `render` refuses with exit `1`. Run `cfgsync harvest` to fold the edit into
  base / overlay (nothing is lost), or `cfgsync render --force` to overwrite it;
- **there is no record yet** (a fresh machine, or a base that just arrived for a
  file this machine already had) → a file that differs from what base + overlay
  would produce is left alone with a warning, and no record is written. A file
  that does not exist yet is simply written, so a fresh checkout renders
  normally.

A repo with no managed file on disk at all — a fresh machine, or a directory
`render` has just created from `path.rules` — has nothing to protect, so an
outdated record does not block it either.

A managed file that is not on disk contributes nothing to the fingerprint, so
pulling a base for a file this machine never had does not invalidate the record.

`harvest` and `render` then run `git add -A && git commit` inside the source
repo (`cfgsync: harvest` / `cfgsync: render`), which is what makes the app's own
history show every change cfgsync made. That is the only reason the source repo
gets a git repo; if it already is one, it is reused as is.

## Formats

| select by | adapter | notes |
| --- | --- | --- |
| `.ini`, `.properties`, `.conf`, `.env` | ini | line-preserving: comments, blank lines and key order in the base survive; an overridden key is rewritten in place, a new key is appended to its section |
| `.json` | json | deep object merge, `null` deletes a key, arrays are patched |
| `.jsonl`, `.ndjson` | jsonl | one JSON value per line, addressed by its line ordinal: `line3.model` is a key of the third record, `line3` the whole record; the overlay is JSONL too, one `{"line3": …}` line per touched record |
| `.toml` | toml | line-preserving: `[table]` headers, `key = value` lines, comments and order keep their place; an array spanning lines is one value; a `[[table]]` repeat is addressed as `bin#2.name` |
| `.yaml`, `.yml` | yaml | line-preserving block YAML: comments, indentation and key order survive; a flow collection (`[1, 2]`), a block scalar (`\|`-style or `>`-style) or a quoted scalar is one value, kept verbatim; a sequence item is `servers#2.host`, a scalar one `hosts#3` |
| file named `config`, `.ghostty` | plain | extensionless `key = value` (ghostty's `config`): line-preserving, no sections; a repeated key is addressed by occurrence, `keybind`, `keybind#2`, … |
| anything else | — | no key split: the file is carried **whole** in `share/`, byte for byte, the same on every platform |

`cfgsync adaptor` lists them; adding one costs a struct (see `AGENTS.md`).

`share/` is how lua scripts, a markdown doc, a lockfile or a binary blob still travel
between machines: `adopt.rules` decides whether the file is managed, and the
absence of an adapter only decides that it is synced as one unit instead of key
by key. `sync.rules` does not apply to those files — there are no key paths —
and the hash guard covers them like any other managed file.

A file the adapter cannot parse — JSONC with comments or a trailing comma, a
top-level JSON array, YAML with a tab indent or a second `---` document, a TOML
`"""` string, a JSONL line that is not JSON — is never rewritten: adoption still
copies it into the base byte for byte, and `render` / `harvest` refuse to touch
it with a warning until it is valid or its `adopt.rules` line is gone.

The line-preserving adapters (ini, plain, toml, yaml) address a *value as text*,
not as a decoded scalar, which is what lets an untouched key keep its exact
bytes. A block scalar or a multi-line array is one value of its key; rewriting
that key re-indents the continuation lines, and every other line of the file is
copied through untouched.

ini-ish values are single-line `key = value` / `key: value`; sections are flat
`[name]` headers. An overlay deletes a base key with the `@@DEL@@` sentinel,
which `harvest` writes for you.

A JSON merge that changes nothing keeps the base's own bytes, so an adopted
(verbatim) base is a fixed point: harvest, render and a second harvest all
produce the same file.

### JSON list patches

Arrays are not replaced wholesale by default — a base list that grows must not
wipe this machine's additions. An overlay can express a delta:

```json
{
  "packages": { "$remove": ["npm:old"], "$append": ["npm:new"] },
  "tools":    { "$prepend": ["+first"] },
  "pinned":   { "$replace": ["only-this"] },
  "obsolete": null
}
```

`$replace` resets the list first, then `$remove`, `$prepend` and `$append` apply in
that order. `harvest` writes the smallest `{$remove, $append}` pair that
reproduces an edited list; when the edit cannot be expressed that way (reordering
or duplicates) it falls back to storing the full list. Nested objects merge
recursively, `null` deletes.

## `push` and `pull`

`push` commits the central repo when it is dirty (default message
`chore: sync <utc timestamp>`) and pushes, adding `-u origin HEAD` when there is
no upstream yet.

`pull` commits local changes first and then pulls with `--rebase --autostash`.
On a central repo that has no commit yet — the state `init` leaves a fresh
machine in — it fetches `origin`, follows the remote's default branch and
checks it out, which is how a second machine gets the first one's work. It never
resolves conflicts and never force-pushes: a conflicting `pull` stops, names the
conflict, and leaves `git rebase --continue` to you.

`pull` only updates the central repo. Run `cfgsync render` afterwards to write
the new base into a source repo — that is deliberate, because a render you did
not ask for would have to decide what to do with your unharvested edits.

## Limitations

- Adapters cover JSON, JSONL, TOML, YAML and the ini family. Each one is
  deliberately line-preserving or (for JSON) a document merge, so a syntax it
  cannot read is refused rather than rewritten.
- The line-preserving adapters keep values as text: a key whose value is a
  nested container of its own is managed as one opaque block, and ini stays flat
  `[section]` + `key = value` with no multi-line values.
- JSON list patching is append/remove based; a reorder on one machine becomes a
  full-list value in the base.
- Overlays are keyed by platform, so two machines on the same platform share
  one overlay.
- Two source repos with the same path on different machines share one central
  directory only if the path is identical; a different home directory means a
  second directory for the same app.

## Development

```sh
cargo test        # unit tests + end-to-end tests in tests/cli.rs
```

`tests/cli.rs` drives the built binary in throwaway temp directories and needs
`git` on `PATH`. No network, nothing outside `std::env::temp_dir()`.

License: MIT.

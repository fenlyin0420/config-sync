//! Engine: discovery, the harvest/render plan, status, add and edit.
//!
//! A *source repo* is a real directory of an app's config files. The engine
//! reads and writes it, and keeps everything sync-side in the central repo:
//!
//!   <central>/<name>/path.rules                      where it lives per platform
//!   <central>/<name>/adopt.rules                     which files are managed
//!   <central>/<name>/sync.rules                      which keys are machine-owned
//!   <central>/<name>/base/<stem>.base.<ext>          shared baseline
//!   <central>/<name>/platform/<os>/<stem>.overlay.<ext>
//!                                                    this machine's delta
//!
//! `harvest` splits what the source repo holds into base + overlay; `render`
//! merges the two back into the source repo. Nothing is generated behind the
//! app's back and nothing is gitignored: the source repo is a plain checkout
//! whose own history records every harvest and render.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::adopt;
use crate::central::{self, Repo};
use crate::formats::{self, ext_of, Adapter, Kind, Op};
use crate::fsx::{read, rel, walk, write};
use crate::paths;
use crate::rules::{self, Route};

pub const RULES_TEMPLATE: &str = "\
# sync.rules -- 本机专属的配置项
# 命中这里的差异 -> platform/<os>/<file>.overlay.<ext>（只有本机生效）
# 未命中的差异   -> base/<file>.base.<ext>（回写共享基准，会同步到别的机器）
# 行首 '!' 的差异 -> 哪里都不去：不进 base、不进 overlay，也不提交到中心仓库，
#                    渲染时保留本机自己的值（密钥、机器专属字段就放这里）
# 语法：一行一个 glob；'#' 注释；'[section]' 行会被忽略
#   ini  : section.key   例如 core.theme / platform.* / limits.*
#   json : a.b.c 点路径  例如 defaultTools / compaction.keepRecentTokens
#   file : 文件glob:键路径 例如 settings.json:defaultTools（任意目录下的同名文件）
#                        /agent/settings.json:defaultTools（'/' 开头锚定到仓库根）
#                        **/settings.json:defaultTools（'**' 跨目录）
#   命中父级即可覆盖整个子树：'!secrets' 排除对象/整个 section 下的所有键。
# 示例（去掉行首的 '#' 即生效）：
# platform.*
# settings.json:defaultTools
# /agent/settings.json:defaultTools
# /agent/settings.json:!apiKey
";

/// how many "looks like config but nothing claims it" lines one harvest prints
const WARN_LIMIT: usize = 5;

/// how many candidates `add` lists in the adopt.rules it writes
const CANDIDATE_LIMIT: usize = 40;

// ---------------------------------------------------------------- discovery

/// One managed file: its path relative to the source repo root, plus the three
/// places it can live.
#[derive(Clone)]
pub struct Item {
    /// `ui/key.json` -- what adopt.rules and sync.rules match against
    pub rel: String,
    /// the adapter that splits it; `None` for a file no adapter can read, which
    /// is carried verbatim in `share/` and shared by every platform
    pub ad: Option<&'static dyn Adapter>,
    pub base: PathBuf,
    pub overlay: PathBuf,
    /// `share/<rel>`: the verbatim copy of a file with no adapter
    pub copy: PathBuf,
    pub target: PathBuf,
    pub ext: String,
}

/// `<repo>/base/ui/key.base.json` style paths, for logs
fn central_rel(repo: &Repo, path: &Path) -> String {
    rel(&repo.dir, path)
}

fn join(dir: Option<&str>, name: String) -> String {
    match dir {
        Some(d) => format!("{d}/{name}"),
        None => name,
    }
}

/// `ui/key.json` -> `ui/key.base.json`; a file with no extension keeps its name
/// and grows the tag: `agent/config` -> `agent/config.base`
fn insert_tag(path: &str, tag: &str) -> Option<String> {
    let (dir, name) = match path.rsplit_once('/') {
        Some((d, n)) => (Some(d), n),
        None => (None, path),
    };
    let ext = ext_of(name);
    let stem = match ext.is_empty() {
        true => name,
        false => &name[..name.len() - ext.len() - 1],
    };
    if stem.is_empty() {
        return None;
    }
    let made = match ext.is_empty() {
        true => format!("{stem}.{tag}"),
        false => format!("{stem}.{tag}.{ext}"),
    };
    Some(join(dir, made))
}

/// `ui/key.base.json` -> `ui/key.json`; the other direction
fn strip_tag(path: &str, tag: &str) -> Option<String> {
    let (dir, name) = match path.rsplit_once('/') {
        Some((d, n)) => (Some(d), n),
        None => (None, path),
    };
    let suffix = format!(".{tag}");
    // extensionless: `agent/config.base` -> `agent/config`
    if let Some(stem) = name.strip_suffix(&suffix) {
        if !stem.is_empty() {
            return Some(join(dir, stem.to_string()));
        }
    }
    let ext = ext_of(name);
    if ext.is_empty() {
        return None;
    }
    let stem = name[..name.len() - ext.len() - 1].strip_suffix(&suffix)?;
    if stem.is_empty() {
        return None;
    }
    Some(join(dir, format!("{stem}.{ext}")))
}

fn item(repo: &Repo, os: &str, path: &str) -> Option<Item> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let ext = ext_of(name).to_string();
    let copy = repo.dir.join("share").join(path);
    let ad = formats::for_file(name);
    let (base, overlay) = match ad.is_some() {
        true => (
            repo.dir.join("base").join(insert_tag(path, "base")?),
            repo.dir
                .join("platform")
                .join(os)
                .join(insert_tag(path, "overlay")?),
        ),
        false => (PathBuf::new(), PathBuf::new()),
    };
    Some(Item {
        rel: path.to_string(),
        ad,
        base,
        overlay,
        copy,
        target: repo.source.join(path),
        ext,
    })
}

/// Relative paths of the committed base files, `ui/key.base.json` included.
fn base_paths(repo: &Repo) -> Vec<String> {
    walk(&repo.dir.join("base"))
}

/// Relative paths of the verbatim copies, i.e. the files with no adapter.
fn share_paths(repo: &Repo) -> Vec<String> {
    walk(&repo.dir.join("share"))
}

/// Every file `adopt.rules` claims, whether or not the central copy (or the
/// file itself) exists yet: the rule table decides what is managed. The base
/// and share copies are walked too because a fresh machine only has those --
/// the source files appear when `render` writes them.
pub fn discover(repo: &Repo, os: &str) -> Vec<Item> {
    let pats = adopt::load(&repo.adopt_rules());
    if pats.is_empty() {
        return Vec::new();
    }
    let mut paths: Vec<String> = walk(&repo.source)
        .into_iter()
        .filter(|path| adopt::adopts(&pats, path))
        .collect();
    for target in base_paths(repo)
        .into_iter()
        .filter_map(|path| strip_tag(&path, "base"))
    {
        if adopt::adopts(&pats, &target) && !paths.contains(&target) {
            paths.push(target);
        }
    }
    for path in share_paths(repo) {
        if adopt::adopts(&pats, &path) && !paths.contains(&path) {
            paths.push(path);
        }
    }
    paths.sort();
    paths.dedup();
    paths.iter().filter_map(|path| item(repo, os, path)).collect()
}

/// Config-looking files of the source repo that no adopt.rules line claims.
pub fn unmanaged(repo: &Repo) -> Vec<String> {
    let pats = adopt::load(&repo.adopt_rules());
    walk(&repo.source)
        .into_iter()
        .filter(|path| {
            let name = path.rsplit('/').next().unwrap_or(path);
            formats::for_file(name).is_some() && !adopt::adopts(&pats, path)
        })
        .collect()
}

fn warn_unmanaged(repo: &Repo) {
    let list = unmanaged(repo);
    if list.is_empty() {
        return;
    }
    let rules = format!("{}/{}", repo.name, adopt::NAME);
    for path in list.iter().take(WARN_LIMIT) {
        eprintln!(
            "[sync] warning: {path} looks like config but no adopt.rules line claims it \
             -> add \"{path}\" to {rules} to sync it"
        );
    }
    if list.len() > WARN_LIMIT {
        eprintln!(
            "[sync] warning: ... and {} more unclaimed config file(s) in {}",
            list.len() - WARN_LIMIT,
            repo.source.display()
        );
    }
}

// ---------------------------------------------------------------- hashing

/// Fingerprint of what the source repo holds right now: relative path plus
/// bytes of every managed file that exists.
///
/// `harvest` and `render` record it in `<central>/<name>/.hash`; `render`
/// refuses to write when the fingerprint no longer matches, because the
/// difference is exactly the content nobody harvested yet. A managed file that
/// is not on disk contributes nothing, so pulling a base for a file this
/// machine never had does not invalidate the record. A file with no adapter is
/// hashed as raw bytes, because it is copied verbatim.
pub fn content_hash(items: &[Item]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for it in items {
        if !it.target.is_file() {
            continue;
        }
        fnv_feed(&mut h, it.rel.as_bytes());
        fnv_feed(&mut h, b"\0");
        match it.ad {
            Some(_) => fnv_feed(&mut h, read(&it.target).as_bytes()),
            None => fnv_feed(&mut h, &std::fs::read(&it.target).unwrap_or_default()),
        }
        fnv_feed(&mut h, b"\n");
    }
    h
}

fn fnv_feed(h: &mut u64, bytes: &[u8]) {
    for b in bytes {
        *h ^= u64::from(*b);
        *h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

// ---------------------------------------------------------------- plan

/// overlay paths sync.rules does not claim -- `sync.rules` decides where every
/// one of them belongs (base, or nowhere for a `!` line)
pub fn stale_overlay(pats: &[String], it: &Item, ad: &dyn Adapter) -> Vec<String> {
    ad.overlay_ops(&read(&it.overlay))
        .into_iter()
        .filter(|op| rules::route(pats, &it.rel, &op.path) != Route::Owned)
        .map(|op| op.path)
        .collect()
}

/// shared -> base, owned -> overlay, vetoed -> nowhere
fn split_ops(pats: &[String], it: &Item, ops: Vec<Op>) -> (Vec<Op>, Vec<Op>, Vec<Op>) {
    let mut shared = Vec::new();
    let mut local = Vec::new();
    let mut vetoed = Vec::new();
    for op in ops {
        match rules::route(pats, &it.rel, &op.path) {
            Route::Owned => local.push(op),
            Route::Excluded => vetoed.push(op),
            Route::Shared => shared.push(op),
        }
    }
    (shared, local, vetoed)
}

/// A key a `!` line excludes is in neither base nor overlay, so it can only
/// come from the file the app already has: move `current` to `merged` with the
/// excluded paths left out of the op list, which keeps them untouched and in
/// place (a fresh file that is not there yet gets `merged` as is).
fn keep_local(pats: &[String], it: &Item, ad: &dyn Adapter, merged: &str, current: &str) -> String {
    if current.trim().is_empty() {
        return merged.to_string();
    }
    let ops: Vec<Op> = ad
        .diff(current, merged)
        .into_iter()
        .filter(|op| !rules::is_excluded(pats, &it.rel, &op.path))
        .collect();
    match ops.is_empty() {
        true => current.to_string(),
        false => ad.apply(current, &ops),
    }
}

/// One file's reconciliation: what the base, the overlay and the source file
/// should hold after harvesting `current`.
struct Plan {
    shared: Vec<Op>,
    local: Vec<Op>,
    pins: Vec<Op>,
    stale: Vec<String>,
    /// keys a `!` line keeps out of the central repo entirely
    veto: Vec<String>,
    /// new base bytes, when the base changes
    base: Option<String>,
    /// new overlay bytes; `None` when nothing is machine-owned yet and the file
    /// does not exist, so a file that has no delta here gets no overlay at all
    overlay: Option<String>,
    target: String,
}

fn plan(
    pats: &[String],
    it: &Item,
    ad: &dyn Adapter,
    os: &str,
    current: &str,
    adopting: bool,
) -> Plan {
    let base_text = read(&it.base);
    // adopting: the app's current content *is* the new base, so that is where
    // machine-owned keys are pinned from
    let pin_source = if adopting {
        current.to_string()
    } else {
        base_text.clone()
    };
    let (shared, mut local, vetoed) = split_ops(pats, it, ad.diff(&base_text, current));
    // a key the table owns belongs in this machine's overlay even when its value
    // still equals base -- otherwise "machine owned" only takes effect after the
    // value diverges, and a later base edit leaks in
    let owned = |path: &str| rules::route(pats, &it.rel, path) == Route::Owned;
    let veto = |path: &str| rules::is_excluded(pats, &it.rel, path);
    let taken: HashSet<String> = local.iter().map(|op| op.path.clone()).collect();
    let pins: Vec<Op> = ad
        .pin_ops(&pin_source, &owned)
        .into_iter()
        .filter(|op| !taken.contains(&op.path))
        .collect();
    local.extend(pins.iter().cloned());
    let overlay = ad.dump_overlay(&local, os);
    // the base never keeps a vetoed key, adoption included -- otherwise
    // `!apiKey` would still commit the value to the central repo
    let base_src = if adopting {
        current.to_string()
    } else {
        base_text.clone()
    };
    let drops = ad.drop_ops(&base_src, &veto);
    let base = if adopting {
        Some(ad.apply(&base_src, &drops))
    } else if shared.is_empty() && drops.is_empty() {
        None
    } else {
        let mut ops = shared.clone();
        ops.extend(drops.iter().cloned());
        Some(ad.apply(&base_text, &ops))
    };
    let target = if adopting {
        current.to_string()
    } else {
        let merged = ad.apply(
            base.as_deref().unwrap_or(&base_text),
            &ad.overlay_ops(&overlay),
        );
        keep_local(pats, it, ad, &merged, current)
    };
    let kept: HashSet<&str> = local.iter().map(|op| op.path.as_str()).collect();
    let stale = stale_overlay(pats, it, ad)
        .into_iter()
        .filter(|p| !kept.contains(p.as_str()))
        .collect();
    // an overlay that carries nothing is not created; one that exists is
    // rewritten even when it empties out, because dropping it is the prune
    let overlay = (!local.is_empty() || it.overlay.is_file()).then_some(overlay);
    let mut veto: Vec<String> = drops.iter().map(|op| op.path.clone()).collect();
    veto.extend(vetoed.iter().map(|op| op.path.clone()));
    veto.sort();
    veto.dedup();
    Plan {
        shared,
        local,
        pins,
        stale,
        veto,
        base,
        overlay,
        target,
    }
}

fn py_repr(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => "None".to_string(),
        serde_json::Value::Bool(true) => "True".to_string(),
        serde_json::Value::Bool(false) => "False".to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => format!("'{s}'"),
        serde_json::Value::Array(items) => {
            let inner: Vec<String> = items.iter().map(py_repr).collect();
            format!("[{}]", inner.join(", "))
        }
        serde_json::Value::Object(map) => {
            let inner: Vec<String> = map
                .iter()
                .map(|(k, v)| format!("'{k}': {}", py_repr(v)))
                .collect();
            format!("{{{}}}", inner.join(", "))
        }
    }
}

fn describe(op: &Op) -> String {
    match op.kind {
        Kind::Delete => return "<delete>".to_string(),
        Kind::Patch => return "<patch>".to_string(),
        Kind::Set => {}
    }
    let s = match &op.data {
        formats::OpData::Ini { value, .. } => value.clone().unwrap_or_default(),
        formats::OpData::Text { value, .. } => value.clone().unwrap_or_default(),
        formats::OpData::Json { value, .. } => match value {
            serde_json::Value::String(s) => s.clone(),
            other => py_repr(other),
        },
    };
    let chars: Vec<char> = s.chars().collect();
    let shown = if chars.len() <= 40 {
        s
    } else {
        format!("{}...", chars[..37].iter().collect::<String>())
    };
    format!("= {shown}")
}

fn report(p: &Plan) {
    for op in &p.shared {
        println!("            base : {:<30} {}", op.path, describe(op));
    }
    for op in p
        .local
        .iter()
        .filter(|op| !p.pins.iter().any(|pin| pin.path == op.path))
    {
        println!("            local: {:<30} {}", op.path, describe(op));
    }
    for op in &p.pins {
        println!("            pin  : {:<30} {}", op.path, describe(op));
    }
    for path in &p.stale {
        println!("            prune: {path:<30} overlay dropped it (not machine-owned)");
    }
    for path in &p.veto {
        println!("            keep : {path:<30} local only (sync.rules '!')");
    }
}

fn warn_stale(repo: &Repo, pats: &[String], it: &Item, ad: &dyn Adapter, overlay: &str) {
    let (mut shared, mut vetoed) = (Vec::new(), Vec::new());
    for op in ad.overlay_ops(overlay) {
        match rules::route(pats, &it.rel, &op.path) {
            Route::Owned => {}
            Route::Excluded => vetoed.push(op.path),
            Route::Shared => shared.push(op.path),
        }
    }
    if !shared.is_empty() {
        eprintln!(
            "[sync] warning: {} holds {}, which sync.rules does not claim -> \
             they belong in the base file (next harvest moves them)",
            central_rel(repo, &it.overlay),
            shared.join(", ")
        );
    }
    if !vetoed.is_empty() {
        eprintln!(
            "[sync] warning: {} holds {}, which a sync.rules '!' line keeps out of \
             the central repo -> the next harvest drops them (the app's own value stays)",
            central_rel(repo, &it.overlay),
            vetoed.join(", ")
        );
    }
}

// ---------------------------------------------------------------- render

pub fn render(repo: &Repo, os: &str, dry: bool, force: bool) -> i32 {
    println!("[render] {}  ({})", repo.source.display(), repo.name);
    let items = discover(repo, os);
    let recorded = central::read_hash(&repo.dir);
    if !force {
        // nothing recorded -> first render into this source repo, decided per
        // file below; a record that no longer matches means unharvested content
        if let Some(h) = recorded {
            if content_hash(&items) != h {
                eprintln!(
                    "[render] {} changed since the last harvest -> refusing to write; \
                     run `cfgsync harvest` first, or `cfgsync render --force` to overwrite",
                    repo.source.display()
                );
                return 1;
            }
        }
    }
    let pats = rules::load(&repo.sync_rules());
    let mut written = 0usize;
    let mut kept = 0usize;
    let mut blocked = false;
    for it in &items {
        let Some(ad) = it.ad else {
            if !it.copy.is_file() {
                eprintln!(
                    "[sync] warning: {} is claimed by adopt.rules but has no share copy yet \
                     -> run `cfgsync harvest` to adopt it",
                    it.rel
                );
                continue;
            }
            let want = std::fs::read(&it.copy).unwrap_or_default();
            let have = std::fs::read(&it.target).unwrap_or_default();
            let present = it.target.is_file();
            if present && have == want {
                kept += 1;
                continue;
            }
            if !force && recorded.is_none() && present && !have.is_empty() {
                eprintln!(
                    "[sync] warning: {} differs from share/{} and there is no harvest record \
                     -> left alone; run `cfgsync harvest` to keep it, or \
                     `cfgsync render --force` to overwrite it",
                    it.rel, it.rel
                );
                blocked = true;
                continue;
            }
            println!("[render] {}  <= share/{}", it.rel, it.rel);
            written += 1;
            if !dry {
                if let Some(dir) = it.target.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                if let Err(e) = std::fs::copy(&it.copy, &it.target) {
                    eprintln!("[sync] warning: cannot write {}: {e}", it.rel);
                }
            }
            continue;
        };
        if !it.base.is_file() {
            eprintln!(
                "[sync] warning: {} is claimed by adopt.rules but has no base file yet \
                 -> run `cfgsync harvest` to adopt it",
                it.rel
            );
            continue;
        }
        let base_text = read(&it.base);
        if !ad.parses(&base_text) {
            eprintln!(
                "[sync] warning: {} is not valid for the .{} adapter -> leaving {} alone \
                 until it is (or drop its adopt.rules line)",
                central_rel(repo, &it.base),
                it.ext,
                it.rel
            );
            continue;
        }
        let overlay_text = read(&it.overlay);
        if !ad.parses(&overlay_text) {
            eprintln!(
                "[sync] warning: {} is not valid for the .{} adapter (unresolved conflict?) \
                 -> refusing to rewrite {}",
                central_rel(repo, &it.overlay),
                it.ext,
                it.rel
            );
            continue;
        }
        let actual = read(&it.target);
        if !actual.trim().is_empty() && !ad.parses(&actual) {
            eprintln!(
                "[sync] warning: {} is not valid for the .{} adapter -> refusing to rewrite it \
                 (fix the file, or drop its adopt.rules line)",
                it.rel, it.ext
            );
            continue;
        }
        warn_stale(repo, &pats, it, ad, &overlay_text);
        let merged = ad.apply(&base_text, &ad.overlay_ops(&overlay_text));
        let text = keep_local(&pats, it, ad, &merged, &actual);
        if actual == text {
            kept += 1;
            continue;
        }
        // we never wrote this file and it holds something the central repo does
        // not: a hand edit and a stale copy look identical, and throwing either
        // away is the user's call
        if !force && recorded.is_none() && !actual.trim().is_empty() {
            eprintln!(
                "[sync] warning: {} differs from what base + overlay would write and there is \
                 no harvest record -> left alone; run `cfgsync harvest` to keep it, or \
                 `cfgsync render --force` to overwrite it",
                it.rel
            );
            blocked = true;
            continue;
        }
        println!("[render] {}  <= base + overlay", it.rel);
        written += 1;
        if !dry {
            write(&it.target, &text);
        }
    }
    if dry {
        println!(
            "[render] {}: {written} to write, {kept} unchanged",
            repo.source.display()
        );
        return 0;
    }
    if !blocked {
        central::write_hash(&repo.dir, content_hash(&items));
    }
    if written > 0 {
        match central::commit_all(&repo.source, "cfgsync: render") {
            Ok(true) => println!("[render] committed in the source repo"),
            Ok(false) => {}
            Err(e) => eprintln!("[sync] warning: could not commit the source repo: {e}"),
        }
    }
    println!(
        "[render] {}: {written} written, {kept} unchanged",
        repo.source.display()
    );
    0
}

// ---------------------------------------------------------------- harvest

/// A file no adapter can read is carried whole: `share/<rel>` is one copy that
/// every platform gets. Returns (changed, newly adopted).
fn harvest_shared(repo: &Repo, it: &Item, dry: bool) -> (bool, bool) {
    let _ = repo;
    if !it.target.is_file() {
        match it.copy.is_file() {
            true => eprintln!(
                "[harvest] {}: not in the source repo yet, run `cfgsync render` first",
                it.rel
            ),
            false => eprintln!(
                "[harvest] {}: claimed by adopt.rules but not on disk yet -- nothing to adopt",
                it.rel
            ),
        }
        return (false, false);
    }
    let source = std::fs::read(&it.target).unwrap_or_default();
    let adopting = !it.copy.is_file();
    if !adopting && std::fs::read(&it.copy).unwrap_or_default() == source {
        println!("[harvest] {}  <-  no changes", it.rel);
        return (false, false);
    }
    println!(
        "[harvest] {}: {} -> share/{}",
        it.rel,
        match adopting {
            true => "carried whole (no adapter)",
            false => "copied",
        },
        it.rel
    );
    if !dry {
        if let Some(dir) = it.copy.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = std::fs::copy(&it.target, &it.copy) {
            eprintln!("[sync] warning: cannot copy {}: {e}", it.rel);
            return (false, false);
        }
    }
    (true, adopting)
}

pub fn harvest(repo: &Repo, os: &str, dry: bool) -> i32 {
    println!("[harvest] {}  ({})", repo.source.display(), repo.name);
    if !repo.adopt_rules().is_file() {
        eprintln!(
            "[harvest] warning: {} is missing -> nothing is managed; \
             `cfgsync add {}` writes it",
            format!("{}/{}", repo.name, adopt::NAME),
            repo.source.display()
        );
        return 0;
    }
    let pats = rules::load(&repo.sync_rules());
    if pats.is_empty() {
        eprintln!(
            "[harvest] warning: sync.rules is missing or empty -> \
             every change goes to the base file"
        );
    }
    warn_unmanaged(repo);
    let mut harvested = 0usize;
    let mut adopted = 0usize;
    for it in &discover(repo, os) {
        let Some(ad) = it.ad else {
            let (changed, newly) = harvest_shared(repo, it, dry);
            harvested += usize::from(changed);
            adopted += usize::from(newly);
            continue;
        };
        let current = read(&it.target);
        // no base file yet: this is the step that turns a claimed file into a
        // sync file, so what the source repo holds becomes the base
        let adopting = !it.base.is_file();
        if adopting {
            if !it.target.is_file() {
                eprintln!(
                    "[harvest] {}: claimed by adopt.rules but not on disk yet -- \
                     nothing to adopt",
                    it.rel
                );
                continue;
            }
            if current.trim().is_empty() {
                eprintln!(
                    "[harvest] {}: is empty -> nothing to adopt (fill it in, or drop \
                     its adopt.rules line)",
                    it.rel
                );
                continue;
            }
        } else {
            if !ad.parses(&read(&it.base)) {
                eprintln!(
                    "[sync] warning: {} is not valid for the .{} adapter -> nothing \
                     harvested from {}",
                    central_rel(repo, &it.base),
                    it.ext,
                    it.rel
                );
                continue;
            }
            if !ad.parses(&read(&it.overlay)) {
                eprintln!(
                    "[sync] warning: {} is not valid for the .{} adapter (unresolved \
                     conflict?) -> refusing to harvest {}",
                    central_rel(repo, &it.overlay),
                    it.ext,
                    it.rel
                );
                continue;
            }
            if !it.target.is_file() {
                eprintln!("[harvest] {}: not in the source repo yet, run `cfgsync render` first", it.rel);
                continue;
            }
            if current.trim().is_empty() {
                eprintln!(
                    "[harvest] {}: is empty -> refusing to harvest it (run `cfgsync render` \
                     to rebuild it)",
                    it.rel
                );
                continue;
            }
            if !ad.parses(&current) {
                eprintln!(
                    "[sync] warning: {} is not valid for the .{} adapter -> refusing to \
                     harvest it",
                    it.rel, it.ext
                );
                continue;
            }
        }
        let p = plan(&pats, it, ad, os, &current, adopting);
        let overlay_same = match &p.overlay {
            Some(text) => *text == read(&it.overlay),
            None => !it.overlay.is_file(),
        };
        let unchanged = !adopting && p.base.is_none() && overlay_same && p.target == current;
        if unchanged {
            println!("[harvest] {}  <-  no changes", it.rel);
            continue;
        }
        if adopting {
            println!(
                "[harvest] {}: adopted -> base, {} machine-owned -> platform/{os}",
                it.rel,
                p.local.len()
            );
        } else {
            println!(
                "[harvest] {}: {} shared -> base, {} machine-owned -> platform/{os}",
                it.rel,
                p.shared.len(),
                p.local.len()
            );
        }
        report(&p);
        harvested += 1;
        if adopting {
            adopted += 1;
        }
        if dry {
            continue;
        }
        if let Some(base) = &p.base {
            write(&it.base, base);
        }
        if let Some(overlay) = &p.overlay {
            write(&it.overlay, overlay);
        }
        if p.target != current {
            write(&it.target, &p.target);
        }
    }
    if dry {
        println!(
            "[harvest] {}: {harvested} to harvest",
            repo.source.display()
        );
        return 0;
    }
    central::write_hash(&repo.dir, content_hash(&discover(repo, os)));
    match central::commit_all(&repo.source, "cfgsync: harvest") {
        Ok(true) => println!("[harvest] committed in the source repo"),
        Ok(false) => {}
        Err(e) => eprintln!("[sync] warning: could not commit the source repo: {e}"),
    }
    println!(
        "[harvest] {}: {harvested} harvested ({adopted} adopted)",
        repo.source.display()
    );
    0
}

// ---------------------------------------------------------------- status

pub fn status(repo: &Repo, os: &str) -> i32 {
    println!("[status] {}  ({})", repo.source.display(), repo.name);
    let items = discover(repo, os);
    let pats = rules::load(&repo.sync_rules());
    let mut changed = 0usize;
    for it in &items {
        let Some(ad) = it.ad else {
            if !it.copy.is_file() {
                println!(
                    "[status]   ?  {}  (claimed by adopt.rules, no share copy yet -- \
                     `cfgsync harvest` adopts it)",
                    it.rel
                );
                changed += 1;
            } else if !it.target.is_file() {
                println!(
                    "[status]   +  {}  (not on disk -- `cfgsync render` writes it)",
                    it.rel
                );
                changed += 1;
            } else if std::fs::read(&it.target).unwrap_or_default()
                != std::fs::read(&it.copy).unwrap_or_default()
            {
                println!("[status]   M  {}  (differs from share/{})", it.rel, it.rel);
                changed += 1;
            }
            continue;
        };
        if !it.base.is_file() {
            println!(
                "[status]   ?  {}  (claimed by adopt.rules, no base yet -- \
                 `cfgsync harvest` adopts it)",
                it.rel
            );
            changed += 1;
            continue;
        }
        let base_text = read(&it.base);
        let overlay_text = read(&it.overlay);
        let merged = ad.apply(&base_text, &ad.overlay_ops(&overlay_text));
        let expected = keep_local(&pats, it, ad, &merged, &read(&it.target));
        if !it.target.is_file() {
            println!(
                "[status]   +  {}  (not on disk -- `cfgsync render` writes it)",
                it.rel
            );
            changed += 1;
            continue;
        }
        let actual = read(&it.target);
        if actual != expected {
            let (shared, local, _) = split_ops(&pats, it, ad.diff(&base_text, &actual));
            println!(
                "[status]   M  {}  ({} to the base, {} to this machine)",
                it.rel,
                shared.len(),
                local.len()
            );
            changed += 1;
        }
    }
    if let Some(recorded) = central::read_hash(&repo.dir) {
        if recorded != content_hash(&items) {
            println!(
                "[status]   render would refuse: content changed since the last harvest \
                 -> `cfgsync harvest`"
            );
        }
    }
    match changed {
        0 => println!("[status] clean"),
        n => println!("[status] {n} file(s) differ from the central copy"),
    }
    0
}

// ---------------------------------------------------------------- add / edit

pub fn add(root: &Path, arg: Option<&str>, wanted: Option<&str>, os: &str, dry: bool) -> i32 {
    let raw = arg.unwrap_or(".");
    let owned = Path::new(raw);
    let absolute = if owned.is_absolute() {
        owned.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|c| c.join(owned))
            .unwrap_or_else(|_| owned.to_path_buf())
    };
    let Some(source) = central::canonical(&absolute) else {
        if absolute.exists() {
            eprintln!(
                "[add] only directories are managed: {} -- add the directory that holds it, \
                 then list the file there in adopt.rules",
                absolute.display()
            );
        } else {
            eprintln!("[add] no such path: {raw}");
        }
        return 2;
    };
    if !root.is_dir() {
        eprintln!(
            "[add] no central repo at {} -- run `cfgsync init` first",
            root.display()
        );
        return 2;
    }
    let wanted = match wanted {
        Some(name) => name.to_string(),
        None => central::suggest_name(&source),
    };
    if !central::valid_name(&wanted) {
        eprintln!(
            "[add] not a usable central name: {wanted} -- one directory name, \
             letters/digits/'.'/'_'/'-', not starting with '.'"
        );
        return 2;
    }
    let managed = central::entry_for_path(root, &source, os);
    let name = match &managed {
        Some(entry) => entry.name.clone(),
        None => match central::claim(root, &source, os, &wanted) {
            Ok(name) => name,
            Err(problem) => {
                eprintln!("[add] {problem}");
                return 1;
            }
        },
    };
    if managed.is_some() && name != wanted {
        println!("[add] already managed as {name}");
    } else if managed.is_none() && name != wanted {
        println!("[add] {wanted} already stands for another directory -> using {name}");
    }
    let dir = root.join(&name);
    println!("[add] {} -> {}", source.display(), dir.display());
    if let Some(parent) = source.parent().and_then(|p| central::entry_for_path(root, p, os)) {
        if parent.source() != Some(source.as_path()) {
            eprintln!(
                "[sync] warning: {} is inside the managed repo {} -> both will manage it",
                source.display(),
                parent.source().unwrap_or(&parent.dir).display()
            );
        }
    }
    let line = format!("{os} = {}", paths::compact(&source));
    if !dry {
        paths::set(&dir, os, &source);
    }
    println!("[add] {name}/{}: {line}", paths::NAME);
    let adopt_rules = dir.join(adopt::NAME);
    let sync_rules = dir.join(rules::NAME);
    if adopt_rules.is_file() && sync_rules.is_file() {
        println!("[add] already in the central repo: adopt.rules, sync.rules kept");
    } else {
        if !adopt_rules.is_file() {
            println!("[add] {} written", format!("{name}/{}", adopt::NAME));
            if !dry {
                write(&adopt_rules, &adopt::template(&candidates(&source)));
            }
        }
        if !sync_rules.is_file() {
            println!("[add] {} written", format!("{name}/{}", rules::NAME));
            if !dry {
                write(&sync_rules, RULES_TEMPLATE);
            }
        }
    }
    if !dry {
        for sub in ["base", &format!("platform/{os}")] {
            let _ = std::fs::create_dir_all(dir.join(sub));
        }
    }
    if !source.join(".git").exists() {
        if dry {
            println!("[add] would git init {}", source.display());
        } else {
            match Command::new("git")
                .arg("-C")
                .arg(&source)
                .args(["init", "-q"])
                .output()
            {
                Ok(o) if o.status.success() => println!("[add] git init {}", source.display()),
                Ok(o) => {
                    eprintln!("[add] git init failed: {}", central::git_err(&o));
                    return 1;
                }
                Err(e) => {
                    eprintln!("[add] cannot run git: {e}");
                    return 1;
                }
            }
        }
    }
    println!(
        "[add] next: `cfgsync edit -a` to list the files, then `cfgsync harvest`"
    );
    0
}

/// Config-looking files of a directory, for the adopt.rules template.
fn candidates(source: &Path) -> Vec<String> {
    let mut found: Vec<String> = walk(source)
        .into_iter()
        .filter(|path| formats::for_file(path.rsplit('/').next().unwrap_or(path)).is_some())
        .collect();
    found.truncate(CANDIDATE_LIMIT);
    found
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RulesFile {
    Adopt,
    Sync,
    Path,
}

pub fn edit(repo: &Repo, which: RulesFile) -> i32 {
    let path = match which {
        RulesFile::Adopt => repo.adopt_rules(),
        RulesFile::Sync => repo.sync_rules(),
        RulesFile::Path => repo.path_rules(),
    };
    if !path.is_file() {
        let text = match which {
            RulesFile::Adopt => adopt::template(&candidates(&repo.source)),
            RulesFile::Sync => RULES_TEMPLATE.to_string(),
            RulesFile::Path => paths::template(),
        };
        write(&path, &text);
        println!("[edit] created {}", path.display());
    }
    let editor = editor_command();
    let mut parts = editor.split_whitespace();
    let Some(binary) = parts.next() else {
        eprintln!("[edit] no editor configured -- set $EDITOR");
        return 2;
    };
    println!("[edit] {} ({editor})", path.display());
    match Command::new(binary).args(parts).arg(&path).status() {
        Ok(status) if status.success() => 0,
        Ok(_) => 1,
        Err(e) => {
            eprintln!("[edit] cannot run {editor}: {e} (set $EDITOR)");
            2
        }
    }
}

/// `$VISUAL` > `$EDITOR` > the first vi the machine actually has. Both variables
/// are usually unset in a non-interactive shell, and the POSIX default `vi` is
/// plain vim on most systems, so probing `nvim` first is what a vi user expects.
fn editor_command() -> String {
    for var in ["VISUAL", "EDITOR"] {
        if let Ok(value) = std::env::var(var) {
            if !value.trim().is_empty() {
                return value;
            }
        }
    }
    for candidate in ["nvim", "vim", "vi"] {
        if on_path(candidate) {
            return candidate.to_string();
        }
    }
    match cfg!(windows) {
        true => "notepad".to_string(),
        false => "vi".to_string(),
    }
}

fn on_path(binary: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| {
        let candidate = dir.join(binary);
        match std::fs::metadata(&candidate) {
            Ok(meta) if meta.is_file() => executable(&candidate),
            _ => false,
        }
    })
}

#[cfg(unix)]
fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn executable(_path: &Path) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_go_before_the_extension() {
        assert_eq!(insert_tag("key.json", "base").unwrap(), "key.base.json");
        assert_eq!(
            insert_tag("ui/key.map.json", "overlay").unwrap(),
            "ui/key.map.overlay.json"
        );
        assert_eq!(strip_tag("ui/key.base.json", "base").unwrap(), "ui/key.json");
        assert_eq!(strip_tag("key.base.json", "base").unwrap(), "key.json");
        // a file with no extension keeps its name and grows the tag
        assert_eq!(insert_tag("agent/config", "base").unwrap(), "agent/config.base");
        assert_eq!(
            insert_tag("agent/config", "overlay").unwrap(),
            "agent/config.overlay"
        );
        assert_eq!(strip_tag("agent/config.base", "base").unwrap(), "agent/config");
        // and a file literally named `x.base` cannot be mistaken for it
        assert_eq!(insert_tag("x.base", "base").unwrap(), "x.base.base");
        assert_eq!(strip_tag("x.base.base", "base").unwrap(), "x.base");
        assert_eq!(strip_tag("key.json", "base"), None);
        assert_eq!(insert_tag(".hidden", "base"), None);
    }

    fn item_at(rel: &str) -> Item {
        Item {
            rel: rel.to_string(),
            ad: Some(formats::get("json").unwrap()),
            base: PathBuf::new(),
            overlay: PathBuf::new(),
            copy: PathBuf::new(),
            target: PathBuf::new(),
            ext: "json".to_string(),
        }
    }

    #[test]
    fn a_missing_target_does_not_change_the_hash() {
        let items = vec![item_at("a.json"), item_at("b.json")];
        let h = content_hash(&items);
        assert_eq!(h, content_hash(&items));
    }
}

//! Format adapters. One struct per extension family, four methods each;
//! the engine only ever reads `Op::path`.

pub mod ini;
pub mod json;
pub mod jsonl;
pub mod plain;
pub mod toml;
pub mod yaml;

use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Set,
    Delete,
    Patch,
}

/// Adapter-private payload. The engine must never look inside.
#[derive(Clone, Debug)]
pub enum OpData {
    Ini {
        section: String,
        key: String,
        value: Option<String>,
    },
    Json {
        path: Vec<String>,
        value: serde_json::Value,
    },
    /// the line-based adapters (yaml, toml) keep their values as text: `key` is
    /// what is written on the left of the value, `None` deletes
    Text {
        key: String,
        value: Option<String>,
    },
}

#[derive(Clone, Debug)]
pub struct Op {
    /// dotted key path, matched against sync.rules
    pub path: String,
    pub kind: Kind,
    pub data: OpData,
}

pub trait Adapter: Send + Sync {
    /// the adapter's name, for `cfgsync adaptor`
    fn name(&self) -> &'static str;
    fn overlay_ops(&self, text: &str) -> Vec<Op>;
    fn diff(&self, base: &str, cur: &str) -> Vec<Op>;
    fn apply(&self, base: &str, ops: &[Op]) -> String;
    fn dump_overlay(&self, ops: &[Op], os: &str) -> String;
    /// copy machine-owned base values out into the overlay, so adding a key to
    /// sync.rules actually pins this machine's copy instead of owning nothing
    fn pin_ops(&self, base: &str, owned: &dyn Fn(&str) -> bool) -> Vec<Op>;
    /// delete ops for every key of `base` the veto excludes -- how a key that
    /// sync.rules `!` lines keep out of the central repo leaves a base file
    fn drop_ops(&self, base: &str, veto: &dyn Fn(&str) -> bool) -> Vec<Op>;
    /// can this adapter read `text` at all? The engine uses it to refuse to
    /// rewrite a file it would only be able to read as empty.
    fn parses(&self, _text: &str) -> bool {
        true
    }
}

pub fn get(ext: &str) -> Option<&'static dyn Adapter> {
    let key = ext.trim_start_matches('.').to_ascii_lowercase();
    registry().get(key.as_str()).copied()
}

/// The adapter for one file of the source repo: by extension, and for a file
/// with no extension at all by its name (`ghostty`'s `config`). `None` means no
/// adapter can split this file, so it is carried verbatim in `share/`.
pub fn for_file(path: &str) -> Option<&'static dyn Adapter> {
    let name = path.rsplit('/').next().unwrap_or(path);
    match ext_of(name) {
        "" => by_name().get(name).copied(),
        ext => get(ext),
    }
}

/// `adapter -> the extensions and file names that select it`, sorted, for
/// `cfgsync adaptor`.
pub fn inventory() -> Vec<(&'static str, Vec<String>)> {
    let mut out: Vec<(&'static str, Vec<String>)> = Vec::new();
    for (ext, ad) in registry() {
        record(&mut out, ad.name(), format!("*.{ext}"));
    }
    for (file, ad) in by_name() {
        record(&mut out, ad.name(), format!("{file} (file name)"));
    }
    for (_, keys) in out.iter_mut() {
        keys.sort();
        keys.dedup();
    }
    out.sort_by_key(|(name, _)| *name);
    out
}

fn record(out: &mut Vec<(&'static str, Vec<String>)>, name: &'static str, key: String) {
    match out.iter_mut().find(|(n, _)| *n == name) {
        Some((_, keys)) => keys.push(key),
        None => out.push((name, vec![key])),
    }
}

fn by_name() -> &'static std::collections::HashMap<&'static str, &'static dyn Adapter> {
    static NAMES: OnceLock<std::collections::HashMap<&'static str, &'static dyn Adapter>> =
        OnceLock::new();
    NAMES.get_or_init(|| {
        let mut m: std::collections::HashMap<&'static str, &'static dyn Adapter> =
            std::collections::HashMap::new();
        // extensionless key = value documents; `config` is ghostty's name for it
        m.insert("config", &PLAIN as &'static dyn Adapter);
        m
    })
}

static INI: ini::IniAdapter = ini::IniAdapter;
static JSON: json::JsonAdapter = json::JsonAdapter;
static JSONL: jsonl::JsonlAdapter = jsonl::JsonlAdapter;
static PLAIN: plain::PlainAdapter = plain::PlainAdapter;
static TOML: toml::TomlAdapter = toml::TomlAdapter;
static YAML: yaml::YamlAdapter = yaml::YamlAdapter;

fn registry() -> &'static std::collections::HashMap<&'static str, &'static dyn Adapter> {
    static REG: OnceLock<std::collections::HashMap<&'static str, &'static dyn Adapter>> =
        OnceLock::new();

    REG.get_or_init(|| {
        let mut m: std::collections::HashMap<&'static str, &'static dyn Adapter> =
            std::collections::HashMap::new();
        // ini's line-preserving key=value logic is exactly what flat
        // .properties/.conf/.env files need, so each alias costs one word.
        for ext in ["ini", "properties", "conf", "env"] {
            m.insert(ext, &INI as &'static dyn Adapter);
        }
        m.insert("json", &JSON as &'static dyn Adapter);
        // a jsonl file is a sequence of records, one per line
        for ext in ["jsonl", "ndjson"] {
            m.insert(ext, &JSONL as &'static dyn Adapter);
        }
        m.insert("toml", &TOML as &'static dyn Adapter);
        for ext in ["yaml", "yml"] {
            m.insert(ext, &YAML as &'static dyn Adapter);
        }
        // ghostty also reads a file named `config.ghostty`
        m.insert("ghostty", &PLAIN as &'static dyn Adapter);
        m
    })
}

/// `key = value` line parsing shared by ini-ish formats.
pub(crate) fn parse_kv(line: &str) -> Option<(&str, &str)> {
    for sep in ['=', ':'] {
        if let Some(i) = line.find(sep) {
            let (k, v) = line.split_at(i);
            return Some((k.trim(), v[sep.len_utf8()..].trim()));
        }
    }
    None
}

/// Last dot-separated extension, like Python's `Path::suffix().lstrip('.')`.
pub(crate) fn ext_of(name: &str) -> &str {
    match name.rfind('.') {
        Some(i) if i + 1 < name.len() => &name[i + 1..],
        _ => "",
    }
}

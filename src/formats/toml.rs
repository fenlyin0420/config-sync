//! toml adapter: line-preserving. `[table]` headers, `key = value` lines, their
//! comments and their order all stay where they are -- an overridden key is
//! rewritten in place, a new one is appended to its table.
//!
//! A value is the text to the right of `=`, and it may span lines: an array or
//! an inline table costs one key the way a single-line value does, kept with the
//! shared indentation stripped so it can be re-indented for the file it is
//! written into. `[[table]]` repeats are addressed by occurrence (`bin#2.name`),
//! matching how the plain adapter addresses repeated keys.
//!
//! A `"""` or `'''` string holds whitespace that must not be re-indented, so a
//! document that contains one is not something this adapter will rewrite.

use super::ini::DEL;
use super::{Kind, Op, OpData};

struct Entry {
    /// dotted path sync.rules matches: `server.port`, `bin#2.name`
    path: String,
    /// the key as written on the left of `=`, so a rewrite keeps its spelling
    key: String,
    /// the table the key sits in, `""` at the document root
    section: String,
    indent: usize,
    start: usize,
    end: usize,
    /// byte offset of the value on the first line
    col: usize,
    value: String,
}

#[derive(Clone, Copy, PartialEq)]
enum Quote {
    None,
    Basic,
    Literal,
    MultiBasic,
    MultiLiteral,
}

/// how much of a value is still open: brackets and quoted strings
struct Scan {
    depth: i32,
    quote: Quote,
}

impl Default for Scan {
    fn default() -> Self {
        Scan {
            depth: 0,
            quote: Quote::None,
        }
    }
}

impl Scan {
    fn feed(&mut self, line: &str) {
        let chars: Vec<char> = line.chars().collect();
        let mut i = 0usize;
        while i < chars.len() {
            let at = |n: usize| chars.get(i + n).copied();
            let c = chars[i];
            match self.quote {
                Quote::Basic => {
                    if c == '\\' {
                        i += 2;
                        continue;
                    }
                    if c == '"' {
                        self.quote = Quote::None;
                    }
                    i += 1;
                }
                Quote::Literal => {
                    if c == '\'' {
                        self.quote = Quote::None;
                    }
                    i += 1;
                }
                Quote::MultiBasic | Quote::MultiLiteral => {
                    let q = match self.quote {
                        Quote::MultiBasic => '"',
                        _ => '\'',
                    };
                    if c == q && at(1) == Some(q) && at(2) == Some(q) {
                        self.quote = Quote::None;
                        i += 3;
                        continue;
                    }
                    if q == '"' && c == '\\' {
                        i += 2;
                        continue;
                    }
                    i += 1;
                }
                Quote::None => match c {
                    // an unquoted `#` is a comment: nothing after it can close a
                    // bracket, and it is kept as part of the value text
                    '#' => return,
                    '"' => {
                        let multi = at(1) == Some('"') && at(2) == Some('"');
                        self.quote = match multi {
                            true => Quote::MultiBasic,
                            false => Quote::Basic,
                        };
                        i += match multi {
                            true => 3,
                            false => 1,
                        };
                    }
                    '\'' => {
                        let multi = at(1) == Some('\'') && at(2) == Some('\'');
                        self.quote = match multi {
                            true => Quote::MultiLiteral,
                            false => Quote::Literal,
                        };
                        i += match multi {
                            true => 3,
                            false => 1,
                        };
                    }
                    '[' | '{' => {
                        self.depth += 1;
                        i += 1;
                    }
                    ']' | '}' => {
                        self.depth -= 1;
                        i += 1;
                    }
                    _ => i += 1,
                },
            }
        }
    }

    fn done(&self) -> bool {
        self.depth <= 0 && self.quote == Quote::None
    }
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start_matches([' ', '\t']).len()
}

fn parent_of(path: &str) -> &str {
    match path.rfind('.') {
        Some(i) => &path[..i],
        None => "",
    }
}

fn last_seg(path: &str) -> &str {
    match path.rfind('.') {
        Some(i) => &path[i + 1..],
        None => path,
    }
}

fn join_seg(parent: &str, key: &str) -> String {
    match parent.is_empty() {
        true => key.to_string(),
        false => format!("{parent}.{key}"),
    }
}

/// a key without its quoting, so `"a b"` and `a b` address the same rule
fn unquote(key: &str) -> &str {
    let bytes = key.as_bytes();
    if bytes.len() >= 2 {
        let (first, last) = (bytes[0], bytes[bytes.len() - 1]);
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return &key[1..key.len() - 1];
        }
    }
    key
}

/// `key = value` -> the key, where the value starts in the line, and the value
fn split_kv(s: &str) -> Option<(&str, usize, &str)> {
    let bytes = s.as_bytes();
    let mut quote = 0u8;
    for (i, &b) in bytes.iter().enumerate() {
        if quote != 0 {
            if b == quote {
                quote = 0;
            }
            continue;
        }
        match b {
            b'"' | b'\'' => quote = b,
            b'=' => {
                let key = s[..i].trim();
                if key.is_empty() {
                    return None;
                }
                let rest = &s[i + 1..];
                let at = i + 1 + (rest.len() - rest.trim_start_matches([' ', '\t']).len());
                return Some((key, at, s[at..].trim_end()));
            }
            _ => {}
        }
    }
    None
}

/// the managed keys of a document and its `[table]` headers
type Parsed = (Vec<Entry>, Vec<(String, usize)>);

/// every managed key of `text` and the table headers it has
fn parse(text: &str) -> Option<Parsed> {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut entries: Vec<Entry> = Vec::new();
    let mut headers: Vec<(String, usize)> = Vec::new();
    let mut section = String::new();
    let mut arrays: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut i = 0usize;
    while i < lines.len() {
        let raw = lines[i];
        let start = raw.len() - raw.trim_start().len();
        let s = &raw[start..];
        if s.trim().is_empty() || s.starts_with('#') {
            i += 1;
            continue;
        }
        if let Some(inner) = s.strip_prefix('[') {
            let (inner, close) = match inner.strip_prefix('[') {
                Some(rest) => (rest, "]]"),
                None => (inner, "]"),
            };
            let array = close == "]]";
            let end = inner.rfind(close)?;
            let mut name = inner[..end].trim().to_string();
            if name.is_empty() {
                return None;
            }
            if array {
                let n = {
                    let counter = arrays.entry(name.clone()).or_insert(0);
                    *counter += 1;
                    *counter
                };
                name = format!("{name}#{n}");
            }
            section = name.clone();
            headers.push((name, i));
            i += 1;
            continue;
        }
        let (key, at, value) = split_kv(s)?;
        let mut scan = Scan::default();
        scan.feed(value);
        let mut body: Vec<&str> = Vec::new();
        let mut end = i;
        while !scan.done() {
            end += 1;
            let line = *lines.get(end)?;
            scan.feed(line);
            body.push(line);
        }
        while matches!(body.last(), Some(l) if l.trim().is_empty()) {
            body.pop();
            end -= 1;
        }
        let shared = body
            .iter()
            .map(|l| indent_of(l))
            .min()
            .unwrap_or(0);
        let mut value = value.to_string();
        for line in body {
            value.push('\n');
            if !line.trim().is_empty() {
                value.push_str(&line[shared.min(line.len())..]);
            }
        }
        // a multi-line string's own whitespace cannot be re-indented, so a
        // document holding one is left alone entirely
        if value.contains("\"\"\"") || value.contains("'''") {
            return None;
        }
        entries.push(Entry {
            path: join_seg(&section, unquote(key)),
            key: key.to_string(),
            section: section.clone(),
            indent: start,
            start: i,
            end,
            col: start + at,
            value,
        });
        i = end + 1;
    }
    Some((entries, headers))
}

fn text_op(path: &str, key: &str, value: Option<String>) -> Op {
    Op {
        path: path.to_string(),
        kind: match value {
            Some(_) => Kind::Set,
            None => Kind::Delete,
        },
        data: OpData::Text {
            key: key.to_string(),
            value,
        },
    }
}

/// where a key appended to `section` goes: after the table's last line, after
/// its header, before the first header at the root, or at the end of the file
fn insert_at(entries: &[Entry], headers: &[(String, usize)], lines: usize, section: &str) -> usize {
    let last = entries
        .iter()
        .filter(|e| e.section == section)
        .map(|e| e.end + 1)
        .max();
    if let Some(at) = last {
        return at;
    }
    if let Some((_, line)) = headers.iter().find(|(name, _)| name == section) {
        return line + 1;
    }
    match section.is_empty() {
        true => headers.first().map(|(_, line)| *line).unwrap_or(lines),
        false => lines,
    }
}

/// line-preserving: an overridden key is rewritten in place, a deleted one drops
/// its lines, a new one is appended to its table
fn apply_ops(base: &str, ops: &[Op]) -> String {
    let Some((entries, headers)) = parse(base) else {
        return base.to_string();
    };
    let lines: Vec<&str> = base.split('\n').collect();
    let mut drop = vec![false; lines.len()];
    // entry start line -> (last line of the entry, the lines replacing them)
    let mut replace: std::collections::HashMap<usize, (usize, Vec<String>)> =
        std::collections::HashMap::new();
    let mut fresh: Vec<(String, String, String)> = Vec::new();
    for op in ops {
        let Some(entry) = entries.iter().find(|e| e.path == op.path) else {
            if let OpData::Text {
                key,
                value: Some(value),
            } = &op.data
            {
                fresh.push((op.path.clone(), key.clone(), value.clone()));
            }
            continue;
        };
        match &op.data {
            OpData::Text {
                value: Some(value),
                ..
            } => {
                let mut block = Vec::new();
                let mut parts = value.split('\n');
                block.push(format!(
                    "{}{}",
                    &lines[entry.start][..entry.col.min(lines[entry.start].len())],
                    parts.next().unwrap_or_default()
                ));
                for line in parts {
                    // the value keeps its own relative indentation, aligned with
                    // the key so an untouched block round-trips byte for byte
                    block.push(match line.is_empty() {
                        true => String::new(),
                        false => format!("{}{line}", " ".repeat(entry.indent)),
                    });
                }
                replace.insert(entry.start, (entry.end, block));
            }
            OpData::Text { value: None, .. } => {
                for slot in &mut drop[entry.start..=entry.end] {
                    *slot = true;
                }
            }
            _ => unreachable!("toml adapter got a non-text op"),
        }
    }
    let mut out: Vec<String> = Vec::new();
    // where each original line landed in `out`, so an insertion point survives
    // the lines that were dropped or replaced before it
    let mut emitted = vec![0usize; lines.len()];
    let mut i = 0usize;
    while i < lines.len() {
        emitted[i] = out.len();
        if let Some((end, block)) = replace.get(&i) {
            for slot in &mut emitted[i..=(*end).min(lines.len().saturating_sub(1))] {
                *slot = out.len();
            }
            out.extend(block.iter().cloned());
            i = end + 1;
            continue;
        }
        if !drop[i] {
            out.push(lines[i].to_string());
        }
        i += 1;
    }

    // new keys: one insertion point per key, applied back to front so the
    // indexes of the earlier ones stay valid
    let mut inserts: Vec<(usize, usize, Vec<String>)> = Vec::new();
    let mut sections_created: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (order, (path, key, value)) in fresh.iter().enumerate() {
        let section = parent_of(path).to_string();
        let at = insert_at(&entries, &headers, lines.len(), &section);
        let mut block = Vec::new();
        if at == lines.len()
            && !section.is_empty()
            && !sections_created.contains(&section)
            && !headers.iter().any(|(name, _)| *name == section)
        {
            if matches!(out.last(), Some(l) if !l.trim().is_empty()) {
                block.push(String::new());
            }
            block.push(format!("[{section}]"));
            sections_created.insert(section.clone());
        }
        let mut parts = value.split('\n');
        block.push(format!("{key} = {}", parts.next().unwrap_or_default()));
        for line in parts {
            block.push(line.to_string());
        }
        inserts.push((at, order, block));
    }
    inserts.sort_by_key(|a| std::cmp::Reverse((a.0, a.1)));
    for (at, _, block) in inserts {
        let at = match at < emitted.len() {
            true => emitted[at],
            false => out.len(),
        };
        let at = at.min(out.len());
        for (offset, line) in block.into_iter().enumerate() {
            out.insert(at + offset, line);
        }
    }
    let text = out.join("\n");
    if text.ends_with('\n') {
        text
    } else {
        text + "\n"
    }
}

pub struct TomlAdapter;

impl super::Adapter for TomlAdapter {
    fn name(&self) -> &'static str {
        "toml"
    }

    fn overlay_ops(&self, text: &str) -> Vec<Op> {
        match parse(text) {
            Some((entries, _)) => entries
                .into_iter()
                .map(|e| match e.value == DEL {
                    true => text_op(&e.path, &e.key, None),
                    false => text_op(&e.path, &e.key, Some(e.value)),
                })
                .collect(),
            None => Vec::new(),
        }
    }

    fn diff(&self, base_text: &str, cur_text: &str) -> Vec<Op> {
        let Some((base, _)) = parse(base_text) else {
            return Vec::new();
        };
        let Some((cur, _)) = parse(cur_text) else {
            return Vec::new();
        };
        let known: std::collections::HashMap<&str, &str> = base
            .iter()
            .map(|e| (e.path.as_str(), e.value.as_str()))
            .collect();
        let mut ops = Vec::new();
        for entry in &cur {
            if known.get(entry.path.as_str()) != Some(&entry.value.as_str()) {
                ops.push(text_op(&entry.path, &entry.key, Some(entry.value.clone())));
            }
        }
        let live: std::collections::HashSet<&str> = cur.iter().map(|e| e.path.as_str()).collect();
        for entry in &base {
            if !live.contains(entry.path.as_str()) {
                ops.push(text_op(&entry.path, &entry.key, None));
            }
        }
        ops
    }

    fn apply(&self, base_text: &str, ops: &[Op]) -> String {
        apply_ops(base_text, ops)
    }

    fn pin_ops(&self, base_text: &str, owned: &dyn Fn(&str) -> bool) -> Vec<Op> {
        match parse(base_text) {
            Some((entries, _)) => entries
                .into_iter()
                .filter(|e| owned(&e.path))
                .map(|e| text_op(&e.path, &e.key, Some(e.value)))
                .collect(),
            None => Vec::new(),
        }
    }

    fn drop_ops(&self, base_text: &str, veto: &dyn Fn(&str) -> bool) -> Vec<Op> {
        match parse(base_text) {
            Some((entries, _)) => entries
                .into_iter()
                .filter(|e| veto(&e.path))
                .map(|e| text_op(&e.path, &e.key, None))
                .collect(),
            None => Vec::new(),
        }
    }

    fn parses(&self, text: &str) -> bool {
        parse(text).is_some()
    }

    fn dump_overlay(&self, ops: &[Op], osname: &str) -> String {
        let mut out = vec![
            "# generated by cfgsync harvest -- platform overlay for".to_string(),
            format!("# os: {osname}"),
        ];
        let mut section: Option<String> = None;
        for op in ops {
            let OpData::Text { value, .. } = &op.data else {
                continue;
            };
            let here = parent_of(&op.path);
            if section.as_deref() != Some(here) {
                if !here.is_empty() {
                    out.push(String::new());
                    out.push(format!("[{here}]"));
                }
                section = Some(here.to_string());
            }
            let key = last_seg(&op.path);
            let written = match value {
                Some(v) => v.clone(),
                None => DEL.to_string(),
            };
            let mut parts = written.split('\n');
            out.push(format!("{key} = {}", parts.next().unwrap_or_default()));
            for line in parts {
                out.push(line.to_string());
            }
        }
        out.join("\n") + "\n"
    }
}

#[cfg(test)]
mod tests {
    use super::super::Adapter;
    use super::*;

    const DOC: &str = "\
# app
title = \"hi\"

[server]
port = 8080
hosts = [
  \"a\",
  \"b\",
]

[[bin]]
name = \"one\"

[[bin]]
name = \"two\"
";

    fn paths(text: &str) -> Vec<String> {
        parse(text)
            .unwrap()
            .0
            .into_iter()
            .map(|e| e.path)
            .collect()
    }

    #[test]
    fn paths_carry_their_table_and_array_ordinal() {
        assert_eq!(
            paths(DOC),
            vec!["title", "server.port", "server.hosts", "bin#1.name", "bin#2.name"]
        );
    }

    #[test]
    fn a_multi_line_array_is_one_value() {
        let (entries, _) = parse(DOC).unwrap();
        let hosts = entries.iter().find(|e| e.path == "server.hosts").unwrap();
        assert_eq!(hosts.value, "[\n  \"a\",\n  \"b\",\n]");
    }

    #[test]
    fn rewriting_one_key_keeps_the_rest() {
        let ops = TomlAdapter.diff(DOC, &DOC.replace("port = 8080", "port = 9090"));
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].path, "server.port");
        assert_eq!(
            TomlAdapter.apply(DOC, &ops),
            DOC.replace("port = 8080", "port = 9090")
        );
    }

    #[test]
    fn an_overlay_round_trips_through_the_base() {
        let owned = |p: &str| p.starts_with("server") || p.starts_with("bin#2");
        let pins = TomlAdapter.pin_ops(DOC, &owned);
        assert_eq!(pins.len(), 3);
        let overlay = TomlAdapter.dump_overlay(&pins, "linux");
        assert!(overlay.contains("[server]"), "{overlay}");
        assert!(overlay.contains("[bin#2]"), "{overlay}");
        assert_eq!(TomlAdapter.apply(DOC, &TomlAdapter.overlay_ops(&overlay)), DOC);
    }

    #[test]
    fn a_new_key_lands_in_its_table() {
        let ops = vec![text_op("server.tls", "tls", Some("true".to_string()))];
        let out = TomlAdapter.apply(DOC, &ops);
        assert!(
            out.contains("  \"b\",\n]\ntls = true\n"),
            "appended to the end of its table: {out}"
        );
        assert_eq!(
            TomlAdapter.diff(DOC, &out)
                .iter()
                .map(|o| o.path.clone())
                .collect::<Vec<_>>(),
            vec!["server.tls"]
        );
    }

    #[test]
    fn a_new_table_is_written_whole() {
        let ops = vec![text_op("cache.size", "size", Some("1".to_string()))];
        let out = TomlAdapter.apply(DOC, &ops);
        assert!(out.contains("[cache]\nsize = 1\n"), "{out}");
        assert_eq!(paths(&out).last().unwrap(), "cache.size");
    }

    #[test]
    fn a_deleted_key_drops_its_line() {
        let ops = TomlAdapter.diff(DOC, &DOC.replace("port = 8080\n", ""));
        assert_eq!(TomlAdapter.apply(DOC, &ops), DOC.replace("port = 8080\n", ""));
    }

    #[test]
    fn reads_key_value_toml_only() {
        assert!(TomlAdapter.parses(DOC));
        assert!(TomlAdapter.parses(""));
        // a multi-line string and an unterminated array are not rewritten
        assert!(!TomlAdapter.parses("a = \"\"\"\nx\n\"\"\"\n"));
        assert!(!TomlAdapter.parses("a = [1,\n"));
        assert!(!TomlAdapter.parses("garbage line\n"));
    }
}

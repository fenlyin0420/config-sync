//! yaml adapter: line-preserving block YAML. Comments, blank lines, key order
//! and indentation of the base survive -- an overridden key keeps its own line
//! and a key nobody has yet is appended under its parent.
//!
//! A value is text, not a decoded scalar: a flow collection (`[1, 2]`,
//! `{a: 1}`), a quoted scalar, an anchor or a tag is stored and written back
//! verbatim, so nothing has to be resolved to round-trip a file. A block scalar
//! (`|`, `>`) and a bracketed value that spans lines are part of their key's
//! value, kept with the shared indentation stripped so it can be re-indented for
//! the document it is written into.
//!
//! Sequences are addressed by occurrence the way the plain adapter addresses
//! repeated keys: `servers#2.host` is the `host` key of the second item of
//! `servers`, and a scalar item is `hosts#3` itself.

use super::ini::DEL;
use super::{Kind, Op, OpData};

/// indentation a key gets when the document offers no sibling to copy
const UNIT: usize = 2;

/// one managed line (or block of lines) of the document
struct Entry {
    /// dotted path sync.rules matches: `ui.theme`, `servers#2.host`, `hosts#3`
    path: String,
    /// the last path segment, as written after the stack is popped
    key: String,
    indent: usize,
    /// first and last physical line of the value, inclusive
    start: usize,
    end: usize,
    /// byte offset of the value on the first line, so rewriting keeps the key's
    /// own spelling and spacing
    col: usize,
    value: String,
}

/// bracket/quote state of a value that may continue on the next line
#[derive(Default)]
struct Scan {
    depth: i32,
    /// `"`, `'`, or `\0` for "not inside a string"
    quote: char,
}

impl Scan {
    fn feed(&mut self, line: &str) {
        let mut escaped = false;
        for c in line.chars() {
            match self.quote {
                '"' => {
                    if escaped {
                        escaped = false;
                    } else if c == '\\' {
                        escaped = true;
                    } else if c == '"' {
                        self.quote = '\0';
                    }
                }
                '\'' => {
                    if c == '\'' {
                        self.quote = '\0';
                    }
                }
                _ => match c {
                    // an unquoted `#` starts a comment: the rest cannot close a
                    // bracket, and it is part of the value text
                    '#' => return,
                    '"' => self.quote = '"',
                    '\'' => self.quote = '\'',
                    '[' | '{' => self.depth += 1,
                    ']' | '}' => self.depth -= 1,
                    _ => {}
                },
            }
        }
    }

    fn done(&self) -> bool {
        self.depth <= 0 && self.quote == '\0'
    }
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

fn join_path(stack: &[(usize, String)]) -> String {
    let mut out = String::new();
    for (_, seg) in stack {
        // an item ordinal is a suffix of its parent, not a key of its own
        if seg.starts_with('#') {
            out.push_str(seg);
        } else {
            if !out.is_empty() {
                out.push('.');
            }
            out.push_str(seg);
        }
    }
    out
}

fn join_seg(parent: &str, key: &str) -> String {
    if parent.is_empty() {
        key.to_string()
    } else {
        format!("{parent}.{key}")
    }
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

/// `- value` -> `Some("value")`, a bare `-` -> `Some("")`, anything else `None`
fn item_rest(s: &str) -> Option<&str> {
    match s {
        "-" => Some(""),
        _ => s.strip_prefix("- "),
    }
}

/// `key: value` / `key:` -> the key and the value; `None` for a line that is not
/// a mapping entry (`- foo`, a folded scalar continuation, garbage)
fn split_key(s: &str) -> Option<(&str, usize, &str)> {
    let bytes = s.as_bytes();
    let mut quote = '\0';
    for (i, &b) in bytes.iter().enumerate() {
        if quote != '\0' {
            if b as char == quote {
                quote = '\0';
            }
            continue;
        }
        match b {
            b'"' | b'\'' => quote = b as char,
            b':' if bytes.get(i + 1).is_none_or(|n| *n == b' ') => {
                let key = s[..i].trim();
                if key.is_empty() || key.contains(':') {
                    return None;
                }
                let rest = &s[i + 1..];
                let at = i + 1 + (rest.len() - rest.trim_start_matches(' ').len());
                return Some((key, at, s[at..].trim_end()));
            }
            _ => {}
        }
    }
    None
}

/// `value` plus the lines it continues on: a block scalar takes every deeper
/// line, a flow collection takes lines until its brackets close.
fn gather(lines: &[&str], indent: usize, first: &str, i: usize) -> Option<(String, usize)> {
    let block = first.starts_with('|') || first.starts_with('>');
    let mut scan = Scan::default();
    if !block {
        scan.feed(first);
        if scan.done() {
            return Some((first.to_string(), i));
        }
    }
    let mut body: Vec<&str> = Vec::new();
    let mut next = i + 1;
    while next < lines.len() {
        let line = lines[next];
        if !block && scan.done() {
            break;
        }
        if block {
            if !line.trim().is_empty() && indent_of(line) <= indent {
                break;
            }
        } else {
            scan.feed(line);
        }
        body.push(line);
        next += 1;
    }
    if !scan.done() {
        // an unclosed bracket: rewriting this file could only make it worse
        return None;
    }
    // blank lines at the tail belong to whatever follows, not to this value
    while matches!(body.last(), Some(l) if l.trim().is_empty()) {
        body.pop();
        next -= 1;
    }
    let shared = body
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| indent_of(l))
        .min()
        .unwrap_or(0);
    let mut value = first.to_string();
    for line in body {
        value.push('\n');
        if !line.trim().is_empty() {
            value.push_str(&line[shared.min(line.len())..]);
        }
    }
    Some((value, next.saturating_sub(1)))
}

/// every managed value of `text`; `None` when the adapter cannot read it at all
fn parse(text: &str) -> Option<Vec<Entry>> {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut out: Vec<Entry> = Vec::new();
    // (indent, segment) of the mapping/sequence path the current line sits under
    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut ordinals: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut begun = false;
    let mut i = 0usize;
    while i < lines.len() {
        let raw = lines[i];
        let mut indent = indent_of(raw);
        let mut s = &raw[indent..];
        if s.starts_with('\t') {
            // tabs are not YAML indentation, and guessing would corrupt the file
            return None;
        }
        if s.trim().is_empty() || s.starts_with('#') {
            i += 1;
            continue;
        }
        if s.trim_end() == "---" || s.trim_end() == "..." {
            if begun {
                return None; // a second document: no single key space to merge into
            }
            i += 1;
            continue;
        }
        begun = true;
        // sequence items: `-`, `- value`, `- key: value`, `- - a`
        let mut opened: Option<String> = None;
        while let Some(rest) = item_rest(s) {
            while stack.last().is_some_and(|(ind, _)| *ind >= indent) {
                stack.pop();
            }
            let n = {
                let counter = ordinals.entry(join_path(&stack)).or_insert(0);
                *counter += 1;
                *counter
            };
            let seg = format!("#{n}");
            stack.push((indent, seg));
            let rest = rest.trim_start_matches(' ');
            indent += s.len() - rest.len();
            s = rest;
            opened = Some(join_path(&stack));
            if s.is_empty() {
                break;
            }
        }
        while stack.last().is_some_and(|(ind, _)| *ind >= indent) {
            stack.pop();
        }
        if s.is_empty() {
            i += 1;
            continue;
        }
        let raw = lines[i];
        match split_key(s) {
            Some((key, at, value)) => {
                let path = join_seg(&join_path(&stack), key);
                stack.push((indent, key.to_string()));
                if value.is_empty() {
                    i += 1; // a container: its children are deeper lines
                    continue;
                }
                let (value, end) = gather(&lines, indent, value, i)?;
                out.push(Entry {
                    path,
                    key: key.to_string(),
                    indent,
                    start: i,
                    end,
                    col: raw.len() - s.len() + at,
                    value,
                });
                i = end + 1;
            }
            None => match opened {
                Some(path) => {
                    let (value, end) = gather(&lines, indent, s, i)?;
                    out.push(Entry {
                        key: stack.last().map(|(_, s)| s.clone()).unwrap_or_default(),
                        path,
                        indent,
                        start: i,
                        end,
                        col: raw.len() - s.len(),
                        value,
                    });
                    i = end + 1;
                }
                // a folded plain scalar (`key: this is` over two lines) belongs
                // to the entry above it, not to a key of its own
                None => match out.last_mut() {
                    Some(prev) if indent > prev.indent => {
                        prev.value.push('\n');
                        prev.value.push_str(s.trim_end());
                        prev.end = i;
                        i += 1;
                    }
                    _ => return None,
                },
            },
        }
    }
    Some(out)
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

/// indentation a key added under `parent` gets
fn indent_under(entries: &[Entry], parent: &str) -> usize {
    let sibling = entries
        .iter()
        .rev()
        .find(|e| parent_of(&e.path) == parent)
        .map(|e| e.indent);
    if let Some(indent) = sibling {
        return indent;
    }
    match entries.iter().rev().find(|e| e.path == parent) {
        Some(e) => e.indent + UNIT,
        None => UNIT * (parent.matches('.').count() + usize::from(!parent.is_empty())),
    }
}

/// the lines a new key adds: the ancestors the document does not have yet, one
/// header each, then the key itself
fn insert_new(entries: &[Entry], unit: usize, path: &str, value: &str) -> Vec<String> {
    let parent = parent_of(path);
    let mut out = Vec::new();
    let mut indent = 0usize;
    if !parent.is_empty() {
        let known = entries.iter().any(|e| e.path == parent)
            || entries.iter().any(|e| e.path.starts_with(&format!("{parent}.")));
        if known {
            indent = indent_under(entries, parent);
        } else {
            let mut prefix = String::new();
            for seg in parent.split('.') {
                prefix = join_seg(&prefix, seg);
                out.push(match seg.starts_with('#') {
                    true => format!("{}-", " ".repeat(indent)),
                    false => format!("{}{seg}:", " ".repeat(indent)),
                });
                indent += unit;
            }
        }
    }
    let key = last_seg(path);
    let head = match key.starts_with('#') {
        true => format!("{}- ", " ".repeat(indent)),
        false => format!("{}{key}: ", " ".repeat(indent)),
    };
    let mut parts = value.split('\n');
    out.push(format!("{head}{}", parts.next().unwrap_or_default()));
    for line in parts {
        out.push(match line.is_empty() {
            true => String::new(),
            false => format!("{}{line}", " ".repeat(indent + unit)),
        });
    }
    out
}

/// line-preserving: an overridden key is rewritten on its own line, a deleted
/// one drops its lines, a new one is appended under its parent
fn apply_ops(base: &str, ops: &[Op]) -> String {
    let Some(entries) = parse(base) else {
        return base.to_string();
    };
    let lines: Vec<&str> = base.split('\n').collect();
    let mut drop = vec![false; lines.len()];
    // entry start line -> (last line of the entry, the lines replacing them)
    let mut replace: std::collections::HashMap<usize, (usize, Vec<String>)> =
        std::collections::HashMap::new();
    let mut fresh: Vec<(String, String)> = Vec::new();
    for op in ops {
        let Some(entry) = entries.iter().find(|e| e.path == op.path) else {
            if let OpData::Text { value: Some(value), .. } = &op.data {
                fresh.push((op.path.clone(), value.clone()));
            }
            continue;
        };
        match &op.data {
            OpData::Text { value: Some(value), .. } => {
                let indent = entry.indent + UNIT;
                let mut block = Vec::new();
                let mut parts = value.split('\n');
                block.push(format!(
                    "{}{}",
                    &lines[entry.start][..entry.col.min(lines[entry.start].len())],
                    parts.next().unwrap_or_default()
                ));
                for line in parts {
                    block.push(match line.is_empty() {
                        true => String::new(),
                        false => format!("{}{line}", " ".repeat(indent)),
                    });
                }
                replace.insert(entry.start, (entry.end, block));
            }
            OpData::Text { value: None, .. } => {
                for slot in &mut drop[entry.start..=entry.end] {
                    *slot = true;
                }
            }
            _ => unreachable!("yaml adapter got a non-text op"),
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
    let unit = entries
        .iter()
        .map(|e| e.indent)
        .filter(|i| *i > 0)
        .min()
        .unwrap_or(UNIT);
    // a new key goes after the last child of its parent, or at the end of the
    // document when the parent is new too
    let mut inserts: Vec<(usize, usize, Vec<String>)> = Vec::new();
    for (order, (path, value)) in fresh.iter().enumerate() {
        let parent = parent_of(path);
        let at = entries
            .iter()
            .filter(|e| !parent.is_empty() && e.path.starts_with(&format!("{parent}.")))
            .map(|e| e.end + 1)
            .max()
            .unwrap_or(lines.len());
        inserts.push((at, order, insert_new(&entries, unit, path, value)));
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

pub struct YamlAdapter;

impl super::Adapter for YamlAdapter {
    fn name(&self) -> &'static str {
        "yaml"
    }

    fn overlay_ops(&self, text: &str) -> Vec<Op> {
        match parse(text) {
            Some(entries) => entries
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
        let Some(base) = parse(base_text) else {
            return Vec::new();
        };
        let Some(cur) = parse(cur_text) else {
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
        parse(base_text)
            .unwrap_or_default()
            .into_iter()
            .filter(|e| owned(&e.path))
            .map(|e| text_op(&e.path, &e.key, Some(e.value)))
            .collect()
    }

    fn drop_ops(&self, base_text: &str, veto: &dyn Fn(&str) -> bool) -> Vec<Op> {
        parse(base_text)
            .unwrap_or_default()
            .into_iter()
            .filter(|e| veto(&e.path))
            .map(|e| text_op(&e.path, &e.key, None))
            .collect()
    }

    fn parses(&self, text: &str) -> bool {
        parse(text).is_some()
    }

    fn dump_overlay(&self, ops: &[Op], osname: &str) -> String {
        let mut out = vec![
            "# generated by cfgsync harvest -- platform overlay for".to_string(),
            format!("# os: {osname}"),
            String::new(),
        ];
        for op in ops {
            let OpData::Text { value, .. } = &op.data else {
                continue;
            };
            let mut value = match value {
                Some(v) => v.split('\n'),
                None => {
                    out.push(format!("{}: {DEL}", op.path));
                    continue;
                }
            };
            let mut line = format!("{}: {}", op.path, value.next().unwrap_or_default());
            for rest in value {
                line.push('\n');
                if !rest.is_empty() {
                    line.push_str(&format!("{}{rest}", " ".repeat(UNIT)));
                }
            }
            out.push(line);
        }
        out.join("\n") + "\n"
    }
}

#[cfg(test)]
mod tests {
    use super::super::Adapter;
    use super::*;

    const DOC: &str = "\
# ui
ui:
  theme: dark
  padding: 2
servers:
  - host: a.example
    port: 1
  - host: b.example
    port: 2
hosts:
  - x
  - y
note: |
  first
  second
";

    fn paths(text: &str) -> Vec<String> {
        parse(text).unwrap().into_iter().map(|e| e.path).collect()
    }

    #[test]
    fn paths_follow_indentation_and_sequence_ordinals() {
        assert_eq!(
            paths(DOC),
            vec![
                "ui.theme",
                "ui.padding",
                "servers#1.host",
                "servers#1.port",
                "servers#2.host",
                "servers#2.port",
                "hosts#1",
                "hosts#2",
                "note",
            ]
        );
    }

    #[test]
    fn a_block_scalar_is_one_value() {
        let entries = parse(DOC).unwrap();
        let note = entries.iter().find(|e| e.path == "note").unwrap();
        assert_eq!(note.value, "|\nfirst\nsecond");
    }

    #[test]
    fn rewriting_one_key_keeps_the_rest_of_the_file() {
        let ops = YamlAdapter.diff(DOC, &DOC.replace("theme: dark", "theme: light"));
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].path, "ui.theme");
        assert_eq!(
            YamlAdapter.apply(DOC, &ops),
            DOC.replace("theme: dark", "theme: light")
        );
    }

    #[test]
    fn a_sequence_item_is_addressed_by_ordinal() {
        let cur = DOC.replace("port: 2", "port: 9");
        let ops = YamlAdapter.diff(DOC, &cur);
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].path, "servers#2.port");
        assert_eq!(YamlAdapter.apply(DOC, &ops), cur);
    }

    #[test]
    fn an_overlay_round_trips_through_the_base() {
        let owned = |p: &str| p.starts_with("ui") || p.starts_with("servers");
        let pins = YamlAdapter.pin_ops(DOC, &owned);
        assert_eq!(pins.len(), 6);
        let overlay = YamlAdapter.dump_overlay(&pins, "linux");
        let ops = YamlAdapter.overlay_ops(&overlay);
        assert_eq!(ops.len(), 6);
        assert_eq!(YamlAdapter.apply(DOC, &ops), DOC);
    }

    #[test]
    fn a_new_key_is_appended_under_its_parent() {
        let ops = vec![text_op("ui.cursor", "cursor", Some("bar".to_string()))];
        let out = YamlAdapter.apply(DOC, &ops);
        assert!(out.contains("  padding: 2\n  cursor: bar\n"), "{out}");
        assert_eq!(YamlAdapter.diff(DOC, &out)[0].path, "ui.cursor");
    }

    #[test]
    fn a_new_parent_chain_is_written_key_by_key() {
        let ops = vec![text_op("a.b.c", "c", Some("1".to_string()))];
        let out = YamlAdapter.apply("ui:\n  theme: dark\n", &ops);
        assert!(out.contains("a:\n  b:\n    c: 1\n"), "{out}");
        assert_eq!(paths(&out).last().unwrap(), "a.b.c");
    }

    #[test]
    fn a_deleted_key_drops_its_lines() {
        let ops = YamlAdapter.diff(DOC, &DOC.replace("  padding: 2\n", ""));
        assert_eq!(YamlAdapter.apply(DOC, &ops), DOC.replace("  padding: 2\n", ""));
    }

    #[test]
    fn reads_block_yaml_only() {
        assert!(YamlAdapter.parses(DOC));
        assert!(YamlAdapter.parses(""));
        assert!(YamlAdapter.parses("ui: {theme: dark, padding: 2}\n"));
        // tabs as indentation, an unclosed collection and a second document are
        // not something this adapter may rewrite
        assert!(!YamlAdapter.parses("ui:\n\ttheme: dark\n"));
        assert!(!YamlAdapter.parses("ui: [1, 2\n"));
        assert!(!YamlAdapter.parses("a: 1\n---\nb: 2\n"));
    }

    #[test]
    fn a_flow_value_is_text_not_structure() {
        let text = "ui: {theme: dark}\nlist: [1, 2]\n";
        let ops = YamlAdapter.diff(text, &text.replace("dark", "light"));
        assert_eq!(YamlAdapter.apply(text, &ops), text.replace("dark", "light"));
    }
}

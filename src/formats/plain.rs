//! plain adapter: extensionless `key = value` configs, ghostty's `config` above
//! all. Line-preserving like ini, with two differences: there are no sections,
//! and a key may legally appear many times (`keybind`), so occurrences after the
//! first are addressed as `key#2`, `key#3`, ... in file order. The overlay file
//! keeps those ordinals, which is what makes an overlay of the 2nd and 4th
//! `keybind` round-trip.

use super::ini::DEL;
use super::{parse_kv, Kind, Op, OpData};

/// one physical `key = value` line
#[derive(Clone, Debug)]
struct Entry {
    /// what sync.rules matches: `key`, or `key#N` for the Nth occurrence
    path: String,
    /// the key as it is written in the file, ordinals stripped
    key: String,
    /// the line this entry occupies, when it comes from a document
    line: usize,
    value: String,
}

/// `keybind#2` -> (`keybind`, Some(2)); a plain key -> (`key`, None)
fn split_ordinal(key: &str) -> (&str, Option<usize>) {
    match key.rsplit_once('#') {
        Some((head, tail))
            if !head.is_empty() && !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()) =>
        {
            (head, tail.parse().ok())
        }
        _ => (key, None),
    }
}

fn entries(text: &str) -> Vec<Entry> {
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut out = Vec::new();
    for (i, raw) in text.split('\n').enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        let Some((key, value)) = parse_kv(line) else {
            continue;
        };
        let (bare, explicit) = split_ordinal(key);
        let n = match explicit {
            Some(n) => n,
            None => {
                let nth = seen.entry(bare.to_string()).or_insert(0);
                *nth += 1;
                *nth
            }
        };
        out.push(Entry {
            // an overlay line states its own ordinal; a document entry derives it
            path: match explicit {
                Some(_) => key.to_string(),
                None if n == 1 => bare.to_string(),
                None => format!("{bare}#{n}"),
            },
            key: bare.to_string(),
            line: i,
            value: value.to_string(),
        });
    }
    out
}

fn set_op(path: &str, key: &str, value: &str) -> Op {
    Op {
        path: path.to_string(),
        kind: Kind::Set,
        data: OpData::Ini {
            section: String::new(),
            key: key.to_string(),
            value: Some(value.to_string()),
        },
    }
}

fn delete_op(path: &str, key: &str) -> Op {
    Op {
        path: path.to_string(),
        kind: Kind::Delete,
        data: OpData::Ini {
            section: String::new(),
            key: key.to_string(),
            value: None,
        },
    }
}

/// line-preserving: an overridden key is rewritten in place, a new one is
/// appended, a deleted one drops its line (comments and order survive)
pub fn apply_ops(base_text: &str, ops: &[Op]) -> String {
    let lines: Vec<&str> = base_text.split('\n').collect();
    let entries = entries(base_text);
    let mut drop: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut sets: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for op in ops {
        match &op.data {
            OpData::Ini { value, .. } if value.is_some() => {
                sets.insert(op.path.clone(), value.clone().unwrap_or_default());
            }
            OpData::Ini { .. } => {
                if let Some(e) = entries.iter().find(|e| e.path == op.path) {
                    drop.insert(e.line);
                }
            }
            _ => unreachable!("plain adapter got a non-plain op"),
        }
    }
    let mut out: Vec<String> = Vec::new();
    for (i, raw) in lines.iter().enumerate() {
        if drop.contains(&i) {
            continue;
        }
        match entries.iter().find(|e| e.line == i) {
            Some(e) if sets.contains_key(&e.path) => {
                out.push(format!("{} = {}", e.key, sets[&e.path]));
            }
            _ => out.push((*raw).to_string()),
        }
    }
    // keys nobody has yet are appended: an occurrence keeps its position by
    // being written after the ones already there
    for op in ops {
        let OpData::Ini { key, value, .. } = &op.data else {
            continue;
        };
        let Some(value) = value else { continue };
        if entries.iter().any(|e| e.path == op.path) {
            continue;
        }
        out.push(format!("{key} = {value}"));
    }
    let text = out.join("\n");
    if text.ends_with('\n') {
        text
    } else {
        text + "\n"
    }
}

pub struct PlainAdapter;

impl super::Adapter for PlainAdapter {
    fn name(&self) -> &'static str {
        "plain"
    }

    fn overlay_ops(&self, text: &str) -> Vec<Op> {
        entries(text)
            .into_iter()
            .map(|e| match e.value == DEL {
                true => delete_op(&e.path, &e.key),
                false => set_op(&e.path, &e.key, &e.value),
            })
            .collect()
    }

    fn diff(&self, base_text: &str, cur_text: &str) -> Vec<Op> {
        let base: std::collections::HashMap<String, String> = entries(base_text)
            .into_iter()
            .map(|e| (e.path, e.value))
            .collect();
        let cur = entries(cur_text);
        let mut ops = Vec::new();
        for e in &cur {
            if base.get(&e.path) != Some(&e.value) {
                ops.push(set_op(&e.path, &e.key, &e.value));
            }
        }
        let live: std::collections::HashSet<&String> = cur.iter().map(|e| &e.path).collect();
        for path in base.keys() {
            if !live.contains(path) {
                let (key, _) = split_ordinal(path);
                ops.push(delete_op(path, key));
            }
        }
        ops
    }

    fn apply(&self, base_text: &str, ops: &[Op]) -> String {
        apply_ops(base_text, ops)
    }

    fn pin_ops(&self, base_text: &str, owned: &dyn Fn(&str) -> bool) -> Vec<Op> {
        entries(base_text)
            .into_iter()
            .filter(|e| owned(&e.path))
            .map(|e| set_op(&e.path, &e.key, &e.value))
            .collect()
    }

    fn drop_ops(&self, base_text: &str, veto: &dyn Fn(&str) -> bool) -> Vec<Op> {
        entries(base_text)
            .into_iter()
            .filter(|e| veto(&e.path))
            .map(|e| delete_op(&e.path, &e.key))
            .collect()
    }

    /// a plain doc is readable when every line is blank, a comment, or
    /// `key = value` -- anything else would be silently dropped by `apply`
    fn parses(&self, text: &str) -> bool {
        text.split('\n').all(|raw| {
            let line = raw.trim();
            line.is_empty()
                || line.starts_with('#')
                || line.starts_with(';')
                || parse_kv(line).is_some_and(|(k, _)| !k.is_empty())
        })
    }

    fn dump_overlay(&self, ops: &[Op], osname: &str) -> String {
        let mut out = vec![
            "# generated by cfgsync harvest -- platform overlay for".to_string(),
            format!("# os: {osname}"),
            String::new(),
        ];
        for op in ops {
            let OpData::Ini { value, .. } = &op.data else {
                continue;
            };
            // the ordinal lives in the path, so `keybind#3` stays the 3rd line
            let written = match value {
                Some(v) => v.clone(),
                None => DEL.to_string(),
            };
            out.push(format!("{} = {written}", op.path));
        }
        out.join("\n") + "\n"
    }
}

#[cfg(test)]
mod tests {
    use super::super::Adapter;
    use super::*;

    const DOC: &str = "\
# ghostty
theme = catppuccin-mocha
window-padding-x = 2

keybind = ctrl+a=text:a
keybind = ctrl+b=text:b
font-family = Iosevka
";

    fn paths(text: &str) -> Vec<String> {
        entries(text).into_iter().map(|e| e.path).collect()
    }

    #[test]
    fn duplicate_keys_get_ordinals_after_the_first() {
        assert_eq!(
            paths(DOC),
            vec![
                "theme",
                "window-padding-x",
                "keybind",
                "keybind#2",
                "font-family"
            ]
        );
    }

    #[test]
    fn an_overlay_line_keeps_the_ordinal_it_states() {
        let overlay = "keybind#2 = ctrl+b=text:b\n";
        assert_eq!(paths(overlay), vec!["keybind#2"]);
    }

    #[test]
    fn diff_addresses_one_occurrence() {
        let cur = DOC.replace("ctrl+b=text:b", "ctrl+b=text:z");
        let ops = PlainAdapter.diff(DOC, &cur);
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].path, "keybind#2");
    }

    #[test]
    fn apply_changes_one_line_and_keeps_comments() {
        let ops = PlainAdapter.diff(DOC, &DOC.replace("ctrl+b=text:b", "ctrl+b=text:z"));
        let out = PlainAdapter.apply(DOC, &ops);
        assert_eq!(out, DOC.replace("ctrl+b=text:b", "ctrl+b=text:z"));
        assert!(out.contains("# ghostty"));
    }

    #[test]
    fn an_overlay_of_one_occurrence_round_trips() {
        let all = PlainAdapter.pin_ops(DOC, &|p| p.starts_with("keybind"));
        let keybinds: Vec<Op> = all;
        assert_eq!(keybinds.len(), 2);
        let overlay = PlainAdapter.dump_overlay(&keybinds, "linux");
        assert!(overlay.contains("keybind = ctrl+a"), "{overlay}");
        assert!(overlay.contains("keybind#2 = ctrl+b"), "{overlay}");
        assert_eq!(PlainAdapter.overlay_ops(&overlay).len(), 2);
        // rendering base + that overlay reproduces the document
        assert_eq!(PlainAdapter.apply(DOC, &PlainAdapter.overlay_ops(&overlay)), DOC);
    }

    #[test]
    fn a_deleted_occurrence_drops_its_line() {
        let cur = DOC.replace("keybind = ctrl+b=text:b\n", "");
        let out = PlainAdapter.apply(DOC, &PlainAdapter.diff(DOC, &cur));
        assert_eq!(out, cur);
    }

    #[test]
    fn reads_only_key_value_documents() {
        assert!(PlainAdapter.parses(DOC));
        assert!(PlainAdapter.parses(""));
        assert!(!PlainAdapter.parses("Host example\n  HostName x\n"));
        assert!(!PlainAdapter.parses("set number\nset relativenumber\n"));
    }
}

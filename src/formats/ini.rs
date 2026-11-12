//! ini adapter: line-preserving. base comments, blank lines and key order are
//! never touched; a key that gets overridden is rewritten in place.

use super::{parse_kv, Kind, Op, OpData};

pub const DEL: &str = "@@DEL@@";

/// [`(section, key)`] -> line index of its first occurrence
fn index(lines: &[&str]) -> (std::collections::HashMap<(String, String), usize>, SecRanges) {
    let mut idx = std::collections::HashMap::new();
    let mut ranges = SecRanges::default();
    let mut cur = String::new();
    let mut start: isize = -1;
    for (i, raw) in lines.iter().enumerate() {
        let s = raw.trim();
        if s.starts_with('[') && s.ends_with(']') {
            push_range(&mut ranges, cur.clone(), (start, i as isize));
            cur = s[1..s.len() - 1].trim().to_string();
            start = i as isize;
            continue;
        }
        if s.is_empty() || s.starts_with('#') || s.starts_with(';') {
            continue;
        }
        if let Some((k, _)) = parse_kv(s) {
            idx.entry((cur.clone(), k.to_string())).or_insert(i);
        }
    }
    push_range(&mut ranges, cur, (start, lines.len() as isize));
    (idx, ranges)
}

type SecRanges = Vec<(String, (isize, isize))>;

fn push_range(ranges: &mut SecRanges, section: String, span: (isize, isize)) {
    ranges.push((section, span));
}

/// last one wins, like assigning into a dict keyed by section
fn get_range(ranges: &SecRanges, section: &str) -> Option<(isize, isize)> {
    ranges
        .iter()
        .rev()
        .find(|(s, _)| s == section)
        .map(|(_, r)| *r)
}

/// ordered `[(section, key, value)]`
pub fn entries(text: &str) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for raw in text.split('\n') {
        let s = raw.trim();
        if s.starts_with('[') && s.ends_with(']') {
            cur = s[1..s.len() - 1].trim().to_string();
            continue;
        }
        if s.is_empty() || s.starts_with('#') || s.starts_with(';') {
            continue;
        }
        if let Some((k, v)) = parse_kv(s) {
            out.push((cur.clone(), k.to_string(), v.to_string()));
        }
    }
    out
}

fn name(section: &str, key: &str) -> String {
    if section.is_empty() {
        key.to_string()
    } else {
        format!("{section}.{key}")
    }
}

/// ops = `[(section, key, Option<value>)]`; `None` deletes
pub fn apply_ops(base_text: &str, ops: &[(String, String, Option<String>)]) -> String {
    let lines: Vec<&str> = base_text.split('\n').collect();
    let (idx, _) = index(&lines);
    let rev: std::collections::HashMap<usize, (String, String)> =
        idx.iter().map(|(k, v)| (*v, k.clone())).collect();

    let drop: std::collections::HashSet<usize> = ops
        .iter()
        .filter(|(s, k, v)| v.is_none() && idx.contains_key(&(s.clone(), k.clone())))
        .map(|(s, k, _)| idx[&(s.clone(), k.clone())])
        .collect();
    let sets: std::collections::HashMap<(String, String), String> = ops
        .iter()
        .filter_map(|(s, k, v)| v.clone().map(|v| ((s.clone(), k.clone()), v)))
        .collect();

    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    for (i, raw) in lines.iter().enumerate() {
        if drop.contains(&i) {
            continue;
        }
        match rev.get(&i) {
            Some(key) if sets.contains_key(key) => {
                out.push(format!("{} = {}", key.1, sets[key]));
            }
            _ => out.push((*raw).to_string()),
        }
    }

    let new: Vec<(String, String, String)> = ops
        .iter()
        .filter(|(s, k, v)| v.is_some() && !idx.contains_key(&(s.clone(), k.clone())))
        .map(|(s, k, v)| (s.clone(), k.clone(), v.clone().unwrap()))
        .collect();
    if !new.is_empty() {
        let joined = out.join("\n");
        let (_idx2, ranges) = index(&joined.split('\n').collect::<Vec<_>>());
        let mut by_sec: Vec<(String, Vec<String>)> = Vec::new();
        for (s, k, v) in &new {
            match by_sec.iter_mut().find(|(sec, _)| sec == s) {
                Some((_, kv)) => kv.push(format!("{k} = {v}")),
                None => by_sec.push((s.clone(), vec![format!("{k} = {v}")])),
            }
        }
        let mut inserts: Vec<(isize, Vec<String>)> = Vec::new();
        let mut tail: Vec<String> = Vec::new();
        for (sec, kv) in by_sec {
            match get_range(&ranges, &sec) {
                Some((start, end)) => {
                    let mut pos = end;
                    while pos > start + 1 && out[(pos - 1) as usize].trim().is_empty() {
                        pos -= 1;
                    }
                    inserts.push((pos, kv));
                }
                None => {
                    if !tail.is_empty() || matches!(out.last(), Some(l) if !l.trim().is_empty()) {
                        tail.push(String::new());
                    }
                    tail.push(format!("[{sec}]"));
                    tail.extend(kv);
                }
            }
        }
        inserts.sort_by_key(|(pos, _)| -pos);
        for (pos, kv) in inserts {
            let at = pos as usize;
            for (off, line) in kv.into_iter().enumerate() {
                out.insert(at + off, line);
            }
        }
        out.extend(tail);
    }

    let text = out.join("\n");
    if text.ends_with('\n') {
        text
    } else {
        text + "\n"
    }
}

pub fn dump_overlay(ops: &[(String, String, Option<String>)], osname: &str) -> String {
    let mut order: Vec<String> = Vec::new();
    let mut sections: Vec<(String, Vec<(String, Option<String>)>)> = Vec::new();
    for (s, k, v) in ops {
        match sections.iter_mut().find(|(sec, _)| sec == s) {
            Some((_, kv)) => kv.push((k.clone(), v.clone())),
            None => {
                order.push(s.clone());
                sections.push((s.clone(), vec![(k.clone(), v.clone())]));
            }
        }
    }
    let mut out = vec![
        "# generated by cfgsync harvest -- platform overlay for".to_string(),
        format!("# os: {osname}"),
    ];
    for s in order {
        let (_, kv) = sections.iter().find(|(sec, _)| *sec == s).unwrap();
        out.push(String::new());
        out.push(format!("[{s}]"));
        for (k, v) in kv {
            out.push(format!("{k} = {}", v.as_deref().unwrap_or(DEL)));
        }
    }
    out.join("\n") + "\n"
}

pub struct IniAdapter;

impl super::Adapter for IniAdapter {
    fn name(&self) -> &'static str {
        "ini"
    }

    fn overlay_ops(&self, text: &str) -> Vec<Op> {
        entries(text)
            .into_iter()
            .map(|(s, k, v)| {
                let deleted = v == DEL;
                Op {
                    path: name(&s, &k),
                    kind: if deleted { Kind::Delete } else { Kind::Set },
                    data: OpData::Ini {
                        section: s,
                        key: k,
                        value: if deleted { None } else { Some(v) },
                    },
                }
            })
            .collect()
    }

    fn diff(&self, base_text: &str, cur_text: &str) -> Vec<Op> {
        let base: std::collections::HashMap<(String, String), String> = entries(base_text)
            .into_iter()
            .map(|(s, k, v)| ((s, k), v))
            .collect();
        let cur_entries = entries(cur_text);
        let mut ops = Vec::new();
        for (s, k, v) in &cur_entries {
            if base.get(&(s.clone(), k.clone())) != Some(v) {
                ops.push(Op {
                    path: name(s, k),
                    kind: Kind::Set,
                    data: OpData::Ini {
                        section: s.clone(),
                        key: k.clone(),
                        value: Some(v.clone()),
                    },
                });
            }
        }
        let cur_set: std::collections::HashSet<(String, String)> = cur_entries
            .iter()
            .map(|(s, k, _)| (s.clone(), k.clone()))
            .collect();
        for (s, k) in base.keys() {
            if !cur_set.contains(&(s.clone(), k.clone())) {
                ops.push(Op {
                    path: name(s, k),
                    kind: Kind::Delete,
                    data: OpData::Ini {
                        section: s.clone(),
                        key: k.clone(),
                        value: None,
                    },
                });
            }
        }
        ops
    }

    fn apply(&self, base_text: &str, ops: &[Op]) -> String {
        let flat: Vec<(String, String, Option<String>)> = ops
            .iter()
            .map(|op| match &op.data {
                OpData::Ini { section, key, value } => {
                    (section.clone(), key.clone(), value.clone())
                }
                _ => unreachable!("ini adapter got a non-ini op"),
            })
            .collect();
        apply_ops(base_text, &flat)
    }

    fn pin_ops(&self, base_text: &str, owned: &dyn Fn(&str) -> bool) -> Vec<Op> {
        entries(base_text)
            .into_iter()
            .filter(|(s, k, _)| owned(&name(s, k)))
            .map(|(s, k, v)| Op {
                path: name(&s, &k),
                kind: Kind::Set,
                data: OpData::Ini {
                    section: s,
                    key: k,
                    value: Some(v),
                },
            })
            .collect()
    }

    fn drop_ops(&self, base_text: &str, veto: &dyn Fn(&str) -> bool) -> Vec<Op> {
        entries(base_text)
            .into_iter()
            .filter(|(s, k, _)| veto(&name(s, k)))
            .map(|(s, k, _)| Op {
                path: name(&s, &k),
                kind: Kind::Delete,
                data: OpData::Ini {
                    section: s,
                    key: k,
                    value: None,
                },
            })
            .collect()
    }

    fn dump_overlay(&self, ops: &[Op], osname: &str) -> String {
        let flat: Vec<(String, String, Option<String>)> = ops
            .iter()
            .map(|op| match &op.data {
                OpData::Ini { section, key, value } => {
                    (section.clone(), key.clone(), value.clone())
                }
                _ => unreachable!("ini adapter got a non-ini op"),
            })
            .collect();
        dump_overlay(&flat, osname)
    }
}

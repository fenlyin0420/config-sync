//! json adapter: deep merge, `null` deletes, `$append`/`$prepend`/`$remove`/
//! `$replace` patch lists; list diffs come back as the smallest patch.

use super::{Kind, Op, OpData};
use serde_json::{Map, Value};

const OPS: [&str; 4] = ["$append", "$prepend", "$remove", "$replace"];

pub(crate) fn has_ops(d: &Map<String, Value>) -> bool {
    OPS.iter().any(|k| d.contains_key(*k))
}

pub fn merge(base: &Value, ov: &Value) -> Value {
    if let (Some(ovd), Value::Array(items)) = (ov.as_object(), base) {
        if has_ops(ovd) {
            let mut out: Vec<Value> = items.clone();
            if let Some(Value::Array(v)) = ovd.get("$replace") {
                out = v.clone();
            }
            if let Some(Value::Array(gone)) = ovd.get("$remove") {
                out.retain(|x| !gone.contains(x));
            }
            if let Some(Value::Array(v)) = ovd.get("$prepend") {
                let mut head = v.clone();
                head.extend(out);
                out = head;
            }
            if let Some(Value::Array(v)) = ovd.get("$append") {
                out.extend(v.clone());
            }
            return Value::Array(out);
        }
    }
    let ovd = match ov.as_object() {
        Some(d) => d,
        None => return ov.clone(),
    };
    let based = match base.as_object() {
        Some(d) => d,
        None => return ov.clone(),
    };
    let mut out: Map<String, Value> = based.clone();
    for (k, v) in ovd {
        if v.is_null() {
            out.remove(k);
        } else if let Some(cur) = out.get(k).cloned() {
            out.insert(k.clone(), merge(&cur, v));
        } else {
            out.insert(k.clone(), v.clone());
        }
    }
    Value::Object(out)
}

/// `base`/`cur` lists -> the smallest `{$remove, $append}` patch, or `None`
/// when the delta cannot reproduce `cur` exactly (reordering, duplicates).
pub fn list_patch(base: &[Value], cur: &[Value]) -> Option<Vec<(String, Value)>> {
    let removed: Vec<Value> = base.iter().filter(|x| !cur.contains(x)).cloned().collect();
    let added: Vec<Value> = cur.iter().filter(|x| !base.contains(x)).cloned().collect();
    let mut rebuilt: Vec<Value> = base
        .iter()
        .filter(|x| !removed.contains(x))
        .cloned()
        .collect();
    rebuilt.extend(added.clone());
    if rebuilt != cur {
        return None;
    }
    let mut patch = Vec::new();
    if !removed.is_empty() {
        patch.push(("$remove".to_string(), Value::Array(removed)));
    }
    if !added.is_empty() {
        patch.push(("$append".to_string(), Value::Array(added)));
    }
    Some(patch)
}

/// flat `[(path, value)]`; `Null` deletes, an object of `$ops` patches a list
pub fn flat(base: &Value, cur: &Value, prefix: &mut Vec<String>, out: &mut Vec<(Vec<String>, Value)>) {
    if let (Some(bd), Some(cd)) = (base.as_object(), cur.as_object()) {
        for (k, v) in cd {
            let mut p = prefix.clone();
            p.push(k.clone());
            match bd.get(k) {
                Some(bv) => flat(bv, v, &mut p, out),
                None => out.push((p, v.clone())),
            }
        }
        for k in bd.keys() {
            if !cd.contains_key(k) {
                let mut p = prefix.clone();
                p.push(k.clone());
                out.push((p, Value::Null));
            }
        }
        return;
    }
    if let (Value::Array(bl), Value::Array(cl)) = (base, cur) {
        if prefix.is_empty() || bl == cl {
            return;
        }
        match list_patch(bl, cl) {
            None => out.push((prefix.clone(), Value::Array(cl.clone()))),
            Some(patch) => {
                if !patch.is_empty() {
                    let mut m = Map::new();
                    for (k, v) in patch {
                        m.insert(k, v);
                    }
                    out.push((prefix.clone(), Value::Object(m)));
                }
            }
        }
        return;
    }
    if !prefix.is_empty() && base != cur {
        out.push((prefix.clone(), cur.clone()));
    }
}

/// leaf paths only: objects are descended, arrays stay whole (they are managed
/// as deltas, pinning a snapshot of one would block base additions)
/// What `pin_ops` walks: an owned key is pinned whole, so a list counts as one
/// value here. `leaves` stops at lists because a diff patches them element by
/// element, and a pin has no element to patch -- it copies the base's value out.
pub(crate) fn pin_leaves(
    v: &Value,
    prefix: &mut Vec<String>,
    out: &mut Vec<(Vec<String>, Value)>,
) {
    let Some(map) = v.as_object() else {
        return;
    };
    for (k, child) in map {
        prefix.push(k.clone());
        match child {
            Value::Object(_) => pin_leaves(child, prefix, out),
            other => out.push((prefix.clone(), other.clone())),
        }
        prefix.pop();
    }
}

pub fn nest(pairs: &[(Vec<String>, Value)]) -> Value {
    let mut root = Map::new();
    for (path, val) in pairs {
        if path.is_empty() {
            continue;
        }
        let mut cur = &mut root;
        for k in &path[..path.len() - 1] {
            let entry = cur
                .entry(k.clone())
                .or_insert_with(|| Value::Object(Map::new()));
            cur = entry.as_object_mut().expect("nest: not an object");
        }
        cur.insert(path[path.len() - 1].clone(), val.clone());
    }
    Value::Object(root)
}

/// overlay document -> `[(kind, path, value)]`; `$op` objects become patches
pub(crate) fn flatten_doc(doc: &Map<String, Value>, prefix: &mut Vec<String>, out: &mut Vec<(Kind, Vec<String>, Value)>) {
    for (k, v) in doc {
        let mut p = prefix.clone();
        p.push(k.clone());
        match v.as_object() {
            Some(d) if has_ops(d) => out.push((Kind::Patch, p, v.clone())),
            Some(d) => flatten_doc(&d.clone(), &mut p, out),
            None => out.push((
                if v.is_null() { Kind::Delete } else { Kind::Set },
                p,
                v.clone(),
            )),
        }
    }
}

fn parse(text: &str) -> Value {
    if text.trim().is_empty() {
        return Value::Object(Map::new());
    }
    serde_json::from_str(text).unwrap_or_else(|_| Value::Object(Map::new()))
}

pub fn dump(value: &Value) -> String {
    serde_json::to_string_pretty(value).expect("json dump") + "\n"
}

pub struct JsonAdapter;

impl super::Adapter for JsonAdapter {
    fn name(&self) -> &'static str {
        "json"
    }

    fn overlay_ops(&self, text: &str) -> Vec<Op> {
        if text.trim().is_empty() {
            return Vec::new();
        }
        let doc = match serde_json::from_str::<Value>(text) {
            Ok(Value::Object(d)) => d,
            _ => return Vec::new(),
        };
        let mut out = Vec::new();
        flatten_doc(&doc, &mut Vec::new(), &mut out);
        out.into_iter()
            .map(|(kind, path, value)| Op {
                path: path.join("."),
                kind,
                data: OpData::Json {
                    path,
                    value,
                },
            })
            .collect()
    }

    fn diff(&self, base_text: &str, cur_text: &str) -> Vec<Op> {
        let base = parse(base_text);
        let cur = parse(cur_text);
        let mut leaves = Vec::new();
        flat(&base, &cur, &mut Vec::new(), &mut leaves);
        leaves
            .into_iter()
            .map(|(path, value)| {
                let kind = if value.is_null() {
                    Kind::Delete
                } else if value.as_object().is_some_and(has_ops) {
                    Kind::Patch
                } else {
                    Kind::Set
                };
                Op {
                    path: path.join("."),
                    kind,
                    data: OpData::Json { path, value },
                }
            })
            .collect()
    }

    fn apply(&self, base_text: &str, ops: &[Op]) -> String {
        let base = parse(base_text);
        let pairs: Vec<(Vec<String>, Value)> = ops
            .iter()
            .map(|op| match &op.data {
                OpData::Json { path, value } => (path.clone(), value.clone()),
                _ => unreachable!("json adapter got a non-json op"),
            })
            .collect();
        let merged = merge(&base, &nest(&pairs));
        // a merge that changes nothing keeps the base's own formatting, so an
        // adopted (verbatim) base is a fixed point of harvest -> render -> harvest
        if merged == base {
            return base_text.to_string();
        }
        dump(&merged)
    }

    fn pin_ops(&self, base_text: &str, owned: &dyn Fn(&str) -> bool) -> Vec<Op> {
        let mut flat = Vec::new();
        pin_leaves(&parse(base_text), &mut Vec::new(), &mut flat);
        flat.into_iter()
            .filter(|(p, _)| owned(&p.join(".")))
            .map(|(path, value)| Op {
                path: path.join("."),
                kind: Kind::Set,
                data: OpData::Json { path, value },
            })
            .collect()
    }

    fn parses(&self, text: &str) -> bool {
        // only an object document can be merged key by key: a top-level array or
        // scalar has no key paths, and merging it would silently produce `{}`
        text.trim().is_empty()
            || matches!(serde_json::from_str::<Value>(text), Ok(Value::Object(_)))
    }

    fn drop_ops(&self, base_text: &str, veto: &dyn Fn(&str) -> bool) -> Vec<Op> {
        // a vetoed object is one delete op for the whole subtree, which is what
        // `!secrets` reads as; arrays are included, unlike `pin_ops`
        fn walk(v: &Value, prefix: &mut Vec<String>, veto: &dyn Fn(&str) -> bool, out: &mut Vec<Op>) {
            let Some(map) = v.as_object() else {
                return;
            };
            for (k, child) in map {
                prefix.push(k.clone());
                if veto(&prefix.join(".")) {
                    out.push(Op {
                        path: prefix.join("."),
                        kind: Kind::Delete,
                        data: OpData::Json {
                            path: prefix.clone(),
                            value: Value::Null,
                        },
                    });
                } else {
                    walk(child, prefix, veto, out);
                }
                prefix.pop();
            }
        }
        let mut out = Vec::new();
        walk(&parse(base_text), &mut Vec::new(), veto, &mut out);
        out
    }

    fn dump_overlay(&self, ops: &[Op], _osname: &str) -> String {
        let pairs: Vec<(Vec<String>, Value)> = ops
            .iter()
            .map(|op| match &op.data {
                OpData::Json { path, value } => (path.clone(), value.clone()),
                _ => unreachable!("json adapter got a non-json op"),
            })
            .collect();
        dump(&nest(&pairs))
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Value {
        serde_json::from_str(s).unwrap()
    }

    #[test]
    fn merge_patches_lists() {
        let base = v(r#"{"a": ["x", "y"], "b": 1}"#);
        let ov = v(r#"{"a": {"$remove": ["x"], "$append": ["z"]}, "b": null}"#);
        assert_eq!(merge(&base, &ov), v(r#"{"a": ["y", "z"]}"#));
    }

    #[test]
    fn list_delta_roundtrip() {
        let base = v(r#"["a", "b"]"#);
        let cur = v(r#"["a", "b", "c"]"#);
        let patch = list_patch(base.as_array().unwrap(), cur.as_array().unwrap()).unwrap();
        let mut m = Map::new();
        for (k, val) in patch {
            m.insert(k, val);
        }
        assert_eq!(merge(&base, &Value::Object(m)), cur);
    }

    #[test]
    fn reorder_falls_back_to_full_list() {
        let base = v(r#"["a", "b"]"#);
        let cur = v(r#"["b", "a"]"#);
        assert!(list_patch(base.as_array().unwrap(), cur.as_array().unwrap()).is_none());
    }
}

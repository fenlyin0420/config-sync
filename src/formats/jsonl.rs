//! jsonl adapter: one JSON value per line, addressed by its 1-based line
//! ordinal -- `line3.model` is the `model` key of the third line, `line3` the
//! whole record. Nested values keep the json adapter's semantics (deep merge,
//! `null` deletes, `$append`/`$remove`/`$prepend`/`$replace` patches).
//!
//! The overlay is JSONL too, one line per touched record:
//!
//! ```text
//! {"line2": {"model": "other"}}
//! {"line4": null}
//! ```
//!
//! Because a record is addressed by its position, inserting or removing a line
//! shifts every ordinal after it; a harvest of the edited file records the new
//! lines under their new numbers.

use super::json;
use super::{Kind, Op, OpData};
use serde_json::{Map, Value};

fn key(n: usize) -> String {
    format!("line{n}")
}

fn ordinal(k: &str) -> Option<usize> {
    k.strip_prefix("line")?.parse().ok()
}

/// one non-empty physical line and where it sits
struct Line {
    n: usize,
    text: String,
    value: Value,
}

fn split(text: &str) -> Option<Vec<Line>> {
    let mut out = Vec::new();
    for (i, raw) in text.split('\n').enumerate() {
        if raw.trim().is_empty() {
            continue;
        }
        let value = serde_json::from_str::<Value>(raw).ok()?;
        out.push(Line {
            n: i + 1,
            text: raw.to_string(),
            value,
        });
    }
    Some(out)
}

/// the file as one object keyed by ordinal, which is what lets the json deep
/// merge, its leaf walk and its list patches work on a line-oriented document
fn document(lines: &[Line]) -> Map<String, Value> {
    lines.iter().map(|l| (key(l.n), l.value.clone())).collect()
}

fn json_op(path: Vec<String>, value: Value) -> Op {
    let kind = match value.is_null() {
        true => Kind::Delete,
        false => match value.as_object().is_some_and(json::has_ops) {
            true => Kind::Patch,
            false => Kind::Set,
        },
    };
    Op {
        path: path.join("."),
        kind,
        data: OpData::Json { path, value },
    }
}

fn pairs(ops: &[Op]) -> Vec<(Vec<String>, Value)> {
    ops.iter()
        .map(|op| match &op.data {
            OpData::Json { path, value } => (path.clone(), value.clone()),
            _ => unreachable!("jsonl adapter got a non-json op"),
        })
        .collect()
}

/// keys a veto line keeps out of the central repo: a vetoed record is one delete
/// for the whole line, a vetoed key one for its subtree
fn walk(v: &Value, prefix: &mut Vec<String>, veto: &dyn Fn(&str) -> bool, out: &mut Vec<Op>) {
    let Some(map) = v.as_object() else {
        return;
    };
    for (k, child) in map {
        prefix.push(k.clone());
        if veto(&prefix.join(".")) {
            out.push(json_op(prefix.clone(), Value::Null));
        } else {
            walk(child, prefix, veto, out);
        }
        prefix.pop();
    }
}

pub struct JsonlAdapter;

impl super::Adapter for JsonlAdapter {
    fn name(&self) -> &'static str {
        "jsonl"
    }

    fn overlay_ops(&self, text: &str) -> Vec<Op> {
        let mut out = Vec::new();
        for raw in text.split('\n') {
            if raw.trim().is_empty() {
                continue;
            }
            let Ok(Value::Object(doc)) = serde_json::from_str::<Value>(raw) else {
                continue;
            };
            for (k, v) in doc {
                let Some(n) = ordinal(&k) else {
                    continue;
                };
                let k = key(n);
                match v {
                    Value::Object(d) => {
                        let mut flat = Vec::new();
                        json::flatten_doc(&d, &mut vec![k.clone()], &mut flat);
                        out.extend(flat.into_iter().map(|(_, path, value)| json_op(path, value)));
                    }
                    other => out.push(json_op(vec![k], other)),
                }
            }
        }
        out
    }

    fn diff(&self, base_text: &str, cur_text: &str) -> Vec<Op> {
        let (Some(base), Some(cur)) = (split(base_text), split(cur_text)) else {
            return Vec::new();
        };
        let known = document(&base);
        let mut ops = Vec::new();
        for line in &cur {
            let k = key(line.n);
            match known.get(&k) {
                Some(b) => {
                    let mut flat = Vec::new();
                    json::flat(b, &line.value, &mut vec![k], &mut flat);
                    ops.extend(flat.into_iter().map(|(path, value)| json_op(path, value)));
                }
                None => ops.push(json_op(vec![k], line.value.clone())),
            }
        }
        let live: std::collections::HashSet<String> = cur.iter().map(|l| key(l.n)).collect();
        for k in known.keys() {
            if !live.contains(k) {
                ops.push(json_op(vec![k.clone()], Value::Null));
            }
        }
        ops
    }

    fn apply(&self, base_text: &str, ops: &[Op]) -> String {
        let Some(base) = split(base_text) else {
            return base_text.to_string();
        };
        let known = document(&base);
        let merged = json::merge(
            &Value::Object(known.clone()),
            &json::nest(&pairs(ops)),
        );
        let Value::Object(merged) = merged else {
            return base_text.to_string();
        };
        // a merge that changes nothing keeps the base bytes, so an adopted file
        // stays a fixed point of harvest -> render -> harvest
        if merged == known {
            return base_text.to_string();
        }
        let mut out: Vec<String> = Vec::new();
        for line in &base {
            match merged.get(&key(line.n)) {
                None => continue,
                Some(v) if *v == line.value => out.push(line.text.clone()),
                Some(v) => out.push(v.to_string()),
            }
        }
        let mut fresh: Vec<(&String, &Value)> = merged
            .iter()
            .filter(|(k, _)| !known.contains_key(*k))
            .collect();
        fresh.sort_by_key(|(k, _)| ordinal(k).unwrap_or(usize::MAX));
        out.extend(fresh.into_iter().map(|(_, v)| v.to_string()));
        match out.is_empty() {
            true => String::new(),
            false => out.join("\n") + "\n",
        }
    }

    fn pin_ops(&self, base_text: &str, owned: &dyn Fn(&str) -> bool) -> Vec<Op> {
        let Some(base) = split(base_text) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for line in &base {
            let mut leaves = Vec::new();
            json::leaves(&line.value, &mut vec![key(line.n)], &mut leaves);
            out.extend(
                leaves
                    .into_iter()
                    .filter(|(path, _)| owned(&path.join(".")))
                    .map(|(path, value)| json_op(path, value)),
            );
        }
        out
    }

    fn drop_ops(&self, base_text: &str, veto: &dyn Fn(&str) -> bool) -> Vec<Op> {
        let Some(base) = split(base_text) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for line in &base {
            let k = key(line.n);
            match veto(&k) {
                true => out.push(json_op(vec![k], Value::Null)),
                false => walk(&line.value, &mut vec![k], veto, &mut out),
            }
        }
        out
    }

    fn parses(&self, text: &str) -> bool {
        split(text).is_some()
    }

    fn dump_overlay(&self, ops: &[Op], _osname: &str) -> String {
        let mut rows: Vec<(usize, String)> = Vec::new();
        for (k, group) in grouped(&pairs(ops)) {
            let written = match group.len() == 1 && group[0].0.is_empty() {
                true => group[0].1.clone(),
                false => json::nest(&group),
            };
            rows.push((ordinal(&k).unwrap_or(usize::MAX), format!("{{\"{k}\":{written}}}")));
        }
        rows.sort_by_key(|(n, _)| *n);
        rows.into_iter().map(|(_, line)| line + "\n").collect()
    }
}

/// the ops of one record: `[(path inside the record, value)]`
type Group = Vec<(Vec<String>, Value)>;

/// `[(line key, its ops)]`, in the order the records were first touched
fn grouped(pairs: &[(Vec<String>, Value)]) -> Vec<(String, Group)> {
    let mut out: Vec<(String, Group)> = Vec::new();
    for (path, value) in pairs {
        let Some(first) = path.first().cloned() else {
            continue;
        };
        let rest = (path[1..].to_vec(), value.clone());
        match out.iter_mut().find(|(k, _)| *k == first) {
            Some((_, group)) => group.push(rest),
            None => out.push((first, vec![rest])),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::Adapter;
    use super::*;

    const DOC: &str = "\
{\"model\": \"a\", \"hosts\": [\"x\"], \"temp\": 1}
{\"model\": \"b\", \"deep\": {\"k\": 2}}
";

    #[test]
    fn paths_name_the_line_and_the_key() {
        let ops = JsonlAdapter.pin_ops(DOC, &|_| true);
        let paths: Vec<String> = ops.into_iter().map(|o| o.path).collect();
        assert_eq!(paths, vec!["line1.model", "line1.temp", "line2.model", "line2.deep.k"]);
    }

    #[test]
    fn a_changed_key_touches_one_line() {
        let cur = DOC.replace("\"model\": \"b\"", "\"model\": \"c\"");
        let ops = JsonlAdapter.diff(DOC, &cur);
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].path, "line2.model");
        let out = JsonlAdapter.apply(DOC, &ops);
        // the untouched line keeps its own bytes
        assert!(out.starts_with("{\"model\": \"a\", \"hosts\": [\"x\"], \"temp\": 1}\n"), "{out}");
        assert!(out.contains("\"model\":\"c\""), "{out}");
    }

    #[test]
    fn an_overlay_round_trips_through_the_base() {
        let pins = JsonlAdapter.pin_ops(DOC, &|p| p.starts_with("line2"));
        let overlay = JsonlAdapter.dump_overlay(&pins, "linux");
        assert_eq!(overlay, "{\"line2\":{\"model\":\"b\",\"deep\":{\"k\":2}}}\n");
        assert_eq!(JsonlAdapter.apply(DOC, &JsonlAdapter.overlay_ops(&overlay)), DOC);
    }

    #[test]
    fn a_removed_line_is_a_delete() {
        let cur = "{\"model\": \"a\", \"hosts\": [\"x\"], \"temp\": 1}\n";
        let ops = JsonlAdapter.diff(DOC, cur);
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].kind, Kind::Delete);
        assert_eq!(JsonlAdapter.apply(DOC, &ops), cur);
    }

    #[test]
    fn a_new_line_is_appended() {
        let cur = format!("{DOC}{{\"model\": \"c\"}}\n");
        let ops = JsonlAdapter.diff(DOC, &cur);
        assert_eq!(ops[0].path, "line3");
        assert_eq!(JsonlAdapter.apply(DOC, &ops), "{\"model\": \"a\", \"hosts\": [\"x\"], \"temp\": 1}\n{\"model\": \"b\", \"deep\": {\"k\": 2}}\n{\"model\":\"c\"}\n");
    }

    #[test]
    fn a_whole_record_op_is_one_overlay_line() {
        let ops = vec![json_op(vec!["line2".to_string()], Value::Null)];
        assert_eq!(
            JsonlAdapter.dump_overlay(&ops, "linux"),
            "{\"line2\":null}\n"
        );
        assert_eq!(JsonlAdapter.overlay_ops("{\"line2\": null}\n")[0].kind, Kind::Delete);
    }

    #[test]
    fn reads_one_json_value_per_line_only() {
        assert!(JsonlAdapter.parses(DOC));
        assert!(JsonlAdapter.parses(""));
        assert!(!JsonlAdapter.parses("{\"a\": 1}\nnot json\n"));
    }
}

//! The ledger's tables as the Node exporter dumps them and the difference against a baseline (the compact record of
//! format 2, test/daemon-transcript-export.ts): every table with rows, rows ordered by all columns,
//! `schema_migrations.applied_at` left out.

use rusqlite::types::ValueRef;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::Path;

/// Every table of the ledger that has rows, `{table: {columns, rows}}`; empty tables are left out.
pub fn dump_tables(file: &Path) -> Value {
    let conn =
        rusqlite::Connection::open_with_flags(file, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("the ledger opens read-only");
    let names: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let mut out = Map::new();
    for name in names {
        let columns: Vec<String> = conn
            .prepare(&format!("PRAGMA table_info({name})"))
            .unwrap()
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .map(Result::unwrap)
            .filter(|c| !(name == "schema_migrations" && c == "applied_at"))
            .collect();
        let order: Vec<String> = (1..=columns.len()).map(|i| i.to_string()).collect();
        let sql = format!(
            "SELECT {} FROM {name} ORDER BY {}",
            columns.join(", "),
            order.join(", ")
        );
        let mut statement = conn.prepare(&sql).unwrap();
        let rows: Vec<Value> = statement
            .query_map([], |row| {
                let mut values = Vec::new();
                for i in 0..columns.len() {
                    values.push(match row.get_ref(i)? {
                        ValueRef::Null => Value::Null,
                        ValueRef::Integer(n) => Value::from(n),
                        ValueRef::Real(f) => capstan_kernel::plan_body::number_value(f),
                        ValueRef::Text(t) => Value::String(String::from_utf8_lossy(t).into_owned()),
                        ValueRef::Blob(b) => json!({"$blob": capstan_kernel::canonical::hex(b)}),
                    });
                }
                Ok(Value::Array(values))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect();
        if !rows.is_empty() {
            out.insert(name, json!({"columns": columns, "rows": rows}));
        }
    }
    Value::Object(out)
}

/// What changed in `current` against `baseline` (both as `dump_tables` returns them), the way the exporter records it: per
/// table the rows only in `current` (`added`) and the rows only in the baseline (`removed`), each in dump order;
/// `columns` where the baseline has no such table or other columns; unchanged tables are left out.
pub fn diff_tables(baseline: &Value, current: &Value) -> Value {
    let mut names: Vec<&String> = baseline
        .as_object()
        .into_iter()
        .chain(current.as_object())
        .flat_map(|tables| tables.keys())
        .collect();
    names.sort();
    names.dedup();
    let rows_of = |tables: &Value, name: &str| -> Vec<Value> {
        tables[name]["rows"].as_array().cloned().unwrap_or_default()
    };
    let mut out = Map::new();
    for name in names {
        let was = rows_of(baseline, name);
        let now = rows_of(current, name);
        let mut count: BTreeMap<String, usize> = BTreeMap::new();
        for row in &was {
            *count.entry(row.to_string()).or_default() += 1;
        }
        let mut added = Vec::new();
        for row in &now {
            match count.get_mut(&row.to_string()) {
                Some(left) if *left > 0 => *left -= 1,
                _ => added.push(row.clone()),
            }
        }
        let mut removed = Vec::new();
        for row in &was {
            if let Some(left) = count.get_mut(&row.to_string()) {
                if *left > 0 {
                    *left -= 1;
                    removed.push(row.clone());
                }
            }
        }
        let columns_changed =
            current.get(name).is_some() && baseline[name]["columns"] != current[name]["columns"];
        if added.is_empty() && removed.is_empty() && !columns_changed {
            continue;
        }
        let mut entry = Map::new();
        if columns_changed {
            entry.insert("columns".into(), current[name]["columns"].clone());
        }
        if !added.is_empty() {
            entry.insert("added".into(), Value::Array(added));
        }
        if !removed.is_empty() {
            entry.insert("removed".into(), Value::Array(removed));
        }
        out.insert(name.clone(), Value::Object(entry));
    }
    Value::Object(out)
}

/// Where two table diffs differ, in the terms of the ledger (the table and the rows only one side has); None when equal.
pub fn describe_tables_difference(expected: &Value, actual: &Value) -> Option<String> {
    if expected == actual {
        return None;
    }
    let mut names: Vec<String> = [expected, actual]
        .iter()
        .flat_map(|v| v.as_object().into_iter().flat_map(|m| m.keys().cloned()))
        .collect();
    names.sort();
    names.dedup();
    for name in names {
        let (e, a) = (&expected[&name], &actual[&name]);
        if e == a {
            continue;
        }
        let rows = |v: &Value, list: &str| -> Vec<String> {
            v[list]
                .as_array()
                .map(|rows| rows.iter().map(Value::to_string).collect())
                .unwrap_or_default()
        };
        for list in ["added", "removed"] {
            let (want, got) = (rows(e, list), rows(a, list));
            let missing: Vec<&String> = want.iter().filter(|r| !got.contains(r)).collect();
            let extra: Vec<&String> = got.iter().filter(|r| !want.contains(r)).collect();
            if !missing.is_empty() || !extra.is_empty() {
                let clip = |text: &String| text.chars().take(240).collect::<String>();
                return Some(format!(
                    "table {name}, {list}: expected but missing {:?}; unexpected {:?}",
                    missing.iter().map(|r| clip(r)).collect::<Vec<_>>(),
                    extra.iter().map(|r| clip(r)).collect::<Vec<_>>()
                ));
            }
        }
        return Some(format!("table {name}: the diffs differ in their columns"));
    }
    Some("the diffs differ".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_diff_lists_added_and_removed_rows_and_leaves_equal_tables_out() {
        let baseline = json!({
            "a": {"columns": ["x"], "rows": [[1], [2]]},
            "b": {"columns": ["y"], "rows": [["same"]]}
        });
        let current = json!({
            "a": {"columns": ["x"], "rows": [[2], [3]]},
            "b": {"columns": ["y"], "rows": [["same"]]},
            "c": {"columns": ["z"], "rows": [[true]]}
        });
        let diff = diff_tables(&baseline, &current);
        assert_eq!(
            diff,
            json!({
                "a": {"added": [[3]], "removed": [[1]]},
                "c": {"columns": ["z"], "added": [[true]]}
            })
        );
        assert!(describe_tables_difference(&diff, &diff).is_none());
        let message = describe_tables_difference(&diff, &json!({})).unwrap();
        assert!(message.contains("table a"), "{message}");
    }
}

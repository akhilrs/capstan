//! The ledger's tables as the Node exporter dumps them and the difference against a baseline (the compact record of
//! format 2, test/daemon-transcript-export.ts): every table with rows, rows ordered by all columns,
//! `schema_migrations.applied_at` left out.

use rusqlite::types::ValueRef;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::Path;

/// DELIBERATE PIN (frozen Node ledger format; never follows the newest migration). The last migration Node knows. Ledgers the Rust build opens carry later migrations that no frozen Node export has, so
/// their `schema_migrations` rows are left out of a dump.
pub const NODE_LAST_MIGRATION: i64 = 37;

/// Tables only the Rust build has. A dump leaves one out while it is empty; a non-empty one stays in, so a replay that
/// writes rows there fails the comparison.
pub const RUST_ONLY_TABLES: [&str; 2] = ["cleanups", "cleanup_branches"];

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
            // Rust-only column (the frozen Node corpus predates message classes).
            .filter(|c| !(name == "messages" && c == "class"))
            // Rust-only columns of plans, revisions and reviews (plan amendments, review classes).
            .filter(|c| !(name == "plans" && c == "max_review_rounds"))
            .filter(|c| {
                !(name == "plan_revisions"
                    && matches!(c.as_str(), "amendment_json" | "amends_review_id"))
            })
            .filter(|c| !(name == "reviews" && c == "findings_class"))
            .filter(|c| {
                !(name == "prompt_relays"
                    && matches!(c.as_str(), "auto_window_id" | "auto_begin_at"))
            })
            .collect();
        let order: Vec<String> = (1..=columns.len()).map(|i| i.to_string()).collect();
        // The controller's prompt:relay grant (migration 0047) is Rust-only, like the later migrations.
        let only_node = match name.as_str() {
            "schema_migrations" => format!(" WHERE version <= {NODE_LAST_MIGRATION}"),
            "role_capabilities" => " WHERE NOT (role = 'controller' AND capability = 'prompt:relay')".to_string(),
            "capability_grants" => " WHERE NOT (capability = 'prompt:relay' AND actor_id IN (SELECT actor_id FROM actors WHERE role = 'controller'))".to_string(),
            _ => String::new(),
        };
        let sql = format!(
            "SELECT {} FROM {name}{only_node} ORDER BY {}",
            columns.join(", "),
            order.join(", ")
        );
        let mut statement = conn.prepare(&sql).unwrap();
        let rows: Vec<Value> = statement
            .query_map([], |row| {
                let mut values = Vec::new();
                for (i, column) in columns.iter().enumerate() {
                    values.push(match row.get_ref(i)? {
                        ValueRef::Null => Value::Null,
                        ValueRef::Integer(n) => Value::from(n),
                        ValueRef::Real(f) => capstan_kernel::plan_body::number_value(f),
                        ValueRef::Text(t) => {
                            let text = String::from_utf8_lossy(t).into_owned();
                            // A stored result holds message records, which carry the Rust-only `class`.
                            Value::String(if column == "result_json" {
                                (1..=20)
                                    .fold(text, |text, n| {
                                        text.replace(&format!("\"maxReviewRounds\":{n},"), "")
                                    })
                                    .replace("\"class\":\"action\",", "")
                                    .replace("\"class\":\"info\",", "")
                                    .replace("\"maxReviewRounds\":null,", "")
                                    .replace("\"design\":false,\"designDecision\":null,", "")
                                    .replace("\"amendment\":null,", "")
                                    .replace("\"amendsReviewId\":null,", "")
                            } else {
                                text
                            })
                        }
                        ValueRef::Blob(b) => json!({"$blob": capstan_kernel::canonical::hex(b)}),
                    });
                }
                if name == "messages" {
                    legacy_plan_texts(&columns, &mut values);
                }
                Ok(Value::Array(values))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect();
        // Empty tables are left out, which is all it takes for a Rust-only table to vanish from a dump until it has rows.
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

#[cfg(test)]
mod rust_only_tests {
    use super::*;

    /// The Rust build's rows after NODE_LAST_MIGRATION and its empty Rust-only tables do not show in a dump; a cleanup row does.
    #[test]
    fn rust_only_ledger_parts_stay_out_of_the_dump_until_they_hold_rows() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("ledger.sqlite");
        let conn = rusqlite::Connection::open(&file).unwrap();
        conn.execute_batch(&format!(
            "CREATE TABLE schema_migrations(version INTEGER, name TEXT, checksum TEXT, applied_at TEXT);
             INSERT INTO schema_migrations VALUES ({NODE_LAST_MIGRATION}, 'a', 'b', 'c'), ({}, 'd', 'e', 'f');
             CREATE TABLE cleanups(cleanup_id TEXT);
             CREATE TABLE cleanup_branches(branch TEXT);",
            NODE_LAST_MIGRATION + 1
        ))
        .unwrap();
        let dump = dump_tables(&file);
        assert_eq!(
            dump["schema_migrations"]["rows"],
            json!([[NODE_LAST_MIGRATION, "a", "b"]])
        );
        for table in RUST_ONLY_TABLES {
            assert!(dump.get(table).is_none(), "{table} is empty");
        }
        conn.execute("INSERT INTO cleanups VALUES ('c-1')", [])
            .unwrap();
        let dump = dump_tables(&file);
        assert_eq!(dump["cleanups"]["rows"], json!([["c-1"]]));
        assert!(dump.get("cleanup_branches").is_none());
    }
}

/// The review request of a plan and the needs-attention notice, as the frozen Node corpus has them (this build's brief adds
/// an exhaustive-list instruction and the finding line format; the notice names the limit). The actual side gets the Node
/// text and its hash back, so the corpus replays unchanged.
fn legacy_plan_texts(columns: &[String], values: &mut [Value]) {
    let (Some(body), Some(hash)) = (
        columns.iter().position(|c| c == "body"),
        columns.iter().position(|c| c == "body_hash"),
    ) else {
        return;
    };
    let Some(text) = values[body].as_str() else {
        return;
    };
    let first = text.lines().next().unwrap_or_default();
    let legacy = if let Some((plan, rest)) = first.split_once(" needs attention: it used ") {
        if !(plan.starts_with("Plan ") && rest.contains(" design review rounds (limit ")) {
            return;
        }
        format!("{plan} needs attention: it used 5 review rounds without a pass. It is a draft; decide whether to replace the architect or open a new plan.")
    } else if first.starts_with("Review request ") && first.contains(" for plan ") {
        let mut lines: Vec<&str> = text
            .lines()
            .filter(|line| {
                !line.starts_with("Give one exhaustive findings list")
                    && !line.starts_with("Write one finding per line")
                    && !line.starts_with("A finding about owns, depends_on")
            })
            .collect();
        let Some(last) = lines.last_mut() else {
            return;
        };
        *last = "Review only that plan. Do not edit any file. Answer exactly once with cstan review pass \"<text>\" or cstan review findings \"<text>\". Findings must say what is wrong and in which package.";
        lines.join("\n")
    } else {
        return;
    };
    values[hash] = Value::String(capstan_kernel::canonical::sha256(&legacy));
    values[body] = Value::String(legacy);
}

/// Fields only the Rust build has (the frozen Node corpus predates them): message records carry `class`, plan records
/// `maxReviewRounds`, package records `design` and `designDecision` (false and null unless a design package), the shown plan revision `amendment` and `amendsReviewId` (null unless amended), and the unread
/// summary `wakeCount` and `infoCount`. A comparison leaves them out of the actual side.
pub fn without_rust_only(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let message_record = map.contains_key("messageId") && map.contains_key("actionNeeded");
            let summary = map.contains_key("oldestQueuedAt") && map.contains_key("messageIds");
            let plan_record = map.contains_key("planId") && map.contains_key("approvedRevision");
            let plan_revision = map.contains_key("bodySha") && map.contains_key("amendsReviewId");
            let package_record = map.contains_key("packageId") && map.contains_key("progress");
            Value::Object(
                map.iter()
                    .filter(|(key, v)| {
                        !(message_record && key.as_str() == "class"
                            || summary && matches!(key.as_str(), "wakeCount" | "infoCount")
                            || plan_record && key.as_str() == "maxReviewRounds"
                            || package_record
                                && matches!(key.as_str(), "design" | "designDecision")
                                && (v.is_null() || v.as_bool() == Some(false))
                            || plan_revision
                                && matches!(key.as_str(), "amendment" | "amendsReviewId")
                                && v.is_null())
                    })
                    .map(|(key, v)| (key.clone(), without_rust_only(v)))
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(items.iter().map(without_rust_only).collect()),
        other => other.clone(),
    }
}

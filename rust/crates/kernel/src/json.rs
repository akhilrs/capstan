//! `JSON.stringify(value)` for a `serde_json::Value`, as Node writes stored JSON.
//!
//! The workspace builds serde_json with `preserve_order`, so an object read back from the ledger keeps its keys in the
//! order they were stored. Writing it back must order them the way V8 lists properties: array-index keys first in
//! ascending numeric order, then the rest in insertion order. (`canonical` is the sorted variant used for digests.)

use crate::canonical::{array_index, js_number, write_string};
use serde_json::Value;

/// `JSON.stringify(value)` (no indent): numbers as `String(n)`, strings escaped as JavaScript escapes them.
pub fn stringify(value: &Value) -> String {
    let mut out = String::new();
    write_value(&mut out, value);
    out
}

fn write_value(out: &mut String, value: &Value) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&js_number(n)),
        Value::String(s) => write_string(out, s),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_value(out, item);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut indexed: Vec<(u64, &String, &Value)> = Vec::new();
            let mut named: Vec<(&String, &Value)> = Vec::new();
            for (key, item) in map {
                match array_index(key) {
                    Some(n) => indexed.push((n, key, item)),
                    None => named.push((key, item)),
                }
            }
            indexed.sort_by_key(|(n, _, _)| *n);
            out.push('{');
            let entries = indexed.into_iter().map(|(_, k, v)| (k, v)).chain(named);
            for (i, (key, item)) in entries.enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_string(out, key);
                out.push(':');
                write_value(out, item);
            }
            out.push('}');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_insertion_order_with_index_keys_first() {
        let value: Value =
            serde_json::from_str(r#"{"b":1,"a":{"z":1,"y":2},"10":0,"2":[{"q":1,"p":2}],"01":3}"#)
                .unwrap();
        assert_eq!(
            stringify(&value),
            r#"{"2":[{"q":1,"p":2}],"10":0,"b":1,"a":{"z":1,"y":2},"01":3}"#
        );
    }
}

//! The parity export files (tests/parity/*.json, tests/parity-integrate/*.json) and how to read them. The unit tests of
//! the areas read them through `parity_group`, the integration tests through `unpack`: one implementation of the format.

use serde_json::Value;

/// The group file `name` (for example `ops`) of the Node parity export, with its dictionary expanded.
#[doc(hidden)]
pub fn parity_group(name: &str) -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/parity")
        .join(format!("{name}.json"));
    unpack(serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap())
}

/// Replaces every `{"$d": n}` of a packed export (`format` 2, see `pack` in test/kernel-parity-export.ts) by dictionary
/// entry `n` (an entry refers only to earlier ones) and drops the dictionary; any other value is returned as it is.
///
/// The exporter refuses a document that has a `$d` key of its own, so an object whose only key is `$d` is always a
/// reference.
#[doc(hidden)]
pub fn unpack(mut value: Value) -> Value {
    fn expand(value: &mut Value, dict: &[Value]) {
        match value {
            Value::Object(map) => {
                if let (1, Some(n)) = (map.len(), map.get("$d").and_then(Value::as_u64)) {
                    *value = dict[n as usize].clone();
                } else {
                    map.values_mut().for_each(|v| expand(v, dict));
                }
            }
            Value::Array(items) => items.iter_mut().for_each(|v| expand(v, dict)),
            _ => {}
        }
    }
    let packed = value["format"] == 2 && value["dict"].is_array();
    if !packed {
        return value;
    }
    let Some(Value::Array(entries)) = value.as_object_mut().and_then(|m| m.remove("dict")) else {
        return value;
    };
    let mut dict: Vec<Value> = Vec::with_capacity(entries.len());
    for mut entry in entries {
        expand(&mut entry, &dict);
        dict.push(entry);
    }
    expand(&mut value, &dict);
    value
}

//! The parity export files the unit tests of the areas read (tests/parity/*.json), expanded.

use serde_json::Value;

/// The group file `name` (for example `ops`) of the Node parity export, with its dictionary expanded (the same as
/// `unpack` in tests/common/mod.rs).
pub(crate) fn parity_group(name: &str) -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/parity")
        .join(format!("{name}.json"));
    let mut value: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let Some(Value::Array(entries)) = value.as_object_mut().and_then(|m| m.remove("dict")) else {
        return value;
    };
    let mut dict: Vec<Value> = Vec::new();
    for mut entry in entries {
        expand(&mut entry, &dict);
        dict.push(entry);
    }
    expand(&mut value, &dict);
    value
}

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

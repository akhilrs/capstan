//! Helpers shared by the parity tests: the fixture files the Node exporter writes (test/herdr-parity-export.ts) and the
//! JSON forms of the Rust values they are compared with.

#![allow(dead_code)]

use serde_json::{json, Map, Value};
use std::path::PathBuf;

pub fn parity_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("parity")
        .join(name)
}

pub fn load(name: &str) -> Value {
    let text = std::fs::read_to_string(parity_path(name))
        .unwrap_or_else(|error| panic!("{name} cannot be read ({error}); run the exporter"));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{name} is not JSON: {error}"))
}

/// A JavaScript number as JSON writes it: an integer when it has no fraction.
pub fn number(value: f64) -> Value {
    if value.is_nan() || value.is_infinite() {
        Value::Null
    } else if value.fract() == 0.0 && value.abs() < 9.2e18 {
        json!(value as i64)
    } else {
        json!(value)
    }
}

pub fn option_text(value: Option<String>) -> Value {
    value.map_or(Value::Null, Value::String)
}

pub fn object(pairs: Vec<(&str, Value)>) -> Value {
    let mut map = Map::new();
    for (key, value) in pairs {
        map.insert(key.to_string(), value);
    }
    Value::Object(map)
}

pub fn sha_hex(text: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `a` equals `b`, or panics naming `what` with both sides.
pub fn expect_eq(what: &str, actual: &Value, expected: &Value) {
    assert!(
        actual == expected,
        "{what} differs\n  rust: {actual}\n  node: {expected}"
    );
}

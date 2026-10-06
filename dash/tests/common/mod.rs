//! Helpers shared by the parity tests: the exported fixtures and a JSON comparison that ignores number formatting.
#![allow(dead_code)]

use std::fs;
use std::path::PathBuf;

use serde_json::Value;

pub fn fixture_directory() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("parity")
}

/// The fixture `name` (with or without `.json`) from `tests/parity`.
pub fn load_fixture(name: &str) -> Value {
    let file = if name.ends_with(".json") {
        name.to_string()
    } else {
        format!("{name}.json")
    };
    let path = fixture_directory().join(&file);
    let text = fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "cannot read {}: {e}; regenerate the fixtures",
            path.display()
        )
    });
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{file} is not JSON: {e}"))
}

/// The names of the per-case fixtures: every file except the shared helper fixtures.
pub fn case_names() -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(fixture_directory())
        .expect("tests/parity exists")
        .filter_map(|entry| {
            let name = entry.ok()?.file_name().into_string().ok()?;
            let stem = name.strip_suffix(".json")?.to_string();
            (!matches!(stem.as_str(), "format" | "actions" | "poller")).then_some(stem)
        })
        .collect();
    names.sort();
    names
}

fn difference(path: &str, actual: &Value, expected: &Value) -> Option<String> {
    match (actual, expected) {
        (Value::Number(a), Value::Number(b)) => {
            let (a, b) = (a.as_f64()?, b.as_f64()?);
            // -0 equals 0 under ==.
            (a != b).then(|| format!("{path}: {a} != {b}"))
        }
        (Value::Object(a), Value::Object(b)) => {
            let mut keys_a: Vec<&String> = a.keys().collect();
            let mut keys_b: Vec<&String> = b.keys().collect();
            keys_a.sort();
            keys_b.sort();
            if keys_a != keys_b {
                return Some(format!("{path}: keys {keys_a:?} != {keys_b:?}"));
            }
            keys_a
                .into_iter()
                .find_map(|key| difference(&format!("{path}.{key}"), &a[key], &b[key]))
        }
        (Value::Array(a), Value::Array(b)) => {
            if a.len() != b.len() {
                return Some(format!("{path}: length {} != {}", a.len(), b.len()));
            }
            a.iter()
                .zip(b)
                .enumerate()
                .find_map(|(i, (x, y))| difference(&format!("{path}[{i}]"), x, y))
        }
        _ => (actual != expected).then(|| format!("{path}: {actual} != {expected}")),
    }
}

/// Panics at the first difference: numbers compare as `f64` (-0 equals 0), objects by key set, arrays in order.
pub fn assert_json_eq(actual: &Value, expected: &Value) {
    if let Some(message) = difference("$", actual, expected) {
        panic!("JSON differs at {message}");
    }
}

/// `assert_json_eq` with a label naming the fixture part, so a failure says where to look.
pub fn assert_json_eq_in(label: &str, actual: &Value, expected: &Value) {
    if let Some(message) = difference(label, actual, expected) {
        panic!("JSON differs at {message}");
    }
}

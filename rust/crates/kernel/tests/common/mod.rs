#![allow(dead_code)]
//! Shared pieces of the kernel's integration tests: where the fixtures are and how to read them.

pub mod replay;

use serde_json::Value;
use std::path::{Path, PathBuf};

/// `rust/crates/kernel`.
pub fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The repository root (three levels above the crate).
pub fn repo_root() -> PathBuf {
    crate_dir().join("..").join("..").join("..")
}

pub fn parity_dir() -> PathBuf {
    crate_dir().join("tests").join("parity")
}

/// A JSON file; an export file that is packed (`format` 2, see `pack` in test/kernel-parity-export.ts) comes back expanded.
pub fn read_json(path: &Path) -> Value {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    unpack(value)
}

/// Replaces every `{"$d": n}` of a packed export by dictionary entry `n` (an entry refers only to earlier ones) and
/// drops the dictionary; any other value is returned as it is.
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

/// A fixture of the repository's `test/fixtures` tree.
pub fn fixture(relative: &str) -> Value {
    read_json(&repo_root().join("test").join("fixtures").join(relative))
}

/// A scratch directory private to the user (mode 0700), as the controller's state directory must be.
pub fn private_tempdir() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("a scratch directory");
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    dir
}

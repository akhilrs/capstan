//! Shared by the parity tests: the corpus files, a scratch project and a few byte helpers.
#![allow(dead_code)]
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use capstan_config::{
    load_role_config_with, record_current, trust_status_with, RoleConfig, TrustStatus, TrustStore,
};
use capstan_wire::js::{self, Value};
use sha2::{Digest, Sha256};

pub fn parity_file(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("parity")
        .join(name)
}

/// A corpus or expectation file, parsed with JavaScript's `JSON.parse` semantics (lone surrogates survive).
pub fn read_json(name: &str) -> Value {
    let text = fs::read_to_string(parity_file(name)).expect("parity file");
    js::parse(&text).expect("valid JSON")
}

pub fn items(value: &Value) -> &[Value] {
    match value {
        Value::Array(items) => items,
        other => panic!("not an array: {other:?}"),
    }
}

pub fn text(value: &Value, key: &str) -> Option<String> {
    match value.get(key) {
        Some(Value::String(s)) => Some(s.to_utf8().expect("well-formed text")),
        _ => None,
    }
}

pub fn number(value: &Value, key: &str) -> Option<f64> {
    match value.get(key) {
        Some(Value::Number(n)) => Some(*n),
        _ => None,
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn base64(text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let (mut accumulator, mut bits) = (0u32, 0u32);
    for byte in text.bytes().filter(|b| *b != b'=') {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => panic!("not base64"),
        };
        accumulator = (accumulator << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((accumulator >> bits) as u8);
            accumulator &= (1 << bits) - 1;
        }
    }
    out
}

/// Makes the project a corpus case describes under `directory`.
pub fn materialize(entries: &[Value], directory: &Path) {
    for entry in entries {
        let relative = text(entry, "path").expect("path");
        let target = directory.join(&relative);
        if matches!(entry.get("dir"), Some(Value::Bool(true))) {
            fs::create_dir_all(&target).unwrap();
        } else if let Some(link) = text(entry, "link") {
            let link = link.replace("$ROOT", directory.to_str().unwrap());
            std::os::unix::fs::symlink(link, &target).unwrap();
        } else {
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            let bytes: Vec<u8> = if let Some(encoded) = text(entry, "base64") {
                base64(&encoded)
            } else {
                match text(entry, "ref").as_deref() {
                    Some("starter") => capstan_config::starter::STARTER_CONFIG.as_bytes().to_vec(),
                    Some("designer") => {
                        capstan_config::starter::DESIGNER_PROMPT.as_bytes().to_vec()
                    }
                    _ => text(entry, "text").unwrap_or_default().into_bytes(),
                }
            };
            fs::write(&target, bytes).unwrap();
            let mode = number(entry, "mode").map_or(0o600, |m| m as u32);
            fs::set_permissions(&target, fs::Permissions::from_mode(mode)).unwrap();
        }
    }
}

/// A private trust store in its own temp directory, apart from any project folder; keep the guard alive while it is used.
pub fn temp_store() -> (tempfile::TempDir, TrustStore) {
    let holder = tempfile::tempdir().unwrap();
    let dir = holder.path().join("trust");
    fs::create_dir(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    let store = TrustStore::at(&dir);
    (holder, store)
}

/// Records the current configuration of `root` as trusted in `store` and checks that it reads so (`NotNeeded` when it
/// holds no checked key).
pub fn trusted_project(root: &Path, store: &TrustStore) {
    record_current(root, store).unwrap_or_else(|e| panic!("trusting {}: {e}", root.display()));
    let status = trust_status_with(root, store);
    assert!(
        matches!(status, TrustStatus::Trusted | TrustStatus::NotNeeded),
        "{status:?}"
    );
}

/// `load_role_config_with` after a best-effort trust of the files as they are now: a configuration the loader refuses
/// for its own reasons (a syntax error, a tracked local file) cannot be trusted and still gets the loader's answer.
pub fn load_trusted(root: &Path, store: &TrustStore) -> capstan_config::Result<RoleConfig> {
    let _ = record_current(root, store);
    load_role_config_with(root, store)
}

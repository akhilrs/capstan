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

pub fn read_json(path: &Path) -> Value {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
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

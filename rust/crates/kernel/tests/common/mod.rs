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

/// Strict mode, the default: a sequence or scenario that reaches an `Unported` operation (pending) is a failure, so
/// `cargo test` fails on any operation that is not ported. `CAPSTAN_KERNEL_PARITY_STRICT=0` allows pending while a
/// package is being developed; any other value, or none, is strict.
pub fn strict() -> bool {
    strict_from(
        std::env::var("CAPSTAN_KERNEL_PARITY_STRICT")
            .ok()
            .as_deref(),
    )
}

/// `strict` for a given value of CAPSTAN_KERNEL_PARITY_STRICT.
pub fn strict_from(value: Option<&str>) -> bool {
    value != Some("0")
}

/// The report lines that fail the run: every failed sequence, and in strict mode every pending one as well.
pub fn failing(reports: &[replay::Report], strict: bool) -> Vec<String> {
    reports
        .iter()
        .filter(|report| match report.status {
            replay::Status::Passed => false,
            replay::Status::Pending(_) => strict,
            replay::Status::Failed(_) => true,
        })
        .map(replay::Report::line)
        .collect()
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

pub use capstan_kernel::export_file::unpack;

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

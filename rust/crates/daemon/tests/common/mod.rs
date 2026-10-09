#![allow(dead_code)]
//! Shared pieces of the daemon's integration tests, read-only for every package: where the fixtures are, how a transcript
//! is read (the packed format 2 of test/daemon-transcript-export.ts), the scratch project and repository a scenario runs
//! in, the daemon process helper, the ledger's table dumps and the replay engine.

pub mod process;
pub mod project;
pub mod replay;
pub mod strict;
pub mod tables;

use capstan_kernel::SeededEnv;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// `rust/crates/daemon`.
pub fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The repository root (three levels above the crate).
pub fn repo_root() -> PathBuf {
    crate_dir().join("..").join("..").join("..")
}

pub fn transcripts_dir() -> PathBuf {
    crate_dir().join("tests").join("transcripts")
}

/// A transcript file with its dictionary expanded.
pub fn read_transcript(path: &Path) -> Value {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    capstan_kernel::export_file::unpack(value)
}

/// The kernel's seeded clock and randomness for `seed`: the Rust half of test/kernel-parity-hooks.ts.
pub fn seeded_env(seed: &str) -> Box<SeededEnv> {
    Box::new(SeededEnv::new(seed))
}

/// A scratch directory private to the user (mode 0700), as the controller's state directory must be.
pub fn private_tempdir() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("a scratch directory");
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    dir
}

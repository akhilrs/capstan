#![allow(dead_code)]
//! Shared pieces of the daemon's integration tests, read-only for every package: where the fixtures are, how a transcript
//! is read (the packed format 2 of test/daemon-transcript-export.ts), the scratch project and repository a scenario runs
//! in, the daemon process helper, the ledger's table dumps and the replay engine.

pub mod merge;
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
pub fn private_tempdir() -> PrivateDir {
    use std::os::unix::fs::PermissionsExt;
    let dir =
        tempfile::tempdir_in(capstan_testkit::short_temp_root()).expect("a scratch directory");
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    PrivateDir {
        _reaper: capstan_testkit::Reaper::new(dir.path()),
        dir,
    }
}

/// The scratch directory of [`private_tempdir`]; whatever still runs inside it (a daemon `ensure_daemon` detached) is killed
/// before the directory goes, however the test ends.
pub struct PrivateDir {
    _reaper: capstan_testkit::Reaper, // dropped first
    dir: tempfile::TempDir,
}

impl PrivateDir {
    pub fn path(&self) -> &std::path::Path {
        self.dir.path()
    }
}

/// A trust store in its own private folder, with the folder: the folder must outlive every use of the store.
pub fn test_trust_store() -> (PrivateDir, capstan_config::TrustStore) {
    let holder = private_tempdir();
    let store = capstan_config::TrustStore::at(holder.path());
    (holder, store)
}

/// `test_trust_store` with its folder kept until the test process ends, for a fixture that has no place to hold it.
pub fn kept_trust_store() -> capstan_config::TrustStore {
    kept_trust_store_at().1
}

/// `kept_trust_store` with the folder of the store, for a daemon process that gets it as `CAPSTAN_TRUST_DIR`.
pub fn kept_trust_store_at() -> (PathBuf, capstan_config::TrustStore) {
    static STORES: std::sync::Mutex<Vec<PrivateDir>> = std::sync::Mutex::new(Vec::new());
    let (holder, store) = test_trust_store();
    let path = holder.path().to_path_buf();
    STORES.lock().unwrap().push(holder);
    (path, store)
}

/// Records the checked keys of the `capstan.toml` in `root` as trusted in `store`, so a gated load of it succeeds. Call it
/// after the last write of the configuration; a rewrite needs a new call.
pub fn trusted_project(root: &Path, store: &capstan_config::TrustStore) {
    capstan_config::record_current(root, store).unwrap_or_else(|e| {
        panic!("trusting the project {}: {e:?}", root.display());
    });
}

/// `trusted_project` with a store in its own temporary folder, kept until the test process ends, so no trust record
/// lands in `root`: use it where `root` is committed to git afterwards and the commit ids must not change. Returns the
/// store, which the loads and the `DaemonOptions` of the test must use.
pub fn trusted_project_outside(root: &Path) -> capstan_config::TrustStore {
    trusted_project_in_kept_store(root).1
}

/// `trusted_project_outside` that also returns the folder of the store.
pub fn trusted_project_in_kept_store(root: &Path) -> (PathBuf, capstan_config::TrustStore) {
    let (path, store) = kept_trust_store_at();
    trusted_project(root, &store);
    (path, store)
}

/// Writes `text` as the `capstan.toml` in `root` (mode 0600) and records nothing, so the project reads as not trusted
/// when the text has a key the trust gate checks.
pub fn write_config_untrusted(root: &Path, text: &str) {
    use std::os::unix::fs::PermissionsExt;
    let file = root.join("capstan.toml");
    std::fs::write(&file, text).expect("the configuration is written");
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
}

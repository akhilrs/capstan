//! The size gate of the daemon phase's fixtures: every file that is not Rust source under the tests directories of the
//! daemon, herdr, launcher and operator crates (the transcripts above all) stays under 6 MiB in total, which is what forces
//! compact scenarios.

use std::path::{Path, PathBuf};

const BUDGET_BYTES: u64 = 6 * 1024 * 1024;

fn crates_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// The size of every file under `directory` that is not a `.rs` file, with its path.
fn fixtures_under(directory: &Path, into: &mut Vec<(PathBuf, u64)>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.map(Result::unwrap) {
        let path = entry.path();
        let kind = entry.file_type().unwrap();
        if kind.is_dir() {
            fixtures_under(&path, into);
        } else if kind.is_file() && path.extension().is_none_or(|e| e != "rs") {
            into.push((path, entry.metadata().unwrap().len()));
        }
    }
}

fn fixtures() -> Vec<(PathBuf, u64)> {
    let mut files = Vec::new();
    for name in ["daemon", "herdr", "launcher", "operator"] {
        fixtures_under(&crates_dir().join(name).join("tests"), &mut files);
    }
    files.sort();
    files
}

#[test]
fn the_fixtures_stay_under_the_budget() {
    let files = fixtures();
    let total: u64 = files.iter().map(|(_, size)| size).sum();
    let listing: Vec<String> = files
        .iter()
        .map(|(path, size)| format!("{size:>10}  {}", path.display()))
        .collect();
    assert!(
        total < BUDGET_BYTES,
        "the fixtures are {total} bytes, the budget is {BUDGET_BYTES}:\n{}",
        listing.join("\n")
    );
}

#[test]
fn the_transcripts_are_counted() {
    let files = fixtures();
    assert!(
        files
            .iter()
            .any(|(path, _)| path.ends_with("transcripts/core.json")),
        "the budget does not see tests/transcripts/core.json"
    );
}

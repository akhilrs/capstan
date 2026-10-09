//! Replays every sequence of test/launcher-sequences (exported from Node by test/launcher-parity-export.ts into
//! tests/parity) on the Rust launcher and compares each step: what it returned or refused with, the calls it made to
//! Herdr, the git commands it ran, the log events it wrote, the state of git and the change in the ledger.
//!
//! One test runs every sequence one after the other: the replay puts a `git` shim first on PATH, which is the process's.
mod common;

use common::{engine, parity_dir, read_json};

#[test]
fn every_sequence_replays_with_the_node_launcher_outcome() {
    let mut files: Vec<_> = std::fs::read_dir(parity_dir())
        .expect("the parity fixtures exist")
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no parity fixtures");
    let mut passed = 0;
    let mut failed = Vec::new();
    for file in files {
        let group = read_json(&file);
        let name = group["group"].as_str().unwrap_or("?").to_string();
        for sequence in group["sequences"].as_array().cloned().unwrap_or_default() {
            let label = format!("{name}/{}", sequence["name"].as_str().unwrap_or("?"));
            match engine::run_sequence(&sequence) {
                Ok(()) => passed += 1,
                Err(message) => failed.push(format!("{label}: {message}")),
            }
        }
    }
    println!(
        "{passed} sequences passed, 0 pending, {} failed",
        failed.len()
    );
    assert!(failed.is_empty(), "\n{}", failed.join("\n\n"));
}

//! Replays every sequence of test/launcher-sequences (exported from Node by test/launcher-parity-export.ts into
//! tests/parity) on the Rust launcher and compares each step: what it returned or refused with, the calls it made to
//! Herdr, the git commands it ran, the log events it wrote, the state of git and the change in the ledger.
//!
//! One test runs every sequence one after the other: the replay puts a `git` shim first on PATH, which is the process's.
mod common;

use common::{case_names, divergences_dir, engine, parity_dir, read_json};

#[test]
fn every_sequence_replays_with_the_node_launcher_outcome() {
    let mut files: Vec<_> = std::fs::read_dir(parity_dir())
        .expect("the parity fixtures exist")
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no parity fixtures");
    // Where Rust differs from Node, an overlay replaces the whole expected output of the case.
    let overlays = capstan_parity_overlay::load(&divergences_dir(), &parity_dir(), &case_names)
        .unwrap_or_else(|e| panic!("{e}"));
    let mut replaced = 0;
    let mut passed = 0;
    let mut failed = Vec::new();
    for file in files {
        let group = read_json(&file);
        let name = group["group"].as_str().unwrap_or("?").to_string();
        for sequence in group["sequences"].as_array().cloned().unwrap_or_default() {
            let case = sequence["name"].as_str().unwrap_or("?").to_string();
            let label = format!("{name}/{case}");
            let fixture = file.file_name().unwrap().to_string_lossy().into_owned();
            let sequence = match overlays.expected(&fixture, &case) {
                Some(expected) => {
                    replaced += 1;
                    expected.clone()
                }
                None => sequence,
            };
            match engine::run_sequence(&sequence) {
                Ok(()) => passed += 1,
                Err(message) => failed.push(format!("{label}: {message}")),
            }
        }
    }
    assert_eq!(replaced, overlays.len(), "every overlay replaces a case");
    println!(
        "{passed} sequences passed ({replaced} by overlay), 0 pending, {} failed",
        failed.len()
    );
    assert!(failed.is_empty(), "\n{}", failed.join("\n\n"));
}

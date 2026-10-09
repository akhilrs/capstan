//! The gate that keeps the Rust kernel at parity: strict mode (a pending sequence fails), every `ControllerCore` method in
//! at least one sequence, no operation that answers `Unported`, and the size of the committed parity files.
//!
//! `cargo test --locked` runs this in check:dash; tests/replay.rs and tests/integrate.rs replay the sequences and scenarios
//! in strict mode by default.
//!
//! On the packed export format (`pack` in test/kernel-parity-export.ts, `unpack` in `capstan_kernel::export_file`, one
//! implementation for the library's unit tests and these tests): a `{"$d": n}` object is a dictionary reference only
//! because the exporter refuses a document that has a `$d` key of its own, so no stored value can be misread as one.
//! A dump is compared as the rows added to and removed from the baseline ledger; the order of the baseline's unchanged
//! rows is therefore not compared (a table is dumped ordered by all its columns, so it cannot differ in an interesting way).

mod common;

use capstan_kernel::{Core, KernelOptions, SeededEnv};
use common::replay::{Report, Status};
use common::{crate_dir, failing, parity_dir, read_json, strict_from};
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// Everything the committed parity files may hold: the steps' operations of a sequence, a scenario or a group.
fn operations_of(value: &Value, into: &mut BTreeMap<String, usize>) {
    for list in ["sequences", "scenarios"] {
        for entry in value[list].as_array().into_iter().flatten() {
            for step in entry["steps"].as_array().into_iter().flatten() {
                if let Some(op) = step["op"].as_str() {
                    *into.entry(op.to_string()).or_default() += 1;
                }
            }
        }
    }
}

/// The count of steps per operation over every group in tests/parity and every scenario in tests/parity-integrate.
fn covered_operations() -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    let mut files: Vec<_> = std::fs::read_dir(parity_dir())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no parity group files");
    for file in files {
        operations_of(&read_json(&file), &mut counts);
    }
    operations_of(
        &read_json(
            &crate_dir()
                .join("tests")
                .join("parity-integrate")
                .join("scenarios.json"),
        ),
        &mut counts,
    );
    counts
}

fn core_methods() -> Vec<String> {
    read_json(
        &crate_dir()
            .join("tests")
            .join("common")
            .join("core-methods.json"),
    )
    .as_array()
    .unwrap()
    .iter()
    .filter_map(|name| name.as_str().map(str::to_string))
    .collect()
}

#[test]
fn every_controller_core_method_is_in_a_sequence() {
    let covered = covered_operations();
    let methods = core_methods();
    let mut missing = Vec::new();
    println!("coverage of the {} ControllerCore methods:", methods.len());
    for method in &methods {
        let steps = covered.get(method).copied().unwrap_or(0);
        println!("  {method}: {steps} step(s)");
        if steps == 0 {
            missing.push(method.as_str());
        }
    }
    println!(
        "{} of {} methods are in a sequence",
        methods.len() - missing.len(),
        methods.len()
    );
    assert!(
        missing.is_empty(),
        "ControllerCore methods in no sequence (add one to test/kernel-sequences): {}",
        missing.join(", ")
    );
}

#[test]
fn a_sequence_names_only_operations_that_exist() {
    let methods = core_methods();
    let known = ["open", "openReadOnly", "close", "dump", "git.commit"];
    let unknown: Vec<String> = covered_operations()
        .into_keys()
        .filter(|op| {
            !methods.contains(op) && !known.contains(&op.as_str()) && !op.starts_with("git.")
        })
        .collect();
    assert!(
        unknown.is_empty(),
        "operations that are not ControllerCore methods: {}",
        unknown.join(", ")
    );
}

#[test]
fn no_controller_core_method_is_unported() {
    let scratch = common::private_tempdir();
    let project: capstan_kernel::types::InitialProject = serde_json::from_value(json!({
        "projectId": "proj1", "name": "Strict", "ownerCredential": "owner-credential-0123456789-abcdefghijklmnopqrstuvwxyz",
        "initialInputs": [
            {"kind": "project_config", "content": {}}, {"kind": "task_brief", "content": {}},
            {"kind": "acceptance_criteria", "content": ["a"]}, {"kind": "policy", "content": {}}, {"kind": "plan", "content": {}}]
    }))
    .unwrap();
    let core = Core::open(
        scratch.path(),
        &project,
        &KernelOptions::default(),
        Box::new(SeededEnv::new("strict")),
    )
    .unwrap();
    let mut unported = Vec::new();
    for name in core_methods() {
        if ["open", "openReadOnly", "close"].contains(&name.as_str()) {
            continue;
        }
        if let Err(error) = core.dispatch(&name, &[]) {
            if error.is_unported() {
                unported.push(format!("{name}: {}", error.message()));
            }
        }
    }
    assert!(unported.is_empty(), "{}", unported.join("\n"));
}

#[test]
fn strict_mode_is_the_default_and_only_an_explicit_zero_relaxes_it() {
    assert!(strict_from(None));
    assert!(strict_from(Some("1")));
    assert!(strict_from(Some("")));
    assert!(strict_from(Some("false")));
    assert!(!strict_from(Some("0")));
}

#[test]
fn a_pending_sequence_fails_in_strict_mode_and_a_failed_one_always_does() {
    let report = |sequence: &str, status: Status| Report {
        group: "g".into(),
        sequence: sequence.into(),
        status,
    };
    let reports = [
        report("passes", Status::Passed),
        report("pending", Status::Pending("agents.endAgent".into())),
        report("fails", Status::Failed("outcome differs".into())),
    ];
    assert_eq!(failing(&reports, true).len(), 2);
    let relaxed = failing(&reports, false);
    assert_eq!(relaxed.len(), 1);
    assert!(relaxed[0].starts_with("g/fails: FAILED"), "{relaxed:?}");
    assert!(failing(&reports[..1], true).is_empty());
}

fn has_reference(value: &Value) -> bool {
    match value {
        Value::Object(map) => map.contains_key("$d") || map.values().any(has_reference),
        Value::Array(items) => items.iter().any(has_reference),
        _ => false,
    }
}

#[test]
fn unpacking_leaves_no_dictionary_reference() {
    for entry in std::fs::read_dir(parity_dir()).unwrap() {
        let path = entry.unwrap().path();
        let value = read_json(&path);
        assert!(value.get("dict").is_none(), "{}", path.display());
        assert!(!has_reference(&value), "{}", path.display());
    }
}

/// The committed parity files are read by every `cargo test`; they stay small.
const PARITY_BUDGET_BYTES: u64 = 8 * 1024 * 1024;

#[test]
fn the_committed_parity_files_stay_within_the_budget() {
    let mut total = 0;
    for directory in ["parity", "parity-integrate"] {
        for entry in std::fs::read_dir(crate_dir().join("tests").join(directory)).unwrap() {
            let entry = entry.unwrap();
            let size = entry.metadata().unwrap().len();
            println!(
                "{directory}/{}: {size} bytes",
                entry.file_name().to_string_lossy()
            );
            total += size;
        }
    }
    println!("{total} bytes in all");
    assert!(
        total < PARITY_BUDGET_BYTES,
        "the parity files total {total} bytes, over the {PARITY_BUDGET_BYTES} budget"
    );
}

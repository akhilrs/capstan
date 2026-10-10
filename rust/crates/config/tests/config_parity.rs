//! The configuration corpus (the Node tests' cases, TOML edge cases and the starter) against what the Node loader made of
//! each, recorded by test/config-parity-export.ts: the same RoleConfig JSON (by SHA-256 and length), the same
//! `ConfigError` text, and for invalid TOML the line and column Node reports.
mod common;

use std::collections::HashMap;

use capstan_config::{load_role_config, ConfigError};
use capstan_wire::js;
use common::*;

#[test]
fn every_corpus_case_gives_the_result_node_gave() {
    let corpus = read_json("config-corpus.json");
    let expected = read_json("config-expected.json");
    let mut by_name: HashMap<String, &capstan_wire::js::Value> = items(&expected)
        .iter()
        .map(|entry| (text(entry, "name").unwrap(), entry.get("outcome").unwrap()))
        .collect();
    // Where Rust deliberately differs (the retired [daemon] table), an overlay replaces the expected outcome of the case.
    let overlays = capstan_parity_overlay::load(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/divergences"),
        &parity_file(""),
        &|fixture| {
            let value = js::parse(&std::fs::read_to_string(fixture).map_err(|e| e.to_string())?)
                .map_err(|e| format!("{e:?}"))?;
            Ok(items(&value)
                .iter()
                .filter_map(|case| text(case, "name"))
                .collect())
        },
    )
    .unwrap();
    let overlaid: Vec<(String, capstan_wire::js::Value)> =
        ["config-corpus.json", "config-edge-cases.json"]
            .iter()
            .flat_map(|fixture| {
                let names: Vec<String> = items(&read_json(fixture))
                    .iter()
                    .filter_map(|case| text(case, "name"))
                    .collect();
                names.into_iter().filter_map(|name| {
                    overlays.expected(fixture, &name).map(|outcome| {
                        (
                            name,
                            js::parse(&outcome.to_string()).expect("an overlay outcome"),
                        )
                    })
                })
            })
            .collect();
    for (name, outcome) in &overlaid {
        by_name.insert(name.clone(), outcome);
    }
    assert!(!overlaid.is_empty(), "the [daemon] overlays were not read");
    let mut cases = items(&corpus).to_vec();
    // The cases built by the exporter itself are in the expectations only; their files are rebuilt here from the same
    // list, so they are read from the second corpus file.
    cases.extend(items(&read_json("config-edge-cases.json")).iter().cloned());
    let mut failures = Vec::new();
    let mut checked = 0;
    for case in &cases {
        let name = text(case, "name").unwrap();
        let want = by_name
            .get(&name)
            .unwrap_or_else(|| panic!("no expectation for {name}"));
        let directory = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(directory.path()).unwrap();
        let entries = match case.get("entries") {
            Some(capstan_wire::js::Value::Array(entries)) => entries,
            _ => panic!("entries"),
        };
        materialize(entries, &root);
        let got = load_role_config(&root);
        checked += 1;
        let kind = text(want, "kind").unwrap();
        let restore = |s: &str| s.replace(root.to_str().unwrap(), "$ROOT");
        match (kind.as_str(), &got) {
            ("ok", Ok(config)) => {
                let json = restore(&config.to_json());
                if text(want, "sha256").unwrap() != sha256_hex(json.as_bytes())
                    || number(want, "bytes").unwrap() as usize != json.len()
                {
                    failures.push(format!(
                        "{name}: the JSON differs from Node's (run --show-config {name})\n{json}"
                    ));
                }
            }
            ("error", Err(ConfigError::Invalid(message))) => {
                let message = restore(message);
                if message != text(want, "message").unwrap() {
                    failures.push(format!(
                        "{name}: error text differs\n  rust: {message}\n  node: {}",
                        text(want, "message").unwrap()
                    ));
                }
            }
            ("parse-error", Err(ConfigError::Invalid(message))) => {
                if *message != text(want, "message").unwrap() {
                    failures.push(format!(
                        "{name}: TOML error differs\n  rust: {message}\n  node: {}",
                        text(want, "message").unwrap()
                    ));
                }
            }
            ("unexpected", Err(_)) => {}
            (_, other) => failures.push(format!("{name}: node {kind}, rust {other:?}")),
        }
    }
    assert!(checked > 600, "only {checked} cases");
    assert!(
        failures.is_empty(),
        "{} of {checked} cases differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn the_starter_and_designer_prompt_are_the_tracked_bytes() {
    assert_eq!(
        capstan_config::starter::DESIGNER_PROMPT,
        std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../roles/designer.md"
        ))
        .unwrap()
    );
}

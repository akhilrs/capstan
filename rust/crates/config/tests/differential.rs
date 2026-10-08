//! The differential TOML run. `node dist/test/config-parity-export.js --differential <count> <seed> <file>` writes mutated
//! configurations with what Node (smol-toml and the whole loader) makes of each; with CAPSTAN_CONFIG_DIFFERENTIAL=<file>
//! this test classifies the same bytes with the Rust loader and counts the agreements. A case Rust reads and Node refuses
//! is a failure (the native `config check` would succeed where Node errors); so is any difference in what the loader makes
//! of text both read. Rust refusing text Node reads is counted: such a command is left to Node.
mod common;

use capstan_config::{parse_config, ConfigError};
use capstan_wire::js;
use common::*;

#[test]
fn rust_and_node_classify_the_mutated_configs_alike() {
    let Ok(file) = std::env::var("CAPSTAN_CONFIG_DIFFERENTIAL") else {
        eprintln!("CAPSTAN_CONFIG_DIFFERENTIAL is not set: the differential run is skipped");
        return;
    };
    let project = tempfile::tempdir().unwrap();
    let project = std::fs::canonicalize(project.path()).unwrap();
    let (mut total, mut both_parse, mut both_reject, mut rust_only_rejects, mut compared) =
        (0, 0, 0, 0, 0);
    let mut rust_accepts_node_rejects = Vec::new();
    let mut rust_defers_by_rule = 0;
    let mut loader_differs = Vec::new();
    let mut rejected_examples: Vec<String> = Vec::new();
    for line in std::fs::read_to_string(file).unwrap().lines() {
        let case = js::parse(line).unwrap();
        let index = number(&case, "index").unwrap() as usize;
        let bytes = base64(&text(&case, "base64").unwrap());
        let node_parses = text(&case, "toml").unwrap() == "ok";
        let rust = parse_config(&bytes, &project);
        total += 1;
        let rust_parse_error = matches!(rust, Err(ConfigError::Parse(_)));
        if let Err(ConfigError::Parse(error)) = &rust {
            if error.detail.starts_with("known disagreement") {
                rust_defers_by_rule += 1;
            }
        }
        match (node_parses, rust_parse_error) {
            (false, false) => rust_accepts_node_rejects.push(index),
            (false, true) => both_reject += 1,
            (true, true) => {
                rust_only_rejects += 1;
                if rejected_examples.len() < 30 {
                    if let Err(ConfigError::Parse(error)) = &rust {
                        rejected_examples.push(format!("#{index}: {}", error.detail));
                    }
                }
            }
            (true, false) => {
                both_parse += 1;
                let loader = case.get("loader").unwrap();
                let want_ok = text(loader, "kind").unwrap() == "ok";
                compared += 1;
                match (&rust, want_ok) {
                    (Ok(config), true) => {
                        let json = config.to_json();
                        if json != text(loader, "json").unwrap() {
                            loader_differs.push(format!("#{index}: JSON differs"));
                        }
                    }
                    (Err(ConfigError::Invalid(message)), false) => {
                        if *message != text(loader, "message").unwrap() {
                            loader_differs.push(format!(
                                "#{index}: rust `{message}` node `{}`",
                                text(loader, "message").unwrap()
                            ));
                        }
                    }
                    (other, _) => loader_differs
                        .push(format!("#{index}: rust {other:?} vs node ok={want_ok}")),
                }
            }
        }
    }
    eprintln!(
        "differential: {total} cases; both reject {both_reject}; both parse {both_parse} (loader results compared: {compared}); Rust refuses what Node reads {rust_only_rejects}; Rust refuses by a disagreement rule {rust_defers_by_rule}; Rust reads what Node refuses {}",
        rust_accepts_node_rejects.len()
    );
    for example in &rejected_examples {
        eprintln!("  Rust-only rejection: {example}");
    }
    if !rust_accepts_node_rejects.is_empty() {
        eprintln!("DANGEROUS {rust_accepts_node_rejects:?}");
    }
    assert!(
        rust_accepts_node_rejects.is_empty(),
        "Rust reads {} cases Node refuses",
        rust_accepts_node_rejects.len()
    );
    assert!(
        loader_differs.is_empty(),
        "{} loader results differ:\n{}",
        loader_differs.len(),
        loader_differs[..loader_differs.len().min(20)].join("\n")
    );
}

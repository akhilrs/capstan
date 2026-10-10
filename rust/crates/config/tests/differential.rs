//! The differential TOML run. `node dist/test/config-parity-export.js --differential <count> <seed> <file>` writes mutated
//! configurations with what Node (smol-toml and the whole loader) makes of each; with CAPSTAN_CONFIG_DIFFERENTIAL=<file>
//! this test classifies the same bytes with the Rust loader. Rust reads exactly what Node reads, so every case must agree:
//! a text Node refuses as TOML is refused by Rust with the same words (the line and column are checked against
//! `smol-toml` itself by `toml_positions.rs`), and a text both read gives the same RoleConfig JSON or the same
//! `ConfigError` text.
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
    let (mut total, mut toml_errors, mut loaded, mut loader_errors) = (0, 0, 0, 0);
    let mut differs = Vec::new();
    for line in std::fs::read_to_string(file).unwrap().lines() {
        let case = js::parse(line).unwrap();
        let index = number(&case, "index").unwrap() as usize;
        let bytes = base64(&text(&case, "base64").unwrap());
        let node_parses = text(&case, "toml").unwrap() == "ok";
        let rust = parse_config(&bytes, &project);
        total += 1;
        if !node_parses {
            toml_errors += 1;
            match &rust {
                Err(ConfigError::Invalid(message))
                    if message.starts_with("capstan.toml is not valid TOML at line ") => {}
                other => differs.push(format!("#{index}: node refuses the TOML, rust {other:?}")),
            }
            continue;
        }
        let loader = case.get("loader").unwrap();
        match (&rust, text(loader, "kind").unwrap().as_str()) {
            (Ok(config), "ok") => {
                loaded += 1;
                if config.to_json() != text(loader, "json").unwrap() {
                    differs.push(format!("#{index}: JSON differs"));
                }
            }
            (Err(ConfigError::Invalid(message)), "error") => {
                loader_errors += 1;
                if *message != text(loader, "message").unwrap() {
                    differs.push(format!(
                        "#{index}: rust `{message}` node `{}`",
                        text(loader, "message").unwrap()
                    ));
                }
            }
            (other, kind) => differs.push(format!("#{index}: rust {other:?}, node {kind}")),
        }
    }
    eprintln!(
        "differential: {total} cases; node refuses the TOML {toml_errors}; both load {loaded}; both refuse in the loader {loader_errors}; differ {}",
        differs.len()
    );
    for example in differs.iter().take(20) {
        eprintln!("  {example}");
    }
    assert!(differs.is_empty(), "{} cases differ", differs.len());
}

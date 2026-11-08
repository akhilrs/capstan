//! Model values and the host kinds they fit.
use capstan_config::model_fit::{check_model_value, fit_for_role, model_fits_host};
use capstan_config::parse_config;
use std::path::Path;

const CONFIG: &str = r#"schema_version = 1
[nexora]
track = "never"
[hosts.claude]
kind = "claude"
[hosts.cx]
kind = "codex"
[roles.pm]
kind = "PM"
host = "claude"
[roles.dev]
kind = "Developer"
host = "cx"
[roles.dev2]
kind = "Developer"
host = "claude"
"#;

#[test]
fn claude_models_fit_claude_and_not_codex() {
    for model in [
        "claude-opus-5-5",
        "Opus",
        "opus[1m]",
        "OPUSPLAN",
        "Claude-X",
    ] {
        assert_eq!(model_fits_host("claude", model), Ok(()), "{model}");
        assert_eq!(
            model_fits_host("codex", model),
            Err(format!(
                "model {model} is a claude model and host codex is codex; use a codex model"
            )),
            "{model}"
        );
    }
}

#[test]
fn codex_models_are_refused_on_claude() {
    assert_eq!(
        model_fits_host("claude", "gpt-5"),
        Err("model gpt-5 is a codex model and host claude is claude; use a claude model".into())
    );
    assert_eq!(model_fits_host("codex", "gpt-5"), Ok(()));
}

#[test]
fn multi_provider_hosts_take_every_value() {
    for host in ["omp", "pi"] {
        for model in ["gpt-5", "claude-opus-5-5", "opus", "any/thing"] {
            assert_eq!(model_fits_host(host, model), Ok(()), "{host} {model}");
        }
    }
}

#[test]
fn an_unclaimed_id_fits_claude_and_codex() {
    for host in ["claude", "codex"] {
        assert_eq!(model_fits_host(host, "mystery-7"), Ok(()), "{host}");
    }
}

#[test]
fn check_model_value_applies_the_toml_rules() {
    assert_eq!(check_model_value("claude-opus-5-5"), Ok(()));
    assert!(check_model_value("-x").is_err());
    assert!(check_model_value("").is_err());
    assert_eq!(check_model_value(&"a".repeat(100)), Ok(()));
    assert!(check_model_value(&"a".repeat(101)).is_err());
    assert!(check_model_value("a\nb").is_err());
    assert!(check_model_value("sk-abcdefghijklmnop").is_err());
}

#[test]
fn fit_for_role_uses_the_hosts_kind() {
    let config = parse_config(CONFIG.as_bytes(), Path::new("/nonexistent")).expect("config");
    assert_eq!(fit_for_role(&config, "dev2", "opus"), Ok(()));
    assert_eq!(
        fit_for_role(&config, "dev", "opus"),
        Err("model opus is a claude model and host cx is codex; use a codex model".into())
    );
    assert_eq!(fit_for_role(&config, "dev", "gpt-5"), Ok(()));
    assert!(fit_for_role(&config, "nobody", "gpt-5").is_err());
}

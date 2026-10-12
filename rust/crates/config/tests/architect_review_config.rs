//! `[architect] max_review_rounds` and `plan_amend`: defaults, bounds and the JSON members.
use capstan_config::{parse_config, ConfigError, RoleConfig};
use std::path::Path;

const BASE: &str = r#"schema_version = 1
[nexora]
track = "never"
[hosts.claude]
kind = "claude"
[roles.pm]
kind = "PM"
host = "claude"
[roles.reviewer]
kind = "Verifier"
host = "claude"
[roles.architect]
kind = "Developer"
host = "claude"
deny = ["Write", "Edit"]
[architect]
enabled = true
"#;

fn load(extra: &str) -> Result<RoleConfig, String> {
    let text = format!("{BASE}{extra}");
    parse_config(text.as_bytes(), Path::new("/nonexistent")).map_err(|error| match error {
        ConfigError::Invalid(message) => message,
        other => format!("{other:?}"),
    })
}

#[test]
fn the_defaults_are_five_rounds_and_amend_on() {
    let config = load("").expect("loads");
    assert_eq!(config.architect.max_review_rounds, 5);
    assert!(config.architect.plan_amend);
    let json = config.to_json();
    assert!(json.contains("\"maxReviewRounds\":5") || json.contains("\"maxReviewRounds\": 5"));
    assert!(json.contains("\"planAmend\":true") || json.contains("\"planAmend\": true"));
}

#[test]
fn valid_values_resolve() {
    for rounds in [1, 7, 20] {
        let config = load(&format!(
            "max_review_rounds = {rounds}\nplan_amend = false\n"
        ))
        .expect("loads");
        assert_eq!(config.architect.max_review_rounds, rounds);
        assert!(!config.architect.plan_amend);
    }
}

#[test]
fn out_of_range_and_non_integer_rounds_are_refused_by_name() {
    for value in ["0", "21", "-1", "2.5", "\"5\"", "true"] {
        let message = load(&format!("max_review_rounds = {value}\n")).expect_err(value);
        assert!(
            message.contains("architect.max_review_rounds"),
            "{value}: {message}"
        );
    }
}

#[test]
fn plan_amend_accepts_booleans_only() {
    for value in ["1", "\"true\"", "0"] {
        let message = load(&format!("plan_amend = {value}\n")).expect_err(value);
        assert!(
            message.contains("architect.plan_amend"),
            "{value}: {message}"
        );
    }
}

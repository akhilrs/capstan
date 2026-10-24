//! `[ledger] retention_days`: default, range and refusals. Rust-only: Node has no such key.
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
"#;

fn load(extra: &str) -> Result<RoleConfig, String> {
    let text = format!("{BASE}{extra}");
    parse_config(text.as_bytes(), Path::new("/nonexistent")).map_err(|error| match error {
        ConfigError::Invalid(message) => message,
        other => format!("{other:?}"),
    })
}

#[test]
fn the_default_is_thirty_days_and_appears_in_the_json() {
    let config = load("").expect("loads");
    assert_eq!(config.ledger.retention_days, 30);
    assert!(config.to_json().contains("\"retentionDays\": 30"));
    assert_eq!(
        load("[ledger]\nretention_days = 30\n").unwrap().to_json(),
        config.to_json()
    );
}

#[test]
fn each_value_in_range_resolves() {
    for days in [0, 1, 90, 3650] {
        let config = load(&format!("[ledger]\nretention_days = {days}\n")).expect("loads");
        assert_eq!(config.ledger.retention_days, days);
    }
}

#[test]
fn values_out_of_range_or_of_the_wrong_type_are_refused() {
    for text in [
        "[ledger]\nretention_days = -1\n",
        "[ledger]\nretention_days = 3651\n",
        "[ledger]\nretention_days = \"30\"\n",
        "[ledger]\nretention_days = 1.5\n",
        "[ledger]\nretention_days = true\n",
    ] {
        let message = load(text).expect_err("refused");
        assert!(
            message.contains("ledger.retention_days"),
            "{text}: {message}"
        );
    }
}

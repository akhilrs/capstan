//! `[architect] stale_plan_days`: default, bounds and the JSON member.
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
fn the_default_is_fourteen_days_and_the_json_carries_it() {
    let config = load("").expect("loads");
    assert_eq!(config.architect.stale_plan_days, 14);
    let json = config.to_json();
    assert!(json.contains("\"stalePlanDays\":14") || json.contains("\"stalePlanDays\": 14"));
}

#[test]
fn zero_and_the_maximum_are_accepted() {
    for days in [0, 7, 3650] {
        let config = load(&format!("stale_plan_days = {days}\n")).expect("loads");
        assert_eq!(config.architect.stale_plan_days, days);
    }
}

#[test]
fn out_of_range_and_non_integer_values_are_refused_by_name() {
    for value in ["-1", "3651", "\"7\"", "2.5", "true"] {
        let message = load(&format!("stale_plan_days = {value}\n")).expect_err(value);
        assert!(
            message.contains("architect.stale_plan_days"),
            "{value}: {message}"
        );
    }
}

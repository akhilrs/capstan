//! The approval-floor and auto-window keys of `[operator]`: defaults, refusals naming the index, window parsing,
//! the deprecated timed keys' warnings and the JSON shape.
use capstan_config::{
    parse_auto_window, parse_config, AutoWindowKind, ConfigError, RoleConfig,
    DEFAULT_APPROVAL_FLOOR,
};
use std::path::Path;

const BASE: &str = r#"schema_version = 1
[nexora]
track = "never"
[hosts.claude]
kind = "claude"
[roles.pm]
kind = "PM"
host = "claude"
[roles.dev]
kind = "Developer"
host = "claude"
[operator]
"#;

fn operator(extra: &str) -> Result<RoleConfig, String> {
    parse_config(
        format!("{BASE}{extra}").as_bytes(),
        Path::new("/nonexistent"),
    )
    .map_err(|error| match error {
        ConfigError::Invalid(message) => message,
        other => format!("{other:?}"),
    })
}

#[test]
fn defaults_resolve_and_are_not_printed() {
    let config = operator("").unwrap();
    let o = &config.operator;
    assert_eq!(o.approval_floor, DEFAULT_APPROVAL_FLOOR);
    assert_eq!(o.auto_window_max_minutes, 240);
    assert!(o.auto_window_tool_prompts);
    assert!(!o.auto_window_written);
    let json = config.to_json();
    assert!(
        !json.contains("approvalFloor") && !json.contains("autoWindow"),
        "{json}"
    );
}

#[test]
fn written_values_are_printed() {
    let config = operator(
        "approval_floor = [\"git push --force\", \"@controller-restart\"]\nauto_window_max_minutes = 90\nauto_window_tool_prompts = false\n",
    )
    .unwrap();
    assert_eq!(config.operator.auto_window_max_minutes, 90);
    assert!(!config.operator.auto_window_tool_prompts);
    let json: String = config
        .to_json()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    assert!(json.contains("\"autoWindowMaxMinutes\":90"), "{json}");
    assert!(json.contains("\"autoWindowToolPrompts\":false"), "{json}");
    assert!(
        json.contains("\"approvalFloor\":[\"gitpush--force\",\"@controller-restart\"]"),
        "{json}"
    );
}

#[test]
fn max_minutes_are_range_checked_and_fall_back_to_the_timed_key() {
    for bad in ["0", "481", "\"x\"", "1.5"] {
        let message = operator(&format!("auto_window_max_minutes = {bad}\n")).unwrap_err();
        assert!(
            message.contains("auto_window_max_minutes"),
            "{bad}: {message}"
        );
    }
    let config = operator("timed_grant_max_minutes = 120\n").unwrap();
    assert_eq!(config.operator.auto_window_max_minutes, 120);
    let config = operator("timed_grant_max_minutes = 120\nauto_window_max_minutes = 30\n").unwrap();
    assert_eq!(config.operator.auto_window_max_minutes, 30);
    assert!(operator("timed_grant_max_minutes = 481\n").is_err());
}

#[test]
fn bad_floor_entries_are_refused_naming_the_index() {
    for entry in [
        "@nope",
        "",
        "git  push",
        "X=1 rm",
        "-rf",
        "rm; ls",
        "a b c d e f g h i",
        "rm $x",
        " git",
    ] {
        let message = operator(&format!(
            "approval_floor = [\"git push -f\", \"{entry}\"]\n"
        ))
        .unwrap_err();
        assert!(
            message.contains("operator.approval_floor[1]"),
            "{entry:?}: {message}"
        );
    }
}

#[test]
fn eight_words_and_every_default_entry_pass() {
    assert!(operator("approval_floor = [\"a b c d e f g h\"]\n").is_ok());
    for entry in DEFAULT_APPROVAL_FLOOR {
        assert_eq!(
            capstan_config::approval_floor_entry_problem(entry),
            None,
            "{entry}"
        );
    }
}

#[test]
fn an_empty_floor_is_accepted_with_a_warning() {
    let config = operator("approval_floor = []\n").unwrap();
    assert!(config.operator.approval_floor.is_empty());
    assert_eq!(config.warnings.len(), 1, "{:?}", config.warnings);
}

#[test]
fn deprecated_keys_warn_once_each_on_every_load() {
    let text = "timed_grant_default_minutes = 10\ntimed_safe_commands = [\"ls\"]\n";
    for _ in 0..2 {
        let config = operator(text).unwrap();
        assert_eq!(config.warnings.len(), 2, "{:?}", config.warnings);
    }
}

#[test]
fn parse_auto_window_reads_hour_session_and_minutes() {
    use AutoWindowKind::*;
    assert_eq!(parse_auto_window("60", 240), Ok((Hour, 60)));
    assert_eq!(parse_auto_window("60", 30), Ok((Hour, 30)));
    assert_eq!(parse_auto_window("session", 240), Ok((Session, 240)));
    assert_eq!(parse_auto_window("15", 240), Ok((Minutes, 15)));
    for bad in ["0", "241", "x", "", "-5", "+5", "99999999999999999999"] {
        assert!(parse_auto_window(bad, 240).is_err(), "{bad:?}");
    }
}

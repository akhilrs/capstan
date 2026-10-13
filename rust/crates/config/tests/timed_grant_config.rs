//! The timed-grant keys of `[operator]`: defaults, written values, refusals that name the key, and the JSON shape.
use capstan_config::{parse_config, ConfigError, RoleConfig, DEFAULT_TIMED_SAFE_COMMANDS};
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
"#;

fn load(extra: &str) -> Result<RoleConfig, String> {
    parse_config(
        format!("{BASE}{extra}").as_bytes(),
        Path::new("/nonexistent"),
    )
    .map_err(|error| match error {
        ConfigError::Invalid(message) => message,
        other => format!("{other:?}"),
    })
}

fn operator(extra: &str) -> Result<RoleConfig, String> {
    load(&format!("[operator]\n{extra}"))
}

fn refusal(extra: &str) -> String {
    operator(extra).expect_err("refused")
}

#[test]
fn absent_keys_resolve_to_the_defaults_and_are_not_printed() {
    for config in [load("").unwrap(), operator("").unwrap()] {
        let o = &config.operator;
        assert_eq!(o.timed_grant_default_minutes, 60);
        assert_eq!(o.timed_grant_max_minutes, 240);
        assert_eq!(o.timed_safe_commands, DEFAULT_TIMED_SAFE_COMMANDS);
        assert!(!config.to_json().contains("timedGrant"));
        assert!(!config.to_json().contains("timedSafeCommands"));
    }
}

#[test]
fn written_values_resolve_and_are_printed() {
    let config = operator(
        "timed_grant_default_minutes = 10\ntimed_grant_max_minutes = 480\ntimed_safe_commands = [\"git status\", \"ls\"]\n",
    )
    .unwrap();
    let o = &config.operator;
    assert_eq!(o.timed_grant_default_minutes, 10);
    assert_eq!(o.timed_grant_max_minutes, 480);
    assert_eq!(o.timed_safe_commands, ["git status", "ls"]);
    let json: String = config
        .to_json()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    assert!(json.contains("\"timedGrantDefaultMinutes\":10"), "{json}");
    assert!(
        json.contains("\"timedSafeCommands\":[\"gitstatus\",\"ls\"]"),
        "{json}"
    );
}

#[test]
fn an_empty_list_disables_timed_grants() {
    let config = operator("timed_safe_commands = []\n").unwrap();
    assert!(config.operator.timed_safe_commands.is_empty());
}

#[test]
fn minutes_are_range_checked_and_default_must_not_exceed_max() {
    assert!(refusal("timed_grant_default_minutes = 0\n").contains("timed_grant_default_minutes"));
    assert!(refusal("timed_grant_max_minutes = 481\n").contains("timed_grant_max_minutes"));
    let message = refusal("timed_grant_default_minutes = 90\ntimed_grant_max_minutes = 60\n");
    assert!(message.contains("must not exceed"), "{message}");
    assert!(refusal("timed_grant_default_minutes = 300\n").contains("must not exceed"));
}

#[test]
fn unusable_entries_are_refused_with_the_key_named() {
    for entry in [
        "git push",
        "rm",
        "git",
        "cstan",
        "ls  -l",
        " ls",
        "ls; rm",
        "ls | head",
        "X=1 ls",
        "ls -R",
        "ssh host",
        "",
        "ls $(id)",
    ] {
        let message = refusal(&format!("timed_safe_commands = [\"ls\", \"{entry}\"]\n"));
        assert!(
            message.contains("operator.timed_safe_commands[1]"),
            "{entry:?}: {message}"
        );
    }
}

#[test]
fn every_default_entry_passes_its_own_check() {
    for entry in DEFAULT_TIMED_SAFE_COMMANDS {
        assert_eq!(
            capstan_config::timed_safe_command_problem(entry),
            None,
            "{entry}"
        );
    }
}

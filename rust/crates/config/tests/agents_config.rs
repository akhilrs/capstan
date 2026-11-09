//! The `[agents]` table: defaults, ranges, refusals, trust and the unchanged config JSON.
use capstan_config::{
    load_role_config_with, parse_config, record_current, Agents, ConfigError, RoleConfig,
    TrustStore,
};
use std::fs;
use std::os::unix::fs::PermissionsExt;
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

fn agents(extra: &str) -> Agents {
    load(&format!("[agents]\n{extra}")).expect("loads").agents
}

fn refusal(extra: &str) -> String {
    load(&format!("[agents]\n{extra}")).expect_err("refused")
}

#[test]
fn defaults_without_the_table() {
    let config = load("").expect("loads");
    assert_eq!(config.agents, Agents::default());
    assert_eq!(config.agents.idle_release_minutes, 120);
    assert_eq!(config.agents.idle_release_warn_minutes, 10);
    assert_eq!(agents(""), Agents::default());
}

#[test]
fn values_and_inclusive_bounds() {
    assert_eq!(agents("idle_release_minutes = 0").idle_release_minutes, 0);
    assert_eq!(agents("idle_release_minutes = 15").idle_release_minutes, 15);
    assert_eq!(
        agents("idle_release_minutes = 10080").idle_release_minutes,
        10080
    );
    assert_eq!(
        agents("idle_release_warn_minutes = 1").idle_release_warn_minutes,
        1
    );
    let a = agents("idle_release_minutes = 200\nidle_release_warn_minutes = 60");
    assert_eq!(
        (a.idle_release_minutes, a.idle_release_warn_minutes),
        (200, 60)
    );
}

#[test]
fn out_of_range_values_are_refused_naming_the_key() {
    for text in [
        "idle_release_minutes = 14",
        "idle_release_minutes = 10081",
        "idle_release_minutes = -1",
        "idle_release_minutes = \"120\"",
    ] {
        assert!(
            refusal(text).contains("agents.idle_release_minutes"),
            "{text}"
        );
    }
    for text in [
        "idle_release_warn_minutes = 0",
        "idle_release_warn_minutes = 61",
        "idle_release_warn_minutes = true",
    ] {
        assert!(
            refusal(text).contains("agents.idle_release_warn_minutes"),
            "{text}"
        );
    }
    assert!(refusal("idle_release_minutes = 14").contains("15"));
}

#[test]
fn warn_must_be_less_than_idle() {
    for text in [
        "idle_release_minutes = 15\nidle_release_warn_minutes = 15",
        "idle_release_minutes = 20\nidle_release_warn_minutes = 30",
        "idle_release_warn_minutes = 120",
    ] {
        let message = refusal(text);
        assert!(
            message.contains("agents.idle_release_warn_minutes"),
            "{text}: {message}"
        );
    }
    // Release off: the warning is unused, so the default warn value is fine.
    assert_eq!(
        agents("idle_release_minutes = 0").idle_release_warn_minutes,
        10
    );
}

#[test]
fn unknown_key_and_non_table_are_refused() {
    let message = refusal("bogus = true");
    assert!(message.contains("agents has 1 unknown key(s)"), "{message}");
    let text = BASE.replace("[nexora]", "agents = true\n[nexora]");
    match parse_config(text.as_bytes(), Path::new("/nonexistent")) {
        Err(ConfigError::Invalid(message)) => assert!(message.contains("agents must be a table")),
        other => panic!("{other:?}"),
    }
}

#[test]
fn config_json_is_the_same_with_and_without_the_table() {
    let without = load("").unwrap().to_json();
    let with = load("[agents]\nidle_release_minutes = 60\n")
        .unwrap()
        .to_json();
    assert_eq!(without, with);
    assert!(!without.contains("agents"));
}

#[test]
fn a_changed_value_asks_for_trust_again() {
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let project = root.join("project");
    fs::create_dir(&project).unwrap();
    let store = TrustStore::at(root.join("store"));
    let write = |text: &str| {
        let file = project.join("capstan.toml");
        fs::write(&file, text).unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
    };
    write(&format!("{BASE}[agents]\nidle_release_minutes = 60\n"));
    record_current(&project, &store).unwrap();
    load_role_config_with(&project, &store).unwrap();
    write(&format!("{BASE}[agents]\nidle_release_minutes = 90\n"));
    match load_role_config_with(&project, &store).unwrap_err() {
        ConfigError::Untrusted(text) => {
            assert!(text.contains("agents.idle_release_minutes"), "{text}")
        }
        other => panic!("unexpected {other:?}"),
    }
}

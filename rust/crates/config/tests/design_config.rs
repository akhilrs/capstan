//! The `[design]` table: the default, `require_approval`, refusals that name the key, and the starter's commented section.
use capstan_config::{parse_config, ConfigError, Design, RoleConfig};
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

#[test]
fn an_absent_table_requires_approval() {
    let design = load("").unwrap().design;
    assert_eq!(
        design,
        Design {
            configured: false,
            require_approval: true,
            ..Default::default()
        }
    );
    assert!(!load("").unwrap().to_json().contains("design"));
}

#[test]
fn require_approval_resolves_and_defaults_to_true() {
    assert_eq!(
        load("[design]\n").unwrap().design,
        Design {
            configured: true,
            require_approval: true,
            ..Default::default()
        }
    );
    assert_eq!(
        load("[design]\nrequire_approval = false\n").unwrap().design,
        Design {
            configured: true,
            require_approval: false,
            ..Default::default()
        }
    );
    assert!(load("[design]\nrequire_approval = false\n")
        .unwrap()
        .to_json()
        .contains("requireApproval"));
}

#[test]
fn a_non_boolean_or_unknown_key_is_refused() {
    let error = load("[design]\nrequire_approval = \"no\"\n").unwrap_err();
    assert!(
        error.contains("design.require_approval must be true or false"),
        "{error}"
    );
    let error = load("[design]\nrequired = true\n").unwrap_err();
    assert!(error.contains("design"), "{error}");
}

#[test]
fn the_starter_has_no_active_design_table() {
    let starter = capstan_config::starter::STARTER_CONFIG;
    assert!(!starter.lines().any(|l| l.starts_with("[design]")));
}

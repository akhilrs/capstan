//! The `[cleanup]` section: defaults, each valid value and each refusal. Rust-only: no corpus case contains `[cleanup]`.
use capstan_config::{parse_config, CleanupBranches, CleanupWorktrees, ConfigError, RoleConfig};
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

fn refusal(extra: &str) -> String {
    load(extra).expect_err("the section is refused")
}

#[test]
fn an_absent_section_is_the_default_and_leaves_the_json_unchanged() {
    let config = load("").expect("loads");
    assert_eq!(config.cleanup.branches, CleanupBranches::Ask);
    assert_eq!(config.cleanup.worktrees, CleanupWorktrees::Release);
    assert!(!config.cleanup.include_unlanded);
    assert!(!config.to_json().contains("cleanup"));
    let explicit =
        load("[cleanup]\nbranches = \"ask\"\nworktrees = \"release\"\ninclude_unlanded = false\n")
            .expect("loads");
    assert_eq!(explicit.to_json(), config.to_json());
}

#[test]
fn each_valid_value_resolves_and_appears_in_the_json() {
    for (value, expected) in [
        ("auto", CleanupBranches::Auto),
        ("ask", CleanupBranches::Ask),
        ("off", CleanupBranches::Off),
    ] {
        let config = load(&format!("[cleanup]\nbranches = \"{value}\"\n")).expect("loads");
        assert_eq!(config.cleanup.branches, expected);
        assert_eq!(config.to_json().contains("\"cleanup\""), value != "ask");
    }
    for (value, expected) in [
        ("release", CleanupWorktrees::Release),
        ("keep", CleanupWorktrees::Keep),
    ] {
        let config = load(&format!("[cleanup]\nworktrees = \"{value}\"\n")).expect("loads");
        assert_eq!(config.cleanup.worktrees, expected);
        assert_eq!(
            config.to_json().contains("\"worktrees\": \"keep\""),
            value == "keep"
        );
    }
    let config = load("[cleanup]\ninclude_unlanded = true\n").expect("loads");
    assert!(config.cleanup.include_unlanded);
    assert!(config.to_json().contains("\"includeUnlanded\": true"));
}

#[test]
fn bad_sections_are_refused_with_a_message_naming_the_key() {
    assert!(refusal("[cleanup]\nbranches = \"yes\"\n").contains("cleanup.branches"));
    assert!(refusal("[cleanup]\nworktrees = \"drop\"\n").contains("cleanup.worktrees"));
    assert!(refusal("[cleanup]\nworktrees = 1\n").contains("cleanup.worktrees"));
    assert!(refusal("[cleanup]\ninclude_unlanded = \"no\"\n").contains("cleanup.include_unlanded"));
    assert!(refusal("[cleanup]\nsurprise = 1\n").contains("cleanup has 1 unknown key"));
    // A top-level key has to come before the first table.
    let text = format!("cleanup = 1\n{BASE}");
    let error = parse_config(text.as_bytes(), Path::new("/nonexistent")).expect_err("refused");
    assert!(error.to_string().contains("cleanup"), "{error}");
}

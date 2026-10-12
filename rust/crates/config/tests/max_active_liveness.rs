//! `roles.<name>.max_active` and the `[liveness]` table: defaults, valid values, refusals and the role hash.
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
[roles.dev]
kind = "Developer"
host = "claude"
[roles.sup]
kind = "Supervisor"
host = "claude"
[roles.rev]
kind = "Verifier"
host = "claude"
"#;

fn load_text(text: &str) -> Result<RoleConfig, String> {
    parse_config(text.as_bytes(), Path::new("/nonexistent")).map_err(|error| match error {
        ConfigError::Invalid(message) => message,
        other => format!("{other:?}"),
    })
}

fn load(extra: &str) -> Result<RoleConfig, String> {
    load_text(&format!("{BASE}{extra}"))
}

fn max_active(config: &RoleConfig, name: &str) -> Option<i64> {
    config
        .roles
        .iter()
        .find(|r| r.name == name)
        .unwrap()
        .max_active
}

#[test]
fn without_max_active_a_supervisor_is_one_and_the_rest_are_unlimited() {
    let config = load("").expect("loads");
    assert_eq!(max_active(&config, "sup"), Some(1));
    assert_eq!(max_active(&config, "pm"), None);
    assert_eq!(max_active(&config, "dev"), None);
    assert_eq!(max_active(&config, "rev"), None);
    assert!(config.to_json().contains("\"maxActive\": null"));
    assert!(config.to_json().contains("\"maxActive\": 1"));
}

#[test]
fn max_active_resolves_as_written_and_is_part_of_the_hash() {
    let text = BASE
        .replace(
            "[roles.dev]\nkind = \"Developer\"",
            "[roles.dev]\nkind = \"Developer\"\nmax_active = 1",
        )
        .replace(
            "[roles.sup]\nkind = \"Supervisor\"",
            "[roles.sup]\nkind = \"Supervisor\"\nmax_active = 2",
        );
    let config = load_text(&text).expect("loads");
    assert_eq!(max_active(&config, "dev"), Some(1));
    assert_eq!(max_active(&config, "sup"), Some(2));
    let plain = load("").unwrap();
    let hash = |c: &RoleConfig, n: &str| {
        c.roles
            .iter()
            .find(|r| r.name == n)
            .unwrap()
            .config_hash
            .clone()
    };
    assert_ne!(hash(&config, "dev"), hash(&plain, "dev"));
    assert_ne!(hash(&config, "sup"), hash(&plain, "sup"));
    assert_eq!(hash(&config, "rev"), hash(&plain, "rev"));
}

#[test]
fn the_hash_covers_only_what_the_user_wrote() {
    let hashes =
        |c: &RoleConfig| -> Vec<String> { c.roles.iter().map(|r| r.config_hash.clone()).collect() };
    let plain = load("").unwrap();
    // A Supervisor's default limit of 1 written out is the same role: the hash (and so the role sync) is unchanged.
    let written = load_text(&BASE.replace(
        "[roles.sup]\nkind = \"Supervisor\"",
        "[roles.sup]\nkind = \"Supervisor\"\nmax_active = 1",
    ))
    .unwrap();
    assert_eq!(hashes(&written), hashes(&plain));
    // The [liveness] table is not part of any role's hash.
    let liveness = load("[liveness]\ncheck_seconds = 45\n").unwrap();
    assert_eq!(hashes(&liveness), hashes(&plain));
}

#[test]
fn bad_max_active_values_name_the_key() {
    for value in ["0", "-1", "1.5", "\"x\"", "true"] {
        let text = BASE.replace(
            "[roles.dev]\nkind = \"Developer\"",
            &format!("[roles.dev]\nkind = \"Developer\"\nmax_active = {value}"),
        );
        let message = load_text(&text).expect_err(value);
        assert!(
            message.contains("roles.dev.max_active"),
            "{value}: {message}"
        );
    }
}

#[test]
fn max_active_on_a_pm_role_is_refused() {
    let text = BASE.replace("kind = \"PM\"", "kind = \"PM\"\nmax_active = 1");
    let message = load_text(&text).expect_err("refused");
    assert!(message.contains("roles.pm.max_active"), "{message}");
}

#[test]
fn liveness_defaults_and_explicit_values() {
    let config = load("").expect("loads");
    let l = &config.liveness;
    assert_eq!(
        (
            l.enabled,
            l.check_seconds,
            l.confirm_checks,
            l.start_grace_seconds
        ),
        (true, 30, 3, 120)
    );
    assert!(config.to_json().contains("\"startGraceSeconds\": 120"));
    assert_eq!(l.stall_child_max_seconds, 3600);
    let custom = load("[liveness]\nstall_child_max_seconds = 7200\n").unwrap();
    assert_eq!(custom.liveness.stall_child_max_seconds, 7200);
    let config = load(
        "[liveness]\nenabled = false\ncheck_seconds = 10\nconfirm_checks = 2\nstart_grace_seconds = 30\n",
    )
    .unwrap();
    let l = &config.liveness;
    assert_eq!(
        (
            l.enabled,
            l.check_seconds,
            l.confirm_checks,
            l.start_grace_seconds
        ),
        (false, 10, 2, 30)
    );
}

#[test]
fn liveness_bounds_and_unknown_keys_are_refused() {
    for (key, value) in [
        ("check_seconds", "9"),
        ("check_seconds", "3601"),
        ("confirm_checks", "1"),
        ("start_grace_seconds", "29"),
        ("stall_child_max_seconds", "59"),
        ("stall_child_max_seconds", "86401"),
        ("enabled", "1"),
        ("check_seconds", "\"x\""),
    ] {
        let message = load(&format!("[liveness]\n{key} = {value}\n")).expect_err(key);
        assert!(
            message.contains(&format!("liveness.{key}")),
            "{key}: {message}"
        );
    }
    let message = load("[liveness]\nbogus = 1\n").expect_err("unknown key");
    assert!(message.contains("liveness"), "{message}");
}

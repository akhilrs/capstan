//! The `[review]` table: defaults per key, every key, and refusals that name the key.
use capstan_config::{parse_config, ConfigError, Review, RoleConfig};
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

fn review(extra: &str) -> Review {
    load(&format!("[review]\n{extra}")).expect("loads").review
}

fn refusal(extra: &str) -> String {
    load(&format!("[review]\n{extra}")).expect_err("refused")
}

#[test]
fn absent_section_is_the_default_and_not_printed() {
    let config = load("").expect("loads");
    assert_eq!(config.review, Review::default());
    assert!(!config.review.configured);
    assert!(config.review.warm_reviewer);
    assert_eq!(config.review.idle_minutes, 30);
    assert_eq!(config.review.max_reviews, 6);
    assert!(!config.review.integration);
    assert!(!config.to_json().contains("\"review\""));
}

#[test]
fn an_empty_section_is_configured_with_every_default() {
    let r = review("");
    assert_eq!(
        r,
        Review {
            configured: true,
            ..Review::default()
        }
    );
    assert!(load("[review]\n").unwrap().to_json().contains("\"review\""));
}

#[test]
fn a_full_section_takes_the_written_values() {
    let r =
        review("warm_reviewer = false\nidle_minutes = 5\nmax_reviews = 20\nintegration = true\n");
    assert_eq!(
        r,
        Review {
            configured: true,
            warm_reviewer: false,
            idle_minutes: 5,
            max_reviews: 20,
            integration: true,
        }
    );
}

#[test]
fn each_missing_key_takes_its_default() {
    assert!(!review("warm_reviewer = false").integration);
    assert_eq!(review("warm_reviewer = false").idle_minutes, 30);
    assert!(review("idle_minutes = 60").warm_reviewer);
    assert_eq!(review("idle_minutes = 60").max_reviews, 6);
    assert_eq!(review("max_reviews = 3").idle_minutes, 30);
    assert!(review("max_reviews = 3").warm_reviewer);
    assert!(review("integration = true").warm_reviewer);
    assert_eq!(review("integration = true").max_reviews, 6);
}

#[test]
fn bounds_are_inclusive() {
    assert_eq!(review("idle_minutes = 240").idle_minutes, 240);
    assert_eq!(review("max_reviews = 1").max_reviews, 1);
}

#[test]
fn invalid_values_are_refused_naming_the_key() {
    for text in [
        "idle_minutes = 4",
        "idle_minutes = 241",
        "idle_minutes = \"30\"",
    ] {
        assert!(refusal(text).contains("review.idle_minutes"), "{text}");
    }
    for text in ["max_reviews = 0", "max_reviews = 21"] {
        assert!(refusal(text).contains("review.max_reviews"), "{text}");
    }
    assert!(refusal("warm_reviewer = \"yes\"").contains("review.warm_reviewer"));
    assert!(refusal("integration = 1").contains("review.integration"));
    let message = refusal("bogus = true");
    assert!(message.contains("review has 1 unknown key(s)"), "{message}");
}

#[test]
fn a_section_that_is_not_a_table_is_refused() {
    let text = BASE.replace("[nexora]", "review = true\n[nexora]");
    let message = parse_config(text.as_bytes(), Path::new("/nonexistent"));
    match message {
        Err(ConfigError::Invalid(message)) => assert!(message.contains("review must be a table")),
        other => panic!("{other:?}"),
    }
}

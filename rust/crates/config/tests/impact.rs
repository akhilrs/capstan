//! The `[impact]` section: defaults, each key, the area and glob rules and the refusals. Rust-only: no corpus case
//! contains `[impact]`.
use capstan_config::impact::glob_matches;
use capstan_config::{parse_config, ConfigError, ImpactConfig, RoleConfig};
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

fn refused(extra: &str) -> String {
    load(extra).expect_err("refused")
}

#[test]
fn an_absent_section_is_the_default_and_leaves_the_json_unchanged() {
    let config = load("").expect("loads");
    assert_eq!(config.impact, ImpactConfig::default());
    assert!(!config.to_json().contains("impact"));
    assert_eq!(config.impact.area_of("rust/crates/a.rs"), "rust");
    assert_eq!(config.impact.area_of("README.md"), "README.md");
    assert!(!config.impact.is_migration("migrations/0001.sql"));
}

#[test]
fn each_key_is_read_and_a_set_section_is_printed() {
    let config = load(
        "[impact]\nareas = [\"rust/crates/*\", \"docs\"]\nmigrations = [\"migrations/*.sql\"]\nnext_steps = [{ paths = [\"migrations/*.sql\"], step = \"restart\" }]\n",
    )
    .unwrap();
    assert_eq!(config.impact.areas, ["rust/crates/*", "docs"]);
    assert_eq!(config.impact.next_steps[0].step, "restart");
    assert!(config.to_json().contains("\"impact\""));
}

#[test]
fn area_patterns_map_paths_as_stated() {
    let config = load("[impact]\nareas = [\"rust/crates/*\", \"docs\", \"rust\"]\n").unwrap();
    let area = |p: &str| config.impact.area_of(p);
    assert_eq!(area("rust/crates/kernel/src/a.rs"), "rust/crates/kernel");
    assert_eq!(area("rust/crates/top.toml"), "rust/crates/top.toml");
    assert_eq!(area("docs/reference/x.md"), "docs");
    assert_eq!(area("docs"), "docs");
    assert_eq!(area("docsy/x"), "docsy");
    assert_eq!(area("rust/other/x.rs"), "rust");
    assert_eq!(area("scripts/run.sh"), "scripts");
}

#[test]
fn globs_match_within_and_across_segments() {
    assert!(glob_matches("migrations/*.sql", "migrations/0001_a.sql"));
    assert!(!glob_matches(
        "migrations/*.sql",
        "migrations/sub/0001_a.sql"
    ));
    assert!(glob_matches("migrations/**", "migrations/sub/0001_a.sql"));
    assert!(glob_matches("a/?.rs", "a/b.rs"));
    assert!(!glob_matches("a/?.rs", "a//.rs"));
}

#[test]
fn bad_input_is_refused_in_the_section_style() {
    assert!(refused("[impact]\nzz = 1\n").contains("impact"));
    let text = BASE.replacen("[nexora]", "impact = 1\n[nexora]", 1);
    let error = parse_config(text.as_bytes(), Path::new("/nonexistent")).expect_err("refused");
    assert!(format!("{error:?}").contains("impact must be a table"));
    assert!(refused("[impact]\nmigrations = [\"a[b\"]\n").contains("impact.migrations[0]"));
    assert!(refused("[impact]\nmigrations = [\"/abs\"]\n").contains("impact.migrations[0]"));
    assert!(refused("[impact]\nmigrations = [\"../x\"]\n").contains("impact.migrations[0]"));
    assert!(refused("[impact]\nareas = [\"a/*/b\"]\n").contains("impact.areas[0]"));
    assert!(refused("[impact]\nareas = \"a\"\n").contains("impact.areas"));
    assert!(
        refused("[impact]\nnext_steps = [{ paths = [\"a\"], step = \"\" }]\n")
            .contains("impact.next_steps[0].step")
    );
    let long = "x".repeat(201);
    assert!(refused(&format!(
        "[impact]\nnext_steps = [{{ paths = [\"a\"], step = \"{long}\" }}]\n"
    ))
    .contains("impact.next_steps[0].step"));
    let ok = "x".repeat(200);
    load(&format!(
        "[impact]\nnext_steps = [{{ paths = [\"a\"], step = \"{ok}\" }}]\n"
    ))
    .unwrap();
    assert!(
        refused("[impact]\nnext_steps = [{ paths = [\"a\"], step = \"s\", zz = 1 }]\n")
            .contains("impact.next_steps[0]")
    );
    assert!(refused("[impact]\nnext_steps = [{ step = \"s\" }]\n").contains("paths"));
    let eleven = (0..11)
        .map(|i| format!("{{ paths = [\"a\"], step = \"s{i}\" }}"))
        .collect::<Vec<_>>()
        .join(", ");
    assert!(refused(&format!("[impact]\nnext_steps = [{eleven}]\n")).contains("impact.next_steps"));
    let ten = (0..10)
        .map(|i| format!("{{ paths = [\"a\"], step = \"s{i}\" }}"))
        .collect::<Vec<_>>()
        .join(", ");
    load(&format!("[impact]\nnext_steps = [{ten}]\n")).unwrap();
}

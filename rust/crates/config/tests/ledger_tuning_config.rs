//! `[ledger] cache_mb`, `mmap_mb`, `backup_max_age_days` and `[daemon] status_cache_ms`, `driver_read_first`,
//! `snapshot_reads_per_minute`: defaults, bounds, refusals, the JSON and the docs. Rust-only keys.
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

const INTEGER_KEYS: &[(&str, &str, i64, i64, i64)] = &[
    ("ledger", "cache_mb", 0, 1024, 64),
    ("ledger", "mmap_mb", 0, 4096, 256),
    ("ledger", "backup_max_age_days", 0, 3650, 14),
    ("daemon", "status_cache_ms", 0, 60_000, 2000),
    ("daemon", "snapshot_reads_per_minute", 0, 600, 0),
];

fn value(config: &RoleConfig, key: &str) -> i64 {
    match key {
        "cache_mb" => config.ledger.cache_mb,
        "mmap_mb" => config.ledger.mmap_mb,
        "backup_max_age_days" => config.ledger.backup_max_age_days,
        "status_cache_ms" => config.daemon.status_cache_ms,
        _ => config.daemon.snapshot_reads_per_minute,
    }
}

#[test]
fn defaults_resolve_and_all_six_keys_are_in_the_json() {
    let config = load("").expect("loads");
    for (_, key, _, _, default) in INTEGER_KEYS {
        assert_eq!(value(&config, key), *default, "{key}");
    }
    assert!(config.daemon.driver_read_first);
    let json = config.to_json();
    for text in [
        "\"cacheMb\": 64",
        "\"mmapMb\": 256",
        "\"backupMaxAgeDays\": 14",
        "\"statusCacheMs\": 2000",
        "\"driverReadFirst\": true",
        "\"snapshotReadsPerMinute\": 0",
    ] {
        assert!(json.contains(text), "{text} missing from {json}");
    }
}

#[test]
fn each_key_is_accepted_at_its_bounds() {
    for (table, key, low, high, _) in INTEGER_KEYS {
        for n in [*low, *high] {
            let config = load(&format!("[{table}]\n{key} = {n}\n")).expect("loads");
            assert_eq!(value(&config, key), n, "{key}");
        }
    }
    for flag in [true, false] {
        let config = load(&format!("[daemon]\ndriver_read_first = {flag}\n")).unwrap();
        assert_eq!(config.daemon.driver_read_first, flag);
        assert!(config
            .to_json()
            .contains(&format!("\"driverReadFirst\": {flag}")));
    }
}

#[test]
fn out_of_range_and_mistyped_values_are_refused_naming_the_key() {
    for (table, key, low, high, _) in INTEGER_KEYS {
        for bad in [
            (low - 1).to_string(),
            (high + 1).to_string(),
            "\"1\"".into(),
            "1.5".into(),
            "true".into(),
        ] {
            let message = load(&format!("[{table}]\n{key} = {bad}\n")).expect_err("refused");
            assert!(
                message.contains(&format!("{table}.{key}")),
                "{bad}: {message}"
            );
        }
    }
    for bad in ["1", "\"true\""] {
        let message = load(&format!("[daemon]\ndriver_read_first = {bad}\n")).expect_err("refused");
        assert!(message.contains("daemon.driver_read_first"), "{message}");
    }
}

#[test]
fn unknown_keys_are_refused() {
    assert!(load("[ledger]\ncache_gb = 1\n").is_err());
    assert!(load("[daemon]\nstatus_cache = 1\n").is_err());
}

fn read(path: &str) -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../..")
            .join(path),
    )
    .expect("the doc is readable")
}

#[test]
fn the_configuration_reference_names_each_key_and_the_switches() {
    let doc = read("docs/reference/configuration.md");
    for key in [
        "cache_mb",
        "mmap_mb",
        "backup_max_age_days",
        "status_cache_ms",
        "driver_read_first",
        "snapshot_reads_per_minute",
    ] {
        assert!(doc.contains(&format!("`{key}`")), "{key} is not documented");
    }
    assert!(
        doc.contains("memory the daemon"),
        "the memory note is missing"
    );
    assert!(doc.contains("`0` turns it off"), "the cache_mb off switch");
    assert!(
        doc.contains("`0` turns both caches off"),
        "the status cache off switch"
    );
    assert!(
        doc.contains("`false` keeps"),
        "the driver_read_first off switch"
    );
    assert!(
        doc.contains("`0` means no limit"),
        "the snapshot limit off switch"
    );
}

#[test]
fn the_daemon_doc_has_the_rollout_note() {
    let doc = read("docs/daemon.md");
    assert!(doc.contains("the first start after upgrade runs migration 0059"));
    assert!(doc.contains("up to 30 s after a backup that needs free disk of one ledger size"));
    assert!(doc.contains("readiness timeouts to at least 60 s"));
}

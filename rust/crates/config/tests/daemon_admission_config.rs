//! `[daemon] max_connections`, `caller_connections`, `wait_connections`, `wait_per_caller`, `repeat_window_seconds`
//! and `busy_retry_ms`: defaults, bounds, the cross-key checks, the JSON and the no-config defaults.
use capstan_config::{daemon_limits, parse_config, ConfigError, DaemonTuning, RoleConfig};
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

fn daemon(lines: &str) -> Result<RoleConfig, String> {
    load(&format!("[daemon]\n{lines}\n"))
}

#[test]
fn defaults_equal_daemon_default_and_are_in_the_json() {
    let config = load("").expect("loads");
    assert_eq!(config.daemon, DaemonTuning::default());
    assert_eq!(config.daemon.max_connections, 64);
    assert_eq!(config.daemon.caller_connections, 32);
    assert_eq!(config.daemon.wait_connections, 32);
    assert_eq!(config.daemon.wait_per_caller, 4);
    assert_eq!(config.daemon.repeat_window_seconds, 60);
    assert_eq!(config.daemon.busy_retry_ms, 500);
    let json = config.to_json();
    for text in [
        "\"maxConnections\": 64",
        "\"callerConnections\": 32",
        "\"waitConnections\": 32",
        "\"waitPerCaller\": 4",
        "\"repeatWindowSeconds\": 60",
        "\"busyRetryMs\": 500",
    ] {
        assert!(json.contains(text), "{text} missing from {json}");
    }
}

#[test]
fn bounds_are_accepted_and_one_past_is_refused() {
    for (key, low, high) in [
        ("max_connections", 8, 1024),
        ("caller_connections", 1, 64),
        ("wait_connections", 1, 64),
        ("wait_per_caller", 1, 4),
        ("repeat_window_seconds", 0, 600),
        ("busy_retry_ms", 50, 10_000),
    ] {
        for value in [low, high] {
            let extra = if key == "max_connections" {
                format!("{key} = {value}\ncaller_connections = 8\nwait_connections = 8\n")
            } else if key == "wait_connections" {
                format!("{key} = {value}\nwait_per_caller = 1")
            } else {
                format!("{key} = {value}")
            };
            daemon(&extra).unwrap_or_else(|e| panic!("{key}={value}: {e}"));
        }
        let below = low - 1;
        let above = high + 1;
        for value in [below, above] {
            if key == "wait_per_caller" && value == above {
                continue; // refused by the cross-key check instead
            }
            if matches!(key, "caller_connections" | "wait_connections") && value == above {
                continue;
            }
            let error = daemon(&format!("{key} = {value}")).expect_err(&format!("{key}={value}"));
            assert!(error.contains(&format!("daemon.{key}")), "{error}");
        }
    }
}

#[test]
fn unknown_key_and_wrong_type_are_refused() {
    assert!(daemon("max_conections = 9")
        .unwrap_err()
        .contains("unknown key"));
    assert!(daemon("busy_retry_ms = \"x\"").is_err());
}

#[test]
fn cross_key_checks_name_the_key() {
    let e = daemon("max_connections = 16").unwrap_err();
    assert!(e.contains("daemon.caller_connections"), "{e}");
    let e = daemon("max_connections = 16\ncaller_connections = 8").unwrap_err();
    assert!(e.contains("daemon.wait_connections"), "{e}");
    let e = daemon("wait_connections = 2").unwrap_err();
    assert!(e.contains("daemon.wait_per_caller"), "{e}");
    daemon(
        "max_connections = 16\ncaller_connections = 16\nwait_connections = 2\nwait_per_caller = 2",
    )
    .expect("equal is allowed");
}

#[test]
fn no_config_defaults_never_fold_a_send() {
    let limits = daemon_limits(None);
    assert_eq!(limits.repeat_window_seconds, 0);
    assert_eq!(
        DaemonTuning {
            repeat_window_seconds: 60,
            ..limits.clone()
        },
        DaemonTuning::default()
    );
    let config = load("").unwrap();
    assert_eq!(daemon_limits(Some(&config)).repeat_window_seconds, 60);
}

#[test]
fn implementation_node_errors_and_rust_warns_with_the_current_texts() {
    let e = daemon("implementation = \"node\"").unwrap_err();
    assert!(
        e.contains("remove implementation from the [daemon] table"),
        "{e}"
    );
    let text = format!("{BASE}[daemon]\nimplementation = \"rust\"\n");
    let parsed = parse_config(text.as_bytes(), Path::new("/nonexistent")).expect("loads");
    assert!(parsed
        .warnings
        .iter()
        .any(|w| w.contains("remove implementation from the [daemon] table")));
}

#![allow(dead_code)]
//! Shared pieces of the launcher's integration tests: where the fixtures are, how a recorded value is normalised, the
//! configuration the sequences run with and the comparison that names the first difference.

pub mod engine;
pub mod exec;
pub mod ledger;
pub mod stub;

use capstan_config::{
    Architect, Host, Layout, Ledger, Limits, McpServer, Nexora, Notifications, Operator,
    PromptRelay, PromptSource, Researcher, Role, RoleConfig, Supervision, Timers, Worktree,
};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// `rust/crates/launcher`.
pub fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

pub fn parity_dir() -> PathBuf {
    crate_dir().join("tests").join("parity")
}

/// The overlays that replace the expected output of the cases where Rust differs from Node.
pub fn divergences_dir() -> PathBuf {
    crate_dir().join("tests").join("divergences")
}

/// The names of the sequences of a parity file.
pub fn case_names(file: &Path) -> Result<Vec<String>, String> {
    let text = std::fs::read_to_string(file).map_err(|e| e.to_string())?;
    let value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    Ok(capstan_kernel::export_file::unpack(value)["sequences"]
        .as_array()
        .map(|all| {
            all.iter()
                .filter_map(|s| s["name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default())
}

/// A parity file, with its dictionary expanded.
pub fn read_json(path: &Path) -> Value {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    capstan_kernel::export_file::unpack(value)
}

/// Strings longer than this are recorded as their SHA-256 (prompts, seeds): their bytes are compared, not shown.
const LONG_STRING: usize = 1500;

/// A value as the fixtures record it: functions are written `<fn>` by the callers, the scratch directory is `<tmp>`, and
/// a long string is its SHA-256 and byte length (`normal` of test/launcher-parity-export.ts).
pub fn normal(value: &Value, scratch: &str) -> Value {
    match value {
        Value::String(text) => {
            let text = text.replace(scratch, "<tmp>");
            if text.encode_utf16().count() > LONG_STRING {
                json!({"bytes": text.len(), "sha256": hex(&Sha256::digest(text.as_bytes()))})
            } else {
                Value::String(text)
            }
        }
        Value::Array(items) => Value::Array(items.iter().map(|v| normal(v, scratch)).collect()),
        Value::Object(map) => {
            let mut out = Map::new();
            for (key, entry) in map {
                out.insert(key.clone(), normal(entry, scratch));
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn hash_of(name: &str) -> String {
    hex(&Sha256::digest(name.as_bytes()))
}

fn clip(value: &Value) -> String {
    let text = serde_json::to_string(value).unwrap_or_default();
    if text.chars().count() > 700 {
        format!("{}...", text.chars().take(700).collect::<String>())
    } else {
        text
    }
}

/// The first place two JSON values differ, with both sides.
pub fn first_difference(expected: &Value, actual: &Value, path: &str) -> Option<String> {
    if expected == actual {
        return None;
    }
    match (expected, actual) {
        (Value::Object(e), Value::Object(a)) => {
            for key in e.keys().chain(a.keys()) {
                let here = format!("{path}.{key}");
                match (e.get(key), a.get(key)) {
                    (Some(x), Some(y)) => {
                        if let Some(diff) = first_difference(x, y, &here) {
                            return Some(diff);
                        }
                    }
                    (x, y) => {
                        return Some(format!(
                            "{here}: expected {} but got {}",
                            x.map_or("nothing".into(), clip),
                            y.map_or("nothing".into(), clip)
                        ))
                    }
                }
            }
            None
        }
        (Value::Array(e), Value::Array(a)) => {
            for i in 0..e.len().max(a.len()) {
                let here = format!("{path}[{i}]");
                match (e.get(i), a.get(i)) {
                    (Some(x), Some(y)) => {
                        if let Some(diff) = first_difference(x, y, &here) {
                            return Some(diff);
                        }
                    }
                    (x, y) => {
                        return Some(format!(
                            "{here}: expected {} but got {}",
                            x.map_or("nothing".into(), clip),
                            y.map_or("nothing".into(), clip)
                        ))
                    }
                }
            }
            None
        }
        _ => Some(format!(
            "{path}: expected {} but got {}",
            clip(expected),
            clip(actual)
        )),
    }
}

// ------------------------------------------------------------------------------------------------ the configuration

fn text(spec: &Value, key: &str) -> Option<String> {
    spec.get(key).and_then(Value::as_str).map(str::to_string)
}

fn role(name: &str, kind: &str, host: &str, prompt_text: Option<&str>) -> Role {
    Role {
        name: name.into(),
        kind: kind.into(),
        host: host.into(),
        model: None,
        permission_mode: "default".into(),
        allow: Vec::new(),
        deny: Vec::new(),
        hooks: "off",
        mcp: Vec::new(),
        prompt: PromptSource {
            source: "none",
            path: None,
            hash: None,
        },
        prompt_text: prompt_text.map(str::to_string),
        config_hash: hash_of(name),
    }
}

/// The `RoleConfig` of a sequence's `config` (`configOf` of test/launcher-parity-export.ts over
/// `config()` of test/launcher-harness.ts).
pub fn test_config(spec: &Value) -> RoleConfig {
    let host_of = |name: &str| -> String {
        spec["hostOf"]
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or("claude")
            .to_string()
    };
    let layout = &spec["layout"];
    let number = |value: &Value, key: &str, default: i64| -> i64 {
        value.get(key).and_then(Value::as_i64).unwrap_or(default)
    };
    let mut roles = vec![
        role("pm", "PM", &host_of("pm"), Some("Keep the plan small.")),
        role("developer", "Developer", &host_of("developer"), None),
        role("developer2", "Developer", &host_of("developer2"), None),
        role("supervisor", "Supervisor", &host_of("supervisor"), None),
    ];
    let developer_host = host_of("developer");
    let architect_spec = spec.get("architect").filter(|v| v.is_object());
    let operator_spec = spec.get("operator").filter(|v| v.is_object());
    let researcher_spec = spec.get("researcher").filter(|v| v.is_object());
    if architect_spec.is_some() {
        roles.push(role("architect", "Developer", &developer_host, None));
    }
    if operator_spec.is_some() {
        roles.push(role("operator", "Developer", &developer_host, None));
    }
    if researcher_spec.is_some() {
        let mut researcher = role("researcher", "Developer", &developer_host, None);
        researcher.allow = vec![
            "WebSearch".into(),
            "Bash(jq *)".into(),
            "mcp__playwright__browser_navigate".into(),
        ];
        researcher.mcp = vec![McpServer {
            name: "playwright".into(),
            command: "npx".into(),
            args: vec![
                "-y".into(),
                "@playwright/mcp@latest".into(),
                "--headless".into(),
            ],
        }];
        roles.push(researcher);
    }
    let flag = |value: Option<&Value>, key: &str| -> bool {
        value
            .and_then(|v| v.get(key))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    };
    let operator_table =
        operator_spec.is_some_and(|o| o.get("table").and_then(Value::as_bool) != Some(false));
    let host = |name: &str| Host {
        name: name.into(),
        kind: name.into(),
        command: name.into(),
        shell_command_timeout_seconds: 120,
        wait_timeout_seconds: 45,
    };
    RoleConfig {
        project_name: None,
        herdr_session: "test".into(),
        notifications: Notifications {
            herdr: true,
            fallback: true,
            pm_stale_minutes: 10,
        },
        timers: Timers {
            max_deferral_seconds: 120,
            max_busy_deferral_seconds: 120,
            pm_ack_timeout_seconds: 600,
            pm_notify_after_seconds: 300,
            notify_interval_seconds: 600,
            stall_after_seconds: 900,
            worker_ack_timeout_seconds: 600,
            finding_check_seconds: 1800,
            pm_wake_after_seconds: 0,
            pm_wake_interval_seconds: 120,
        },
        supervision: Supervision {
            enabled: false,
            check_seconds: 300,
        },
        architect: Architect {
            enabled: architect_spec.is_some(),
            role: "architect".into(),
            plan_review: "high_risk".into(),
            reviewer_role: None,
            max_packages: 8,
            count_toward_worker_limit: flag(architect_spec, "counts"),
            high_risk_triggers: vec!["schema or migrations".into()],
        },
        operator: Operator {
            configured: operator_table,
            enabled: operator_table && flag(operator_spec, "enabled"),
            role: "operator".into(),
            auto_approve: if operator_table {
                vec!["ls -l".into()]
            } else {
                Vec::new()
            },
            auto_approve_prefix: Vec::new(),
            timeout_seconds: 60,
            max_timeout_seconds: 600,
            output_tail_bytes: 4096,
            proposal_ttl_minutes: 30,
            approval_ttl_minutes: 10,
            max_pending_proposals: 5,
            count_toward_worker_limit: flag(operator_spec, "counts"),
            restart_health_timeout_seconds: 60,
            restart_idle_wait_seconds: 60,
            session_grant_max_minutes: 60,
            full_auto_default_minutes: 30,
            full_auto_max_minutes: 120,
        },
        researcher: Researcher {
            configured: researcher_spec.is_some(),
            enabled: flag(researcher_spec, "enabled"),
            role: "researcher".into(),
            output_dir: "docs/research".into(),
            user_agent: "capstan-researcher/1.0 (test)".into(),
        },
        mcp_servers: Vec::new(),
        prompt_relay: PromptRelay {
            present: spec.get("promptRelay").is_some_and(|v| !v.is_null()),
            enabled: spec.get("promptRelay").and_then(Value::as_bool) == Some(true),
            capture_ttl_seconds: 300,
        },
        nexora: Nexora {
            track: "never",
            default_action: "none",
        },
        limits: Limits {
            max_workers: number(spec, "maxWorkers", 3),
        },
        ledger: Ledger {
            keep_migration_backups: 3,
        },
        layout: Layout {
            spawn: if text(layout, "spawn").as_deref() == Some("pane") {
                "pane"
            } else {
                "tab"
            },
            pm_width_percent: number(layout, "pmWidthPercent", 60),
            min_pane_columns: number(layout, "minPaneColumns", 60),
            min_pane_rows: number(layout, "minPaneRows", 12),
        },
        worktree: spec
            .get("worktree")
            .filter(|v| v.is_object())
            .map(|w| Worktree {
                setup: text(w, "setup"),
                setup_timeout_seconds: number(w, "setupTimeoutSeconds", 600),
                teardown: text(w, "teardown"),
                teardown_timeout_seconds: w.get("teardownTimeoutSeconds").and_then(Value::as_i64),
            }),
        daemon_implementation: None,
        env_pass: spec["pass"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
        hosts: vec![host("claude"), host("codex"), host("omp")],
        roles,
        warnings: Vec::new(),
    }
}

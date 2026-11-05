//! One role switched across hosts by changing only `host`: each host gets its own argument shape, with the model the
//! configuration resolved for that host and the Claude-only allow/deny rules on the Claude host alone.
mod common;

use capstan_launcher::api::LauncherService;
use common::engine::Scenario;
use serde_json::{json, Value};

fn toml(host: &str) -> String {
    format!(
        r#"schema_version = 1
[nexora]
track = "never"
[defaults.Developer]
model = "claude-default"
[hosts.claude]
kind = "claude"
[hosts.codex]
kind = "codex"
model = "gpt-x"
[hosts.omp]
kind = "omp"
model = "o/m"
[hosts.pi]
kind = "pi"
model = "p/m"
[roles.pm]
kind = "PM"
host = "claude"
[roles.developer]
kind = "Developer"
host = "{host}"
permission_mode = "acceptEdits"
allow = ["Bash(git status)"]
deny = ["WebFetch"]
"#
    )
}

fn started_arguments(host: &str) -> (String, Vec<String>) {
    let sc = Scenario::new(
        &format!("host-switch-{host}"),
        json!({"toml": toml(host), "hostOf": {"developer": host}}),
    );
    sc.launcher.launch_pm().expect("the PM launches");
    sc.launcher
        .spawn("developer", &Default::default())
        .expect("the developer spawns");
    let calls = sc.take_calls();
    let start = calls
        .iter()
        .rev()
        .find(|c| c["m"] == "startAgent")
        .expect("the developer was started");
    let input = &start["a"][0];
    let args = input["args"]
        .as_array()
        .expect("an argument vector")
        .iter()
        .map(|a: &Value| a.as_str().map_or_else(|| a.to_string(), str::to_string))
        .collect();
    (input["kind"].as_str().unwrap().to_string(), args)
}

fn after(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
}

#[test]
fn the_same_role_gets_each_hosts_argument_shape_and_model() {
    let (kind, claude) = started_arguments("claude");
    assert_eq!(kind, "claude");
    assert_eq!(
        after(&claude, "--model").as_deref(),
        Some("claude-default"),
        "{claude:?}"
    );
    assert!(
        claude.iter().any(|a| a.contains("Bash(git status)")),
        "{claude:?}"
    );
    assert!(claude.iter().any(|a| a.contains("WebFetch")), "{claude:?}");

    for (host, model) in [("codex", "gpt-x"), ("omp", "o/m"), ("pi", "p/m")] {
        let (kind, args) = started_arguments(host);
        assert_eq!(kind, host);
        assert_eq!(
            after(&args, "--model").as_deref(),
            Some(model),
            "{host}: {args:?}"
        );
        assert!(
            !args
                .iter()
                .any(|a| a.contains("Bash(git status)") || a.contains("WebFetch")),
            "{host}: {args:?}"
        );
        assert!(
            !args
                .iter()
                .any(|a| a == "--allowedTools" || a == "--disallowedTools"),
            "{host}: {args:?}"
        );
    }
}

#[test]
fn the_pm_gets_the_memory_guard_instead_of_the_bare_write_and_edit_deny() {
    let sc = Scenario::new("pm-memory-guard", json!({"toml": toml("claude")}));
    sc.launcher.launch_pm().expect("the PM launches");
    let calls = sc.take_calls();
    let start = calls
        .iter()
        .find(|c| c["m"] == "startAgent")
        .expect("the PM started");
    let args: Vec<String> = start["a"][0]["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap().to_string())
        .collect();
    let from = args
        .iter()
        .position(|a| a == "--disallowedTools")
        .expect("a deny list");
    let denied: Vec<&String> = args[from + 1..]
        .iter()
        .take_while(|a| !a.starts_with("--"))
        .collect();
    let settings = after(&args, "--settings").expect("settings");
    assert!(settings.contains("guard-write"), "{settings}");
    assert!(
        denied.iter().all(|r| *r != "Write" && *r != "Edit"),
        "{denied:?}"
    );
    assert!(denied.iter().any(|r| *r == "NotebookEdit"));
    assert!(denied.iter().any(|r| *r == "Agent") && denied.iter().any(|r| *r == "Task"));
    assert!(
        args.iter()
            .any(|a| a.starts_with("Edit(//") && a.ends_with("/memory/**)")),
        "{args:?}"
    );
    // Every file-editing tool is either denied outright or matched by the guard hook.
    let parsed: Value = serde_json::from_str(&settings).unwrap();
    let matcher = parsed["hooks"]["PreToolUse"][0]["matcher"]
        .as_str()
        .unwrap();
    for tool in ["Write", "Edit", "MultiEdit", "NotebookEdit"] {
        assert!(
            denied.iter().any(|r| *r == tool) || matcher.split('|').any(|m| m == tool),
            "{tool} is neither denied nor guarded: {denied:?} {matcher}"
        );
    }
    assert!(settings.contains("\"disableAllHooks\":false"), "{settings}");
    assert!(
        settings.contains("/projects/") && settings.contains("/memory'"),
        "{settings}"
    );
}

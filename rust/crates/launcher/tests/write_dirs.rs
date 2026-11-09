//! A developer-kind role with `write_dirs` gets the write guard hook with absolute folders under its worktree, and
//! loses the bare Write and Edit deny; without a worktree it keeps the full deny and gets no guard.
mod common;

use capstan_launcher::api::LauncherService;
use common::engine::Scenario;
use serde_json::{json, Value};

fn toml(dirs: &str) -> String {
    format!(
        r#"schema_version = 1
[nexora]
track = "never"
[hosts.claude]
kind = "claude"
[roles.pm]
kind = "PM"
host = "claude"
[roles.architect]
kind = "Developer"
host = "claude"
allow = ["Bash(git status)"]
deny = ["Write", "Edit", "NotebookEdit", "Write(rust/**)"]
{dirs}
"#
    )
}

fn denied(args: &[String]) -> Vec<String> {
    let from = args.iter().position(|a| a == "--disallowedTools").unwrap();
    args[from + 1..]
        .iter()
        .take_while(|a| !a.starts_with("--"))
        .cloned()
        .collect()
}

fn settings(args: &[String]) -> Value {
    let at = args.iter().position(|a| a == "--settings").unwrap();
    serde_json::from_str(&args[at + 1]).unwrap()
}

fn spawned_arguments(name: &str, dirs: &str) -> Vec<String> {
    let sc = Scenario::new(name, json!({"toml": toml(dirs)}));
    sc.launcher.launch_pm().expect("the PM launches");
    sc.launcher
        .spawn("architect", &Default::default())
        .expect("the architect spawns");
    let calls = sc.take_calls();
    let start = calls
        .iter()
        .rev()
        .find(|c| c["m"] == "startAgent")
        .expect("the architect was started");
    start["a"][0]["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap().to_string())
        .collect()
}

#[test]
fn the_guard_gets_absolute_folders_under_the_worktree() {
    let args = spawned_arguments(
        "write-dirs-on",
        "write_dirs = [\"docs/architecture\", \"docs/research\"]",
    );
    let deny = denied(&args);
    assert!(deny.iter().all(|r| r != "Write" && r != "Edit"), "{deny:?}");
    assert!(deny.iter().any(|r| r == "NotebookEdit"));
    assert!(deny.iter().any(|r| r == "Write(rust/**)"));
    let command = settings(&args)["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        command.starts_with("cstan guard-write --dirs '") && command.ends_with("|| exit 2"),
        "{command}"
    );
    assert!(
        command.contains("/architect-1/docs/architecture' '")
            && command.contains("/docs/research'")
    );
    assert!(args
        .iter()
        .any(|a| a.starts_with("Edit(/") && a.ends_with("/architect-1/docs/architecture/**)")));
}

#[test]
fn a_role_without_write_dirs_keeps_its_arguments() {
    let args = spawned_arguments("write-dirs-off", "");
    let deny = denied(&args);
    assert!(deny.iter().any(|r| r == "Write") && deny.iter().any(|r| r == "Edit"));
    assert!(!settings(&args).to_string().contains("guard-write"));
}

#[test]
fn without_a_worktree_the_full_deny_stays_and_no_guard_is_installed() {
    let sc = Scenario::new(
        "write-dirs-no-tree",
        json!({"toml": toml("write_dirs = [\"docs/architecture\"]")}),
    );
    let role = sc
        .launcher
        .kernel()
        .role_named("architect")
        .expect("the role");
    let args = sc
        .launcher
        .kernel()
        .arguments(role, "prompt", "/tmp/prompt.md", None)
        .unwrap();
    let deny = denied(&args);
    assert!(deny.iter().any(|r| r == "Write") && deny.iter().any(|r| r == "Edit"));
    assert!(!settings(&args).to_string().contains("guard-write"));
    assert!(!args.iter().any(|a| a.starts_with("Edit(//")));
}

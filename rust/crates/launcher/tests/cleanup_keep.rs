//! `[cleanup] worktrees = "keep"`: release closes the pane and ends the agent but leaves the worktree and its branch.
mod common;

use capstan_launcher::api::{LaunchState, LauncherService};
use common::engine::Scenario;
use serde_json::{json, Value};
use std::path::Path;

fn released(name: &str, spec: Value) -> (Scenario, Value, String, Vec<Value>) {
    let sc = Scenario::new(name, spec);
    let launch = sc.launcher.launch_pm().expect("the PM launches");
    assert_eq!(launch.state, Some(LaunchState::Started));
    let spawned = sc
        .launcher
        .spawn("developer", &Default::default())
        .expect("spawn")
        .to_value();
    let path = spawned["worktreePath"].as_str().unwrap().to_string();
    sc.take_events();
    let outcome = sc
        .launcher
        .release(spawned["agentId"].as_str().unwrap())
        .expect("released")
        .to_value();
    let events = sc.take_events();
    (sc, outcome, path, events)
}

#[test]
fn keep_leaves_the_worktree_and_branch_and_logs_config_keep() {
    let (sc, outcome, path, events) = released(
        "cleanup-keep",
        json!({"maxWorkers": 3, "cleanup": {"worktrees": "keep"}}),
    );
    assert_eq!(outcome["state"], "released", "{outcome}");
    assert_eq!(outcome["paneClosed"], true, "{outcome}");
    assert_eq!(outcome["worktreeRemoved"], false, "{outcome}");
    assert_eq!(outcome["branchKept"], true, "{outcome}");
    assert!(Path::new(&path).exists(), "the worktree directory stays");
    let kept: Vec<&Value> = events
        .iter()
        .filter(|e| e["event"] == "worktree_kept_by_config")
        .collect();
    assert_eq!(kept.len(), 1, "{events:?}");
    assert_eq!(kept[0]["details"]["reason"], "config_keep");
    // Not a failed removal: no worktree_kept event and nothing listed as cleanupFailed.
    assert!(
        events.iter().all(|e| e["event"] != "worktree_kept"),
        "{events:?}"
    );
    assert_eq!(sc.launcher.status().to_value()["cleanupFailed"], json!([]));
    // The pane row is cleared, so a restart has nothing to retry.
    sc.take_events();
    sc.launcher.adopt_all().ok();
    assert!(sc
        .take_events()
        .iter()
        .all(|e| e["event"] != "worktree_removing"));
    assert!(Path::new(&path).exists());
}

#[test]
fn release_mode_still_removes_the_worktree() {
    let (_sc, outcome, path, events) = released("cleanup-release", json!({"maxWorkers": 3}));
    assert_eq!(outcome["worktreeRemoved"], true, "{outcome}");
    assert!(!Path::new(&path).exists());
    assert!(events
        .iter()
        .all(|e| e["event"] != "worktree_kept_by_config"));
}

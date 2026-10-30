//! `cstan decide integration:<id> ... merge` through the executable against a real daemon: a reviewed integration merges
//! into the project's branch, a retry with the same nonce is a repeat, the kill switch stops a new merge and not a stored
//! answer, a project without a git identity says so, and a marker left across a daemon restart blocks the merge.

use capstan_blackbox::*;
use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;

const MARKER: &str = "capstan-remote-merge";

fn config(merge: bool) -> String {
    format!("{TEAM_CONFIG}\n[remote]\nmerge = {merge}\n")
}

fn world(label: &str, identity: bool) -> (World, String, String) {
    let mut w = World::builder(label)
        .config(&config(true))
        .agents(&[
            agent("pm-1", "pm", "PM"),
            agent("dev-1", "developer", "Developer"),
            agent("rev-1", "reviewer", "Verifier"),
        ])
        .build();
    // The merge targets the remote's default branch.
    w.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
    w.git(&[
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/main",
    ]);
    if identity {
        w.git(&["config", "user.name", "Merge Test"]);
        w.git(&["config", "user.email", "merge@example.com"]);
    }
    let (integration, _branch, head) = w.with_reviewed_integration("pm-1", "dev-1", "rev-1");
    (w, integration, head)
}

fn set_merge(w: &World, merge: bool) {
    let file = w.project.join("capstan.toml");
    std::fs::write(&file, config(merge)).unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
}

/// The first 12 characters of the hash the feed holds for `request`.
fn hash12(w: &World, request: &str) -> String {
    let answer = w.wire_op("feed", &["hash", request]);
    answer["result"]["contentHash"]
        .as_str()
        .unwrap_or_else(|| panic!("{answer}"))[..12]
        .to_string()
}

fn merge(w: &World, integration: &str, hash: &str, nonce: &str) -> Out {
    let root = w.project.to_str().unwrap().to_string();
    let request = format!("integration:{integration}");
    w.decide(&[
        "--project",
        &root,
        "--json",
        &request,
        "--expect",
        hash,
        "--nonce",
        nonce,
        "merge",
    ])
}

fn answer(out: &Out) -> Value {
    assert!(out.code == 0 || out.code == 1, "{}", out.text());
    out.json()
}

#[test]
fn a_merge_applies_repeats_with_its_nonce_and_a_stored_nonce_outlives_the_switch() {
    let (w, integration, head) = world("merge-apply", true);
    let hash = hash12(&w, &format!("integration:{integration}"));
    let before = w.git(&["rev-parse", "HEAD"]);
    let first = answer(&merge(&w, &integration, &hash, "nonce-0001"));
    assert_eq!(first["outcome"], "applied", "{first}");
    assert_eq!(first["repeat"], false, "{first}");
    let parents = w.git(&["rev-list", "--parents", "-n1", "HEAD"]);
    assert_eq!(
        parents.split(' ').skip(1).collect::<Vec<_>>(),
        [before.as_str(), head.as_str()]
    );
    let again = answer(&merge(&w, &integration, &hash, "nonce-0001"));
    assert_eq!(again["repeat"], true, "{again}");
    // The kill switch stops new merges; a stored answer is still repeated.
    set_merge(&w, false);
    let repeated = answer(&merge(&w, &integration, &hash, "nonce-0001"));
    assert_eq!(repeated["repeat"], true, "{repeated}");
}

#[test]
fn with_merge_off_a_new_merge_is_disabled() {
    let (w, integration, _head) = world("merge-off", true);
    let hash = hash12(&w, &format!("integration:{integration}"));
    set_merge(&w, false);
    let out = merge(&w, &integration, &hash, "nonce-0002");
    assert_eq!(out.code, 1, "{}", out.text());
    assert_eq!(out.json()["error"]["code"], "merge_disabled");
}

#[test]
fn a_repository_without_a_git_identity_answers_no_identity() {
    let (w, integration, _head) = world("merge-noid", false);
    let hash = hash12(&w, &format!("integration:{integration}"));
    let before = w.git(&["rev-parse", "HEAD"]);
    let out = merge(&w, &integration, &hash, "nonce-0003");
    assert_eq!(out.code, 1, "{}", out.text());
    assert_eq!(out.json()["error"]["code"], "no_identity");
    assert_eq!(w.git(&["rev-parse", "HEAD"]), before);
}

#[test]
fn a_marker_left_across_a_daemon_restart_is_reported_and_kept() {
    let (mut w, integration, head) = world("merge-marker", true);
    let hash = hash12(&w, &format!("integration:{integration}"));
    let marker = w.project.join(w.git(&["rev-parse", "--git-path", MARKER]));
    std::fs::write(
        &marker,
        json!({"headSha": head, "nonce": "old", "integrationId": integration}).to_string(),
    )
    .unwrap();
    w.restart();
    let before = w.git(&["rev-parse", "HEAD"]);
    let out = merge(&w, &integration, &hash, "nonce-0004");
    assert_eq!(out.code, 1, "{}", out.text());
    let refused = out.json();
    assert_eq!(refused["error"]["code"], "merge_failed", "{refused}");
    assert!(
        refused["error"]["message"]
            .as_str()
            .unwrap()
            .contains("interrupted"),
        "{refused}"
    );
    assert!(marker.exists());
    assert_eq!(w.git(&["rev-parse", "HEAD"]), before);
}

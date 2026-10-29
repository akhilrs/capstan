//! `cstan decide` through the executable against a real daemon: run from outside the project with an agent environment
//! that points nowhere, it decides by the project root, a retry with the same nonce is a repeat, a stale hash is refused,
//! an approval is only forwarded to the PM, and a controller that is not running is not started.

use capstan_blackbox::*;
use serde_json::Value;
use std::time::{Duration, Instant};

fn world(label: &str) -> World {
    World::builder(label)
        .config(TEAM_CONFIG)
        .launching()
        .agents(&[
            agent("pm-1", "pm", "PM"),
            agent("ops-1", "operator", "Developer"),
            agent("dev-1", "developer", "Developer"),
        ])
        .build()
}

fn proposal(w: &World, command: &str) -> String {
    let out = w
        .as_agent("ops-1", &["op", "propose", command, "a reason"])
        .ok();
    let id = out
        .stdout
        .lines()
        .find_map(|l| l.strip_prefix("proposalId: "))
        .unwrap_or_else(|| panic!("no proposalId in {}", out.stdout));
    format!("op:{}", id.trim())
}

/// The first 12 characters of the hash the feed holds for `request`.
fn hash12(w: &World, request: &str) -> String {
    let answer = w.wire_op("feed", &["hash", request]);
    answer["result"]["contentHash"]
        .as_str()
        .unwrap_or_else(|| panic!("{answer}"))[..12]
        .to_string()
}

fn root(w: &World) -> String {
    w.project.to_str().unwrap().to_string()
}

fn decided(w: &World, args: &[&str]) -> Value {
    let root = root(w);
    let mut all = vec!["--project", root.as_str(), "--json"];
    all.extend_from_slice(args);
    let out = w.decide(&all);
    assert!(out.code == 0 || out.code == 1, "{}", out.text());
    out.json()
}

#[test]
fn deny_applies_then_repeats_and_a_stale_hash_is_refused() {
    let w = world("decide-deny");
    let request = proposal(&w, "echo one");
    let hash = hash12(&w, &request);
    let root = root(&w);
    let args = [
        "--project",
        root.as_str(),
        "--json",
        request.as_str(),
        "--expect",
        hash.as_str(),
        "--nonce",
        "nonce-0001",
        "deny",
    ];
    let first = w.decide(&args).ok().json();
    assert_eq!(first["outcome"], "applied", "{first}");
    assert_eq!(first["repeat"], false);
    let again = w.decide(&args).ok().json();
    assert_eq!(again["repeat"], true, "{again}");
    assert_eq!(again["decisionId"], first["decisionId"]);
    assert_eq!(
        w.event_rows("remote_decision"),
        1,
        "a repeat writes no event"
    );

    // Plain output is one line.
    let other = proposal(&w, "echo two");
    let other_hash = hash12(&w, &other);
    let plain = w
        .decide(&[
            "--project",
            root.as_str(),
            other.as_str(),
            "--expect",
            other_hash.as_str(),
            "--nonce",
            "nonce-0002",
            "deny",
        ])
        .ok();
    assert_eq!(plain.stdout.lines().count(), 1, "{}", plain.stdout);
    assert!(plain.stdout.contains("applied"), "{}", plain.stdout);

    // The state changes under a stale hash.
    let old = hash12(&w, "agent:dev-1");
    w.op(&["pause", "dev-1", "--reason", "looking"]).ok();
    let stale = decided(
        &w,
        &[
            "agent:dev-1",
            "--expect",
            &old,
            "--nonce",
            "nonce-0003",
            "resume",
        ],
    );
    assert_eq!(stale["error"]["code"], "hash_mismatch", "{stale}");
    assert_eq!(stale["v"], 1);
}

#[test]
fn approve_is_forwarded_to_the_pm_as_a_command_block() {
    let w = world("decide-approve");
    let request = proposal(&w, "echo approve me");
    let hash = hash12(&w, &request);
    let answer = decided(
        &w,
        &[
            &request,
            "--expect",
            &hash,
            "--nonce",
            "nonce-0001",
            "approve",
        ],
    );
    assert_eq!(answer["outcome"], "forwarded", "{answer}");
    let inbox = w.as_agent("pm-1", &["inbox"]).ok();
    assert!(
        inbox.stdout.contains("cstan op decide") && inbox.stdout.contains("approve --hash"),
        "{}",
        inbox.stdout
    );
}

#[test]
fn pause_resume_pause_with_three_nonces_all_apply_and_leave_one_event_each() {
    let w = world("decide-pause");
    let request = "agent:dev-1";
    let mut applied = 0;
    let mut last = String::new();
    for (nonce, verb) in [
        ("nonce-0001", "pause"),
        ("nonce-0002", "resume"),
        ("nonce-0003", "pause"),
    ] {
        let hash = hash12(&w, request);
        let answer = decided(&w, &[request, "--expect", &hash, "--nonce", nonce, verb]);
        assert_eq!(answer["outcome"], "applied", "{answer}");
        applied += 1;
        last = hash;
        assert_eq!(w.event_rows("remote_decision"), applied);
    }
    // A repeat of the last one (same hash and verb) adds no row.
    let repeat = decided(
        &w,
        &[request, "--expect", &last, "--nonce", "nonce-0003", "pause"],
    );
    assert_eq!(repeat["repeat"], true, "{repeat}");
    assert_eq!(w.event_rows("remote_decision"), applied);
}

#[test]
fn a_stopped_controller_and_a_stale_socket_are_not_answering_and_nothing_is_started() {
    let mut w = world("decide-stopped");
    let request = proposal(&w, "echo gone");
    let hash = hash12(&w, &request);
    w.stop();
    let _ = std::fs::remove_file(w.socket());
    let root = root(&w);
    let args = [
        "--project",
        root.as_str(),
        "--json",
        request.as_str(),
        "--expect",
        hash.as_str(),
        "--nonce",
        "nonce-0001",
        "deny",
    ];
    let started = Instant::now();
    let out = w.decide(&args);
    assert_eq!(out.code, 1, "{}", out.text());
    assert_eq!(out.json()["error"]["code"], "not_answering");
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(!w.socket().exists(), "no controller was started");
    assert!(!w.daemon_alive());

    // A socket file that nothing listens on.
    drop(std::os::unix::net::UnixListener::bind(w.socket()).unwrap());
    assert!(w.socket().exists());
    let started = Instant::now();
    let out = w.decide(&args);
    assert_eq!(out.code, 1, "{}", out.text());
    assert_eq!(out.json()["error"]["code"], "not_answering");
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(!w.daemon_alive());
}

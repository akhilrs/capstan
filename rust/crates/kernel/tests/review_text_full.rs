//! A reviewer's text is stored whole (up to MAX_REVIEW_TEXT_BYTES); the verdict notice quotes a long one cut, with a
//! pointer to `cstan review show <review-id>`.

mod common;
#[path = "rust_only/fixture.rs"]
mod fixture;

use capstan_kernel::records::MAX_REVIEW_TEXT_BYTES;
use fixture::{context, step, Fixture};
use serde_json::json;

const BASE: &str = "0123456789abcdef0123456789abcdef01234567";
const COMMIT: &str = "1111111111111111111111111111111111111111";
const STAMP: &str = "2026-01-02T00:00:00.000Z";
/// The most a message body may hold (the daemon's MAX_SEND_BODY_BYTES: 16 KiB less its 1 KiB frame reserve).
const MAX_SEND_BODY_BYTES: usize = 16 * 1024 - 1024;

struct Rig {
    f: Fixture,
    reviewer: String,
}

fn rig() -> Rig {
    let f = Fixture::new();
    step(
        f.core(),
        "syncRoleDefinitions",
        json!([[
            {"name": "pm", "kind": "PM", "host": "claude", "configHash": "b".repeat(64)},
            {"name": "developer", "kind": "Developer", "host": "claude", "configHash": "a".repeat(64)},
            {"name": "reviewer", "kind": "Verifier", "host": "claude", "configHash": "f".repeat(64)},
        ]]),
    );
    step(
        f.core(),
        "createSeat",
        json!([{"seatId": "seat-rev-1", "name": "seat-rev-1", "role": "Verifier"}]),
    );
    let actor = step(
        f.core(),
        "createActor",
        json!([{"displayName": "rev-1", "role": "Verifier", "seatId": "seat-rev-1"}]),
    );
    step(
        f.core(),
        "registerAgent",
        json!([{"agentId": "rev-1", "roleName": "reviewer", "seatId": "seat-rev-1", "actorId": actor["actorId"]}]),
    );
    let reviewer = actor["credential"].as_str().unwrap().to_string();
    Rig { f, reviewer }
}

fn db(f: &Fixture) -> rusqlite::Connection {
    rusqlite::Connection::open(f.database()).unwrap()
}

/// An accepted report of dev-1; returns its id.
fn report(f: &Fixture, n: i64) -> String {
    let db = db(f);
    let (project, actor): (String, String) = db
        .query_row(
            "SELECT project_id, actor_id FROM agents WHERE agent_id = 'dev-1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let id = format!("report-{n}");
    db.execute(
        "INSERT INTO agent_reports(project_id, report_id, sequence, agent_id, generation, actor_id, commit_sha, branch, summary, state, evidence_json, created_at)
         VALUES (?, ?, ?, 'dev-1', 1, ?, ?, 'b', 'did the work', 'accepted', ?, ?)",
        rusqlite::params![project, id, n, actor, COMMIT, json!({"baseSha": BASE}).to_string(), STAMP],
    )
    .unwrap();
    id
}

/// Reviews a fresh report with `text`; the completed review, or the refusal message.
fn review(r: &Rig, n: i64, text: &str) -> Result<serde_json::Value, String> {
    let report = report(&r.f, n);
    let core = r.f.core();
    core.begin_review(
        &context(core, &r.f.pm_credential),
        &json!({"subjectId": report, "reviewerRole": "reviewer", "reviewerAgentId": "rev-1"}),
    )
    .unwrap();
    core.complete_review(
        &context(core, &r.reviewer),
        &json!({"verdict": "findings", "text": text}),
    )
    .map_err(|e| e.message().to_string())
}

fn notice(f: &Fixture, review_id: &str) -> String {
    db(f)
        .query_row(
            "SELECT m.body FROM reviews r JOIN messages m ON m.message_id = r.notified_message_id WHERE r.review_id = ?",
            [review_id],
            |x| x.get(0),
        )
        .unwrap()
}

#[test]
fn a_long_text_is_stored_whole_and_the_notice_points_at_it() {
    let r = rig();
    let text = format!("{}\n{}", "a".repeat(4499), "b".repeat(4500));
    assert_eq!(text.len(), 9000);
    let done = review(&r, 1, &text).unwrap();
    let id = done["reviewId"].as_str().unwrap();
    let inspected = r.f.core().inspect(id).unwrap();
    assert_eq!(inspected["record"]["verdict_text"], text);

    let body = notice(&r.f, id);
    let shown = json!(&text[..3600]).to_string();
    let marker = format!(" [cut: showing 3600 of 9000 bytes; full text: cstan review show {id}]");
    assert!(
        body.ends_with(&format!("{shown}{marker}")),
        "{}",
        &body[body.len().saturating_sub(300)..]
    );
    assert!(body.len() < MAX_SEND_BODY_BYTES);
}

#[test]
fn a_text_within_the_notice_limit_is_quoted_whole() {
    let r = rig();
    let text = "c".repeat(3999);
    let done = review(&r, 1, &text).unwrap();
    let id = done["reviewId"].as_str().unwrap();
    let body = notice(&r.f, id);
    let last = body.lines().last().unwrap();
    assert_eq!(
        last,
        format!(
            "The reviewer's text, not verified: \"{}\"",
            "c".repeat(3999)
        )
    );
    assert!(!body.contains("[cut:"));
}

#[test]
fn a_text_over_the_limit_is_refused() {
    let r = rig();
    let at = "d".repeat(MAX_REVIEW_TEXT_BYTES);
    assert!(review(&r, 1, &at).is_ok());
    let message = review(&r, 2, &"d".repeat(MAX_REVIEW_TEXT_BYTES + 1)).unwrap_err();
    assert!(message.contains("at most 16000 bytes"), "{message}");
}

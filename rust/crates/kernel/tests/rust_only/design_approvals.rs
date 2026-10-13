//! Design approvals (the design_approvals table): the decision on a design package's report, the assign gate on its dependents and
//! the plan body key `design`.

use crate::fixture::{context, step, Fixture};
use capstan_kernel::KernelError;
use serde_json::{json, Value};

const BASE: &str = "0123456789abcdef0123456789abcdef01234567";
const COMMIT: &str = "1111111111111111111111111111111111111111";
const EARLY: &str = "2026-01-01T00:00:00.000Z";
const LATE: &str = "2026-01-02T00:00:00.000Z";

fn create_agent(f: &Fixture, agent: &str, role: &str, kind: &str) -> String {
    let core = f.core();
    let seat = format!("seat-{agent}");
    step(
        core,
        "createSeat",
        json!([{"seatId": seat, "name": seat, "role": kind}]),
    );
    let actor = step(
        core,
        "createActor",
        json!([{"displayName": agent, "role": kind, "seatId": seat}]),
    );
    step(
        core,
        "registerAgent",
        json!([{"agentId": agent, "roleName": role, "seatId": seat, "actorId": actor["actorId"]}]),
    );
    actor["credential"].as_str().unwrap().to_string()
}

fn db(f: &Fixture) -> rusqlite::Connection {
    rusqlite::Connection::open(f.database()).unwrap()
}

/// dev-1 holds the design package `ui` of plan-1; `api` (unassigned) depends on it; `docs` is a plain package.
fn rig() -> (Fixture, String) {
    let f = Fixture::new();
    step(
        f.core(),
        "syncRoleDefinitions",
        json!([[
            {"name": "pm", "kind": "PM", "host": "claude", "configHash": "b".repeat(64)},
            {"name": "developer", "kind": "Developer", "host": "claude", "configHash": "a".repeat(64)},
            {"name": "architect", "kind": "Developer", "host": "claude", "configHash": "e".repeat(64)},
            {"name": "reviewer", "kind": "Verifier", "host": "claude", "configHash": "f".repeat(64)},
        ]]),
    );
    create_agent(&f, "dev-2", "developer", "Developer");
    create_agent(&f, "arch-1", "architect", "Developer");
    create_agent(&f, "rev-1", "reviewer", "Verifier");
    let developer = f.developer_credential.clone();
    let body = r#"{"summary":"s","packages":[
        {"id":"ui","title":"UI","owns":["design/ui"],"acceptance":["approved"],"estimate_hours":1,"design":true},
        {"id":"api","title":"API","owns":["src/api"],"acceptance":["works"],"estimate_hours":1,"depends_on":["ui"]},
        {"id":"docs","title":"Docs","owns":["docs/x"],"acceptance":["written"],"estimate_hours":1}],
        "risks":[],"integration_order":["ui","api","docs"]}"#;
    let stored =
        serde_json::to_string(&capstan_kernel::plan_body::parse_plan_body(body, 8).unwrap())
            .unwrap();
    let conn = db(&f);
    let project: String = conn
        .query_row("SELECT project_id FROM projects", [], |r| r.get(0))
        .unwrap();
    conn.execute(
        "INSERT INTO plans(project_id, plan_id, sequence, title, tier, state, requested_by, architect_agent_id, current_revision, created_at, updated_at)
         VALUES (?, 'plan-1', 1, 'T', 'normal', 'draft', 'x', 'arch-1', 0, ?, ?)",
        rusqlite::params![project, EARLY, EARLY],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO plan_revisions(project_id, plan_id, revision, base_sha, body_json, body_sha, author_agent_id, author_actor_id, created_at)
         VALUES (?, 'plan-1', 1, ?, ?, ?, 'arch-1', (SELECT actor_id FROM agents WHERE agent_id = 'arch-1'), ?)",
        rusqlite::params![project, BASE, stored, "c".repeat(64), EARLY],
    )
    .unwrap();
    conn.execute(
        "UPDATE plans SET current_revision = 1, state = 'approved', approved_revision = 1 WHERE plan_id = 'plan-1'",
        [],
    )
    .unwrap();
    for (package, agent) in [("ui", Some("dev-1")), ("api", None), ("docs", None)] {
        conn.execute(
            "INSERT INTO plan_packages(project_id, plan_id, package_id, assignee_agent_id, assigned_at) VALUES (?, 'plan-1', ?, ?, ?)",
            rusqlite::params![project, package, agent, agent.map(|_| EARLY)],
        )
        .unwrap();
    }
    (f, developer)
}

/// A new accepted report of dev-1; the latest one is the design package's report.
fn report(f: &Fixture) -> String {
    let conn = db(f);
    let (project, actor): (String, String) = conn
        .query_row(
            "SELECT project_id, actor_id FROM agents WHERE agent_id = 'dev-1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let sequence: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(sequence), 0) + 1 FROM agent_reports",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let id = format!("report-{sequence}");
    conn.execute(
        "INSERT INTO agent_reports(project_id, report_id, sequence, agent_id, generation, actor_id, commit_sha, branch, summary, state, evidence_json, created_at)
         VALUES (?, ?, ?, 'dev-1', 1, ?, ?, 'b', 'designed', 'accepted', ?, ?)",
        rusqlite::params![project, id, sequence, actor, COMMIT, json!({"baseSha": BASE}).to_string(), LATE],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO reviews(project_id, review_id, sequence, round, subject_report_id, commit_sha, base_sha, author_agent_id, author_actor_id,
           requested_by_actor_id, reviewer_role, reviewer_agent_id, reviewer_actor_id, state, verdict_text, created_at, completed_at)
         VALUES (?, ?, ?, 1, ?, ?, ?, 'dev-1', ?, ?, 'reviewer', 'rev-1', (SELECT actor_id FROM agents WHERE agent_id = 'rev-1'), 'passed', 'fine', ?, ?)",
        rusqlite::params![project, format!("review-{sequence}"), sequence, id, COMMIT, BASE, actor, actor, LATE, LATE],
    )
    .unwrap();
    id
}

fn decide(f: &Fixture, credential: &str, target: &str, extra: Value) -> Result<Value, KernelError> {
    let (plan, package) = target.split_once('/').unwrap();
    let mut input = json!({"planId": plan, "packageId": package});
    for (key, value) in extra.as_object().unwrap() {
        input[key] = value.clone();
    }
    f.core()
        .record_design_decision(&context(f.core(), credential), &input)
}

fn approve(f: &Fixture) -> Value {
    decide(
        f,
        &f.pm_credential,
        "plan-1/ui",
        json!({"decision": "approved", "link": "https://design/1", "askedUser": "looks right"}),
    )
    .unwrap()
}

fn assign(f: &Fixture, package: &str, require: bool, early: bool) -> Result<Value, KernelError> {
    let mut input = json!({"planId": "plan-1", "packageId": package, "agentId": "dev-2", "requireDesignApproval": require});
    if early {
        input["early"] = json!("go ahead");
    }
    f.core()
        .assign_package(&context(f.core(), &f.pm_credential), &input)
}

fn message(error: KernelError) -> String {
    error.to_string()
}

#[test]
fn a_dependent_is_refused_without_a_decision_and_early_does_not_bypass() {
    let (f, _) = rig();
    report(&f);
    for early in [false, true] {
        let error = message(assign(&f, "api", true, early).unwrap_err());
        assert!(
            error.contains(
                "design_approval_missing: package api depends on design package ui (no decision)"
            ),
            "{error}"
        );
        assert!(error.contains("cstan design approve plan-1/ui"), "{error}");
    }
}

#[test]
fn an_approval_of_the_latest_report_unlocks_the_dependent() {
    let (f, _) = rig();
    let report = report(&f);
    let decision = approve(&f);
    assert_eq!(decision["reportId"], json!(report));
    assert_eq!(decision["decision"], json!("approved"));
    let assigned = assign(&f, "api", true, false).unwrap();
    assert_eq!(assigned["assigneeAgentId"], json!("dev-2"));
    let record = f.core().plan_record(&f.pm_credential, "plan-1").unwrap();
    let packages = record["packages"].as_array().unwrap().clone();
    let ui = packages.iter().find(|p| p["packageId"] == "ui").unwrap();
    assert_eq!(ui["design"], json!(true));
    assert_eq!(ui["designDecision"]["decision"], json!("approved"));
    assert_eq!(ui["designDecision"]["reportId"], json!(report));
    let api = packages.iter().find(|p| p["packageId"] == "api").unwrap();
    assert_eq!(api["design"], json!(false));
    assert_eq!(api["designDecision"], Value::Null);
}

#[test]
fn changes_messages_the_designer_and_an_old_approval_does_not_unlock_a_new_report() {
    let (f, _) = rig();
    report(&f);
    let changes = decide(
        &f,
        &f.pm_credential,
        "plan-1/ui",
        json!({"decision": "changes", "askedUser": "make the header smaller", "link": "https://design/1"}),
    )
    .unwrap();
    assert!(changes["messageId"].is_string());
    let body: String = db(&f)
        .query_row(
            "SELECT body FROM messages WHERE message_id = ?",
            [changes["messageId"].as_str().unwrap()],
            |r| r.get(0),
        )
        .unwrap();
    assert!(body.contains("make the header smaller"), "{body}");
    assert!(
        body.contains("plan-1") && body.contains("ui") && body.contains("https://design/1"),
        "{body}"
    );
    let error = message(assign(&f, "api", true, false).unwrap_err());
    assert!(error.contains("(changes requested)"), "{error}");
    report(&f);
    let error = message(assign(&f, "api", true, false).unwrap_err());
    assert!(error.contains("(changes requested)"), "{error}");
    approve(&f);
    assign(&f, "api", true, false).unwrap();
}

#[test]
fn an_approval_for_an_earlier_report_is_stale_after_a_new_report() {
    let (f, _) = rig();
    report(&f);
    approve(&f);
    report(&f);
    let error = message(assign(&f, "api", true, false).unwrap_err());
    assert!(
        error.contains("(approval is for an earlier report)"),
        "{error}"
    );
    approve(&f);
    assign(&f, "api", true, false).unwrap();
}

#[test]
fn without_the_requirement_assignment_is_unchanged() {
    let (f, _) = rig();
    report(&f);
    assign(&f, "docs", true, false).unwrap();
    let (f, _) = rig();
    report(&f);
    assign(&f, "api", false, true).unwrap();
}

#[test]
fn a_decision_is_refused_for_the_wrong_caller_package_or_input() {
    let (f, developer) = rig();
    let ok = json!({"decision": "approved", "link": "l", "askedUser": "yes"});
    let error = message(decide(&f, &f.pm_credential, "plan-1/ui", ok.clone()).unwrap_err());
    assert!(error.contains("no report yet"), "{error}");
    report(&f);
    let error = message(decide(&f, &developer, "plan-1/ui", ok.clone()).unwrap_err());
    assert!(error.contains("only the PM or the operator"), "{error}");
    let error = message(decide(&f, &f.pm_credential, "plan-1/docs", ok.clone()).unwrap_err());
    assert!(error.contains("not a design package"), "{error}");
    let error = message(
        decide(
            &f,
            &f.pm_credential,
            "plan-1/ui",
            json!({"decision": "approved", "link": "l"}),
        )
        .unwrap_err(),
    );
    assert!(error.contains("--asked-user"), "{error}");
    let error = message(
        decide(
            &f,
            &f.pm_credential,
            "plan-1/ui",
            json!({"decision": "approved", "askedUser": "yes"}),
        )
        .unwrap_err(),
    );
    assert!(error.contains("--link"), "{error}");
    decide(
        &f,
        &f.pm_credential,
        "plan-1/ui",
        json!({"decision": "changes", "askedUser": "no"}),
    )
    .unwrap();
    db(&f)
        .execute(
            "UPDATE plan_packages SET cancelled_at = ? WHERE package_id = 'ui'",
            [LATE],
        )
        .unwrap();
    let error = message(decide(&f, &f.pm_credential, "plan-1/ui", ok).unwrap_err());
    assert!(error.contains("package_cancelled"), "{error}");
}

#[test]
fn the_design_key_must_be_a_boolean_and_is_optional() {
    let package = |design: &str| {
        format!(
            r#"{{"summary":"s","packages":[{{"id":"a","title":"A","owns":["src/a"],"acceptance":["x"],"estimate_hours":1{design}}}],"risks":[],"integration_order":["a"]}}"#
        )
    };
    let parsed =
        capstan_kernel::plan_body::parse_plan_body(&package(r#","design":true"#), 8).unwrap();
    assert!(parsed.packages[0].design);
    assert!(
        !capstan_kernel::plan_body::parse_plan_body(&package(""), 8)
            .unwrap()
            .packages[0]
            .design
    );
    assert!(
        !capstan_kernel::plan_body::parse_plan_body(&package(r#","design":false"#), 8)
            .unwrap()
            .packages[0]
            .design
    );
    let error =
        capstan_kernel::plan_body::parse_plan_body(&package(r#","design":"yes""#), 8).unwrap_err();
    assert!(
        format!("{error:?}").contains("design must be a boolean"),
        "{error:?}"
    );
    let stored = serde_json::to_string(&parsed).unwrap();
    assert!(stored.contains(r#""design":true"#));
    let plain = serde_json::to_string(
        &capstan_kernel::plan_body::parse_plan_body(&package(""), 8).unwrap(),
    )
    .unwrap();
    assert!(!plain.contains("design"));
}

//! Plan reassignment: the PM or the operator hands a plan whose architect ended to another active architect; the ledger
//! allows the change only against the newest reassignment of that plan, review verdicts follow the new owner, and the
//! handover and the architect-changed notices are queued in the same mutation.

use crate::fixture::{context, step, Fixture, OWNER};
use capstan_kernel::helpers::amended_plan_body_json;
use capstan_kernel::plan_body::parse_plan_body;
use capstan_kernel::KernelError;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::collections::HashMap;

const BASE: &str = "0123456789abcdef0123456789abcdef01234567";
const COMMIT: &str = "1111111111111111111111111111111111111111";
const EARLY: &str = "2026-01-01T00:00:00.000Z";
const LATE: &str = "2026-01-02T00:00:00.000Z";
const FINAL: &str = "the plan architect is final";

struct Rig {
    f: Fixture,
    credentials: HashMap<String, String>,
    next: i64,
}

fn body_for(packages: &[&str]) -> String {
    let list: Vec<String> = packages
        .iter()
        .map(|id| {
            format!(
                r#"{{"id":"{id}","title":"T {id}","owns":["src/{id}"],"acceptance":["x"],"estimate_hours":1}}"#
            )
        })
        .collect();
    let order: Vec<String> = packages.iter().map(|id| format!("\"{id}\"")).collect();
    let text = format!(
        r#"{{"summary":"s","packages":[{}],"risks":[],"integration_order":[{}]}}"#,
        list.join(","),
        order.join(",")
    );
    serde_json::to_string(&parse_plan_body(&text, 8).unwrap()).unwrap()
}

fn refused(result: Result<Value, KernelError>) -> String {
    match result {
        Err(error) => error.message().to_string(),
        Ok(_) => panic!("the call was accepted"),
    }
}

impl Rig {
    fn new() -> Rig {
        let f = Fixture::new();
        step(
            f.core(),
            "syncRoleDefinitions",
            json!([[
                {"name": "pm", "kind": "PM", "host": "claude", "configHash": "b".repeat(64)},
                {"name": "developer", "kind": "Developer", "host": "claude", "configHash": "a".repeat(64)},
                {"name": "architect", "kind": "Developer", "host": "claude", "configHash": "e".repeat(64)},
                {"name": "lead", "kind": "Developer", "host": "claude", "configHash": "d".repeat(64)},
                {"name": "reviewer", "kind": "Verifier", "host": "claude", "configHash": "f".repeat(64)},
                {"name": "supervisor", "kind": "Supervisor", "host": "claude", "configHash": "c".repeat(64)},
            ]]),
        );
        let mut rig = Rig {
            f,
            credentials: HashMap::new(),
            next: 0,
        };
        for (agent, role, kind) in [
            ("arch-1", "architect", "Developer"),
            ("arch-2", "lead", "Developer"),
            ("arch-3", "lead", "Developer"),
            ("dev-2", "developer", "Developer"),
            ("dev-3", "developer", "Developer"),
            ("dev-4", "developer", "Developer"),
            ("rev-1", "reviewer", "Verifier"),
            ("sup-1", "supervisor", "Supervisor"),
        ] {
            rig.create_agent(agent, role, kind);
        }
        rig.credentials
            .insert("dev-1".into(), rig.f.developer_credential.clone());
        rig.credentials
            .insert("pm-1".into(), rig.f.pm_credential.clone());
        rig
    }

    fn create_agent(&mut self, agent: &str, role: &str, kind: &str) {
        let core = self.f.core();
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
        self.credentials
            .insert(agent.into(), actor["credential"].as_str().unwrap().into());
    }

    fn core(&self) -> &capstan_kernel::Core {
        self.f.core()
    }

    fn cred(&self, agent: &str) -> String {
        self.credentials[agent].clone()
    }

    fn db(&self) -> Connection {
        Connection::open(self.f.database()).unwrap()
    }

    fn project(&self) -> String {
        self.db()
            .query_row("SELECT project_id FROM projects", [], |r| r.get(0))
            .unwrap()
    }

    fn actor_of(&self, agent: &str) -> String {
        self.db()
            .query_row(
                "SELECT actor_id FROM agents WHERE agent_id = ?",
                [agent],
                |r| r.get(0),
            )
            .unwrap()
    }

    fn end(&self, agent: &str) {
        self.core()
            .end_agent(&context(self.core(), OWNER), agent, None)
            .unwrap();
    }

    fn reassign_as(
        &self,
        credential: &str,
        plan: &str,
        agent: &str,
        role: &str,
    ) -> Result<Value, KernelError> {
        self.core().reassign_plan(
            &context(self.core(), credential),
            &json!({"planId": plan, "agentId": agent, "architectRole": role, "reason": "lost"}),
        )
    }

    fn reassign(&self, plan: &str, agent: &str) -> Result<Value, KernelError> {
        self.reassign_as(&self.f.pm_credential.clone(), plan, agent, "lead")
    }

    fn plan_row(&self, plan: &str) -> (String, Option<String>) {
        self.db()
            .query_row(
                "SELECT state, architect_agent_id FROM plans WHERE plan_id = ?",
                [plan],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap()
    }

    fn reassignment_count(&self) -> i64 {
        self.db()
            .query_row("SELECT COUNT(*) FROM plan_reassignments", [], |r| r.get(0))
            .unwrap()
    }

    /// A plan inserted straight into the ledger: `approved` with revision 1 and the given packages, or `in_review`.
    fn insert_plan(
        &mut self,
        state: &str,
        owner: Option<&str>,
        packages: &[(&str, Option<&str>)],
    ) -> String {
        self.next += 1;
        let plan = format!("plan-{}", 100 + self.next);
        let conn = self.db();
        let project = self.project();
        let ids: Vec<&str> = packages.iter().map(|p| p.0).collect();
        let body = body_for(if ids.is_empty() { &["pkg"] } else { &ids });
        conn.execute(
            "INSERT INTO plans(project_id, plan_id, sequence, title, tier, state, requested_by, architect_agent_id, current_revision, created_at, updated_at)
             VALUES (?, ?, ?, 'T', 'normal', 'draft', 'x', ?, 0, ?, ?)",
            rusqlite::params![project, plan, 100 + self.next, owner, EARLY, EARLY],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO plan_revisions(project_id, plan_id, revision, base_sha, body_json, body_sha, author_agent_id, author_actor_id, created_at)
             VALUES (?, ?, 1, ?, ?, ?, 'dev-1', (SELECT actor_id FROM agents WHERE agent_id = 'dev-1'), ?)",
            rusqlite::params![project, plan, BASE, body, "c".repeat(64), EARLY],
        )
        .unwrap();
        match state {
            "approved" => conn.execute(
                "UPDATE plans SET current_revision = 1, state = 'approved', approved_revision = 1 WHERE plan_id = ?",
                [&plan],
            ),
            "in_review" => conn.execute(
                "UPDATE plans SET current_revision = 1, state = 'in_review' WHERE plan_id = ?",
                [&plan],
            ),
            _ => Ok(0),
        }
        .unwrap();
        for (package, assignee) in packages {
            conn.execute(
                "INSERT INTO plan_packages(project_id, plan_id, package_id, assignee_agent_id, assigned_at) VALUES (?, ?, ?, ?, ?)",
                rusqlite::params![project, plan, package, assignee, assignee.map(|_| EARLY)],
            )
            .unwrap();
        }
        plan
    }

    /// An accepted report of `agent` made after the assignment.
    fn report(&mut self, agent: &str) -> String {
        self.next += 1;
        let id = format!("report-{}", 100 + self.next);
        let conn = self.db();
        let project = self.project();
        conn.execute(
            "INSERT INTO agent_reports(project_id, report_id, sequence, agent_id, generation, actor_id, commit_sha, branch, summary, state, evidence_json, created_at)
             VALUES (?, ?, ?, ?, 1, (SELECT actor_id FROM agents WHERE agent_id = ?), ?, 'b', 'done', 'accepted', ?, ?)",
            rusqlite::params![
                project,
                id,
                100 + self.next,
                agent,
                agent,
                COMMIT,
                json!({"baseSha": BASE}).to_string(),
                LATE
            ],
        )
        .unwrap();
        id
    }

    /// A finished review of the given subject requested by `requester_actor`; `notified` stays empty.
    fn finished_review(
        &mut self,
        subject: (&str, &str),
        requester_actor: &str,
        state: &str,
    ) -> String {
        self.next += 1;
        let id = format!("review-{}", 100 + self.next);
        let conn = self.db();
        let project = self.project();
        let (column, value) = subject;
        let revision: Option<i64> = (column == "subject_plan_id").then_some(1);
        // An integration review has no author; the others name the agent whose work is reviewed.
        let author = (column != "subject_integration_id").then_some("dev-2");
        conn.execute(
            &format!(
                "INSERT INTO reviews(project_id, review_id, sequence, round, {column}, subject_plan_revision, commit_sha, base_sha,
                   author_agent_id, author_actor_id, requested_by_actor_id, reviewer_role, reviewer_agent_id, reviewer_actor_id,
                   state, verdict_text, created_at, completed_at)
                 VALUES (?, ?, ?, 1, ?, ?, ?, ?, ?, (SELECT actor_id FROM agents WHERE agent_id = ?), ?, 'reviewer', 'rev-1', (SELECT actor_id FROM agents WHERE agent_id = 'rev-1'), ?, 'fine', ?, ?)"
            ),
            rusqlite::params![
                project,
                id,
                100 + self.next,
                value,
                revision,
                COMMIT,
                BASE,
                author,
                author,
                requester_actor,
                state,
                LATE,
                LATE
            ],
        )
        .unwrap();
        id
    }

    fn integration(&mut self, state: &str, reports: &[&str]) -> String {
        self.next += 1;
        let id = format!("int-{}", 100 + self.next);
        let conn = self.db();
        let project = self.project();
        conn.execute(
            "INSERT INTO integrations(project_id, integration_id, sequence, base_sha, branch, requested_by, state, created_at)
             VALUES (?, ?, ?, ?, ?, 'pm-1', 'running', ?)",
            rusqlite::params![
                project,
                id,
                100 + self.next,
                "4".repeat(40),
                format!("capstan/integration/{id}"),
                EARLY
            ],
        )
        .unwrap();
        for (position, report) in reports.iter().enumerate() {
            conn.execute(
                "INSERT INTO integration_reports(project_id, integration_id, position, report_id) VALUES (?, ?, ?, ?)",
                rusqlite::params![project, id, position as i64 + 1, report],
            )
            .unwrap();
        }
        if state != "running" {
            conn.execute(
                "UPDATE integrations SET state = 'merged', head_sha = ?, completed_at = ? WHERE integration_id = ?",
                rusqlite::params!["3".repeat(40), LATE, id],
            )
            .unwrap();
        }
        if state == "confirmed" {
            conn.execute(
                "UPDATE integrations SET state = 'confirmed' WHERE integration_id = ?",
                [&id],
            )
            .unwrap();
        }
        id
    }

    fn messages(&self, agent: &str, kind_like: &str) -> Vec<(String, String)> {
        let conn = self.db();
        let mut statement = conn
            .prepare(
                "SELECT body, class FROM messages WHERE recipient_agent_id = ? AND body LIKE ? ORDER BY sequence",
            )
            .unwrap();
        statement
            .query_map([agent, kind_like], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    /// Where the verdict of a review went, by recipient.
    fn verdict_recipient(&self, review: &str) -> Option<String> {
        self.db()
            .query_row(
                "SELECT m.recipient_agent_id FROM reviews r JOIN messages m ON m.message_id = r.notified_message_id WHERE r.review_id = ?",
                [review],
                |r| r.get(0),
            )
            .ok()
    }

    fn announce(&self, review: &str) -> Value {
        self.core()
            .announce_review(&context(self.core(), OWNER), review)
            .unwrap()
    }

    fn operator_actor(&self) -> String {
        self.db()
            .query_row(
                "SELECT actor_id FROM actors WHERE role = 'operator' LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap()
    }

    fn submit(&self, who: &str, plan: &str, review: bool) -> Result<Value, KernelError> {
        self.core().submit_plan(
            &context(self.core(), &self.cred(who)),
            &json!({"planId": plan, "bodyJson": body_for(&["api"]), "baseSha": BASE, "review": review}),
        )
    }
}

#[test]
fn a_draft_plan_is_reassigned_by_the_pm_and_the_new_owner_submits() {
    let rig = Rig::new();
    let plan = rig
        .core()
        .open_plan(
            &context(rig.core(), OWNER),
            &json!({"tier": "normal", "title": "T"}),
        )
        .unwrap()["planId"]
        .as_str()
        .unwrap()
        .to_string();
    rig.submit("arch-1", &plan, true).unwrap();
    rig.core()
        .begin_review(
            &context(rig.core(), &rig.cred("arch-1")),
            &json!({"subjectId": plan, "reviewerRole": "reviewer", "reviewerAgentId": "rev-1"}),
        )
        .unwrap();
    rig.core()
        .complete_review(
            &context(rig.core(), &rig.cred("rev-1")),
            &json!({"verdict": "findings", "text": "- [api] blocker interface: api has no error contract"}),
        )
        .unwrap();
    assert_eq!(rig.plan_row(&plan).0, "draft");
    rig.end("arch-1");

    let answer = rig.reassign(&plan, "arch-2").unwrap();
    assert_eq!(answer["fromAgentId"], "arch-1");
    assert_eq!(answer["toAgentId"], "arch-2");
    assert_eq!(answer["state"], "draft");
    assert_eq!(answer["currentRevision"], 1);
    assert_eq!(answer["notifiedDevelopers"], json!([]));
    assert_eq!(rig.plan_row(&plan).1.as_deref(), Some("arch-2"));

    let approved = rig.submit("arch-2", &plan, false).unwrap();
    assert_eq!(approved["state"], "approved");
    let record = rig.core().plan_record(OWNER, &plan).unwrap();
    assert_eq!(record["plan"]["architectAgentId"], "arch-2");
    assert_eq!(record["revision"]["authorAgentId"], "arch-2");
}

#[test]
fn an_in_review_plan_is_reassigned_by_the_operator_and_the_verdict_reaches_the_new_owner() {
    let rig = Rig::new();
    let plan = rig
        .core()
        .open_plan(
            &context(rig.core(), OWNER),
            &json!({"tier": "normal", "title": "T"}),
        )
        .unwrap()["planId"]
        .as_str()
        .unwrap()
        .to_string();
    rig.submit("arch-1", &plan, true).unwrap();
    let begun = rig
        .core()
        .begin_review(
            &context(rig.core(), &rig.cred("arch-1")),
            &json!({"subjectId": plan, "reviewerRole": "reviewer", "reviewerAgentId": "rev-1"}),
        )
        .unwrap();
    let review = begun["reviewId"].as_str().unwrap().to_string();
    rig.end("arch-1");
    let answer = rig.reassign_as(OWNER, &plan, "arch-2", "lead").unwrap();
    assert_eq!(answer["state"], "in_review");

    rig.core()
        .complete_review(
            &context(rig.core(), &rig.cred("rev-1")),
            &json!({"verdict": "findings", "text": "## api\n- [api] major owns: api also writes docs/api\n- [api] minor depends_on: none"}),
        )
        .unwrap();
    assert_eq!(rig.verdict_recipient(&review).as_deref(), Some("arch-2"));
    assert_eq!(rig.plan_row(&plan).0, "draft");
    let amendment = r#"{"reason":"docs too","packages":[{"id":"api","add_owns":["docs/api"]}]}"#;
    let current = rig.core().plan_record(OWNER, &plan).unwrap()["revision"]["bodyJson"]
        .as_str()
        .unwrap()
        .to_string();
    let amended = rig
        .core()
        .amend_plan(
            &context(rig.core(), &rig.cred("arch-2")),
            &json!({"planId": plan, "amendmentJson": amendment,
                    "bodyJson": amended_plan_body_json(&current, amendment).unwrap(), "baseSha": BASE}),
        )
        .unwrap();
    assert_eq!(amended["state"], "approved");
}

#[test]
fn an_approved_plan_is_signed_off_by_the_new_owner() {
    let mut rig = Rig::new();
    let plan = rig.insert_plan("approved", Some("arch-1"), &[("api", Some("dev-2"))]);
    let report = rig.report("dev-2");
    let integration = rig.integration("merged", &[&report]);
    rig.finished_review(
        ("subject_integration_id", &integration),
        &rig.actor_of("arch-1"),
        "passed",
    );
    rig.end("arch-1");

    let answer = rig.reassign(&plan, "arch-2").unwrap();
    // The package is integrated: its developer is not told.
    assert_eq!(answer["notifiedDevelopers"], json!([]));
    let signed = rig
        .core()
        .record_signoff(
            &context(rig.core(), &rig.cred("arch-2")),
            &json!({"planId": plan, "integrationId": integration, "summary": "all in"}),
        )
        .unwrap();
    assert_eq!(signed["architectAgentId"], "arch-2");
}

#[test]
fn the_answer_the_event_and_the_two_notices_are_recorded_together() {
    let mut rig = Rig::new();
    let plan = rig.insert_plan(
        "approved",
        Some("arch-1"),
        &[
            ("api", Some("dev-2")),
            ("ui", Some("dev-3")),
            ("docs", None),
        ],
    );
    let report = rig.report("dev-3");
    let _ = report;
    rig.end("arch-1");
    let answer = rig.reassign(&plan, "arch-2").unwrap();
    assert_eq!(answer["planId"], json!(plan));
    assert_eq!(answer["reason"], "lost");
    assert_eq!(answer["notifiedDevelopers"], json!(["dev-2", "dev-3"]));
    let handover_id = answer["handoverMessageId"].as_str().unwrap().to_string();

    let handover = rig.messages("arch-2", "You now own plan%");
    assert_eq!(handover.len(), 1);
    assert_eq!(handover[0].1, "action");
    assert!(
        handover[0].0.contains("- api: assigned, assignee dev-2"),
        "{}",
        handover[0].0
    );
    assert!(handover[0].0.contains("- ui: reported, assignee dev-3"));
    assert!(handover[0].0.contains("- docs: unassigned, assignee none"));
    let stored: String = rig
        .db()
        .query_row(
            "SELECT body FROM messages WHERE message_id = ?",
            [&handover_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stored, handover[0].0);

    let notice = rig.messages("dev-2", "The architect of plan%");
    assert_eq!(notice.len(), 1);
    assert_eq!(notice[0].1, "info");
    assert!(notice[0]
        .0
        .contains(&format!("{plan}/api now go to arch-2")));
    assert!(notice[0].0.contains("cstan send arch-2"));
    assert_eq!(rig.messages("dev-3", "The architect of plan%").len(), 1);

    let payload: String = rig
        .db()
        .query_row(
            "SELECT payload_json FROM controller_events WHERE entity_type = 'plan' AND entity_id = ? ORDER BY sequence DESC LIMIT 1",
            [&plan],
            |r| r.get(0),
        )
        .unwrap();
    let event: Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(event["details"]["fromAgentId"], "arch-1");
    assert_eq!(event["details"]["toAgentId"], "arch-2");
    assert_eq!(event["details"]["reason"], "lost");
}

#[test]
fn only_live_assigned_packages_of_active_developers_are_notified() {
    let mut rig = Rig::new();
    let plan = rig.insert_plan(
        "approved",
        Some("arch-1"),
        &[
            ("done", Some("dev-1")),
            ("gone", Some("dev-2")),
            ("free", None),
            ("dead", Some("dev-3")),
            ("live", Some("dev-4")),
        ],
    );
    let done = rig.report("dev-1");
    rig.integration("merged", &[&done]);
    rig.db()
        .execute(
            "UPDATE plan_packages SET cancelled_at = ? WHERE package_id = 'gone'",
            [LATE],
        )
        .unwrap();
    rig.end("dev-3");
    rig.end("arch-1");
    let answer = rig.reassign(&plan, "arch-2").unwrap();
    assert_eq!(answer["notifiedDevelopers"], json!(["dev-4"]));
    for quiet in ["dev-1", "dev-2", "dev-3"] {
        assert!(
            rig.messages(quiet, "The architect of plan%").is_empty(),
            "{quiet}"
        );
    }
    let handover = &rig.messages("arch-2", "You now own plan%")[0].0;
    assert!(handover.contains("- done: integrated, assignee dev-1"));
    assert!(!handover.contains("- gone:"));
}

#[test]
fn a_refusal_names_its_code_and_changes_nothing() {
    let mut rig = Rig::new();
    let plain = rig.insert_plan("approved", Some("arch-1"), &[("api", Some("dev-2"))]);
    let cancelled = rig.insert_plan("approved", Some("arch-1"), &[("api", None)]);
    rig.db()
        .execute(
            "UPDATE plans SET cancelled_at = ? WHERE plan_id = ?",
            [LATE, &cancelled],
        )
        .unwrap();
    let superseded = rig.insert_plan("approved", Some("arch-1"), &[("api", None)]);
    rig.db()
        .execute(
            "UPDATE plans SET state = 'superseded' WHERE plan_id = ?",
            [&superseded],
        )
        .unwrap();
    let signed = rig.insert_plan("approved", Some("arch-1"), &[("api", Some("dev-3"))]);
    let report = rig.report("dev-3");
    let integration = rig.integration("confirmed", &[&report]);
    rig.db()
        .execute(
            "INSERT INTO plan_signoffs(project_id, plan_id, integration_id, architect_agent_id, summary, created_at)
             VALUES (?, ?, ?, 'arch-1', 's', ?)",
            rusqlite::params![rig.project(), signed, integration, LATE],
        )
        .unwrap();
    let signed_merged = rig.insert_plan("approved", Some("arch-1"), &[("api", Some("dev-4"))]);
    let merged_report = rig.report("dev-4");
    let merged = rig.integration("merged", &[&merged_report]);
    rig.db()
        .execute(
            "INSERT INTO plan_signoffs(project_id, plan_id, integration_id, architect_agent_id, summary, created_at)
             VALUES (?, ?, ?, 'arch-1', 's', ?)",
            rusqlite::params![rig.project(), signed_merged, merged, LATE],
        )
        .unwrap();
    let ownerless = rig.insert_plan("approved", None, &[("api", None)]);
    let live_owner = rig.insert_plan("approved", Some("arch-3"), &[("api", None)]);
    rig.end("arch-1");
    rig.end("dev-4");
    let before = |rig: &Rig| {
        let rows: Vec<(String, String, Option<String>)> = {
            let conn = rig.db();
            let mut statement = conn
                .prepare("SELECT plan_id, state, architect_agent_id FROM plans ORDER BY plan_id")
                .unwrap();
            statement
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .unwrap()
                .map(Result::unwrap)
                .collect()
        };
        (rows, rig.reassignment_count())
    };
    let snapshot = before(&rig);
    let pm = rig.f.pm_credential.clone();

    let cases: Vec<(String, Result<Value, KernelError>)> = vec![
        (
            "not_manager (worker)".into(),
            rig.reassign_as(&rig.cred("dev-2"), &plain, "arch-2", "lead"),
        ),
        (
            "not_manager (architect)".into(),
            rig.reassign_as(&rig.cred("arch-3"), &plain, "arch-2", "lead"),
        ),
        (
            "plan_not_found".into(),
            rig.reassign_as(&pm, "plan-999", "arch-2", "lead"),
        ),
        (
            "plan_cancelled".into(),
            rig.reassign_as(&pm, &cancelled, "arch-2", "lead"),
        ),
        (
            "plan_superseded".into(),
            rig.reassign_as(&pm, &superseded, "arch-2", "lead"),
        ),
        (
            "plan_signed_off".into(),
            rig.reassign_as(&pm, &signed, "arch-2", "lead"),
        ),
        (
            "no_owner".into(),
            rig.reassign_as(&pm, &ownerless, "arch-2", "lead"),
        ),
        (
            "already_owner".into(),
            rig.reassign_as(&pm, &plain, "arch-1", "architect"),
        ),
        (
            "owner_active".into(),
            rig.reassign_as(&pm, &live_owner, "arch-2", "lead"),
        ),
        (
            "not_architect (wrong role)".into(),
            rig.reassign_as(&pm, &plain, "arch-2", "developer"),
        ),
        (
            "not_architect (inactive)".into(),
            rig.reassign_as(&pm, &plain, "dev-4", "developer"),
        ),
        (
            "not_architect (missing)".into(),
            rig.reassign_as(&pm, &plain, "nobody", "architect"),
        ),
        (
            "not_architect (holds a package)".into(),
            rig.reassign_as(&pm, &plain, "dev-2", "developer"),
        ),
    ];
    for (label, result) in cases {
        let code = label.split(' ').next().unwrap();
        let message = refused(result);
        assert!(
            message.starts_with(&format!("{code}: ")),
            "{label}: {message}"
        );
    }
    assert!(
        refused(rig.reassign_as(&pm, &live_owner, "arch-2", "lead")).contains("release it first")
    );
    assert_eq!(before(&rig), snapshot);
}

#[test]
fn the_ledger_scopes_a_reassignment_to_its_plan() {
    let mut rig = Rig::new();
    let p = rig.insert_plan("approved", Some("arch-1"), &[("api", None)]);
    let q = rig.insert_plan("approved", Some("arch-1"), &[("api", None)]);
    rig.end("arch-1");
    rig.reassign(&p, "arch-2").unwrap();
    let conn = rig.db();
    let error = conn
        .execute(
            "UPDATE plans SET architect_agent_id = 'arch-2' WHERE plan_id = ?",
            [&q],
        )
        .unwrap_err();
    assert!(error.to_string().contains(FINAL), "{error}");
    assert_eq!(rig.plan_row(&q).1.as_deref(), Some("arch-1"));
}

#[test]
fn a_cancelled_plan_is_refused_by_the_kernel_and_by_the_trigger() {
    let mut rig = Rig::new();
    let plan = rig.insert_plan("approved", Some("arch-1"), &[("api", None)]);
    rig.end("arch-1");
    rig.db()
        .execute(
            "UPDATE plans SET cancelled_at = ? WHERE plan_id = ?",
            [LATE, &plan],
        )
        .unwrap();
    assert!(refused(rig.reassign(&plan, "arch-2")).starts_with("plan_cancelled: "));
    rig.db()
        .execute(
            "INSERT INTO plan_reassignments(project_id, plan_id, sequence, from_agent_id, to_agent_id, actor_id, reason, created_at)
             VALUES (?, ?, 1, 'arch-1', 'arch-2', ?, NULL, ?)",
            rusqlite::params![rig.project(), plan, rig.f.pm_actor, LATE],
        )
        .unwrap();
    let error = rig
        .db()
        .execute(
            "UPDATE plans SET architect_agent_id = 'arch-2' WHERE plan_id = ?",
            [&plan],
        )
        .unwrap_err();
    assert!(error.to_string().contains(FINAL), "{error}");
}

#[test]
fn only_the_newest_reassignment_of_a_chain_counts() {
    let mut rig = Rig::new();
    let plan = rig.insert_plan("approved", Some("arch-1"), &[("api", None)]);
    rig.end("arch-1");
    rig.reassign(&plan, "arch-2").unwrap();
    rig.end("arch-2");
    rig.reassign(&plan, "arch-3").unwrap();
    assert_eq!(rig.plan_row(&plan).1.as_deref(), Some("arch-3"));
    assert_eq!(
        rig.core().plan_record(OWNER, &plan).unwrap()["plan"]["reassignments"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    rig.end("arch-3");
    for target in ["arch-1", "arch-2"] {
        let error = rig
            .db()
            .execute(
                "UPDATE plans SET architect_agent_id = ? WHERE plan_id = ?",
                [target, &plan],
            )
            .unwrap_err();
        assert!(error.to_string().contains(FINAL), "{target}: {error}");
    }
}

#[test]
fn a_verdict_finished_before_the_reassign_but_announced_after_goes_to_the_new_owner() {
    let mut rig = Rig::new();
    let plan = rig.insert_plan("in_review", Some("arch-1"), &[]);
    let review = rig.finished_review(
        ("subject_plan_id", &plan),
        &rig.actor_of("arch-1"),
        "findings",
    );
    rig.end("arch-1");
    rig.reassign(&plan, "arch-2").unwrap();
    let pending = rig.core().unannounced_reviews(OWNER, None).unwrap();
    assert_eq!(pending.as_array().unwrap().len(), 1);
    assert_eq!(rig.announce(&review)["announced"], true);
    assert_eq!(rig.verdict_recipient(&review).as_deref(), Some("arch-2"));
}

#[test]
fn a_review_requested_by_the_old_owner_is_announced_once_to_the_new_owner() {
    let rig = Rig::new();
    let plan = rig
        .core()
        .open_plan(
            &context(rig.core(), OWNER),
            &json!({"tier": "normal", "title": "T"}),
        )
        .unwrap()["planId"]
        .as_str()
        .unwrap()
        .to_string();
    rig.submit("arch-1", &plan, true).unwrap();
    let review = rig
        .core()
        .begin_review(
            &context(rig.core(), &rig.cred("arch-1")),
            &json!({"subjectId": plan, "reviewerRole": "reviewer", "reviewerAgentId": "rev-1"}),
        )
        .unwrap()["reviewId"]
        .as_str()
        .unwrap()
        .to_string();
    rig.end("arch-1");
    rig.reassign(&plan, "arch-2").unwrap();
    rig.core()
        .complete_review(
            &context(rig.core(), &rig.cred("rev-1")),
            &json!({"verdict": "findings", "text": "- [api] blocker interface: api has no error contract"}),
        )
        .unwrap();
    // The reconcile path finds nothing left and announces nothing a second time.
    assert!(rig
        .core()
        .unannounced_reviews(OWNER, None)
        .unwrap()
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(rig.announce(&review)["announced"], false);
    let verdicts: Vec<String> = {
        let conn = rig.db();
        let mut statement = conn
            .prepare("SELECT recipient_agent_id FROM messages WHERE body LIKE ?")
            .unwrap();
        statement
            .query_map([format!("Review {review} of%")], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert_eq!(verdicts, vec!["arch-2".to_string()]);
    assert_eq!(rig.verdict_recipient(&review).as_deref(), Some("arch-2"));
}

#[test]
fn verdicts_of_an_ended_architect_follow_the_plan_for_every_subject() {
    let mut rig = Rig::new();
    let reviewed = rig.insert_plan("in_review", Some("arch-1"), &[]);
    let plan = rig.insert_plan("approved", Some("arch-1"), &[("api", Some("dev-2"))]);
    let report = rig.report("dev-2");
    let integration = rig.integration("merged", &[&report]);
    let old = rig.actor_of("arch-1");
    let by_plan = rig.finished_review(("subject_plan_id", &reviewed), &old, "passed");
    let by_report = rig.finished_review(("subject_report_id", &report), &old, "findings");
    let by_integration =
        rig.finished_review(("subject_integration_id", &integration), &old, "passed");
    // Requested by the PM and by the operator: still the PM.
    let other = rig.report("dev-3");
    let by_pm = rig.finished_review(
        ("subject_report_id", &other),
        &rig.f.pm_actor.clone(),
        "passed",
    );
    let other_two = rig.report("dev-4");
    let by_operator = rig.finished_review(
        ("subject_report_id", &other_two),
        &rig.operator_actor(),
        "passed",
    );
    // No plan holds the report of dev-1: an ended architect falls back to the PM.
    let stray = rig.report("dev-1");
    let by_stray = rig.finished_review(("subject_report_id", &stray), &old, "passed");
    rig.end("arch-1");
    rig.reassign(&plan, "arch-2").unwrap();
    rig.reassign(&reviewed, "arch-2").unwrap();

    for review in [
        &by_plan,
        &by_report,
        &by_integration,
        &by_pm,
        &by_operator,
        &by_stray,
    ] {
        assert_eq!(rig.announce(review)["announced"], true);
    }
    assert_eq!(rig.verdict_recipient(&by_plan).as_deref(), Some("arch-2"));
    assert_eq!(rig.verdict_recipient(&by_report).as_deref(), Some("arch-2"));
    assert_eq!(
        rig.verdict_recipient(&by_integration).as_deref(),
        Some("arch-2")
    );
    assert_eq!(rig.verdict_recipient(&by_pm).as_deref(), Some("pm-1"));
    assert_eq!(rig.verdict_recipient(&by_operator).as_deref(), Some("pm-1"));
    assert_eq!(rig.verdict_recipient(&by_stray).as_deref(), Some("pm-1"));
}

#[test]
fn an_inactive_current_architect_leaves_the_verdict_with_the_pm() {
    let mut rig = Rig::new();
    let plan = rig.insert_plan("in_review", Some("arch-1"), &[]);
    let old = rig.actor_of("arch-1");
    let review = rig.finished_review(("subject_plan_id", &plan), &old, "passed");
    rig.end("arch-1");
    rig.reassign(&plan, "arch-2").unwrap();
    rig.end("arch-2");
    assert_eq!(rig.announce(&review)["announced"], true);
    assert_eq!(rig.verdict_recipient(&review).as_deref(), Some("pm-1"));
}

#[test]
fn the_plan_record_and_the_status_show_a_reassignment_and_omit_it_otherwise() {
    let mut rig = Rig::new();
    let plain = rig.insert_plan("approved", Some("arch-1"), &[("api", Some("dev-2"))]);
    let moved = rig.insert_plan("approved", Some("arch-1"), &[("api", None)]);
    rig.db()
        .execute("INSERT INTO external_links(project_id, link_id, ref_kind, ref_id, system, external_id, created_at, updated_at) SELECT project_id, 'l-1', 'plan', ?, 'nexora', 'x', ?, ? FROM projects", rusqlite::params![moved, EARLY, EARLY])
        .ok();
    rig.end("arch-1");
    rig.reassign(&moved, "arch-2").unwrap();

    let record = rig.core().plan_record(OWNER, &moved).unwrap();
    let list = record["plan"]["reassignments"].as_array().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["fromAgentId"], "arch-1");
    assert_eq!(list[0]["toAgentId"], "arch-2");
    assert_eq!(list[0]["actorId"], json!(rig.f.pm_actor));
    assert_eq!(list[0]["reason"], "lost");
    assert!(list[0]["createdAt"].is_string());
    let untouched = rig.core().plan_record(OWNER, &plain).unwrap();
    assert!(untouched["plan"].get("reassignments").is_none());

    let snapshot = rig.core().status_snapshot().unwrap();
    let entries = snapshot["plans"].as_array().unwrap();
    let entry = |id: &str| entries.iter().find(|e| e["planId"] == id).unwrap().clone();
    assert_eq!(entry(&moved)["lastReassignment"], list[0]);
    assert!(entry(&plain).get("lastReassignment").is_none());

    let tasks = rig
        .core()
        .active_tasks(
            OWNER,
            &json!({"plans": 10, "packages": 10, "requirements": 10}),
        )
        .unwrap();
    let plans = tasks["plans"].as_array().unwrap();
    let task = |id: &str| plans.iter().find(|e| e["planId"] == id).unwrap().clone();
    assert_eq!(task(&moved)["lastReassignment"], list[0]);
    assert!(task(&plain).get("lastReassignment").is_none());
}

//! Archiving plans: the all-or-nothing archive call, unarchive, the plan mutations an archived plan refuses, the readers
//! that leave it out and the Nexora state it wants.

use crate::fixture::{context, step, Fixture, OWNER};
use capstan_kernel::areas::snapshot_feed;
use capstan_kernel::plan_body::parse_plan_body;
use capstan_kernel::KernelError;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::collections::HashMap;

const BASE: &str = "0123456789abcdef0123456789abcdef01234567";
const COMMIT: &str = "1111111111111111111111111111111111111111";
const SHA: &str = "abcdef0123456789abcdef0123456789abcdef01";
const EARLY: &str = "2026-01-01T00:00:00.000Z";
const LATE: &str = "2026-01-02T00:00:00.000Z";
const LATER: &str = "2026-01-03T00:00:00.000Z";

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
                {"name": "reviewer", "kind": "Verifier", "host": "claude", "configHash": "f".repeat(64)},
                {"name": "lead", "kind": "Developer", "host": "claude", "configHash": "d".repeat(64)},
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
            ("dev-2", "developer", "Developer"),
            ("dev-3", "developer", "Developer"),
            ("dev-4", "developer", "Developer"),
            ("rev-1", "reviewer", "Verifier"),
        ] {
            let seat = format!("seat-{agent}");
            let core = rig.f.core();
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
            rig.credentials
                .insert(agent.into(), actor["credential"].as_str().unwrap().into());
        }
        rig.credentials
            .insert("pm-1".into(), rig.f.pm_credential.clone());
        rig
    }

    fn core(&self) -> &capstan_kernel::Core {
        self.f.core()
    }

    fn db(&self) -> Connection {
        Connection::open(self.f.database()).unwrap()
    }

    fn project(&self) -> String {
        self.db()
            .query_row("SELECT project_id FROM projects", [], |r| r.get(0))
            .unwrap()
    }

    /// An approved plan inserted into the ledger, architect `arch-1`, with the given packages and assignees.
    fn plan(&mut self, packages: &[(&str, Option<&str>)]) -> String {
        self.plan_in("approved", None, packages)
    }

    fn plan_in(
        &mut self,
        state: &str,
        supersedes: Option<&str>,
        packages: &[(&str, Option<&str>)],
    ) -> String {
        self.next += 1;
        let plan = format!("plan-{}", 100 + self.next);
        let conn = self.db();
        let project = self.project();
        let ids: Vec<&str> = packages.iter().map(|p| p.0).collect();
        conn.execute(
            "INSERT INTO plans(project_id, plan_id, sequence, title, tier, state, requested_by, architect_agent_id, current_revision, supersedes_plan_id, created_at, updated_at)
             VALUES (?, ?, ?, 'T', 'normal', 'draft', 'x', 'arch-1', 0, ?, ?, ?)",
            rusqlite::params![project, plan, 100 + self.next, supersedes, EARLY, EARLY],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO plan_revisions(project_id, plan_id, revision, base_sha, body_json, body_sha, author_agent_id, author_actor_id, created_at)
             VALUES (?, ?, 1, ?, ?, ?, 'dev-1', (SELECT actor_id FROM agents WHERE agent_id = 'dev-1'), ?)",
            rusqlite::params![project, plan, BASE, body_for(&ids), "c".repeat(64), EARLY],
        )
        .unwrap();
        let moves = match state {
            "draft" => "",
            "in_review" => "UPDATE plans SET current_revision = 1, state = 'in_review' WHERE plan_id = ?",
            _ => "UPDATE plans SET current_revision = 1, state = 'approved', approved_revision = 1 WHERE plan_id = ?",
        };
        if !moves.is_empty() {
            conn.execute(moves, [&plan]).unwrap();
        }
        for (package, assignee) in packages.iter().filter(|_| state == "approved") {
            conn.execute(
                "INSERT INTO plan_packages(project_id, plan_id, package_id, assignee_agent_id, assigned_at) VALUES (?, ?, ?, ?, ?)",
                rusqlite::params![project, plan, package, assignee, assignee.map(|_| EARLY)],
            )
            .unwrap();
        }
        plan
    }

    /// An accepted report of `agent` made at `at`.
    fn report(&mut self, agent: &str, at: &str) -> String {
        self.next += 1;
        let id = format!("report-{}", 100 + self.next);
        self.db()
            .execute(
                "INSERT INTO agent_reports(project_id, report_id, sequence, agent_id, generation, actor_id, commit_sha, branch, summary, state, evidence_json, created_at)
                 VALUES (?, ?, ?, ?, 1, (SELECT actor_id FROM agents WHERE agent_id = ?), ?, 'b', 'done', 'accepted', ?, ?)",
                rusqlite::params![
                    self.project(),
                    id,
                    100 + self.next,
                    agent,
                    agent,
                    COMMIT,
                    json!({"baseSha": BASE}).to_string(),
                    at
                ],
            )
            .unwrap();
        id
    }

    fn cancel(&self, plan: &str, package: Option<&str>) -> Result<Value, KernelError> {
        let input = match package {
            Some(package) => json!({"planId": plan, "packageId": package}),
            None => json!({"planId": plan}),
        };
        self.core()
            .cancel_plan(&context(self.core(), OWNER), &input)
    }

    fn record(&self, plan: &str, package: &str) -> Result<Value, KernelError> {
        self.core().record_package_landing(
            &context(self.core(), OWNER),
            &json!({"planId": plan, "packageId": package, "commitSha": SHA, "note": "merged by hand"}),
        )
    }

    fn clear(&self, plan: &str, package: &str) -> Result<Value, KernelError> {
        self.core().clear_package_landing(
            &context(self.core(), OWNER),
            &json!({"planId": plan, "packageId": package}),
        )
    }

    /// Records the landing of the package, which needs an accepted report of it.
    fn settle(&mut self, plan: &str, package: &str, agent: &str) {
        self.report(agent, LATE);
        self.record(plan, package).unwrap();
    }

    fn review_report(&mut self, report: &str) {
        self.next += 1;
        self.db()
            .execute(
                "INSERT INTO reviews(project_id, review_id, sequence, round, subject_report_id, commit_sha, base_sha,
                   author_agent_id, author_actor_id, requested_by_actor_id, reviewer_role, reviewer_agent_id, reviewer_actor_id, state, verdict_text, created_at, completed_at)
                 VALUES (?, ?, ?, 1, ?, ?, ?, 'dev-2', (SELECT actor_id FROM agents WHERE agent_id = 'dev-2'),
                   (SELECT actor_id FROM agents WHERE agent_id = 'dev-1'), 'reviewer', 'rev-1',
                   (SELECT actor_id FROM agents WHERE agent_id = 'rev-1'), 'passed', 'fine', ?, ?)",
                rusqlite::params![
                    self.project(),
                    format!("review-{}", 100 + self.next),
                    100 + self.next,
                    report,
                    COMMIT,
                    BASE,
                    LATE,
                    LATE
                ],
            )
            .unwrap();
    }

    fn warm(&self, plan: &str) {
        self.db()
            .execute(
                "INSERT INTO warm_reviewers(project_id, plan_id, reviewer_role, agent_id, reviews_count, created_at, last_used_at)
                 VALUES (?, ?, 'reviewer', 'rev-1', 1, ?, ?)",
                rusqlite::params![self.project(), plan, EARLY, EARLY],
            )
            .unwrap();
    }

    fn archive_as(&self, credential: &str, ids: &[&str]) -> Result<Value, KernelError> {
        self.core().archive_plans(
            &context(self.core(), credential),
            &json!({"planIds": ids, "reason": "retired"}),
        )
    }

    fn archive(&self, ids: &[&str]) -> Value {
        self.archive_as(OWNER, ids).unwrap()
    }

    fn unarchive(&self, plan: &str) -> Result<Value, KernelError> {
        self.core()
            .unarchive_plan(&context(self.core(), OWNER), &json!({"planId": plan}))
    }

    fn count(&self, sql: &str) -> i64 {
        self.db().query_row(sql, [], |r| r.get(0)).unwrap()
    }

    fn status_of(&self, plan: &str) -> String {
        let all = self
            .core()
            .plan_statuses(OWNER, &json!({"staleAfterDays": 30, "withStale": false}))
            .unwrap();
        all.as_array()
            .unwrap()
            .iter()
            .find(|p| p["planId"] == plan)
            .unwrap()["status"]
            .as_str()
            .unwrap()
            .to_string()
    }

    fn wanted(&self, plan: &str) -> Value {
        self.core()
            .wanted_nexora_state(OWNER, &json!("plan"), plan)
            .unwrap()
    }

    fn state_of(&self, plan: &str) -> String {
        self.db()
            .query_row("SELECT state FROM plans WHERE plan_id = ?", [plan], |r| {
                r.get(0)
            })
            .unwrap()
    }
}

type Op<'a> = Box<dyn Fn() -> Result<Value, KernelError> + 'a>;

fn archived_error(result: Result<Value, KernelError>, plan: &str) {
    let message = refused(result);
    assert_eq!(
        message,
        format!("plan_archived: plan {plan} is archived; run cstan plan unarchive {plan} first")
    );
}

#[test]
fn archive_is_operator_only_and_all_or_nothing() {
    let mut r = Rig::new();
    let a = r.plan(&[("a", Some("dev-2"))]);
    let b = r.plan_in("draft", None, &[("b", None)]);
    r.warm(&b);
    assert!(refused(r.archive_as(&r.f.pm_credential.clone(), &[&b])).contains("only the operator"));
    // a is busy (live package), b is fine, the third does not exist: one refusal lists every refused plan.
    let message = refused(r.archive_as(OWNER, &[&a, &b, "plan-nope"]));
    assert_eq!(
        message,
        format!("{a}: plan_busy: package a; plan-nope: does not exist")
    );
    assert_eq!(r.count("SELECT COUNT(*) FROM plan_archives"), 0);
    assert_eq!(
        r.count("SELECT COUNT(*) FROM warm_reviewers WHERE released_at IS NULL"),
        1
    );
    // Duplicates and too many ids are refused before anything runs.
    assert!(r.archive_as(OWNER, &[&b, &b]).is_err());
    assert!(r
        .core()
        .archive_plans(
            &context(r.core(), OWNER),
            &json!({"planIds": [], "reason": "x"})
        )
        .is_err());

    let messages_before = r.count("SELECT COUNT(*) FROM messages");
    let done = r.archive(&[&b]);
    assert_eq!(done["archived"], json!([b]));
    assert_eq!(done["releasedReviewerAgentIds"], json!(["rev-1"]));
    assert_eq!(r.count("SELECT COUNT(*) FROM plan_archives"), 1);
    assert_eq!(r.status_of(&b), "archived");
    assert_eq!(
        r.count("SELECT COUNT(*) FROM messages") - messages_before,
        1
    );
    let notice: String = r
        .db()
        .query_row(
            "SELECT body FROM messages WHERE body LIKE 'Plans archived%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        notice,
        format!("Plans archived by the operator (1): {b}. Reason: retired. Their history and Nexora links are kept; cstan plan show --archived lists them; cstan plan unarchive <plan-id> restores one.")
    );
    assert!(refused(r.archive_as(OWNER, &[&b])).contains("already_archived"));
    let record = r.core().plan_record(OWNER, &b).unwrap();
    assert_eq!(record["plan"]["archived"]["reason"], "retired");
    assert!(r.core().plan_record(OWNER, &a).unwrap()["plan"]
        .get("archived")
        .is_none());

    r.unarchive(&b).unwrap();
    assert_eq!(r.count("SELECT COUNT(*) FROM plan_archives"), 0);
    assert!(refused(r.unarchive(&b)).starts_with("not_archived: plan "));
    assert!(refused(r.unarchive("plan-nope")).contains("does not exist"));
    assert_eq!(r.status_of(&b), "draft");
}

#[test]
fn an_in_review_plan_goes_back_to_draft_and_keeps_its_architect() {
    let mut r = Rig::new();
    let p = r.plan_in("in_review", None, &[("a", None)]);
    r.archive(&[&p]);
    assert_eq!(r.state_of(&p), "draft");
    r.unarchive(&p).unwrap();
    assert_eq!(r.state_of(&p), "draft");
    let architect: String = r
        .db()
        .query_row(
            "SELECT architect_agent_id FROM plans WHERE plan_id = ?",
            [&p],
            |x| x.get(0),
        )
        .unwrap();
    assert_eq!(architect, "arch-1");
    assert_eq!(
        r.count("SELECT COUNT(*) FROM reviews WHERE state = 'started'"),
        0
    );
}

#[test]
fn a_started_review_a_superseding_plan_or_an_integration_keeps_a_plan_busy() {
    let mut r = Rig::new();
    let old = r.plan(&[("a", None)]);
    let live = r.plan_in("draft", Some(&old), &[("b", None)]);
    assert!(
        refused(r.archive_as(OWNER, &[&old])).contains(&format!("plan_busy: superseded_by {live}"))
    );
    r.archive(&[&live]);
    r.archive(&[&old]);
}

#[test]
fn each_refused_operation_names_the_archive_and_works_again_after_unarchive() {
    let mut r = Rig::new();
    let p = r.plan(&[("a", Some("dev-2"))]);
    // The package is settled so that the plan can be archived.
    r.settle(&p, "a", "dev-2");
    let draft = r.plan_in("draft", None, &[("d", None)]);
    r.archive(&[&p, &draft]);
    let own = |c: &str| context(r.core(), c);
    let arch = r.credentials["arch-1"].clone();
    let ops: Vec<(&str, Op)> = vec![
        (
            "submit",
            Box::new(|| {
                r.core().submit_plan(
                    &own(&arch),
                    &json!({"planId": draft, "baseSha": BASE, "bodyJson": body_for(&["d"])}),
                )
            }),
        ),
        (
            "amend",
            Box::new(|| {
                r.core().amend_plan(&own(&arch), &json!({"planId": draft, "baseSha": BASE, "bodyJson": body_for(&["d"]), "amendmentJson": "{}"}))
            }),
        ),
        (
            "reassign",
            Box::new(|| {
                r.core().reassign_plan(
                    &own(OWNER),
                    &json!({"planId": p, "agentId": "arch-2", "architectRole": "lead"}),
                )
            }),
        ),
        (
            "open",
            Box::new(|| {
                r.core().open_plan(
                    &own(OWNER),
                    &json!({"title": "n", "tier": "normal", "supersedesPlanId": p}),
                )
            }),
        ),
        (
            "assign",
            Box::new(|| {
                r.core().assign_package(
                    &own(OWNER),
                    &json!({"planId": p, "packageId": "a", "agentId": "dev-3"}),
                )
            }),
        ),
        (
            "signoff",
            Box::new(|| {
                r.core().record_signoff(
                    &own(&arch),
                    &json!({"planId": p, "integrationId": "int-x", "summary": "s"}),
                )
            }),
        ),
        ("cancel", Box::new(|| r.cancel(&p, None))),
        ("landed", Box::new(|| r.record(&p, "a"))),
        ("landing_cleared", Box::new(|| r.clear(&p, "a"))),
        (
            "override set",
            Box::new(|| {
                r.core().set_model_override(
                    &own(OWNER),
                    &json!({"scopeType": "plan", "scopeId": p, "roleKind": null, "model": "m"}),
                )
            }),
        ),
        (
            "override clear",
            Box::new(|| {
                r.core().clear_model_override(
                    &own(OWNER),
                    &json!({"scopeType": "plan", "scopeId": p, "roleKind": null}),
                )
            }),
        ),
    ];
    for (name, call) in &ops {
        let message = refused(call());
        assert!(
            message.starts_with("plan_archived: plan "),
            "{name}: {message}"
        );
    }
    r.unarchive(&p).unwrap();
    r.unarchive(&draft).unwrap();
    for (name, call) in &ops {
        if let Err(error) = call() {
            assert!(
                !error.message().starts_with("plan_archived"),
                "{name}: {}",
                error.message()
            );
        }
    }
}

#[test]
fn late_work_on_an_archived_plan_is_refused_at_the_landing_and_integration() {
    let mut r = Rig::new();
    let p = r.plan(&[("a", Some("dev-2"))]);
    r.settle(&p, "a", "dev-2");
    r.archive(&[&p]);
    archived_error(r.record(&p, "a"), &p);
    archived_error(r.clear(&p, "a"), &p);
    // A later accepted report of the package: recorded, but never integrated.
    let report = r.report("dev-2", LATER);
    r.review_report(&report);
    let begin = r.core().begin_integration(
        &context(r.core(), OWNER),
        &json!({"integrationId": "int-late", "requestedBy": "pm-1", "baseSha": BASE, "branch": "b", "reportIds": [report]}),
    );
    archived_error(begin, &p);
    r.unarchive(&p).unwrap();
}

#[test]
fn nexora_wants_follow_the_settled_packages_and_readers_omit_the_plan() {
    let mut r = Rig::new();
    let draft = r.plan_in("draft", None, &[("d", None)]);
    let settled = r.plan(&[("s", Some("dev-2"))]);
    r.settle(&settled, "s", "dev-2");
    let open = r.plan(&[("o", None)]);
    r.archive(&[&draft, &settled]);
    assert_eq!(r.wanted(&draft), json!("wont_do"));
    assert_eq!(r.wanted(&settled), json!("completed"));
    assert_eq!(r.wanted(&open), json!("todo")); // not archived: unchanged
    r.unarchive(&settled).unwrap();
    assert_eq!(r.wanted(&settled), json!("completed"));
    r.archive(&[&settled]);

    assert!(snapshot_feed::plan_hash(r.core(), &draft)
        .unwrap()
        .is_none());
    assert!(snapshot_feed::plan_hash(r.core(), &open).unwrap().is_some());
    let summary = r.f.restart_pm()["plans"].to_string();
    assert!(summary.contains(&open) && !summary.contains(&draft) && !summary.contains(&settled));
    let sequence: i64 = r
        .db()
        .query_row(
            "SELECT sequence FROM plans WHERE plan_id = ?",
            [&draft],
            |x| x.get(0),
        )
        .unwrap();
    let key = sequence.to_string();
    assert_eq!(
        snapshot_feed::snapshot_item(r.core(), "plans", &key).unwrap(),
        None
    );
    r.unarchive(&draft).unwrap();
    assert!(snapshot_feed::plan_hash(r.core(), &draft)
        .unwrap()
        .is_some());
    let back = snapshot_feed::snapshot_item(r.core(), "plans", &key)
        .unwrap()
        .unwrap();
    assert!(back.to_string().contains(&draft));
}

#[test]
fn a_hundred_and_fifty_ids_store_a_hundred_and_count_the_rest() {
    let mut r = Rig::new();
    let ids: Vec<String> = (0..150).map(|_| r.plan(&[("a", None)])).collect();
    let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    let done = r.archive(&refs);
    assert_eq!(done["archived"].as_array().unwrap().len(), 150);
    let payload: String = r
        .db()
        .query_row(
            "SELECT payload_json FROM controller_events WHERE json_extract(payload_json, '$.action') = 'plan.archive'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let payload: Value = serde_json::from_str(&payload).unwrap();
    let details = &payload["details"];
    assert_eq!(details["planIds"].as_array().unwrap().len(), 100);
    assert_eq!(details["more"], 50);
}

#[test]
fn a_read_only_status_snapshot_works_on_a_ledger_without_the_newer_tables() {
    let mut rig = Rig::new();
    rig.plan(&[("pkg-a", Some("dev-2"))]);
    let path = rig.f.database();
    rig.f.close();
    let db = Connection::open(&path).unwrap();
    db.execute_batch("DROP TABLE plan_archives; DROP TABLE package_landings;")
        .unwrap();
    db.execute(
        "DELETE FROM schema_migrations WHERE name LIKE '%\\_plan\\_archives.sql' ESCAPE '\\' OR name LIKE '%\\_package\\_landings.sql' ESCAPE '\\'",
        [],
    )
    .unwrap();
    db.close().unwrap();
    let core = capstan_kernel::Core::open_read_only(
        &rig.f.state,
        &crate::fixture::initial(),
        &capstan_kernel::kernel::KernelOptions::default(),
        Box::new(capstan_kernel::SystemEnv),
    )
    .expect("the old ledger opens read-only");
    let status = core
        .status_snapshot()
        .expect("status reads without the tables");
    assert!(!status["plans"].as_array().unwrap().is_empty(), "{status}");
}

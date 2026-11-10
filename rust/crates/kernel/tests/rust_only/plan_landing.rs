//! One landing check for plan cancel, sign-off and package progress: a package landed when a report of it is in a merged or
//! confirmed integration, covered by a confirmed one, or when the operator recorded the landing.

use crate::fixture::{context, step, Fixture, OWNER};
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
        self.next += 1;
        let plan = format!("plan-{}", 100 + self.next);
        let conn = self.db();
        let project = self.project();
        let ids: Vec<&str> = packages.iter().map(|p| p.0).collect();
        conn.execute(
            "INSERT INTO plans(project_id, plan_id, sequence, title, tier, state, requested_by, architect_agent_id, current_revision, created_at, updated_at)
             VALUES (?, ?, ?, 'T', 'normal', 'draft', 'x', 'arch-1', 0, ?, ?)",
            rusqlite::params![project, plan, 100 + self.next, EARLY, EARLY],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO plan_revisions(project_id, plan_id, revision, base_sha, body_json, body_sha, author_agent_id, author_actor_id, created_at)
             VALUES (?, ?, 1, ?, ?, ?, 'dev-1', (SELECT actor_id FROM agents WHERE agent_id = 'dev-1'), ?)",
            rusqlite::params![project, plan, BASE, body_for(&ids), "c".repeat(64), EARLY],
        )
        .unwrap();
        conn.execute(
            "UPDATE plans SET current_revision = 1, state = 'approved', approved_revision = 1 WHERE plan_id = ?",
            [&plan],
        )
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

    fn cover(&self, integration: &str, report: &str) {
        self.db()
            .execute(
                "INSERT INTO integration_covered_reports(project_id, integration_id, report_id, how) VALUES (?, ?, ?, 'tree')",
                rusqlite::params![self.project(), integration, report],
            )
            .unwrap();
    }

    /// A passed review of the integration, the latest verdict.
    fn pass_review(&mut self, integration: &str) {
        self.next += 1;
        self.db()
            .execute(
                "INSERT INTO reviews(project_id, review_id, sequence, round, subject_integration_id, commit_sha, base_sha,
                   requested_by_actor_id, reviewer_role, reviewer_agent_id, reviewer_actor_id, state, verdict_text, created_at, completed_at)
                 VALUES (?, ?, ?, 1, ?, ?, ?, (SELECT actor_id FROM agents WHERE agent_id = 'dev-1'), 'reviewer', 'rev-1',
                   (SELECT actor_id FROM agents WHERE agent_id = 'rev-1'), 'passed', 'fine', ?, ?)",
                rusqlite::params![
                    self.project(),
                    format!("review-{}", 100 + self.next),
                    100 + self.next,
                    integration,
                    COMMIT,
                    BASE,
                    LATE,
                    LATE
                ],
            )
            .unwrap();
    }

    /// A merged integration holding a report of `dev-4` (no package), with a passed review: the sign-off target.
    fn target(&mut self, extra: &[&str]) -> String {
        let outsider = self.report("dev-4", LATE);
        let mut reports = vec![outsider.as_str()];
        reports.extend_from_slice(extra);
        let id = self.integration("merged", &reports);
        self.pass_review(&id);
        id
    }

    fn signoff(&self, plan: &str, integration: &str) -> Result<Value, KernelError> {
        self.core().record_signoff(
            &context(self.core(), &self.credentials["arch-1"]),
            &json!({"planId": plan, "integrationId": integration, "summary": "done"}),
        )
    }

    fn cancel(&self, plan: &str, package: Option<&str>) -> Result<Value, KernelError> {
        let input = match package {
            Some(package) => json!({"planId": plan, "packageId": package}),
            None => json!({"planId": plan}),
        };
        self.core()
            .cancel_plan(&context(self.core(), OWNER), &input)
    }

    fn record_as(&self, credential: &str, plan: &str, package: &str) -> Result<Value, KernelError> {
        self.core().record_package_landing(
            &context(self.core(), credential),
            &json!({"planId": plan, "packageId": package, "commitSha": SHA, "note": "merged by hand"}),
        )
    }

    fn record(&self, plan: &str, package: &str) -> Result<Value, KernelError> {
        self.record_as(OWNER, plan, package)
    }

    fn clear(&self, plan: &str, package: &str) -> Result<Value, KernelError> {
        self.core().clear_package_landing(
            &context(self.core(), OWNER),
            &json!({"planId": plan, "packageId": package}),
        )
    }

    fn progress(&self, plan: &str, package: &str) -> String {
        let record = self.core().plan_record(OWNER, plan).unwrap();
        record["packages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["packageId"] == package)
            .unwrap()["progress"]
            .as_str()
            .unwrap()
            .to_string()
    }

    fn wanted(&self, plan: &str, package: &str) -> Value {
        self.core()
            .wanted_nexora_state(OWNER, &json!("package"), &format!("{plan}/{package}"))
            .unwrap()
    }

    fn notices(&self, like: &str) -> Vec<String> {
        let conn = self.db();
        let mut statement = conn
            .prepare("SELECT body FROM messages WHERE body LIKE ? ORDER BY sequence")
            .unwrap();
        statement
            .query_map([like], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    fn event_types(&self, action: &str) -> i64 {
        self.db()
            .query_row(
                "SELECT COUNT(*) FROM controller_events WHERE action = ?",
                [action],
                |r| r.get(0),
            )
            .unwrap_or(0)
    }

    /// Hands the package to `agent` now (after every report so far).
    fn reassign_package(&self, plan: &str, package: &str, agent: &str) {
        self.db()
            .execute(
                "UPDATE plan_packages SET assignee_agent_id = ?, assigned_at = ? WHERE plan_id = ? AND package_id = ?",
                rusqlite::params![agent, LATER, plan, package],
            )
            .unwrap();
    }
}

#[test]
fn a_covered_package_is_integrated_and_cannot_be_cancelled() {
    let mut rig = Rig::new();
    let plan = rig.plan(&[("a", Some("dev-2")), ("b", Some("dev-3"))]);
    let ra = rig.report("dev-2", LATE);
    let rb = rig.report("dev-3", LATE);
    let outsider = rig.report("dev-4", LATE);
    let confirmed = rig.integration("confirmed", &[&outsider]);
    rig.cover(&confirmed, &ra);
    assert_eq!(rig.progress(&plan, "a"), "integrated");
    assert_eq!(rig.progress(&plan, "b"), "reported");
    assert_eq!(rig.wanted(&plan, "a"), json!("completed"));
    let message = refused(rig.cancel(&plan, Some("a")));
    assert!(message.starts_with("package_confirmed: package a is already integrated and confirmed"));
    let target = rig.target(&[&rb]);
    rig.signoff(&plan, &target).unwrap();
}

#[test]
fn cancel_and_signoff_agree_for_every_kind_of_landing() {
    // (kind, cancel refused, sign-off accepted)
    let cases = [
        ("none", false, false),
        ("merged-earlier", false, true),
        ("merged-same", false, true),
        ("merged-later", false, false),
        ("confirmed", true, true),
        ("covered", true, true),
        ("recorded", true, true),
    ];
    for (kind, cancel_refused, signoff_ok) in cases {
        let mut rig = Rig::new();
        let plan = rig.plan(&[("a", Some("dev-2"))]);
        let r = rig.report("dev-2", LATE);
        let target = match kind {
            "merged-earlier" => {
                rig.integration("merged", &[&r]);
                rig.target(&[])
            }
            "merged-same" => rig.target(&[&r]),
            "merged-later" => {
                let target = rig.target(&[]);
                rig.integration("merged", &[&r]);
                target
            }
            "confirmed" => {
                let target = rig.target(&[]);
                rig.integration("confirmed", &[&r]);
                target
            }
            "covered" => {
                let target = rig.target(&[]);
                let outsider = rig.report("dev-4", LATE);
                let c = rig.integration("confirmed", &[&outsider]);
                rig.cover(&c, &r);
                target
            }
            "recorded" => {
                rig.record(&plan, "a").unwrap();
                rig.target(&[])
            }
            _ => rig.target(&[]),
        };
        let signed = rig.signoff(&plan, &target);
        assert_eq!(signed.is_ok(), signoff_ok, "{kind}: sign-off {signed:?}");
        if kind == "merged-later" {
            assert_eq!(
                refused(signed),
                format!("package a of plan {plan} is not integrated in integration {target} or an earlier merged integration")
            );
        }
        let cancelled = rig.cancel(&plan, Some("a"));
        assert_eq!(
            cancelled.is_err(),
            cancel_refused,
            "{kind}: cancel {cancelled:?}"
        );
        if cancel_refused {
            let message = refused(cancelled);
            let expected = if kind == "recorded" {
                "package_landed"
            } else {
                "package_confirmed"
            };
            assert!(message.starts_with(expected), "{kind}: {message}");
        }
    }
}

#[test]
fn an_earlier_integrated_report_counts_after_a_later_unintegrated_one() {
    let mut rig = Rig::new();
    let plan = rig.plan(&[("a", Some("dev-2"))]);
    let first = rig.report("dev-2", LATE);
    rig.integration("confirmed", &[&first]);
    rig.report("dev-2", LATER);
    assert_eq!(rig.progress(&plan, "a"), "integrated");
}

#[test]
fn a_whole_plan_cancel_skips_a_recorded_package() {
    let mut rig = Rig::new();
    let plan = rig.plan(&[("a", Some("dev-2")), ("b", Some("dev-3"))]);
    rig.report("dev-2", LATE);
    rig.record(&plan, "a").unwrap();
    rig.cancel(&plan, None).unwrap();
    let cancelled: Vec<(String, Option<String>)> = {
        let conn = rig.db();
        let mut s = conn
            .prepare("SELECT package_id, cancelled_at FROM plan_packages WHERE plan_id = ? ORDER BY package_id")
            .unwrap();
        s.query_map([&plan], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert!(cancelled[0].1.is_none());
    assert!(cancelled[1].1.is_some());
    let rows: i64 = rig
        .db()
        .query_row("SELECT COUNT(*) FROM package_landings", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 1);
}

#[test]
fn record_and_clear_refusals() {
    let mut rig = Rig::new();
    let plan = rig.plan(&[("a", Some("dev-2")), ("b", Some("dev-3")), ("c", None)]);
    let pm = rig.credentials["pm-1"].clone();
    assert_eq!(
        refused(rig.record_as(&pm, &plan, "a")),
        "only the operator records a landed package"
    );
    assert_eq!(refused(rig.record("nope", "a")), "plan nope does not exist");
    assert_eq!(
        refused(rig.record(&plan, "zz")),
        format!("plan {plan} has no package zz")
    );
    assert_eq!(
        refused(rig.record(&plan, "a")),
        "no_report: package a has no accepted report; cancel it instead"
    );
    let bad = rig.core().record_package_landing(
        &context(rig.core(), OWNER),
        &json!({"planId": plan, "packageId": "a", "commitSha": "ABC", "note": "x"}),
    );
    assert!(matches!(bad, Err(KernelError::Type(_))) || bad.is_err());
    rig.report("dev-2", LATE);
    rig.record(&plan, "a").unwrap();
    assert_eq!(
        refused(rig.record(&plan, "a")),
        format!(
            "already_landed: package a already landed (recorded {})",
            &SHA[..12]
        )
    );
    assert_eq!(
        refused(rig.clear(&plan, "b")),
        "no_landing: package b has no recorded landing"
    );
    assert_eq!(refused(rig.clear("nope", "a")), "plan nope does not exist");
    assert_eq!(
        refused(rig.clear(&plan, "zz")),
        format!("plan {plan} has no package zz")
    );
    rig.cancel(&plan, Some("c")).unwrap();
    rig.report("dev-3", LATE);
    assert_eq!(
        refused(rig.record(&plan, "c")),
        "already_cancelled: package c was cancelled"
    );
    let ro = rig.record_as(&rig.credentials["dev-2"].clone(), &plan, "b");
    assert!(ro.is_err());
    // an integration of a confirmed report makes the package settled
    let rb = rig.report("dev-3", LATE);
    rig.integration("confirmed", &[&rb]);
    assert!(refused(rig.record(&plan, "b"))
        .starts_with("already_landed: package b already landed (confirmed int-"));
    // sign-off then freezes the landings
    let target = rig.target(&[]);
    rig.signoff(&plan, &target).unwrap();
    assert_eq!(
        refused(rig.clear(&plan, "a")),
        format!("plan_signed_off: plan {plan} was signed off; its landings are final")
    );
    assert_eq!(
        refused(rig.record(&plan, "a")),
        format!("plan_signed_off: plan {plan} was signed off; its landings are final")
    );
    // the plan itself
    let other = rig.plan(&[("a", Some("dev-4"))]);
    rig.cancel(&other, None).unwrap();
    assert_eq!(
        refused(rig.record(&other, "a")),
        format!("plan_cancelled: plan {other} was cancelled")
    );
}

#[test]
fn record_completes_the_item_and_clear_restores_it() {
    let mut rig = Rig::new();
    let plan = rig.plan(&[("a", Some("dev-2")), ("b", Some("dev-3"))]);
    let latest = rig.report("dev-2", LATE);
    assert_eq!(rig.progress(&plan, "a"), "reported");
    assert_eq!(rig.wanted(&plan, "a"), json!("in_progress"));
    // a link already synced
    rig.db()
        .execute(
            "INSERT INTO external_links(project_id, ref_kind, ref_id, system, external_id, synced_state, linked_by, linked_at, synced_at)
             VALUES (?, 'package', ?, 'nexora', 'PM-1', 'in_progress', (SELECT actor_id FROM actors WHERE role = 'operator' LIMIT 1), ?, ?)",
            rusqlite::params![rig.project(), format!("{plan}/a"), EARLY, EARLY],
        )
        .unwrap();
    let answer = rig.record(&plan, "a").unwrap();
    assert_eq!(answer["commitSha"], SHA);
    assert_eq!(answer["latestReportNotLanded"], json!(latest));
    assert_eq!(rig.progress(&plan, "a"), "integrated");
    assert_eq!(rig.wanted(&plan, "a"), json!("completed"));
    let notice = rig.notices(&format!(
        "Package {plan}/a was recorded as landed at {}%",
        &SHA[..12]
    ));
    assert_eq!(notice.len(), 1);
    assert!(notice[0].ends_with(&format!(
        "by the operator: merged by hand. It counts as integrated for sign-off. Its latest report {latest} is not integrated."
    )));
    // the unlanded report is in no integration, so sign-off does not list it
    let rb = rig.report("dev-3", LATE);
    let target = rig.target(&[&rb]);
    let signed = rig.signoff(&plan, &target).unwrap();
    assert_eq!(signed["extraReports"].as_array().unwrap().len(), 1);
    assert!(!signed["extraReports"].to_string().contains(&latest));
    // the row cannot be updated
    let update = rig
        .db()
        .execute("UPDATE package_landings SET note = 'x'", []);
    assert!(update.is_err());
    // clear on a fresh plan: the link goes back to drift
    let mut rig = Rig::new();
    let plan = rig.plan(&[("a", Some("dev-2"))]);
    let report = rig.report("dev-2", LATE);
    let review_target = rig.integration("merged", &[&report]);
    rig.integration("merged", &[]);
    let _ = review_target;
    rig.db()
        .execute(
            "INSERT INTO external_links(project_id, ref_kind, ref_id, system, external_id, synced_state, linked_by, linked_at, synced_at)
             VALUES (?, 'package', ?, 'nexora', 'PM-2', 'completed', (SELECT actor_id FROM actors WHERE role = 'operator' LIMIT 1), ?, ?)",
            rusqlite::params![rig.project(), format!("{plan}/a"), EARLY, EARLY],
        )
        .unwrap();
    let before = rig.wanted(&plan, "a");
    assert_eq!(before, json!("in_review"));
    // record on a package whose report is merged (unsettled) is allowed; clear returns the old state
    let answer = rig.record(&plan, "a").unwrap();
    assert!(answer.get("latestReportNotLanded").is_none());
    assert_eq!(rig.wanted(&plan, "a"), json!("completed"));
    rig.clear(&plan, "a").unwrap();
    assert_eq!(rig.wanted(&plan, "a"), before);
    assert_eq!(rig.progress(&plan, "a"), "integrated");
    let links = rig.core().plan_links(OWNER, Some(&plan)).unwrap();
    let link = links
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["refKind"] == "package")
        .unwrap();
    assert_eq!(link["wanted"], "in_review");
    assert_eq!(link["drift"], true);
    assert_eq!(
        rig.notices("The recorded landing of %was cleared by the operator.")
            .len(),
        1
    );
}

#[test]
fn a_recorded_landing_follows_the_package_but_report_landings_follow_the_assignee() {
    let mut rig = Rig::new();
    let plan = rig.plan(&[("a", Some("dev-2"))]);
    rig.report("dev-2", LATE);
    rig.record(&plan, "a").unwrap();
    rig.reassign_package(&plan, "a", "dev-3");
    assert_eq!(rig.progress(&plan, "a"), "integrated");
    assert_eq!(
        refused(rig.record(&plan, "a")),
        "no_report: package a has no accepted report; cancel it instead"
    );
    assert!(refused(rig.cancel(&plan, Some("a")))
        .starts_with("package_landed: package a was recorded as landed at "));
    let target = rig.target(&[]);
    rig.signoff(&plan, &target).unwrap();
    // reports of a previous assignee stop counting, whatever their kind
    for kind in ["merged", "confirmed", "covered"] {
        let mut rig = Rig::new();
        let plan = rig.plan(&[("a", Some("dev-2"))]);
        let r = rig.report("dev-2", LATE);
        match kind {
            "merged" => {
                rig.integration("merged", &[&r]);
            }
            "confirmed" => {
                rig.integration("confirmed", &[&r]);
            }
            _ => {
                let outsider = rig.report("dev-4", LATE);
                let c = rig.integration("confirmed", &[&outsider]);
                rig.cover(&c, &r);
            }
        }
        assert_eq!(rig.progress(&plan, "a"), "integrated", "{kind}");
        rig.reassign_package(&plan, "a", "dev-3");
        assert_eq!(rig.progress(&plan, "a"), "assigned", "{kind}");
        let target = rig.target(&[]);
        assert!(rig.signoff(&plan, &target).is_err(), "{kind}");
        assert!(rig.cancel(&plan, Some("a")).is_ok(), "{kind}");
    }
}

#[test]
fn a_landing_survives_reopening_the_ledger() {
    let mut rig = Rig::new();
    let plan = rig.plan(&[("a", Some("dev-2"))]);
    rig.report("dev-2", LATE);
    rig.record(&plan, "a").unwrap();
    rig.f.close();
    rig.f.reopen();
    assert_eq!(rig.progress(&plan, "a"), "integrated");
    let _ = rig.event_types("plan.landed");
}

#[test]
fn status_open_work_and_handover_treat_a_covered_package_as_integrated() {
    let mut rig = Rig::new();
    let plan = rig.plan(&[("a", Some("dev-2")), ("b", Some("dev-3"))]);
    let ra = rig.report("dev-2", LATE);
    rig.report("dev-3", LATE);
    let outsider = rig.report("dev-4", LATE);
    let confirmed = rig.integration("confirmed", &[&outsider]);
    rig.cover(&confirmed, &ra);
    // status.rs plan rows count the covered package as integrated
    let status = rig.core().status_snapshot().unwrap();
    let entry = status["plans"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["planId"] == plan.as_str())
        .unwrap();
    assert_eq!(entry["packages"]["integrated"], 1);
    assert_eq!(entry["packages"]["reported"], 1);
    // idle_release: the covered package holds no work for its assignee
    let work = rig.core().open_work("dev-2").unwrap();
    assert!(
        !work.iter().any(|w| w.contains("not yet integrated")),
        "{work:?}"
    );
    let other = rig.core().open_work("dev-3").unwrap();
    assert!(
        other.iter().any(|w| w.contains("not yet integrated")),
        "{other:?}"
    );
    // handover_facts: the new architect is told the package is integrated
    rig.core()
        .end_agent(&context(rig.core(), OWNER), "arch-1", None)
        .unwrap();
    let pm = rig.credentials["pm-1"].clone();
    rig.core()
        .reassign_plan(
            &context(rig.core(), &pm),
            &json!({"planId": plan, "agentId": "arch-2", "architectRole": "lead", "reason": "lost"}),
        )
        .unwrap();
    let bodies: Vec<String> = {
        let conn = rig.db();
        let mut s = conn
            .prepare("SELECT body FROM messages WHERE recipient_agent_id = 'arch-2'")
            .unwrap();
        s.query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert!(
        bodies
            .iter()
            .any(|b| b.contains("- a: integrated, assignee dev-2")),
        "{bodies:?}"
    );
}

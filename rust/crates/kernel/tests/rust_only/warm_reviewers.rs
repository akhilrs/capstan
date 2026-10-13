//! Warm reviewers of migration 0043: the per-plan reviewer row, its independence checks, its release at sign-off and
//! cancellation, the restart-recovery exclusion and the review brief's scope and history.

use crate::fixture::{context, step, Fixture, OWNER};
use capstan_kernel::helpers::{
    review_task, PackageScope, PriorVerdict, ReviewBrief, ScopeBrief, ScopeFile,
};
use capstan_kernel::records::ReviewRow;
use serde_json::{json, Value};

const BASE: &str = "0123456789abcdef0123456789abcdef01234567";
const COMMIT: &str = "1111111111111111111111111111111111111111";
const EARLY: &str = "2026-01-01T00:00:00.000Z";
const LATE: &str = "2026-01-02T00:00:00.000Z";

struct Rig {
    f: Fixture,
    reviewers: Vec<(String, String)>,
    architect: String,
}

fn create_agent(f: &Fixture, agent: &str, role: &str, kind: &str) -> (String, String) {
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
    (
        actor["credential"].as_str().unwrap().to_string(),
        actor["actorId"].as_str().unwrap().to_string(),
    )
}

fn rig() -> Rig {
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
    create_agent(&f, "dev-3", "developer", "Developer");
    let architect = create_agent(&f, "arch-1", "architect", "Developer").0;
    let reviewers = ["rev-1", "rev-2", "rev-3"]
        .iter()
        .map(|id| {
            let (credential, _) = create_agent(&f, id, "reviewer", "Verifier");
            (id.to_string(), credential)
        })
        .collect();
    Rig {
        f,
        reviewers,
        architect,
    }
}

fn db(f: &Fixture) -> rusqlite::Connection {
    rusqlite::Connection::open(f.database()).unwrap()
}

fn project(f: &Fixture) -> String {
    db(f)
        .query_row("SELECT project_id FROM projects", [], |r| r.get(0))
        .unwrap()
}

/// An approved plan `plan` whose packages are held by the given `(package, agent)` pairs.
fn plan(f: &Fixture, plan: &str, packages: &[(&str, &str)]) {
    let db = db(f);
    let project = project(f);
    let sequence: i64 = db
        .query_row(
            "SELECT COALESCE(MAX(sequence), 0) + 1 FROM plans",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let items: Vec<String> = packages
        .iter()
        .map(|(package, _)| {
            format!(r#"{{"id":"{package}","title":"P","owns":["src/{package}"],"acceptance":["{package} works"],"estimate_hours":1,"interfaces":["i"]}}"#)
        })
        .collect();
    let order: Vec<String> = packages.iter().map(|(p, _)| format!("\"{p}\"")).collect();
    let body = format!(
        r#"{{"summary":"s","packages":[{}],"risks":[],"integration_order":[{}]}}"#,
        items.join(","),
        order.join(",")
    );
    let stored =
        serde_json::to_string(&capstan_kernel::plan_body::parse_plan_body(&body, 8).unwrap())
            .unwrap();
    db.execute(
        "INSERT INTO plans(project_id, plan_id, sequence, title, tier, state, requested_by, architect_agent_id, current_revision, created_at, updated_at)
         VALUES (?, ?, ?, 'T', 'normal', 'draft', 'x', 'arch-1', 0, ?, ?)",
        rusqlite::params![project, plan, sequence, EARLY, EARLY],
    )
    .unwrap();
    db.execute(
        "INSERT INTO plan_revisions(project_id, plan_id, revision, base_sha, body_json, body_sha, author_agent_id, author_actor_id, created_at)
         VALUES (?, ?, 1, ?, ?, ?, 'arch-1', (SELECT actor_id FROM agents WHERE agent_id = 'arch-1'), ?)",
        rusqlite::params![project, plan, BASE, stored, "c".repeat(64), EARLY],
    )
    .unwrap();
    db.execute(
        "UPDATE plans SET current_revision = 1, state = 'approved', approved_revision = 1 WHERE plan_id = ?",
        [plan],
    )
    .unwrap();
    for (package, agent) in packages {
        db.execute(
            "INSERT INTO plan_packages(project_id, plan_id, package_id, assignee_agent_id, assigned_at) VALUES (?, ?, ?, ?, ?)",
            rusqlite::params![project, plan, package, agent, EARLY],
        )
        .unwrap();
    }
}

/// An accepted report of `agent` (by `actor`, or the agent's own actor) with the base commit in its evidence.
fn report(f: &Fixture, agent: &str, actor: Option<&str>) -> String {
    let db = db(f);
    let (project, own_actor): (String, String) = db
        .query_row(
            "SELECT project_id, actor_id FROM agents WHERE agent_id = ?",
            [agent],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let sequence: i64 = db
        .query_row(
            "SELECT COALESCE(MAX(sequence), 0) + 1 FROM agent_reports",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let id = format!("report-{sequence}");
    db.execute(
        "INSERT INTO agent_reports(project_id, report_id, sequence, agent_id, generation, actor_id, commit_sha, branch, summary, state, evidence_json, created_at)
         VALUES (?, ?, ?, ?, 1, ?, ?, 'b', 'did the work', 'accepted', ?, ?)",
        rusqlite::params![
            project,
            id,
            sequence,
            agent,
            actor.unwrap_or(&own_actor),
            COMMIT,
            json!({"baseSha": BASE}).to_string(),
            LATE
        ],
    )
    .unwrap();
    id
}

fn integration(f: &Fixture, id: &str, reports: &[&str]) {
    let db = db(f);
    let project = project(f);
    db.execute(
        "INSERT INTO integrations(project_id, integration_id, sequence, base_sha, branch, requested_by, state, created_at)
         VALUES (?, ?, (SELECT COALESCE(MAX(sequence), 0) + 1 FROM integrations), ?, ?, 'x', 'running', ?)",
        rusqlite::params![project, id, BASE, format!("br-{id}"), EARLY],
    )
    .unwrap();
    for (n, report) in reports.iter().enumerate() {
        db.execute(
            "INSERT INTO integration_reports(project_id, integration_id, position, report_id) VALUES (?, ?, ?, ?)",
            rusqlite::params![project, id, n as i64 + 1, report],
        )
        .unwrap();
    }
    db.execute(
        "UPDATE integrations SET state = 'merged', head_sha = ?, completed_at = ? WHERE integration_id = ?",
        rusqlite::params![COMMIT.replace('1', "2"), LATE, id],
    )
    .unwrap();
}

impl Rig {
    fn core(&self) -> &capstan_kernel::Core {
        self.f.core()
    }

    fn credential(&self, reviewer: &str) -> &str {
        &self
            .reviewers
            .iter()
            .find(|(id, _)| id == reviewer)
            .unwrap()
            .1
    }

    fn begin(
        &self,
        subject: &str,
        reviewer: &str,
        extra: Value,
    ) -> Result<Value, capstan_kernel::KernelError> {
        let mut input =
            json!({"subjectId": subject, "reviewerRole": "reviewer", "reviewerAgentId": reviewer});
        for (key, value) in extra.as_object().unwrap() {
            input[key] = value.clone();
        }
        self.core()
            .begin_review(&context(self.core(), &self.f.pm_credential), &input)
    }

    fn complete(&self, reviewer: &str, verdict: &str) -> Value {
        self.core()
            .complete_review(
                &context(self.core(), self.credential(reviewer)),
                &json!({"verdict": verdict, "text": "looked"}),
            )
            .unwrap()
    }

    fn warm_for(&self, subject: &str, max: i64) -> Value {
        self.core()
            .warm_reviewer_for(subject, "reviewer", max)
            .unwrap()
    }

    fn warm_rows(&self) -> Vec<(String, String, i64, Option<String>)> {
        let db = db(&self.f);
        let mut statement = db
            .prepare("SELECT plan_id, agent_id, reviews_count, release_reason FROM warm_reviewers ORDER BY plan_id, agent_id")
            .unwrap();
        statement
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }
}

#[test]
fn two_reports_of_one_plan_reviewed_by_one_reviewer_make_one_warm_row() {
    let r = rig();
    plan(&r.f, "plan-1", &[("a", "dev-1"), ("b", "dev-2")]);
    let (one, two) = (report(&r.f, "dev-1", None), report(&r.f, "dev-2", None));
    assert_eq!(
        r.core().review_plan_of(&one).unwrap(),
        json!({"planId": "plan-1", "packageId": "a"})
    );
    assert_eq!(r.warm_for(&one, 5)["reason"], "none");
    let begun = r
        .begin(&one, "rev-1", json!({"warmPlanId": "plan-1"}))
        .unwrap();
    assert_eq!(begun["warm"], true);
    assert_eq!(r.warm_for(&two, 5)["reason"], "busy");
    let done = r.complete("rev-1", "pass");
    assert_eq!(done["warmPlanId"], "plan-1");
    assert_eq!(done["warmReviews"], 1);
    assert_eq!(
        r.warm_for(&two, 5),
        json!({"agentId": "rev-1", "reason": "ok"})
    );
    assert_eq!(r.warm_for(&two, 1)["reason"], "exhausted");
    let begun = r
        .begin(&two, "rev-1", json!({"warmPlanId": "plan-1"}))
        .unwrap();
    assert_eq!(begun["warm"], true);
    let done = r.complete("rev-1", "findings");
    assert_eq!(done["warmReviews"], 2);
    assert_eq!(
        r.warm_rows(),
        vec![("plan-1".into(), "rev-1".into(), 2, None)]
    );
    let listed = r.core().warm_reviewers(OWNER).unwrap();
    assert_eq!(listed[0]["planId"], "plan-1");
    assert_eq!(listed[0]["reviewsCount"], 2);
    assert_eq!(listed[0]["busy"], false);
}

#[test]
fn a_review_without_a_warm_plan_or_with_a_different_warm_reviewer_stays_one_shot() {
    let r = rig();
    plan(&r.f, "plan-1", &[("a", "dev-1"), ("b", "dev-2")]);
    let (one, two) = (report(&r.f, "dev-1", None), report(&r.f, "dev-2", None));
    let plain = r.begin(&one, "rev-1", json!({})).unwrap();
    assert!(plain.get("warm").is_none());
    assert!(r.complete("rev-1", "pass").get("warmPlanId").is_none());
    assert!(r.warm_rows().is_empty());
    r.begin(&one, "rev-2", json!({"warmPlanId": "plan-1"}))
        .unwrap();
    r.complete("rev-2", "pass");
    let other = r
        .begin(&two, "rev-1", json!({"warmPlanId": "plan-1"}))
        .unwrap();
    assert!(other.get("warm").is_none());
    assert!(r.complete("rev-1", "pass").get("warmPlanId").is_none());
    assert_eq!(
        r.warm_rows(),
        vec![("plan-1".into(), "rev-2".into(), 1, None)]
    );
    // A warm plan that is not the subject's plan is refused.
    plan(&r.f, "plan-2", &[("c", "dev-3")]);
    assert!(r
        .begin(&two, "rev-3", json!({"warmPlanId": "plan-2"}))
        .is_err());
}

#[test]
fn an_author_is_never_the_warm_reviewer() {
    let r = rig();
    plan(&r.f, "plan-1", &[("a", "dev-1"), ("b", "dev-2")]);
    let warm = report(&r.f, "dev-1", None);
    r.begin(&warm, "rev-1", json!({"warmPlanId": "plan-1"}))
        .unwrap();
    r.complete("rev-1", "pass");
    let refused = |subject: &str, reviewer: &str| {
        r.begin(subject, reviewer, json!({}))
            .unwrap_err()
            .to_string()
    };
    // The warm reviewer itself wrote the report.
    let own = report(&r.f, "rev-1", None);
    assert_eq!(
        r.warm_for(&own, 5)["reason"],
        "none",
        "a Verifier's report belongs to no package"
    );
    assert!(refused(&own, "rev-1").contains("cannot be an author"));
    // A report of dev-2 whose actor is an actor the reviewer's seat held in an earlier generation.
    let old_actor: String = db(&r.f)
        .query_row(
            "SELECT actor_id FROM agents WHERE agent_id = 'rev-1'",
            [],
            |x| x.get(0),
        )
        .unwrap();
    r.core()
        .replace_agent_generation(&context(r.core(), OWNER), "rev-1")
        .unwrap();
    let earlier = report(&r.f, "dev-2", Some(&old_actor));
    assert_eq!(r.warm_for(&earlier, 5)["reason"], "author");
    assert!(refused(&earlier, "rev-1").contains("cannot be an author"));
    // The author's predecessor and successor in the recorded replacements.
    r.core()
        .record_agent_replaced(
            &context(r.core(), OWNER),
            &json!({"predecessorId": "rev-2", "successorId": "rev-3"}),
        )
        .unwrap();
    let by_rev2 = report(&r.f, "rev-2", None);
    assert!(refused(&by_rev2, "rev-3").contains("cannot be an author"));
    let by_rev3 = report(&r.f, "rev-3", None);
    assert!(refused(&by_rev3, "rev-2").contains("cannot be an author"));
    // An unrelated Verifier is not refused.
    r.begin(&by_rev2, "rev-1", json!({})).unwrap();
}

#[test]
fn signoff_and_cancellation_release_the_warm_row() {
    let r = rig();
    plan(&r.f, "plan-1", &[("a", "dev-1"), ("b", "dev-2")]);
    let (one, two) = (report(&r.f, "dev-1", None), report(&r.f, "dev-2", None));
    integration(&r.f, "int-1", &[&one, &two]);
    assert_eq!(
        r.core().review_plan_of("int-1").unwrap(),
        json!({"planId": "plan-1", "packageId": null})
    );
    let mixed_report = report(&r.f, "dev-3", None);
    plan(&r.f, "plan-2", &[("c", "dev-3")]);
    integration(&r.f, "int-2", &[&one, &mixed_report]);
    assert_eq!(r.core().review_plan_of("int-2").unwrap(), Value::Null);
    assert_eq!(r.core().review_plan_of("nothing").unwrap(), Value::Null);

    r.begin("int-1", "rev-1", json!({"warmPlanId": "plan-1"}))
        .unwrap();
    // While rev-1 holds a started review, restart recovery has nothing of its to release; after it, a held warm reviewer
    // is skipped.
    r.complete("rev-1", "pass");
    let to_release = r.core().reviews_to_release(OWNER).unwrap();
    assert!(to_release.as_array().unwrap().is_empty(), "{to_release}");
    let signed = r
        .core()
        .record_signoff(
            &context(r.core(), &r.architect),
            &json!({"planId": "plan-1", "integrationId": "int-1", "summary": "done"}),
        )
        .unwrap();
    assert_eq!(signed["releasedReviewerAgentIds"], json!(["rev-1"]));
    assert_eq!(
        r.warm_rows(),
        vec![("plan-1".into(), "rev-1".into(), 1, Some("signoff".into()))]
    );
    let to_release = r.core().reviews_to_release(OWNER).unwrap();
    assert_eq!(to_release.as_array().unwrap().len(), 1);
    assert_eq!(to_release[0]["reviewerAgentId"], "rev-1");

    // Cancelling a plan releases its reviewer; cancelling a package does not.
    let three = report(&r.f, "dev-3", None);
    r.begin(&three, "rev-2", json!({"warmPlanId": "plan-2"}))
        .unwrap();
    r.complete("rev-2", "pass");
    let package = r
        .core()
        .cancel_plan(
            &context(r.core(), OWNER),
            &json!({"planId": "plan-2", "packageId": "c"}),
        )
        .unwrap();
    assert!(package.get("releasedReviewerAgentIds").is_none());
    let cancelled = r
        .core()
        .cancel_plan(&context(r.core(), OWNER), &json!({"planId": "plan-2"}))
        .unwrap();
    assert_eq!(cancelled["releasedReviewerAgentIds"], json!(["rev-2"]));
    assert_eq!(
        r.warm_rows()[1],
        ("plan-2".into(), "rev-2".into(), 1, Some("cancel".into()))
    );
}

#[test]
fn a_held_warm_reviewer_is_skipped_by_restart_recovery() {
    let r = rig();
    plan(&r.f, "plan-1", &[("a", "dev-1")]);
    let one = report(&r.f, "dev-1", None);
    r.begin(&one, "rev-1", json!({"warmPlanId": "plan-1"}))
        .unwrap();
    r.complete("rev-1", "pass");
    assert!(r
        .core()
        .reviews_to_release(OWNER)
        .unwrap()
        .as_array()
        .unwrap()
        .is_empty());
    let released = r
        .core()
        .release_warm_reviewer(
            &context(r.core(), OWNER),
            &json!({"agentId": "rev-1", "reason": "idle"}),
        )
        .unwrap();
    assert_eq!(released, json!({"released": true, "planId": "plan-1"}));
    assert_eq!(
        r.core()
            .reviews_to_release(OWNER)
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn release_only_if_idle_refuses_a_reviewer_with_a_started_review() {
    let r = rig();
    plan(&r.f, "plan-1", &[("a", "dev-1"), ("b", "dev-2")]);
    let (one, two) = (report(&r.f, "dev-1", None), report(&r.f, "dev-2", None));
    r.begin(&one, "rev-1", json!({"warmPlanId": "plan-1"}))
        .unwrap();
    r.complete("rev-1", "pass");
    r.begin(&two, "rev-1", json!({"warmPlanId": "plan-1"}))
        .unwrap();
    assert_eq!(r.core().warm_reviewers(OWNER).unwrap()[0]["busy"], true);
    let release = |idle: bool| {
        r.core()
            .release_warm_reviewer(
                &context(r.core(), OWNER),
                &json!({"agentId": "rev-1", "reason": "evicted", "onlyIfIdle": idle}),
            )
            .unwrap()
    };
    assert_eq!(release(true), json!({"released": false, "planId": null}));
    assert!(r.warm_rows()[0].3.is_none());
    r.complete("rev-1", "pass");
    assert_eq!(release(true), json!({"released": true, "planId": "plan-1"}));
    // Idempotent.
    assert_eq!(release(false), json!({"released": false, "planId": null}));
    assert_eq!(r.warm_rows()[0].3.as_deref(), Some("evicted"));
}

#[test]
fn the_brief_of_a_warm_review_names_the_scope_and_the_earlier_verdicts() {
    let r = rig();
    plan(&r.f, "plan-1", &[("a", "dev-1"), ("b", "dev-2")]);
    let (one, two) = (report(&r.f, "dev-1", None), report(&r.f, "dev-2", None));
    r.begin(&one, "rev-1", json!({"warmPlanId": "plan-1"}))
        .unwrap();
    r.complete("rev-1", "findings");
    r.begin(&two, "rev-1", json!({"warmPlanId": "plan-1", "diffScope": {
        "files": [{"path": "src/a.rs", "added": 3, "deleted": 1}], "filesTotal": 1, "insertions": 3, "deletions": 1}}))
        .unwrap();
    let task: String = db(&r.f)
        .query_row(
            "SELECT body FROM messages WHERE recipient_agent_id = 'rev-1' ORDER BY sequence DESC LIMIT 1",
            [],
            |x| x.get(0),
        )
        .unwrap();
    assert!(
        task.contains(&format!("Scope: {BASE}..{COMMIT}: 1 files changed, +3 -1")),
        "{task}"
    );
    assert!(task.contains("Package plan-1/b owns: \"src/b\""), "{task}");
    assert!(
        task.contains("Touched outside the owned paths: \"src/a.rs\""),
        "{task}"
    );
    assert!(task.contains("\"b works\""), "{task}");
    assert!(
        task.contains("Your earlier verdicts in plan plan-1:"),
        "{task}"
    );
    assert!(
        task.contains(&format!("round 1 of report {one} at {COMMIT}: findings")),
        "{task}"
    );
    assert!(task.contains("Judge this commit on its own; an earlier verdict does not carry over."));
    // A bad scope is refused before anything is written.
    r.complete("rev-1", "pass");
    for scope in [
        json!({"files": [{"path": "a\nb", "added": 1, "deleted": 1}], "filesTotal": 1, "insertions": 1, "deletions": 1}),
        json!({"files": [{"path": "x".repeat(513), "added": 1, "deleted": 1}], "filesTotal": 1, "insertions": 1, "deletions": 1}),
        json!({"files": [], "filesTotal": -1, "insertions": 1, "deletions": 1}),
    ] {
        assert!(r.begin(&two, "rev-1", json!({"diffScope": scope})).is_err());
    }
}

fn row() -> ReviewRow {
    ReviewRow {
        review_id: "rv".into(),
        sequence: 1,
        round: 1,
        subject_report_id: Some("rp".into()),
        subject_integration_id: None,
        subject_plan_id: None,
        subject_plan_revision: None,
        commit_sha: COMMIT.into(),
        base_sha: BASE.into(),
        author_agent_id: None,
        author_actor_id: None,
        requested_by_actor_id: "a".into(),
        reviewer_role: "r".into(),
        reviewer_agent_id: "r1".into(),
        reviewer_actor_id: "ra".into(),
        state: "started".into(),
        verdict_text: None,
        failure_reason: None,
        notified_message_id: None,
        created_at: "t".into(),
        completed_at: None,
        findings_class: None,
    }
}

fn file(path: &str) -> ScopeFile {
    ScopeFile {
        path: path.into(),
        added: Some(2),
        deleted: Some(1),
    }
}

#[test]
fn the_task_renders_scope_files_package_acceptance_and_outside_files_with_the_gate() {
    let files: Vec<ScopeFile> = (0..52).map(|n| file(&format!("src/f{n}.rs"))).collect();
    let mut files = files;
    files.push(file("docs/readme.md"));
    let brief = ReviewBrief {
        verify: Some(capstan_kernel::helpers::VerifySummary {
            run_id: "run-1".into(),
            state: "passed".into(),
            commit_sha: COMMIT.into(),
            duration_ms: Some(1000),
            failed_step: None,
            exit_code: Some(0),
            tail_excerpt: String::new(),
        }),
        scope: Some(ScopeBrief {
            files,
            files_total: 60,
            insertions: 120,
            deletions: 40,
            package: Some(PackageScope {
                plan_id: "plan-1".into(),
                package_id: "a".into(),
                owns: vec!["src".into()],
                acceptance: vec!["it works".into(), "it is fast".into()],
            }),
        }),
        history: vec![],
    };
    let task = review_task(&row(), &[], None, &brief);
    assert!(task.contains(&format!(
        "Scope: {BASE}..{COMMIT}: 60 files changed, +120 -40"
    )));
    assert_eq!(task.matches("Files touched: \"").count(), 50);
    assert!(task.contains("Files touched: \"src/f0.rs\" (+2 -1)"));
    assert!(task.contains("Files touched: and 10 more"));
    assert!(task.contains("Package plan-1/a owns: \"src\""));
    assert!(task.contains(
        "Acceptance (from the approved plan, not verified):\n- \"it works\"\n- \"it is fast\""
    ));
    assert!(task.contains("Touched outside the owned paths:"));
    assert!(!task.contains("\"src/f1.rs\", "));
    assert!(task.contains("Gate passed at"));
    assert!(task.find("Scope:").unwrap() < task.find("Gate passed").unwrap());
    assert!(!task.contains("earlier verdicts"));

    let history = ReviewBrief {
        history: vec![PriorVerdict {
            plan_id: "plan-1".into(),
            review_id: "rv0".into(),
            round: 2,
            subject: "report rp0".into(),
            commit_sha: COMMIT.into(),
            verdict: "passed".into(),
        }],
        ..ReviewBrief::default()
    };
    let task = review_task(&row(), &[], None, &history);
    assert!(task.contains(&format!(
        "Your earlier verdicts in plan plan-1:\n- review rv0 round 2 of report rp0 at {COMMIT}: passed\nJudge this commit on its own; an earlier verdict does not carry over."
    )));
}

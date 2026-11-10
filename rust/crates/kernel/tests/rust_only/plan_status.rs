//! `planStatuses`: the status of every plan, each stale reason true and false, each guard, and what the call costs in
//! statements. The clock is a cell the test moves; every row the test inserts is stamped `T0`.

use crate::fixture::{initial, step, Fixture, OWNER};
use capstan_kernel::env::{Env, SystemEnv};
use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::plan_body::parse_plan_body;
use capstan_kernel::Core;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::cell::Cell;
use std::rc::Rc;

const T0: &str = "2026-01-01T00:00:00.000Z";
const T0_MS: i64 = 1_767_225_600_000;
const DAY_MS: i64 = 86_400_000;
const BASE: &str = "0123456789abcdef0123456789abcdef01234567";
const COMMIT: &str = "1111111111111111111111111111111111111111";
const SHA: &str = "abcdef0123456789abcdef0123456789abcdef01";

/// The canonical plan body naming the packages.
fn body_for(packages: &[&str]) -> String {
    if packages.is_empty() {
        return r#"{"summary":"s","packages":[],"risks":[],"integration_order":[]}"#.into();
    }
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
const AGENTS: usize = 12;

struct ClockEnv(Rc<Cell<i64>>);

impl Env for ClockEnv {
    fn now(&self) -> i64 {
        self.0.get()
    }
    fn uuid(&self) -> String {
        SystemEnv.uuid()
    }
    fn random_bytes(&self, n: usize) -> Vec<u8> {
        SystemEnv.random_bytes(n)
    }
}

struct World {
    f: Fixture,
    core: Core,
    clock: Rc<Cell<i64>>,
    next: i64,
    supersedes: Option<String>,
}

fn add_agent(f: &Fixture, agent: &str) {
    let core = f.core();
    let seat = format!("seat-{agent}");
    step(
        core,
        "createSeat",
        json!([{"seatId": seat, "name": seat, "role": "Developer"}]),
    );
    let actor = step(
        core,
        "createActor",
        json!([{"displayName": agent, "role": "Developer", "seatId": seat}]),
    );
    step(
        core,
        "registerAgent",
        json!([{"agentId": agent, "roleName": "developer", "seatId": seat, "actorId": actor["actorId"]}]),
    );
}

impl World {
    fn new() -> World {
        let mut f = Fixture::new();
        for n in 1..=AGENTS {
            add_agent(&f, &format!("ag-{n}"));
        }
        f.close();
        let clock = Rc::new(Cell::new(T0_MS));
        let core = Core::open(
            &f.state,
            &initial(),
            &KernelOptions::default(),
            Box::new(ClockEnv(clock.clone())),
        )
        .unwrap();
        World {
            f,
            core,
            clock,
            next: 0,
            supersedes: None,
        }
    }

    fn db(&self) -> Connection {
        Connection::open(self.f.database()).unwrap()
    }

    fn project(&self, conn: &Connection) -> String {
        conn.query_row("SELECT project_id FROM projects", [], |r| r.get(0))
            .unwrap()
    }

    fn set_clock(&self, ms_after_t0: i64) {
        self.clock.set(T0_MS + ms_after_t0);
    }

    /// A plan of `state` (`draft`, `in_review`, `approved` or `superseded`) with its packages `(id, assignee)`, all
    /// stamped `T0`; `rounds` is the review limit.
    fn plan(
        &mut self,
        state: &str,
        packages: &[(&str, Option<&str>)],
        rounds: Option<i64>,
    ) -> String {
        self.next += 1;
        let plan = format!("plan-{}", 100 + self.next);
        let conn = self.db();
        let project = self.project(&conn);
        let ids: Vec<&str> = packages.iter().map(|p| p.0).collect();
        let body = body_for(&ids);
        conn.execute(
            "INSERT INTO plans(project_id, plan_id, sequence, title, tier, state, requested_by, architect_agent_id, current_revision, max_review_rounds, supersedes_plan_id, created_at, updated_at)
             VALUES (?, ?, ?, 'T', 'normal', 'draft', 'x', 'dev-1', 0, ?, ?, ?, ?)",
            rusqlite::params![project, plan, 100 + self.next, rounds, self.supersedes.take(), T0, T0],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO plan_revisions(project_id, plan_id, revision, base_sha, body_json, body_sha, author_agent_id, author_actor_id, created_at)
             VALUES (?, ?, 1, ?, ?, ?, 'dev-1', (SELECT actor_id FROM agents WHERE agent_id = 'dev-1'), ?)",
            rusqlite::params![project, plan, BASE, body, "c".repeat(64), T0],
        )
        .unwrap();
        let moves = match state {
            "draft" => "",
            "in_review" => "UPDATE plans SET current_revision = 1, state = 'in_review' WHERE plan_id = ?1",
            _ => "UPDATE plans SET current_revision = 1, state = 'approved', approved_revision = 1 WHERE plan_id = ?1",
        };
        if !moves.is_empty() {
            conn.execute(moves, [&plan]).unwrap();
        }
        if state == "superseded" {
            conn.execute(
                "UPDATE plans SET state = 'superseded' WHERE plan_id = ?",
                [&plan],
            )
            .unwrap();
        }
        for (package, assignee) in packages {
            conn.execute(
                "INSERT INTO plan_packages(project_id, plan_id, package_id, assignee_agent_id, assigned_at) VALUES (?, ?, ?, ?, ?)",
                rusqlite::params![project, plan, package, assignee, assignee.map(|_| T0)],
            )
            .unwrap();
        }
        plan
    }

    /// A finished plan review of the current revision of the plan, then the plan back in `state`.
    fn review(&mut self, plan: &str, round: i64, state: &str, back_to: &str) {
        self.next += 1;
        let conn = self.db();
        let project = self.project(&conn);
        let was: String = conn
            .query_row("SELECT state FROM plans WHERE plan_id = ?", [plan], |r| {
                r.get(0)
            })
            .unwrap();
        if was == "draft" {
            conn.execute(
                "UPDATE plans SET current_revision = 1, state = 'in_review' WHERE plan_id = ?",
                [plan],
            )
            .unwrap();
        }
        let verdict = (state == "findings" || state == "passed").then_some("v");
        conn.execute(
            "INSERT INTO reviews(project_id, review_id, sequence, round, subject_plan_id, subject_plan_revision, commit_sha, base_sha,
               author_agent_id, author_actor_id, requested_by_actor_id, reviewer_role, reviewer_agent_id, reviewer_actor_id, state, verdict_text, created_at, completed_at)
             VALUES (?, ?, ?, ?, ?, 1, ?, ?, 'dev-1', (SELECT actor_id FROM agents WHERE agent_id = 'dev-1'),
               (SELECT actor_id FROM agents WHERE agent_id = 'dev-1'), 'reviewer', 'pm-1',
               (SELECT actor_id FROM agents WHERE agent_id = 'pm-1'), ?, ?, ?, ?)",
            rusqlite::params![
                project,
                format!("review-{}", 100 + self.next),
                100 + self.next,
                round,
                plan,
                COMMIT,
                BASE,
                state,
                verdict,
                T0,
                verdict.map(|_| T0)
            ],
        )
        .unwrap();
        if back_to != "in_review" {
            conn.execute(
                "UPDATE plans SET state = ? WHERE plan_id = ?",
                [back_to, plan],
            )
            .unwrap();
        }
    }

    fn report(&mut self, agent: &str, at: &str) -> String {
        self.next += 1;
        let id = format!("report-{}", 100 + self.next);
        let conn = self.db();
        conn.execute(
            "INSERT INTO agent_reports(project_id, report_id, sequence, agent_id, generation, actor_id, commit_sha, branch, summary, state, evidence_json, created_at)
             VALUES (?, ?, ?, ?, 1, (SELECT actor_id FROM agents WHERE agent_id = ?), ?, 'b', 'done', 'accepted', ?, ?)",
            rusqlite::params![
                self.project(&conn),
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

    /// An integration in `state` (`running`, `merged` or `confirmed`) holding the reports.
    fn integration(&mut self, state: &str, reports: &[&str]) -> String {
        self.next += 1;
        let id = format!("int-{}", 100 + self.next);
        let conn = self.db();
        let project = self.project(&conn);
        conn.execute(
            "INSERT INTO integrations(project_id, integration_id, sequence, base_sha, branch, requested_by, state, created_at)
             VALUES (?, ?, ?, ?, ?, 'pm-1', 'running', ?)",
            rusqlite::params![project, id, 100 + self.next, "4".repeat(40), format!("capstan/integration/{id}"), T0],
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
                rusqlite::params!["3".repeat(40), T0, id],
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

    fn signoff(&self, plan: &str, integration: &str) {
        let conn = self.db();
        conn.execute(
            "INSERT INTO plan_signoffs(project_id, plan_id, integration_id, architect_agent_id, summary, created_at)
             VALUES (?, ?, ?, 'dev-1', 'done', ?)",
            rusqlite::params![self.project(&conn), plan, integration, T0],
        )
        .unwrap();
    }

    fn archive(&self, plan: &str) {
        let conn = self.db();
        conn.execute(
            "INSERT INTO plan_archives(project_id, plan_id, archived_at, archived_by, reason) VALUES (?, ?, ?, 'operator', 'retired')",
            rusqlite::params![self.project(&conn), plan, T0],
        )
        .unwrap();
    }

    fn cancel(&self, plan: &str) {
        self.db()
            .execute(
                "UPDATE plans SET cancelled_at = ? WHERE plan_id = ?",
                [T0, plan],
            )
            .unwrap();
    }

    fn record_landing(&self, plan: &str, package: &str) {
        let conn = self.db();
        conn.execute(
            "INSERT INTO package_landings(project_id, plan_id, package_id, commit_sha, recorded_by, recorded_at, note)
             VALUES (?, ?, ?, ?, 'operator', ?, 'by hand')",
            rusqlite::params![self.project(&conn), plan, package, SHA, T0],
        )
        .unwrap();
    }

    fn statuses(&self, days: i64, with_stale: bool) -> Vec<Value> {
        self.core
            .plan_statuses(
                OWNER,
                &json!({"staleAfterDays": days, "withStale": with_stale}),
            )
            .unwrap()
            .as_array()
            .unwrap()
            .clone()
    }

    fn of(&self, plan: &str, days: i64) -> Value {
        self.statuses(days, true)
            .into_iter()
            .find(|v| v["planId"] == plan)
            .unwrap()
    }

    fn stale(&self, plan: &str, days: i64) -> Vec<String> {
        self.of(plan, days)["stale"]
            .as_array()
            .map(|a| a.iter().map(|v| v.as_str().unwrap().to_string()).collect())
            .unwrap_or_default()
    }
}

#[test]
fn every_status_with_its_precedence() {
    let mut w = World::new();
    let draft = w.plan("draft", &[], None);
    let in_review = w.plan("in_review", &[], None);
    let approved = w.plan("approved", &[], None);
    let signed = w.plan("approved", &[], None);
    let integration = w.integration("merged", &[]);
    w.signoff(&signed, &integration);
    let cancelled = w.plan("draft", &[], None);
    w.cancel(&cancelled);
    let superseded = w.plan("superseded", &[], None);
    let archived_cancelled = w.plan("draft", &[], None);
    w.cancel(&archived_cancelled);
    w.archive(&archived_cancelled);
    let archived_signed = w.plan("approved", &[], None);
    w.signoff(&archived_signed, &integration);
    w.archive(&archived_signed);
    let seen: Vec<(String, String)> = w
        .statuses(14, false)
        .iter()
        .map(|v| {
            (
                v["planId"].as_str().unwrap().into(),
                v["status"].as_str().unwrap().into(),
            )
        })
        .collect();
    let expect = [
        (&draft, "draft"),
        (&in_review, "in_review"),
        (&approved, "approved"),
        (&signed, "signed_off"),
        (&cancelled, "cancelled"),
        (&superseded, "superseded"),
        (&archived_cancelled, "archived"),
        (&archived_signed, "archived"),
    ];
    assert_eq!(seen.len(), expect.len());
    for ((id, status), (want_id, want)) in seen.iter().zip(expect) {
        assert_eq!((id, status.as_str()), (want_id, want));
    }
    let all = w.statuses(14, false);
    assert_eq!(
        all[6]["archived"],
        json!({"archivedAt": T0, "archivedBy": "operator", "reason": "retired"})
    );
    assert!(all[0].get("archived").is_none());
    assert!(all.iter().all(|v| v.get("stale").is_none()));
}

#[test]
fn idle_at_the_threshold_and_never_at_zero() {
    let mut w = World::new();
    let draft = w.plan("draft", &[], None);
    let in_review = w.plan("in_review", &[], None);
    let approved = w.plan("approved", &[("a", None)], None);
    w.set_clock(14 * DAY_MS - 1000);
    for plan in [&draft, &in_review, &approved] {
        assert!(w.stale(plan, 14).is_empty(), "{plan} at N-1 days");
    }
    w.set_clock(14 * DAY_MS + 1000);
    for plan in [&draft, &in_review, &approved] {
        assert_eq!(w.stale(plan, 14), ["idle"], "{plan} at N+1 days");
        assert!(w.stale(plan, 0).is_empty(), "{plan} with staleAfterDays 0");
    }
    assert_eq!(w.of(&approved, 14)["lastActivityAt"], T0);
}

#[test]
fn a_package_assignment_inside_the_window_keeps_an_approved_plan_alive() {
    let mut w = World::new();
    let plan = w.plan("approved", &[("a", None)], None);
    w.db()
        .execute("UPDATE plan_packages SET assignee_agent_id = 'ag-1', assigned_at = '2026-01-10T00:00:00.000Z' WHERE plan_id = ?", [&plan])
        .unwrap();
    // Assigned 9 days after T0; 20 days after T0 the plan was last touched 11 days ago.
    w.set_clock(20 * DAY_MS);
    assert!(w.stale(&plan, 14).is_empty());
    assert_eq!(
        w.of(&plan, 14)["lastActivityAt"],
        "2026-01-10T00:00:00.000Z"
    );
}

#[test]
fn a_review_exhausted_draft_is_stale() {
    let mut w = World::new();
    let spent = w.plan("draft", &[], Some(1));
    w.review(&spent, 1, "findings", "draft");
    let fresh = w.plan("draft", &[], Some(2));
    w.review(&fresh, 1, "findings", "draft");
    assert_eq!(w.stale(&spent, 14), ["review_exhausted"]);
    assert!(w.stale(&fresh, 14).is_empty());
    // Review activity counts as activity, and only a draft can be review_exhausted.
    let in_review = w.plan("in_review", &[], Some(1));
    w.review(&in_review, 1, "findings", "in_review");
    assert!(w.stale(&in_review, 14).is_empty());
}

fn done_plan(w: &mut World, agent: &str, how: &str) -> String {
    let plan = w.plan("approved", &[("a", Some(agent))], None);
    match how {
        "covered" => {
            let held = w.report(agent, T0);
            let other = w.report("ag-12", T0);
            let integration = w.integration("confirmed", &[&other]);
            let conn = w.db();
            conn.execute(
                "INSERT INTO integration_covered_reports(project_id, integration_id, report_id, how) VALUES (?, ?, ?, 'tree')",
                rusqlite::params![w.project(&conn), integration, held],
            )
            .unwrap();
        }
        "recorded" => w.record_landing(&plan, "a"),
        "confirmed" => {
            let report = w.report(agent, T0);
            w.integration("confirmed", &[&report]);
        }
        _ => {
            let report = w.report(agent, T0);
            w.integration("merged", &[&report]);
        }
    }
    plan
}

#[test]
fn done_unsigned_with_a_covered_recorded_or_confirmed_landing() {
    let mut w = World::new();
    w.set_clock(1000);
    let covered = done_plan(&mut w, "ag-1", "covered");
    let recorded = done_plan(&mut w, "ag-2", "recorded");
    let confirmed = done_plan(&mut w, "ag-3", "confirmed");
    for plan in [&covered, &recorded, &confirmed] {
        assert_eq!(w.stale(plan, 14), ["done_unsigned"], "{plan}");
    }
    // A signed-off plan is done and signed: not a candidate.
    let signed = done_plan(&mut w, "ag-4", "recorded");
    let integration = w.integration("merged", &[]);
    w.signoff(&signed, &integration);
    assert!(w.stale(&signed, 14).is_empty());
    // A merge that is not confirmed is not settled: the package is live and guards the plan.
    let merged = done_plan(&mut w, "ag-5", "merged");
    assert!(w.stale(&merged, 14).is_empty());
    w.set_clock(30 * DAY_MS);
    assert!(w.stale(&merged, 14).is_empty(), "guarded though idle");
    assert_eq!(w.stale(&recorded, 14), ["idle", "done_unsigned"]);
}

#[test]
fn each_guard_makes_a_would_be_stale_plan_not_stale() {
    let mut w = World::new();
    // Each plan is idle (and the drafts exhausted) at 30 days unless a guard holds.
    let bare_draft = w.plan("draft", &[], Some(1));
    w.review(&bare_draft, 1, "findings", "draft");
    let started = w.plan("draft", &[], Some(1));
    w.review(&started, 1, "findings", "in_review");
    w.review(&started, 2, "started", "in_review");
    w.db()
        .execute(
            "UPDATE plans SET state = 'draft' WHERE plan_id = ?",
            [&started],
        )
        .unwrap();
    let assigned = w.plan("approved", &[("a", Some("ag-1"))], None);
    let released = w.plan("approved", &[("a", Some("ag-2"))], None);
    w.db()
        .execute(
            "UPDATE plan_packages SET cancelled_at = ? WHERE plan_id = ?",
            [T0, &released],
        )
        .unwrap();
    let merged_plan = w.plan("approved", &[("a", Some("ag-3"))], None);
    w.db()
        .execute(
            "UPDATE plan_packages SET cancelled_at = ? WHERE plan_id = ?",
            [T0, &merged_plan],
        )
        .unwrap();
    let merged_report = w.report("ag-3", T0);
    w.integration("merged", &[&merged_report]);
    let running_report = w.report("ag-2", T0);
    w.integration("running", &[&running_report]);
    let old = w.plan("approved", &[("a", None)], None);
    w.supersedes = Some(old.clone());
    let newer = w.plan("approved", &[("a", None)], None);
    w.set_clock(30 * DAY_MS);
    assert_eq!(w.stale(&bare_draft, 14), ["review_exhausted", "idle"]);
    for (plan, what) in [
        (&started, "a started review"),
        (&assigned, "an assigned live package"),
        (&released, "a running integration"),
        (&merged_plan, "a merged unconfirmed integration"),
        (&old, "a live superseding plan"),
    ] {
        assert!(w.stale(plan, 14).is_empty(), "{what}");
    }
    // The superseding plan itself is not guarded, and a cancelled or archived superseding plan does not guard.
    assert_eq!(w.stale(&newer, 14), ["idle"]);
    w.cancel(&newer);
    assert_eq!(w.stale(&old, 14), ["idle"]);
}

#[test]
fn an_archived_superseding_plan_does_not_guard() {
    let mut w = World::new();
    let old = w.plan("approved", &[("a", None)], None);
    w.supersedes = Some(old.clone());
    let newer = w.plan("draft", &[], None);
    w.set_clock(30 * DAY_MS);
    assert!(w.stale(&old, 14).is_empty());
    w.archive(&newer);
    assert_eq!(w.stale(&old, 14), ["idle"]);
}

#[test]
fn input_is_checked_and_stale_is_only_asked_for() {
    let w = World::new();
    for input in [
        json!({"staleAfterDays": -1, "withStale": true}),
        json!({"staleAfterDays": 3651, "withStale": true}),
        json!({"staleAfterDays": "14", "withStale": true}),
        json!({"withStale": true}),
        json!({"staleAfterDays": 14}),
    ] {
        assert!(w.core.plan_statuses(OWNER, &input).is_err(), "{input}");
    }
    assert!(w
        .core
        .plan_statuses("nobody", &json!({"staleAfterDays": 14, "withStale": true}))
        .is_err());
}

#[test]
fn the_cost_of_a_call_is_bounded_by_the_candidates() {
    let w = World::new();
    // 300 plans with 2 packages each: 280 closed (signed off or archived), 10 drafts, 10 approved candidates.
    {
        let conn = w.db();
        let project = w.project(&conn);
        conn.execute_batch("BEGIN").unwrap();
        let body = body_for(&["a", "b"]);
        let integration_id = "int-closed";
        conn.execute(
            "INSERT INTO integrations(project_id, integration_id, sequence, base_sha, branch, requested_by, state, created_at)
             VALUES (?, ?, 1, ?, 'capstan/integration/int-closed', 'pm-1', 'running', ?)",
            rusqlite::params![project, integration_id, "4".repeat(40), T0],
        )
        .unwrap();
        for n in 1..=300 {
            let plan = format!("p-{n:03}");
            conn.execute(
                "INSERT INTO plans(project_id, plan_id, sequence, title, tier, state, requested_by, architect_agent_id, current_revision, created_at, updated_at)
                 VALUES (?, ?, ?, 'T', 'normal', 'draft', 'x', 'dev-1', 0, ?, ?)",
                rusqlite::params![project, plan, 1000 + n, T0, T0],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO plan_revisions(project_id, plan_id, revision, base_sha, body_json, body_sha, author_agent_id, author_actor_id, created_at)
                 VALUES (?, ?, 1, ?, ?, ?, 'dev-1', (SELECT actor_id FROM agents WHERE agent_id = 'dev-1'), ?)",
                rusqlite::params![project, plan, BASE, if n > 10 { &body } else { "{}" }, "c".repeat(64), T0],
            )
            .unwrap();
            let approved = n > 10;
            if approved {
                conn.execute(
                    "UPDATE plans SET current_revision = 1, state = 'approved', approved_revision = 1 WHERE plan_id = ?",
                    [&plan],
                )
                .unwrap();
                for package in ["a", "b"] {
                    conn.execute(
                        "INSERT INTO plan_packages(project_id, plan_id, package_id) VALUES (?, ?, ?)",
                        rusqlite::params![project, plan, package],
                    )
                    .unwrap();
                }
            }
            if n > 20 && n % 2 == 0 {
                conn.execute(
                    "INSERT INTO plan_signoffs(project_id, plan_id, integration_id, architect_agent_id, summary, created_at)
                     VALUES (?, ?, ?, 'dev-1', 'done', ?)",
                    rusqlite::params![project, plan, integration_id, T0],
                )
                .unwrap();
            } else if n > 20 {
                conn.execute(
                    "INSERT INTO plan_archives(project_id, plan_id, archived_at, archived_by, reason) VALUES (?, ?, ?, 'operator', 'old')",
                    rusqlite::params![project, plan, T0],
                )
                .unwrap();
            }
        }
        conn.execute_batch("COMMIT").unwrap();
    }
    w.set_clock(30 * DAY_MS);
    let input = |stale: bool| json!({"staleAfterDays": 14, "withStale": stale});
    // The cost of authorising the call, with the input refused after it.
    let before = w.core.statements_run();
    let _ = w.core.plan_statuses(OWNER, &json!({}));
    let auth = w.core.statements_run() - before;

    let before = w.core.statements_run();
    let plain = w.core.plan_statuses(OWNER, &input(false)).unwrap();
    assert_eq!(w.core.statements_run() - before - auth, 1);
    assert_eq!(plain.as_array().unwrap().len(), 300);

    let before = w.core.statements_run();
    let stale = w.core.plan_statuses(OWNER, &input(true)).unwrap();
    let used = w.core.statements_run() - before - auth;
    let drafts = 10;
    let approved = 10;
    let packages = 20;
    assert!(
        used <= 5 + drafts + approved + packages,
        "{used} statements"
    );
    assert!(used > 5, "the candidates were read: {used}");
    let idle = stale
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v.get("stale").is_some())
        .count();
    assert_eq!(idle, 20);

    // With the 20 candidates closed, the other 280 plans run no further statement.
    for n in 1..=20 {
        w.archive(&format!("p-{n:03}"));
    }
    let before = w.core.statements_run();
    w.core.plan_statuses(OWNER, &input(true)).unwrap();
    assert_eq!(w.core.statements_run() - before - auth, 1);
}

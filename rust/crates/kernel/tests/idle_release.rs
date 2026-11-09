//! The ledger side of automatic release: every open-work reason alone, the idle-since sources, who is never a
//! candidate, the keep record and the two ledger rows.

mod common;
#[path = "rust_only/fixture.rs"]
mod fixture;

use capstan_kernel::areas::messaging::parse_iso_ms;
use fixture::{context, step, Fixture, OWNER};
use serde_json::{json, Value};

const SHA: &str = "1111111111111111111111111111111111111111";
const DAY: i64 = 24 * 3_600_000;

fn project(f: &Fixture) -> String {
    rusqlite::Connection::open(f.database())
        .unwrap()
        .query_row("SELECT project_id FROM projects", [], |r| r.get(0))
        .unwrap()
}

/// Runs raw SQL on the ledger file (the project id is `@P@`), with the core closed meanwhile.
fn sql(f: &mut Fixture, statements: &str) {
    let p = project(f);
    f.close();
    {
        let db = rusqlite::Connection::open(f.database()).unwrap();
        db.execute_batch(&statements.replace("@P@", &format!("'{p}'")))
            .unwrap_or_else(|e| panic!("{statements}: {e}"));
    }
    f.reopen();
}

fn actor_of(f: &Fixture, agent: &str) -> String {
    f.core().agent_record(agent).unwrap()["actorId"]
        .as_str()
        .unwrap()
        .to_string()
}

fn work(f: &Fixture, agent: &str) -> Vec<String> {
    f.core().open_work(agent).unwrap()
}

/// Real time plus ten days: far beyond every grace of the agents the fixture created a moment ago.
fn later() -> i64 {
    capstan_ledger::now_millis() + 10 * DAY
}

fn input(idle_minutes: i64) -> Value {
    json!({"idleMinutes": idle_minutes, "graceFloorMs": 0, "childActivity": {}})
}

fn candidate_ids(f: &Fixture, now: i64, input: &Value) -> Vec<String> {
    f.core()
        .idle_candidates(now, input)
        .unwrap()
        .iter()
        .map(|c| c["agentId"].as_str().unwrap().to_string())
        .collect()
}

/// A plan in `state` with the given packages (id, assignee) in its approved revision.
fn add_plan(
    f: &mut Fixture,
    plan: &str,
    sequence: i64,
    state: &str,
    architect: &str,
    packages: &[(&str, &str)],
) {
    let body =
        json!({"packages": packages.iter().map(|(id, _)| json!({"id": id})).collect::<Vec<_>>()})
            .to_string();
    let arch = if architect.is_empty() {
        "NULL".to_string()
    } else {
        format!("'{architect}'")
    };
    let revision = if state == "draft" { 0 } else { 1 };
    let author = actor_of(f, "dev-1");
    let mut script = format!(
        "INSERT INTO plans(project_id, plan_id, sequence, title, tier, state, requested_by, architect_agent_id, current_revision,
           created_at, updated_at)
         VALUES (@P@, '{plan}', {sequence}, 't', 'normal', 'draft', 'pm-1', {arch}, 0, '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z');"
    );
    if revision == 1 {
        script += &format!(
            "INSERT INTO plan_revisions(project_id, plan_id, revision, base_sha, body_json, body_sha, author_agent_id, author_actor_id, created_at)
             VALUES (@P@, '{plan}', 1, '{SHA}', '{body}', '{}', 'dev-1', '{author}', '2026-01-01T00:00:00.000Z');
             UPDATE plans SET current_revision = 1 WHERE plan_id = '{plan}';",
            "c".repeat(64)
        );
        if state == "approved" {
            script += &format!("UPDATE plans SET state = 'approved', approved_revision = 1 WHERE plan_id = '{plan}';");
        } else if state != "in_review" {
            panic!("unsupported state {state}");
        } else {
            script += &format!("UPDATE plans SET state = 'in_review' WHERE plan_id = '{plan}';");
        }
    }
    for (package, assignee) in packages {
        if !assignee.is_empty() {
            script += &format!(
                "INSERT INTO plan_packages(project_id, plan_id, package_id, assignee_agent_id, assigned_at)
                 VALUES (@P@, '{plan}', '{package}', '{assignee}', '2026-01-02T00:00:00.000Z');"
            );
        }
    }
    sql(f, &script);
}

/// A merged integration that covers `report`.
fn merge_report(f: &mut Fixture, integration: &str, sequence: i64, report: &str) {
    add_integration(f, integration, sequence, "running", "pm-1");
    sql(
        f,
        &format!(
            "INSERT INTO integration_reports(project_id, integration_id, position, report_id) VALUES (@P@, '{integration}', 1, '{report}');
             UPDATE integrations SET state = 'merged', head_sha = '{}', completed_at = '2026-01-09T00:00:00.000Z'
              WHERE integration_id = '{integration}';",
            "2".repeat(40)
        ),
    );
}

fn add_report(f: &mut Fixture, report: &str, agent: &str, sequence: i64) {
    let actor = actor_of(f, agent);
    sql(
        f,
        &format!(
            "INSERT INTO agent_reports(project_id, report_id, sequence, agent_id, generation, actor_id, commit_sha, branch,
               summary, state, reason, evidence_json, created_at)
             VALUES (@P@, '{report}', {sequence}, '{agent}', 1, '{actor}', '{SHA}', 'b', 's', 'accepted', NULL, '{{}}',
               '2026-01-0{sequence}T00:00:00.000Z')"
        ),
    );
}

fn add_integration(f: &mut Fixture, id: &str, sequence: i64, state: &str, requested_by: &str) {
    let (head, completed) = match state {
        "running" => ("NULL".to_string(), "NULL".to_string()),
        _ => (
            format!("'{}'", "2".repeat(40)),
            "'2026-01-09T00:00:00.000Z'".to_string(),
        ),
    };
    sql(
        f,
        &format!(
            "INSERT INTO integrations(project_id, integration_id, sequence, base_sha, branch, requested_by, state, head_sha,
               created_at, completed_at)
             VALUES (@P@, '{id}', {sequence}, '{SHA}', 'integration/{id}', '{requested_by}', '{state}', {head},
               '2026-01-08T00:00:00.000Z', {completed})"
        ),
    );
}

#[test]
fn a_fresh_worker_has_no_open_work() {
    let f = Fixture::new();
    assert!(work(&f, "dev-1").is_empty());
    assert!(work(&f, "no-such-agent").is_empty());
}

#[test]
fn an_assigned_package_is_open_until_integrated_or_cancelled() {
    let mut f = Fixture::new();
    add_plan(&mut f, "plan-1", 1, "approved", "", &[("one", "dev-1")]);
    let reasons = work(&f, "dev-1");
    assert_eq!(reasons.len(), 1, "{reasons:?}");
    assert!(reasons[0].contains("plan-1/one") && reasons[0].contains("assigned"));
    // A report moves the packages to reported; the report itself is not merged either.
    add_report(&mut f, "rep-1", "dev-1", 3);
    let reasons = work(&f, "dev-1");
    assert!(
        reasons.iter().any(|r| r.contains("report rep-1")),
        "{reasons:?}"
    );
    // Integrated with its report: nothing holds the agent.
    merge_report(&mut f, "int-1", 1, "rep-1");
    assert!(work(&f, "dev-1").is_empty(), "{:?}", work(&f, "dev-1"));
    // A cancelled package never holds an agent.
    add_plan(&mut f, "plan-0", 2, "approved", "", &[("solo", "dev-1")]);
    assert_eq!(work(&f, "dev-1").len(), 1);
    sql(&mut f, "UPDATE plan_packages SET cancelled_at = '2026-01-10T00:00:00.000Z' WHERE plan_id = 'plan-0'");
    assert!(work(&f, "dev-1").is_empty(), "{:?}", work(&f, "dev-1"));
}

#[test]
fn a_report_not_covered_by_a_merged_integration_is_open_work() {
    let mut f = Fixture::new();
    add_report(&mut f, "rep-1", "dev-1", 3);
    assert_eq!(work(&f, "dev-1").len(), 1);
    // A newer report is the one that counts.
    add_report(&mut f, "rep-2", "dev-1", 4);
    add_integration(&mut f, "int-1", 1, "running", "pm-1");
    sql(
        &mut f,
        "INSERT INTO integration_reports(project_id, integration_id, position, report_id) VALUES (@P@, 'int-1', 1, 'rep-2')",
    );
    assert_eq!(
        work(&f, "dev-1").len(),
        1,
        "a running integration does not cover it"
    );
    sql(
        &mut f,
        "UPDATE integrations SET state = 'merged', head_sha = '2222222222222222222222222222222222222222',
           completed_at = '2026-01-09T00:00:00.000Z' WHERE integration_id = 'int-1'",
    );
    assert!(work(&f, "dev-1").is_empty(), "{:?}", work(&f, "dev-1"));
}

#[test]
fn a_queued_or_running_verify_run_holds_the_author_and_the_requester() {
    let mut f = Fixture::new();
    add_report(&mut f, "rep-1", "dev-1", 3);
    merge_report(&mut f, "int-1", 1, "rep-1");
    assert!(work(&f, "dev-1").is_empty());
    let pm_actor = f.pm_actor.clone();
    sql(
        &mut f,
        &format!(
            "INSERT INTO verify_runs(project_id, run_id, sequence, subject_report_id, commit_sha, commands_json, commands_sha,
               trigger, requested_by_actor_id, state, queued_at)
             VALUES (@P@, 'run-1', 1, 'rep-1', '{SHA}', '[]', 'x', 'auto', '{pm_actor}', 'queued', '2026-01-05T00:00:00.000Z')"
        ),
    );
    let reasons = work(&f, "dev-1");
    assert_eq!(reasons.len(), 1);
    assert!(reasons[0].contains("run-1"));
    // Another agent that requested a run holds it too.
    let dev_actor = actor_of(&f, "dev-1");
    sql(
        &mut f,
        "UPDATE verify_runs SET state = 'cancelled', finished_at = '2026-01-06T00:00:00.000Z'",
    );
    assert!(work(&f, "dev-1").is_empty());
    sql(
        &mut f,
        &format!(
            "INSERT INTO verify_runs(project_id, run_id, sequence, subject_integration_id, commit_sha, commands_json, commands_sha,
               trigger, requested_by_actor_id, state, queued_at, started_at)
             VALUES (@P@, 'run-2', 2, 'int-1', '{}', '[]', 'y', 'manual', '{dev_actor}', 'running', '2026-01-07T00:00:00.000Z',
               '2026-01-07T00:00:01.000Z')",
            "3".repeat(40)
        ),
    );
    assert_eq!(work(&f, "dev-1").len(), 1);
}

#[test]
fn a_running_review_holds_the_reviewer_and_the_author() {
    let mut f = Fixture::new();
    // dev-1 authored rep-1 (merged), pm-1's seat stands in for a reviewer agent row.
    add_report(&mut f, "rep-1", "dev-1", 3);
    merge_report(&mut f, "int-1", 1, "rep-1");
    let dev = actor_of(&f, "dev-1");
    let pm = f.pm_actor.clone();
    sql(
        &mut f,
        &format!(
            "INSERT INTO reviews(project_id, review_id, sequence, round, subject_report_id, commit_sha, base_sha, author_agent_id,
               author_actor_id, requested_by_actor_id, reviewer_role, reviewer_agent_id, reviewer_actor_id, state, created_at)
             VALUES (@P@, 'rev-1', 1, 1, 'rep-1', '{SHA}', '{SHA}', 'dev-1', '{dev}', '{pm}', 'reviewer', 'pm-1', '{pm}', 'started',
               '2026-01-05T00:00:00.000Z')"
        ),
    );
    let author = work(&f, "dev-1");
    assert_eq!(author.len(), 1, "{author:?}");
    assert!(author[0].contains("rev-1"));
    assert_eq!(work(&f, "pm-1").len(), 1);
    sql(
        &mut f,
        "UPDATE reviews SET state = 'cancelled', failure_reason = 'x', completed_at = '2026-01-06T00:00:00.000Z'",
    );
    assert!(work(&f, "dev-1").is_empty());
}

#[test]
fn an_architect_with_a_live_plan_is_held_until_every_package_is_integrated() {
    let mut f = Fixture::new();
    add_plan(&mut f, "plan-d", 1, "draft", "dev-1", &[]);
    let reasons = work(&f, "dev-1");
    assert_eq!(reasons.len(), 1, "{reasons:?}");
    assert!(reasons[0].contains("draft"));
    sql(
        &mut f,
        "UPDATE plans SET cancelled_at = '2026-01-04T00:00:00.000Z' WHERE plan_id = 'plan-d'",
    );
    assert!(work(&f, "dev-1").is_empty());
    add_plan(&mut f, "plan-r", 2, "in_review", "dev-1", &[]);
    assert!(work(&f, "dev-1")[0].contains("in_review"));
    sql(
        &mut f,
        "UPDATE plans SET cancelled_at = '2026-01-04T00:00:00.000Z' WHERE plan_id = 'plan-r'",
    );
    assert!(work(&f, "dev-1").is_empty());
    // Approved: open while a live package is not integrated, free once all are cancelled or integrated.
    add_plan(&mut f, "plan-a", 3, "approved", "dev-1", &[("one", "pm-1")]);
    let reasons = work(&f, "dev-1");
    assert_eq!(reasons.len(), 1, "{reasons:?}");
    assert!(reasons[0].contains("approved") && reasons[0].contains("one"));
    sql(
        &mut f,
        "UPDATE plan_packages SET cancelled_at = '2026-01-03T00:00:00.000Z'",
    );
    assert!(work(&f, "dev-1").is_empty());
    // A cancelled plan holds nothing.
    add_plan(&mut f, "plan-b", 4, "approved", "dev-1", &[("one", "pm-1")]);
    assert_eq!(work(&f, "dev-1").len(), 1);
    sql(
        &mut f,
        "UPDATE plans SET cancelled_at = '2026-01-04T00:00:00.000Z' WHERE plan_id = 'plan-b'",
    );
    assert!(work(&f, "dev-1").is_empty());
}

#[test]
fn an_integration_it_created_holds_while_running_or_under_review() {
    let mut f = Fixture::new();
    add_integration(&mut f, "int-1", 1, "running", "dev-1");
    assert_eq!(work(&f, "dev-1").len(), 1);
    sql(
        &mut f,
        &format!(
            "UPDATE integrations SET state = 'merged', head_sha = '{}', completed_at = '2026-01-09T00:00:00.000Z'",
            "2".repeat(40)
        ),
    );
    assert!(work(&f, "dev-1").is_empty());
    let pm = f.pm_actor.clone();
    sql(
        &mut f,
        &format!(
            "INSERT INTO reviews(project_id, review_id, sequence, round, subject_integration_id, commit_sha, base_sha,
               requested_by_actor_id, reviewer_role, reviewer_agent_id, reviewer_actor_id, state, created_at)
             VALUES (@P@, 'rev-9', 1, 1, 'int-1', '{SHA}', '{SHA}', '{pm}', 'reviewer', 'pm-1', '{pm}', 'started',
               '2026-01-05T00:00:00.000Z')"
        ),
    );
    let reasons = work(&f, "dev-1");
    assert_eq!(reasons.len(), 1, "{reasons:?}");
    assert!(reasons[0].contains("int-1"));
}

#[test]
fn mail_in_either_direction_and_a_prompt_relay_hold_an_agent() {
    let mut f = Fixture::new();
    // Mail to it.
    let core = f.core();
    let queued = core
        .enqueue_message(
            &context(core, OWNER),
            &json!({"recipientAgentId": "dev-1", "body": "please do x"}),
        )
        .unwrap();
    let reasons = work(&f, "dev-1");
    assert_eq!(reasons.len(), 1, "{reasons:?}");
    assert!(reasons[0].contains("to it"));
    let id = queued["messageId"].as_str().unwrap().to_string();
    sql(
        &mut f,
        &format!("UPDATE messages SET state = 'cancelled', state_reason = 'test' WHERE message_id = '{id}'"),
    );
    assert!(work(&f, "dev-1").is_empty(), "{:?}", work(&f, "dev-1"));
    // A question it sent that nobody acknowledged; an info message does not count.
    let core = f.core();
    let question = core
        .enqueue_message(
            &context(core, &f.developer_credential),
            &json!({"recipientAgentId": "pm-1", "body": "which one?", "actionNeeded": true}),
        )
        .unwrap();
    let reasons = work(&f, "dev-1");
    assert_eq!(reasons.len(), 1, "{reasons:?}");
    assert!(reasons[0].contains("it sent"));
    let id = question["messageId"].as_str().unwrap().to_string();
    sql(
        &mut f,
        &format!("UPDATE messages SET state = 'cancelled', state_reason = 'test' WHERE message_id = '{id}'"),
    );
    let core = f.core();
    core.enqueue_message(
        &context(core, &f.developer_credential),
        &json!({"recipientAgentId": "pm-1", "body": "fyi", "class": "info"}),
    )
    .unwrap();
    assert!(work(&f, "dev-1").is_empty(), "{:?}", work(&f, "dev-1"));
    // A prompt relay waiting for an answer.
    let pm = f.pm_actor.clone();
    sql(
        &mut f,
        &format!(
            "INSERT INTO prompt_relays(project_id, relay_id, sequence, agent_id, pane_id, host_kind, prompt_text, options_json,
               prompt_sha, captured_by_actor_id, captured_at, expires_at, state)
             VALUES (@P@, 'relay-1', 1, 'dev-1', 'w:p', 'claude', 'ok?', '[]', '{}', '{pm}', '2026-01-05T00:00:00.000Z',
               '2026-01-05T00:10:00.000Z', 'captured')",
            "a".repeat(64)
        ),
    );
    let reasons = work(&f, "dev-1");
    assert_eq!(reasons.len(), 1, "{reasons:?}");
    assert!(reasons[0].contains("prompt"));
    sql(
        &mut f,
        "UPDATE prompt_relays SET state = 'expired', outcome_reason = 'gone'",
    );
    assert!(work(&f, "dev-1").is_empty());
}

fn observe(f: &Fixture, agent: &str, state: &str) {
    let core = f.core();
    core.record_agent_observation(&context(core, OWNER), agent, &json!(state))
        .unwrap();
}

fn since_of(f: &Fixture, now: i64, minutes: i64) -> Option<i64> {
    f.core()
        .idle_candidates(now, &input(minutes))
        .unwrap()
        .iter()
        .find(|c| c["agentId"] == "dev-1")
        .map(|c| c["idleSinceMs"].as_i64().unwrap())
}

#[test]
fn idle_since_is_the_latest_of_the_four_sources() {
    let mut f = Fixture::new();
    let now = later();
    observe(&f, "dev-1", "idle");
    let base = since_of(&f, now, 60).expect("a candidate");
    // The baseline is the agent's own creation or call time, never the far future.
    assert!(base < now - 9 * DAY);
    // 1. Last authenticated call.
    let t1 = base + DAY;
    sql(
        &mut f,
        &format!(
            "UPDATE agents SET last_activity_at = '{}' WHERE agent_id = 'dev-1'",
            capstan_ledger::iso_from_millis(t1)
        ),
    );
    assert_eq!(since_of(&f, now, 60), Some(t1));
    // 2. A working observation.
    let t2 = base + 2 * DAY;
    sql(
        &mut f,
        &format!(
            "INSERT INTO agent_state_history(project_id, agent_id, sequence, herdr_state, observed_at)
             VALUES (@P@, 'dev-1', 1000, 'working', '{}'), (@P@, 'dev-1', 1001, 'idle', '{}')",
            capstan_ledger::iso_from_millis(t2),
            capstan_ledger::iso_from_millis(t2 + 1000),
        ),
    );
    assert_eq!(since_of(&f, now, 60), Some(t2));
    // 3. Child activity from the daemon.
    let t3 = base + 3 * DAY;
    let with_child = json!({"idleMinutes": 60, "graceFloorMs": 0, "childActivity": {"dev-1": t3}});
    let found = f.core().idle_candidates(now, &with_child).unwrap();
    assert_eq!(found[0]["idleSinceMs"], t3);
    // 4. A message delivery or acknowledgement.
    let t4 = base + 4 * DAY;
    let core = f.core();
    let sent = core
        .enqueue_message(
            &context(core, OWNER),
            &json!({"recipientAgentId": "dev-1", "body": "hello"}),
        )
        .unwrap();
    let id = sent["messageId"].as_str().unwrap().to_string();
    sql(
        &mut f,
        &format!(
            "UPDATE messages SET state = 'acked', sent_at = '{}', acked_at = '{}' WHERE message_id = '{id}'",
            capstan_ledger::iso_from_millis(t4 - 1000),
            capstan_ledger::iso_from_millis(t4)
        ),
    );
    let found = f.core().idle_candidates(now, &with_child).unwrap();
    assert_eq!(found[0]["idleSinceMs"], t4);
    assert_eq!(found[0]["idleSince"], capstan_ledger::iso_from_millis(t4));
}

#[test]
fn a_busy_agent_is_not_a_candidate_but_idle_done_and_unknown_are() {
    let f = Fixture::new();
    let now = later();
    for (state, expect) in [
        ("working", false),
        ("blocked", false),
        ("idle", true),
        ("done", true),
        ("unknown", true),
    ] {
        observe(&f, "dev-1", state);
        assert_eq!(
            candidate_ids(&f, now, &input(60)).contains(&"dev-1".to_string()),
            expect,
            "{state}"
        );
    }
}

#[test]
fn the_pm_the_supervisor_and_agents_in_grace_are_never_candidates() {
    let f = Fixture::new();
    let now = later();
    assert_eq!(
        candidate_ids(&f, now, &input(60)),
        vec!["dev-1".to_string()]
    );
    // A Supervisor is never released this way.
    step(
        f.core(),
        "syncRoleDefinitions",
        json!([[
            {"name": "pm", "kind": "PM", "host": "claude", "configHash": "b".repeat(64)},
            {"name": "developer", "kind": "Developer", "host": "claude", "configHash": "a".repeat(64)},
            {"name": "supervisor", "kind": "Supervisor", "host": "claude", "configHash": "c".repeat(64)},
        ]]),
    );
    let core = f.core();
    step(
        core,
        "createSeat",
        json!([{"seatId": "seat-sup", "name": "seat-sup", "role": "Supervisor"}]),
    );
    let actor = step(
        core,
        "createActor",
        json!([{"displayName": "sup-1", "role": "Supervisor", "seatId": "seat-sup"}]),
    );
    step(
        core,
        "registerAgent",
        json!([{"agentId": "sup-1", "roleName": "supervisor", "seatId": "seat-sup", "actorId": actor["actorId"]}]),
    );
    assert_eq!(
        candidate_ids(&f, now, &input(60)),
        vec!["dev-1".to_string()]
    );
    // Grace after a spawn: a candidate only once idle_minutes have passed since the agent was created.
    let spawn = {
        let db = rusqlite::Connection::open(f.database()).unwrap();
        let text: String = db
            .query_row(
                "SELECT created_at FROM agents WHERE agent_id = 'dev-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        parse_iso_ms(&text)
    };
    assert!(!candidate_ids(&f, spawn + 59 * 60_000, &input(60)).contains(&"dev-1".to_string()));
    assert!(candidate_ids(&f, spawn + 60 * 60_000, &input(60)).contains(&"dev-1".to_string()));
    // Grace after a daemon start.
    let started =
        json!({"idleMinutes": 60, "graceFloorMs": now - 10 * 60_000, "childActivity": {}});
    assert!(candidate_ids(&f, now, &started).is_empty());
    let started =
        json!({"idleMinutes": 60, "graceFloorMs": now - 61 * 60_000, "childActivity": {}});
    assert_eq!(candidate_ids(&f, now, &started), vec!["dev-1".to_string()]);
    // A paused agent, a paused run.
    let core = f.core();
    core.pause_agent(
        &context(core, OWNER),
        &json!({"agentId": "dev-1", "reason": "test"}),
    )
    .unwrap();
    assert!(candidate_ids(&f, now, &input(60)).is_empty());
    let core = f.core();
    core.resume_agent(
        &context(core, OWNER),
        &json!({"agentId": "dev-1", "reason": "test"}),
    )
    .unwrap();
    assert_eq!(
        candidate_ids(&f, now, &input(60)),
        vec!["dev-1".to_string()]
    );
}

#[test]
fn a_warm_reviewer_is_not_a_candidate() {
    let mut f = Fixture::new();
    let now = later();
    sql(
        &mut f,
        "INSERT INTO plans(project_id, plan_id, sequence, title, tier, state, requested_by, architect_agent_id, current_revision,
           approved_revision, created_at, updated_at)
         VALUES (@P@, 'plan-1', 1, 't', 'normal', 'approved', 'pm-1', NULL, 1, 1, '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z');
         INSERT INTO warm_reviewers(project_id, plan_id, reviewer_role, agent_id, reviews_count, created_at, last_used_at)
         VALUES (@P@, 'plan-1', 'developer', 'dev-1', 1, '2026-01-02T00:00:00.000Z', '2026-01-02T00:00:00.000Z');",
    );
    assert!(candidate_ids(&f, now, &input(60)).is_empty());
    sql(
        &mut f,
        "UPDATE warm_reviewers SET released_at = '2026-01-03T00:00:00.000Z', release_reason = 'idle'",
    );
    assert_eq!(
        candidate_ids(&f, now, &input(60)),
        vec!["dev-1".to_string()]
    );
}

#[test]
fn a_keep_record_excludes_an_agent_until_it_expires() {
    let f = Fixture::new();
    let now = later();
    assert_eq!(f.core().keep_until("dev-1").unwrap(), None);
    let write = |until: &str| {
        let core = f.core();
        let kernel = core.kernel();
        let actor = kernel.internal_principal().unwrap();
        kernel
            .append_event(
                &actor,
                &context(core, OWNER),
                "agent_keep",
                "dev-1",
                "",
                "",
                "agent.keep",
                1,
                &json!({"until": until, "by": "operator"}),
            )
            .unwrap();
    };
    write(&capstan_ledger::iso_from_millis(now + DAY));
    assert_eq!(
        f.core().keep_until("dev-1").unwrap().as_deref(),
        Some(capstan_ledger::iso_from_millis(now + DAY).as_str())
    );
    assert!(candidate_ids(&f, now, &input(60)).is_empty());
    // The newest row wins: clearing writes an until that has passed.
    write(&capstan_ledger::iso_from_millis(now - 1000));
    assert_eq!(
        candidate_ids(&f, now, &input(60)),
        vec!["dev-1".to_string()]
    );
    // An expired keep lets a later time through: a keep ends at its until.
    write(&capstan_ledger::iso_from_millis(now + 5 * 60_000));
    assert!(candidate_ids(&f, now, &input(60)).is_empty());
    assert_eq!(
        candidate_ids(&f, now + 6 * 60_000, &input(60)),
        vec!["dev-1".to_string()]
    );
}

fn events_of(
    f: &Fixture,
    entity_type: &str,
) -> Vec<(String, Option<String>, Option<String>, Value)> {
    let db = rusqlite::Connection::open(f.database()).unwrap();
    let mut stmt = db
        .prepare("SELECT entity_id, from_state, to_state, payload_json FROM controller_events WHERE entity_type = ? ORDER BY sequence")
        .unwrap();
    stmt.query_map([entity_type], |r| {
        let text: String = r.get(3)?;
        Ok((
            r.get(0)?,
            r.get(1)?,
            r.get(2)?,
            serde_json::from_str(&text).unwrap(),
        ))
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

#[test]
fn the_warning_and_the_release_are_recorded_in_the_ledger() {
    let f = Fixture::new();
    assert!(f.core().last_idle_warning("dev-1").unwrap().is_none());
    let core = f.core();
    core.record_idle_warning(
        &context(core, OWNER),
        &json!({"agentId": "dev-1", "role": "developer", "idleSince": "2026-01-01T00:00:00.000Z", "releaseAt": "2026-01-01T02:00:00.000Z"}),
    )
    .unwrap();
    let warning = f.core().last_idle_warning("dev-1").unwrap().unwrap();
    assert_eq!(warning["idleSince"], "2026-01-01T00:00:00.000Z");
    assert_eq!(warning["releaseAt"], "2026-01-01T02:00:00.000Z");
    assert!(warning["warnedAt"].is_string());
    let rows = events_of(&f, "agent_idle_warning");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "dev-1");

    let core = f.core();
    core.record_auto_release(
        &context(core, OWNER),
        &json!({"agentId": "dev-1", "idleSince": "2026-01-01T00:00:00.000Z", "idleMinutes": 120,
                "warnedAt": "2026-01-01T01:50:00.000Z", "keptBranches": ["feat/x"]}),
    )
    .unwrap();
    let rows = events_of(&f, "agent_auto_release");
    assert_eq!(rows.len(), 1);
    let release = &rows[0];
    assert_eq!(release.0, "dev-1");
    assert!(release.2.is_none());
    // The record is no second end of the agent.
    assert!(events_of(&f, "agent")
        .iter()
        .all(|(_, _, to, _)| to.as_deref() != Some("ended")));
    let details = &release.3["details"];
    assert_eq!(details["idleSince"], "2026-01-01T00:00:00.000Z");
    assert_eq!(details["idleMinutes"], 120);
    assert_eq!(details["warnedAt"], "2026-01-01T01:50:00.000Z");
    assert_eq!(details["keptBranches"], json!(["feat/x"]));
    assert_eq!(details["branch"], "feat/x");
}

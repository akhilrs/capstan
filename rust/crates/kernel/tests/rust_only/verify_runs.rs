//! The verify runs of migration 0042: queueing and its deduplication, the claim limit, finishing with its notices, the
//! interruption of a restart, the gates and the review brief.

use crate::fixture::{context, step, Fixture, OWNER};
use capstan_kernel::helpers::{review_task, ReviewBrief, VerifySummary};
use capstan_kernel::records::ReviewRow;
use capstan_kernel::Core;
use serde_json::{json, Value};

const COMMIT: &str = "1111111111111111111111111111111111111111";
const COMMANDS: &str = "cmd-sha-a";

/// An accepted report of dev-1 at `commit`, written to the ledger directly (the git evidence is not the point here).
fn report(f: &Fixture, commit: &str) -> String {
    let db = rusqlite::Connection::open(f.database()).unwrap();
    let (project, actor): (String, String) = db
        .query_row(
            "SELECT project_id, actor_id FROM agents WHERE agent_id = 'dev-1'",
            [],
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
         VALUES (?, ?, ?, 'dev-1', 1, ?, ?, 'b', 'did the work', 'accepted', '{}', '2026-01-01T00:00:00.000Z')",
        rusqlite::params![project, id, sequence, actor, commit],
    )
    .unwrap();
    id
}

fn queue(f: &Fixture, subject: &str, commands_sha: &str) -> Value {
    let core = f.core();
    core.queue_verify_run(
        &context(core, &f.developer_credential),
        &json!({"subjectId": subject, "commands": ["make lint", "make test"], "commandsSha": commands_sha, "trigger": "manual"}),
    )
    .unwrap()
}

fn claim(core: &Core, run: &str, max: i64) -> bool {
    core.claim_verify_run(
        &context(core, OWNER),
        &json!({"runId": run, "scratchPath": "/tmp/s", "logPath": "/tmp/l", "maxParallel": max}),
    )
    .unwrap()["claimed"]
        .as_bool()
        .unwrap()
}

fn finish(core: &Core, run: &str, input: Value) -> Result<Value, capstan_kernel::KernelError> {
    let mut input = input;
    input["runId"] = json!(run);
    core.finish_verify_run(&context(core, OWNER), &input)
}

fn passed(core: &Core, run: &str) {
    assert!(claim(core, run, 4));
    finish(
        core,
        run,
        json!({"state": "passed", "exitCode": 0, "durationMs": 12_300, "outputTail": "ok"}),
    )
    .unwrap();
}

fn notices(f: &Fixture, agent: &str) -> Vec<String> {
    f.close_for_read(|db| {
        let mut statement = db
            .prepare("SELECT body FROM messages WHERE recipient_agent_id = ? AND body LIKE 'Verify run %' ORDER BY sequence")
            .unwrap();
        statement
            .query_map([agent], |r| r.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    })
}

impl Fixture {
    fn close_for_read<T>(&self, read: impl FnOnce(&rusqlite::Connection) -> T) -> T {
        let db = rusqlite::Connection::open(self.database()).unwrap();
        read(&db)
    }
}

#[test]
fn queueing_returns_the_active_run_for_the_same_commit_and_commands() {
    let f = Fixture::new();
    let report = report(&f, COMMIT);
    let first = queue(&f, &report, COMMANDS);
    assert_eq!(first["state"], "queued");
    assert_eq!(first["subjectKind"], "report");
    assert_eq!(first["commitSha"], COMMIT);
    assert_eq!(first["requestedBy"], "dev-1");
    assert_eq!(first["commands"], json!(["make lint", "make test"]));
    assert!(first.get("existing").is_none());
    let again = queue(&f, &report, COMMANDS);
    assert_eq!(again["runId"], first["runId"]);
    assert_eq!(again["existing"], true);
    let other = queue(&f, &report, "cmd-sha-b");
    assert_ne!(other["runId"], first["runId"]);
    // A finished run does not hold the slot.
    let run = first["runId"].as_str().unwrap();
    assert!(claim(f.core(), run, 4));
    finish(
        f.core(),
        run,
        json!({"state": "failed", "exitCode": 1, "failedStep": 2}),
    )
    .unwrap();
    assert_ne!(queue(&f, &report, COMMANDS)["runId"], first["runId"]);
}

#[test]
fn an_unknown_subject_and_an_integration_without_a_merged_commit_are_refused() {
    let f = Fixture::new();
    let core = f.core();
    let input = |subject: &str| json!({"subjectId": subject, "commands": ["x"], "commandsSha": "c", "trigger": "auto"});
    let ctx = || context(core, &f.developer_credential);
    assert!(core.queue_verify_run(&ctx(), &input("nothing")).is_err());
    let cases = [
        ("running", "NULL"),
        ("merged", &format!("'{}'", "3".repeat(40))),
    ];
    for (n, (state, head)) in cases.iter().enumerate() {
        let id = format!("integ-{n}");
        let db = rusqlite::Connection::open(f.database()).unwrap();
        let project: String = db
            .query_row("SELECT project_id FROM projects", [], |r| r.get(0))
            .unwrap();
        db.execute(
            &format!("INSERT INTO integrations(project_id, integration_id, sequence, base_sha, branch, requested_by, state, head_sha, created_at, completed_at)
              VALUES (?, ?, ?, ?, ?, 'pm-1', '{state}', {head}, '2026-01-01T00:00:00.000Z', {})",
              if *state == "running" { "NULL" } else { "'2026-01-01T00:00:00.000Z'" }),
            rusqlite::params![project, id, n as i64 + 1, "4".repeat(40), format!("capstan/integration/{id}")],
        )
        .unwrap();
        drop(db);
        let result = core.queue_verify_run(&ctx(), &input(&id));
        if *state == "running" {
            assert!(result.unwrap_err().message().contains("no merged commit"));
        } else {
            let run = result.unwrap();
            assert_eq!(run["subjectKind"], "integration");
            assert_eq!(run["commitSha"], "3".repeat(40));
        }
    }
}

#[test]
fn maxparallel_limits_the_running_runs() {
    let f = Fixture::new();
    let core = f.core();
    let a = queue(&f, &report(&f, COMMIT), COMMANDS);
    let b = queue(&f, &report(&f, &"5".repeat(40)), COMMANDS);
    let (a, b) = (a["runId"].as_str().unwrap(), b["runId"].as_str().unwrap());
    assert!(claim(core, a, 1));
    assert!(!claim(core, b, 1));
    assert!(!claim(core, a, 1), "a running run is not claimed twice");
    assert_eq!(
        core.next_queued_verify_runs(10).unwrap()["runs"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(claim(core, b, 2));
    assert!(core.next_queued_verify_runs(10).unwrap()["runs"]
        .as_array()
        .unwrap()
        .is_empty());
    core.record_verify_process(
        &context(core, OWNER),
        &json!({"runId": a, "pgid": 4242, "leaderStart": "123"}),
    )
    .unwrap();
    let error = core.record_verify_process(
        &context(core, OWNER),
        &json!({"runId": a, "pgid": 1, "leaderStart": "123"}),
    );
    assert_eq!(error.unwrap_err().name(), "TypeError");
}

#[test]
fn cancelling_a_queued_run_cancels_it_and_a_running_run_needs_an_abort() {
    let f = Fixture::new();
    let core = f.core();
    let queued = queue(&f, &report(&f, COMMIT), COMMANDS);
    let id = queued["runId"].as_str().unwrap();
    let cancel = || {
        core.cancel_verify_run(
            &context(core, &f.developer_credential),
            &json!({"runId": id}),
        )
    };
    let cancelled = cancel().unwrap();
    assert_eq!(cancelled["state"], "cancelled");
    assert_eq!(cancelled["needsAbort"], false);
    assert!(cancel().is_err());
    let running = queue(&f, &report(&f, &"6".repeat(40)), COMMANDS);
    let rid = running["runId"].as_str().unwrap();
    assert!(claim(core, rid, 4));
    let answer = core
        .cancel_verify_run(
            &context(core, &f.developer_credential),
            &json!({"runId": rid}),
        )
        .unwrap();
    assert_eq!(answer["needsAbort"], true);
    assert_eq!(answer["state"], "running");
}

#[test]
fn finishing_queues_one_notice_per_recipient_and_bounds_the_tail() {
    let f = Fixture::new();
    let core = f.core();
    let id = queue(&f, &report(&f, COMMIT), COMMANDS)["runId"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(claim(core, &id, 4));
    let long = format!("{}\nlast line", "x".repeat(20_000));
    let done = finish(core, &id, json!({"state": "failed", "exitCode": 3, "failedStep": 2, "durationMs": 61_000, "outputTail": long})).unwrap();
    assert_eq!(done["state"], "failed");
    assert_eq!(done["outputTruncated"], true);
    assert!(done["outputTail"].as_str().unwrap().len() <= 12288);
    assert!(done["outputTail"].as_str().unwrap().ends_with("last line"));
    for agent in ["pm-1", "dev-1"] {
        let got = notices(&f, agent);
        assert_eq!(got.len(), 1, "{agent}");
        assert!(
            got[0].starts_with(&format!("Verify run {id} for report ")),
            "{}",
            got[0]
        );
        assert!(
            got[0].contains(&format!(
                "at {COMMIT}: failed at step 2 (exit 3) after 1m01s"
            )),
            "{}",
            got[0]
        );
        assert!(got[0].contains("last line"));
        assert!(got[0].len() < 2500);
    }
    // A second finish is refused and queues nothing more.
    assert!(finish(core, &id, json!({"state": "passed", "exitCode": 0})).is_err());
    assert_eq!(notices(&f, "pm-1").len(), 1);
    let other = queue(&f, &report(&f, &"7".repeat(40)), COMMANDS)["runId"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(claim(core, &other, 4));
    assert!(finish(core, &other, json!({"state": "passed", "exitCode": 1})).is_err());
    finish(
        core,
        &other,
        json!({"state": "passed", "exitCode": 0, "durationMs": 850}),
    )
    .unwrap();
    assert!(notices(&f, "pm-1")[1].ends_with(": passed in 850ms"));
}

#[test]
fn a_plan_architect_hears_of_the_run_once() {
    let f = Fixture::new();
    let core = f.core();
    step(
        core,
        "syncRoleDefinitions",
        json!([[
            {"name": "pm", "kind": "PM", "host": "claude", "configHash": "b".repeat(64)},
            {"name": "developer", "kind": "Developer", "host": "claude", "configHash": "a".repeat(64)},
            {"name": "architect", "kind": "Developer", "host": "claude", "configHash": "e".repeat(64)},
        ]]),
    );
    step(
        core,
        "createSeat",
        json!([{"seatId": "seat-arch", "name": "seat-arch", "role": "Developer"}]),
    );
    let actor = step(
        core,
        "createActor",
        json!([{"displayName": "arch", "role": "Developer", "seatId": "seat-arch"}]),
    );
    step(
        core,
        "registerAgent",
        json!([{"agentId": "arch-1", "roleName": "architect", "seatId": "seat-arch", "actorId": actor["actorId"]}]),
    );
    let report = report(&f, COMMIT);
    {
        let db = rusqlite::Connection::open(f.database()).unwrap();
        let project: String = db
            .query_row("SELECT project_id FROM projects", [], |r| r.get(0))
            .unwrap();
        db.execute_batch("PRAGMA foreign_keys = OFF").unwrap();
        db.execute(
            "INSERT INTO plans(project_id, plan_id, sequence, title, tier, state, requested_by, architect_agent_id, current_revision, approved_revision, created_at, updated_at)
             VALUES (?, 'plan-x', 1, 't', 'normal', 'draft', 'pm', 'arch-1', 0, NULL, '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z')",
            [&project],
        ).unwrap();
        db.execute(
            "INSERT INTO plan_revisions(project_id, plan_id, revision, base_sha, body_json, body_sha, author_agent_id, author_actor_id, created_at)
             VALUES (?, 'plan-x', 1, ?, '{\"packages\":[{\"id\":\"p1\"}]}', ?, 'arch-1', 'actor-arch', '2026-01-01T00:00:00.000Z')",
            rusqlite::params![project, "a".repeat(40), "f".repeat(64)],
        )
        .unwrap();
        db.execute("UPDATE plans SET current_revision = 1", [])
            .unwrap();
        db.execute(
            "UPDATE plans SET state = 'approved', approved_revision = 1",
            [],
        )
        .unwrap();
        db.execute(
            "INSERT INTO plan_packages(project_id, plan_id, package_id, assignee_agent_id, assigned_at) VALUES (?, 'plan-x', 'p1', 'dev-1', '2026-01-01T00:00:00.000Z')",
            [&project],
        ).unwrap();
    }
    let id = queue(&f, &report, COMMANDS)["runId"]
        .as_str()
        .unwrap()
        .to_string();
    passed(core, &id);
    for agent in ["pm-1", "dev-1", "arch-1"] {
        assert_eq!(notices(&f, agent).len(), 1, "{agent}");
    }
}

#[test]
fn a_cancelled_running_run_finishes_as_interrupted_and_its_notice_says_so() {
    let f = Fixture::new();
    let core = f.core();
    let id = queue(&f, &report(&f, COMMIT), COMMANDS)["runId"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(claim(core, &id, 4));
    let done = finish(
        core,
        &id,
        json!({"state": "interrupted", "durationMs": 2_000, "outputTail": "cancelled"}),
    )
    .unwrap();
    assert_eq!(done["state"], "interrupted");
    let text = &notices(&f, "pm-1")[0];
    assert!(text.contains(": interrupted after 2.0s"), "{text}");
    let gate = core.verify_gate(COMMIT, COMMANDS).unwrap();
    assert_eq!(gate["ok"], false);
    assert_eq!(gate["code"], "verify_failed");
    let subjects = core
        .verify_gate_for_subjects(&[done["subjectId"].as_str().unwrap().to_string()], COMMANDS)
        .unwrap();
    assert_eq!(subjects["ok"], false);
}

#[test]
fn a_passed_notice_is_info_and_every_other_ending_is_action() {
    let f = Fixture::new();
    let core = f.core();
    let classes = |f: &Fixture| -> Vec<String> {
        f.close_for_read(|db| {
            let mut statement = db
                .prepare("SELECT class FROM messages WHERE recipient_agent_id = 'pm-1' AND body LIKE 'Verify run %' ORDER BY sequence")
                .unwrap();
            statement
                .query_map([], |r| r.get::<_, String>(0))
                .unwrap()
                .map(Result::unwrap)
                .collect()
        })
    };
    let first = queue(&f, &report(&f, COMMIT), COMMANDS)["runId"]
        .as_str()
        .unwrap()
        .to_string();
    passed(core, &first);
    let second = queue(&f, &report(&f, &"6".repeat(40)), COMMANDS)["runId"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(claim(core, &second, 4));
    finish(
        core,
        &second,
        json!({"state": "failed", "exitCode": 1, "failedStep": 1}),
    )
    .unwrap();
    assert_eq!(classes(&f), ["info", "action"]);
}

#[test]
fn a_restart_interrupts_running_runs_and_an_interrupted_run_never_passes_the_gate() {
    let f = Fixture::new();
    let core = f.core();
    let id = queue(&f, &report(&f, COMMIT), COMMANDS)["runId"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(claim(core, &id, 4));
    core.record_verify_process(
        &context(core, OWNER),
        &json!({"runId": id, "pgid": 777, "leaderStart": "99"}),
    )
    .unwrap();
    let queued = queue(&f, &report(&f, &"8".repeat(40)), COMMANDS)["runId"]
        .as_str()
        .unwrap()
        .to_string();
    let answer = core
        .interrupt_active_verify_runs(&context(core, OWNER))
        .unwrap();
    assert_eq!(
        answer["runs"],
        json!([{"runId": id, "pgid": 777, "leaderStart": "99", "scratchPath": "/tmp/s"}])
    );
    assert_eq!(core.verify_run(&id).unwrap()["state"], "interrupted");
    assert_eq!(core.verify_run(&queued).unwrap()["state"], "queued");
    let gate = core.verify_gate(COMMIT, COMMANDS).unwrap();
    assert_eq!(gate["ok"], false);
    assert_eq!(gate["code"], "verify_failed");
    core.clear_verify_orphan(&context(core, OWNER), &json!({"runId": id}))
        .unwrap();
}

#[test]
fn the_gate_tells_missing_running_failed_stale_and_passed_apart() {
    let f = Fixture::new();
    let core = f.core();
    let gate = |commands: &str| core.verify_gate(COMMIT, commands).unwrap();
    let report = report(&f, COMMIT);
    assert_eq!(gate(COMMANDS)["code"], "verify_missing");
    let id = queue(&f, &report, COMMANDS)["runId"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(gate(COMMANDS)["code"], "verify_running");
    assert!(claim(core, &id, 4));
    assert_eq!(gate(COMMANDS)["code"], "verify_running");
    finish(core, &id, json!({"state": "timeout", "durationMs": 5})).unwrap();
    assert_eq!(gate(COMMANDS)["code"], "verify_failed");
    assert_eq!(gate(COMMANDS)["run"]["state"], "timeout");
    let again = queue(&f, &report, COMMANDS)["runId"]
        .as_str()
        .unwrap()
        .to_string();
    passed(core, &again);
    assert_eq!(gate(COMMANDS)["ok"], true);
    assert_eq!(gate(COMMANDS)["run"]["runId"], again);
    assert_eq!(gate("cmd-sha-other")["code"], "verify_stale_commands");
    assert_eq!(gate("cmd-sha-other")["ok"], false);
    let cancelled = queue(&f, &report, "cmd-sha-c")["runId"]
        .as_str()
        .unwrap()
        .to_string();
    core.cancel_verify_run(&context(core, OWNER), &json!({"runId": cancelled}))
        .unwrap();
    assert_eq!(gate("cmd-sha-c")["code"], "verify_failed");
}

#[test]
fn the_gate_for_subjects_has_one_failure_per_subject_and_the_error_names_them() {
    let f = Fixture::new();
    let core = f.core();
    let (a, b, c) = (
        report(&f, COMMIT),
        report(&f, &"9".repeat(40)),
        report(&f, &"a".repeat(40)),
    );
    passed(core, queue(&f, &a, COMMANDS)["runId"].as_str().unwrap());
    let running = queue(&f, &b, COMMANDS)["runId"]
        .as_str()
        .unwrap()
        .to_string();
    let ids = [a.clone(), b.clone(), c.clone()];
    let answer = core.verify_gate_for_subjects(&ids, COMMANDS).unwrap();
    assert_eq!(answer["ok"], false);
    let failures = answer["failures"].as_array().unwrap();
    assert_eq!(failures.len(), 2);
    assert_eq!(failures[0]["subjectId"], b.as_str());
    assert_eq!(failures[0]["code"], "verify_running");
    assert_eq!(failures[0]["runId"], running.as_str());
    assert_eq!(failures[1]["code"], "verify_missing");
    assert_eq!(failures[1]["runId"], Value::Null);
    let error = capstan_kernel::helpers::verify_required_error(failures);
    let text = error.message();
    assert!(text.starts_with("verify_required: "), "{text}");
    for id in [&b, &c] {
        assert!(text.contains(&format!("cstan verify {id}")), "{text}");
    }
    assert!(!text.contains(&format!("cstan verify {a}")));
    assert_eq!(
        core.verify_gate_for_subjects(&[a], COMMANDS).unwrap(),
        json!({"ok": true, "failures": []})
    );
    assert!(core
        .verify_gate_for_subjects(&["nothing".to_string()], COMMANDS)
        .is_err());
}

#[test]
fn status_is_empty_without_rows_and_lists_active_and_recent_runs_without_output() {
    let f = Fixture::new();
    let core = f.core();
    assert_eq!(
        core.verify_status().unwrap(),
        json!({"active": [], "recent": []})
    );
    let report = report(&f, COMMIT);
    for n in 0..7 {
        let id = queue(&f, &report, &format!("c{n}"))["runId"]
            .as_str()
            .unwrap()
            .to_string();
        passed(core, &id);
    }
    queue(&f, &report, "pending");
    let status = core.verify_status().unwrap();
    assert_eq!(status["active"].as_array().unwrap().len(), 1);
    assert_eq!(status["recent"].as_array().unwrap().len(), 5);
    assert!(status["recent"][0].get("outputTail").is_none());
    assert_eq!(
        core.verify_runs(Some(&report), 3).unwrap()["runs"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}

fn review_row() -> ReviewRow {
    ReviewRow {
        review_id: "rv".into(),
        sequence: 1,
        round: 1,
        subject_report_id: Some("rp".into()),
        subject_integration_id: None,
        subject_plan_id: None,
        subject_plan_revision: None,
        commit_sha: COMMIT.into(),
        base_sha: "2".repeat(40),
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

fn summary(state: &str) -> VerifySummary {
    VerifySummary {
        run_id: "run-1".into(),
        state: state.into(),
        commit_sha: COMMIT.into(),
        duration_ms: Some(12_300),
        failed_step: Some(2),
        exit_code: Some(7),
        tail_excerpt: format!("{}\nboom", "y\n".repeat(100)),
    }
}

#[test]
fn the_review_brief_adds_one_gate_line_and_the_default_brief_adds_none() {
    let row = review_row();
    let plain = review_task(&row, &[], None, &ReviewBrief::default());
    assert!(!plain.contains("Gate"));
    let with = |s: VerifySummary| {
        review_task(
            &row,
            &[],
            None,
            &ReviewBrief {
                verify: Some(s),
                ..ReviewBrief::default()
            },
        )
    };
    let passed = with(summary("passed"));
    assert!(passed.contains(&format!("Gate passed at {COMMIT} (run run-1, 12.3s): do not rerun the full gate; run only targeted checks.")));
    let failed = with(summary("failed"));
    assert!(failed.contains(&format!(
        "Gate failed at {COMMIT} (run run-1, step 2, exit 7). Tail: \""
    )));
    assert!(failed.contains("boom"));
    assert!(failed.len() < plain.len() + 2500, "the tail is bounded");
    assert!(with(summary("running")).contains(&format!("Gate running at {COMMIT} (run run-1).")));
    let mut missing = summary("missing");
    missing.run_id.clear();
    assert!(with(missing).contains(&format!("No gate run is recorded at {COMMIT}.")));
    // Only the one line differs.
    let lines: Vec<&str> = passed.lines().collect();
    let base: Vec<&str> = plain.lines().collect();
    assert_eq!(lines.len(), base.len() + 1);
}

#[test]
fn the_brief_summary_follows_the_latest_run_at_the_commit() {
    use capstan_kernel::areas::verify_runs::summary_at;
    let f = Fixture::new();
    let core = f.core();
    let report = report(&f, COMMIT);
    assert_eq!(
        summary_at(core.kernel(), COMMIT, COMMANDS).unwrap().state,
        "missing"
    );
    let id = queue(&f, &report, COMMANDS)["runId"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        summary_at(core.kernel(), COMMIT, COMMANDS).unwrap().state,
        "queued"
    );
    assert!(claim(core, &id, 4));
    finish(core, &id, json!({"state": "failed", "exitCode": 2, "failedStep": 1, "durationMs": 9, "outputTail": "a\nb\nboom"})).unwrap();
    let failed = summary_at(core.kernel(), COMMIT, COMMANDS).unwrap();
    assert_eq!(
        (failed.state.as_str(), failed.failed_step, failed.exit_code),
        ("failed", Some(1), Some(2))
    );
    assert!(failed.tail_excerpt.ends_with("boom"));
    // Other commands: the latest run at the commit is shown, not a pass.
    assert_eq!(
        summary_at(core.kernel(), COMMIT, "other").unwrap().state,
        "failed"
    );
}

//! The role limit of `registerAgent`, the successor and lost reads, `recordAgentLost` reasons and the architect rebinding
//! of `recordAgentReplaced`.

use crate::fixture::{context, initial, step, Fixture, OWNER};
use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::types::MutationContext;
use capstan_kernel::{Core, KernelError};
use serde_json::{json, Value};
use std::sync::{Arc, Barrier};

fn sync_roles(f: &Fixture) {
    step(
        f.core(),
        "syncRoleDefinitions",
        json!([[
            {"name": "pm", "kind": "PM", "host": "claude", "configHash": "b".repeat(64)},
            {"name": "developer", "kind": "Developer", "host": "claude", "configHash": "a".repeat(64)},
            {"name": "supervisor", "kind": "Supervisor", "host": "claude", "configHash": "c".repeat(64)},
            {"name": "limited", "kind": "Developer", "host": "claude", "configHash": "d".repeat(64)},
            {"name": "architect", "kind": "Developer", "host": "claude", "configHash": "e".repeat(64)},
        ]]),
    );
}

/// A seat and an actor of the kind for an agent that is registered later; returns the actor id.
fn prepare(core: &Core, agent: &str, kind: &str) -> String {
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
    actor["actorId"].as_str().unwrap().to_string()
}

fn input(agent: &str, role: &str, actor: &str, max_active: Option<i64>) -> Value {
    let mut input = json!({"agentId": agent, "roleName": role, "seatId": format!("seat-{agent}"), "actorId": actor});
    if let Some(limit) = max_active {
        input["maxActive"] = json!(limit);
    }
    input
}

fn register(
    f: &Fixture,
    agent: &str,
    role: &str,
    kind: &str,
    max_active: Option<i64>,
) -> Result<Value, KernelError> {
    let actor = prepare(f.core(), agent, kind);
    f.core().register_agent(
        &context(f.core(), OWNER),
        &input(agent, role, &actor, max_active),
    )
}

fn refused(result: Result<Value, KernelError>) -> String {
    let error = result.expect_err("the call is refused");
    assert_eq!(error.code(), Some("role_limit"), "{error}");
    assert!(error.is_mutation_conflict());
    error.message()
}

fn end(f: &Fixture, agent: &str) {
    step(f.core(), "endAgent", json!([agent, {}]));
}

#[test]
fn a_second_supervisor_is_refused_and_a_second_developer_is_allowed_without_maxactive() {
    let f = Fixture::new();
    sync_roles(&f);
    register(&f, "sup-1", "supervisor", "Supervisor", None).unwrap();
    let message = refused(register(&f, "sup-2", "supervisor", "Supervisor", None));
    assert_eq!(
        message,
        "role_limit: sup-1 is already active (max_active = 1)"
    );
    register(&f, "dev-2", "developer", "Developer", None).unwrap();
    register(&f, "dev-3", "developer", "Developer", None).unwrap();
}

#[test]
fn maxactive_overrides_the_supervisor_default_and_documents_that_callers_pass_it() {
    let f = Fixture::new();
    sync_roles(&f);
    register(&f, "sup-1", "supervisor", "Supervisor", None).unwrap();
    // Raw call, no maxActive: refused at 1. With the configured limit of 2 it succeeds at 1 and is refused at 2.
    let actor = prepare(f.core(), "sup-2", "Supervisor");
    let attempt = |max_active| {
        f.core().register_agent(
            &context(f.core(), OWNER),
            &input("sup-2", "supervisor", &actor, max_active),
        )
    };
    refused(attempt(None));
    attempt(Some(2)).unwrap();
    let message = refused(register(&f, "sup-3", "supervisor", "Supervisor", Some(2)));
    assert_eq!(
        message,
        "role_limit: sup-1, sup-2 are already active (max_active = 2)"
    );
    let error = register(&f, "sup-4", "supervisor", "Supervisor", Some(0)).unwrap_err();
    assert_eq!(error.name(), "TypeError");
}

#[test]
fn an_ended_agent_does_not_count_and_a_lost_active_one_does_and_is_named() {
    let f = Fixture::new();
    sync_roles(&f);
    register(&f, "lim-1", "limited", "Developer", Some(1)).unwrap();
    end(&f, "lim-1");
    register(&f, "lim-2", "limited", "Developer", Some(1)).unwrap();
    f.core()
        .record_agent_lost(&context(f.core(), OWNER), &json!({"agentId": "lim-2"}))
        .unwrap();
    let message = refused(register(&f, "lim-3", "limited", "Developer", Some(1)));
    assert_eq!(
        message,
        "role_limit: lim-2 is already active (max_active = 1); lim-2 is recorded lost, replace or release it"
    );
    assert_eq!(f.core().lost_agent_ids().unwrap(), json!(["lim-2"]));
}

#[test]
fn active_successor_finds_a_later_agent_of_the_same_role_only() {
    let f = Fixture::new();
    sync_roles(&f);
    let core = f.core();
    assert_eq!(core.active_successor("dev-1").unwrap(), Value::Null);
    end(&f, "dev-1");
    assert_eq!(core.active_successor("dev-1").unwrap(), Value::Null);
    std::thread::sleep(std::time::Duration::from_millis(5));
    register(&f, "sup-1", "supervisor", "Supervisor", None).unwrap();
    assert_eq!(core.active_successor("dev-1").unwrap(), Value::Null);
    register(&f, "dev-2", "developer", "Developer", None).unwrap();
    assert_eq!(
        core.active_successor("dev-1").unwrap(),
        json!({"agentId": "dev-2"})
    );
}

#[test]
fn active_successor_follows_the_chain_and_skips_an_ended_or_lost_agent() {
    let f = Fixture::new();
    sync_roles(&f);
    let core = f.core();
    register(&f, "lim-1", "limited", "Developer", Some(1)).unwrap();
    end(&f, "lim-1");
    register(&f, "lim-2", "limited", "Developer", Some(1)).unwrap();
    let replaced = |predecessor: &str, successor: &str| {
        core.record_agent_replaced(
            &context(core, OWNER),
            &json!({"predecessorId": predecessor, "successorId": successor}),
        )
        .unwrap();
    };
    replaced("lim-1", "lim-2");
    assert_eq!(
        core.active_successor("lim-1").unwrap(),
        json!({"agentId": "lim-2"})
    );
    assert_eq!(
        core.replacement_tip("lim-1").unwrap(),
        json!({"agentId": "lim-2"})
    );
    // Recorded lost: it holds its place but is not a successor.
    core.record_agent_lost(&context(core, OWNER), &json!({"agentId": "lim-2"}))
        .unwrap();
    assert_eq!(core.active_successor("lim-1").unwrap(), Value::Null);
    // Ended: not a successor either; the chain's tip is still it.
    end(&f, "lim-2");
    assert_eq!(core.active_successor("lim-1").unwrap(), Value::Null);
    assert_eq!(
        core.replacement_tip("lim-1").unwrap(),
        json!({"agentId": "lim-2"})
    );
    // A further replacement extends the chain, and the first agent resolves to its end.
    register(&f, "lim-3", "limited", "Developer", Some(1)).unwrap();
    replaced("lim-2", "lim-3");
    assert_eq!(
        core.active_successor("lim-1").unwrap(),
        json!({"agentId": "lim-3"})
    );
    assert_eq!(
        core.replacement_tip("lim-3").unwrap(),
        json!({"agentId": "lim-3"})
    );
}

#[test]
fn active_successor_without_a_record_skips_a_lost_candidate_and_finds_a_later_one() {
    let f = Fixture::new();
    sync_roles(&f);
    let core = f.core();
    end(&f, "dev-1");
    std::thread::sleep(std::time::Duration::from_millis(5));
    register(&f, "dev-2", "developer", "Developer", None).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(5));
    register(&f, "dev-3", "developer", "Developer", None).unwrap();
    core.record_agent_lost(&context(core, OWNER), &json!({"agentId": "dev-2"}))
        .unwrap();
    assert_eq!(
        core.active_successor("dev-1").unwrap(),
        json!({"agentId": "dev-3"})
    );
}

#[test]
fn process_exited_is_recorded_once_per_generation_with_a_notice_that_names_it() {
    let f = Fixture::new();
    let core = f.core();
    let record = || {
        core.record_agent_lost(
            &context(core, OWNER),
            &json!({"agentId": "dev-1", "reason": "process_exited"}),
        )
        .unwrap()
    };
    assert_eq!(record(), json!({"recorded": true}));
    assert_eq!(record(), json!({"recorded": false}));
    let pane_gone = core.record_agent_lost(&context(core, OWNER), &json!({"agentId": "dev-1"}));
    assert_eq!(pane_gone.unwrap(), json!({"recorded": false}));
    let bad = core.record_agent_lost(
        &context(core, OWNER),
        &json!({"agentId": "dev-1", "reason": "bored"}),
    );
    assert_eq!(bad.unwrap_err().name(), "TypeError");
    let database = rusqlite::Connection::open(f.database()).unwrap();
    let notices: Vec<String> = database
        .prepare(
            "SELECT body FROM messages WHERE recipient_agent_id = 'pm-1' AND body LIKE '%is lost%'",
        )
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(notices.len(), 1);
    assert!(notices[0].contains("process exited"), "{}", notices[0]);
}

#[test]
fn replacing_an_architect_rebinds_its_open_plans_so_the_verified_notice_follows() {
    let mut f = Fixture::new();
    sync_roles(&f);
    register(&f, "architect-32", "architect", "Developer", None).unwrap();
    register(&f, "architect-34", "architect", "Developer", None).unwrap();
    f.close();
    {
        let database = rusqlite::Connection::open(f.database()).unwrap();
        let now = "2026-01-01T00:00:00.000Z";
        database
            .execute_batch(&format!(
                "INSERT INTO plans(project_id, plan_id, sequence, title, tier, state, requested_by, architect_agent_id, current_revision, approved_revision, created_at, updated_at)
                   VALUES ('prustonly', 'plan-open', 1, 'open', 'normal', 'draft', 'pm-1', 'architect-32', 0, NULL, '{now}', '{now}');
                 INSERT INTO plans(project_id, plan_id, sequence, title, tier, state, requested_by, architect_agent_id, current_revision, approved_revision, created_at, updated_at)
                   VALUES ('prustonly', 'plan-old', 2, 'old', 'normal', 'superseded', 'pm-1', 'architect-32', 1, 1, '{now}', '{now}');
                 INSERT INTO plan_revisions(project_id, plan_id, revision, base_sha, body_json, body_sha, author_agent_id, author_actor_id, created_at)
                   SELECT 'prustonly', 'plan-open', 1, '{base}', '{{\"summary\":\"s\",\"packages\":[{{\"id\":\"core\"}}]}}', '{sha}', 'architect-32', actor_id, '{now}' FROM agents WHERE agent_id = 'architect-32';
                 UPDATE plans SET state = 'approved', current_revision = 1, approved_revision = 1 WHERE plan_id = 'plan-open';
                 INSERT INTO plan_packages(project_id, plan_id, package_id, assignee_agent_id, assigned_at)
                   VALUES ('prustonly', 'plan-open', 'core', 'dev-1', '{now}');
                 INSERT INTO agent_panes(project_id, agent_id, branch, base_sha, generation, created_at, updated_at)
                   VALUES ('prustonly', 'dev-1', 'feat/core', '{base}', 1, '{now}', '{now}');",
                base = "a".repeat(40),
                sha = "f".repeat(64)
            ))
            .unwrap();
    }
    f.reopen();
    end(&f, "architect-32");
    let core = f.core();
    let replaced = core
        .record_agent_replaced(
            &context(core, OWNER),
            &json!({"predecessorId": "architect-32", "successorId": "architect-34"}),
        )
        .unwrap();
    assert_eq!(replaced, json!({"recorded": true}));
    let architect_of = |plan: &str| -> String {
        let database = rusqlite::Connection::open(f.database()).unwrap();
        database
            .query_row(
                "SELECT architect_agent_id FROM plans WHERE plan_id = ?",
                [plan],
                |row| row.get(0),
            )
            .unwrap()
    };
    assert_eq!(architect_of("plan-open"), "architect-34");
    assert_eq!(
        architect_of("plan-old"),
        "architect-32",
        "history is untouched"
    );
    let details: String = rusqlite::Connection::open(f.database())
        .unwrap()
        .query_row(
            "SELECT payload_json FROM controller_events WHERE to_state = 'replaced'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(details.contains("\"plansRebound\":1"), "{details}");

    let evidence = json!({"generation": 1, "branch": "feat/core", "commitExists": true, "isAncestorOfTip": true,
        "isAncestorOfBase": false, "checkedAt": "2026-01-01T00:00:00.000Z", "baseSha": "a".repeat(40), "branchTip": "c".repeat(40)});
    let mut ctx = context(core, &f.developer_credential);
    ctx.credential = f.developer_credential.clone();
    core.record_agent_report(
        &ctx,
        &json!({"commitSha": "c".repeat(40), "summary": "done", "evidence": evidence}),
    )
    .unwrap();
    let recipient: String = rusqlite::Connection::open(f.database())
        .unwrap()
        .query_row(
            "SELECT recipient_agent_id FROM messages WHERE body LIKE '%Work package: plan-open/core%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(recipient, "architect-34");
}

#[test]
fn two_connections_racing_for_the_last_slot_never_both_win() {
    let f = Fixture::new();
    sync_roles(&f);
    for round in 0..50 {
        let a = format!("racer-{round}-a");
        let b = format!("racer-{round}-b");
        let actors = [
            prepare(f.core(), &a, "Developer"),
            prepare(f.core(), &b, "Developer"),
        ];
        let barrier = Arc::new(Barrier::new(2));
        let handles: Vec<_> = [a.clone(), b.clone()]
            .into_iter()
            .zip(actors)
            .enumerate()
            .map(|(n, (agent, actor))| {
                let (state, barrier) = (f.state.clone(), barrier.clone());
                std::thread::spawn(move || {
                    let core = Core::open_shared(
                        &state,
                        &initial(),
                        &KernelOptions::default(),
                        Box::new(capstan_kernel::SystemEnv),
                    )
                    .expect("the second connection opens");
                    barrier.wait();
                    // A caller that loses the version race refreshes and tries again, as the launcher does.
                    for attempt in 0..1000 {
                        let version = core.state_version().unwrap();
                        let ctx = MutationContext {
                            credential: OWNER.into(),
                            request_id: format!("race-{round}-{n}-{attempt}"),
                            idempotency_key: format!("race-key-{round}-{n}-{attempt}"),
                            expected_version: version,
                            input_revision: core.input_revision().unwrap(),
                        };
                        match core.register_agent(&ctx, &input(&agent, "limited", &actor, Some(1)))
                        {
                            Err(KernelError::StateVersionConflict(_)) => continue,
                            other => return other,
                        }
                    }
                    panic!("the version never settled");
                })
            })
            .collect();
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let won = results.iter().position(|r| r.is_ok());
        assert_eq!(
            results.iter().filter(|r| r.is_ok()).count(),
            1,
            "round {round}: {results:?}"
        );
        let loser = results.into_iter().find_map(Result::err).unwrap();
        assert_eq!(loser.code(), Some("role_limit"), "round {round}: {loser}");
        let active = f
            .core()
            .active_agents()
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .filter(|agent| agent["roleName"] == "limited")
            .count();
        assert_eq!(active, 1, "round {round}");
        end(&f, if won == Some(0) { &a } else { &b });
    }
}

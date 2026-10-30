//! The integration merge verb (`areas::remote_decisions`): its parser rows, the gates that decide whether a merge may go
//! ahead and in which order they answer, and the record of a merge that was made. The ledger is built through the kernel's
//! own mutations except for the integration, review and verify rows, which are written as the other tests of this suite do.

use crate::event_feed::{Rig, BASE};
use capstan_kernel::areas::remote_decisions::{
    decide, decide_with, decision_for_nonce, merge_gates, parse, record_merge, DecideInput,
    DecideOptions, Kind, MergeFacts, MergeGate, VerifyState, ERROR_CODES, MERGE_ERROR_CODES,
    PENDING_ERROR_CODES, PENDING_RESULT_FIELDS, PENDING_VERBS, RESULT_FIELDS, VERBS,
};
use capstan_kernel::areas::snapshot_feed::content_hash;
use capstan_kernel::errors::KernelError;
use serde_json::{json, Value};

const COMMANDS: &str = "commands-sha-a";
const HEAD: &str = "3333333333333333333333333333333333333333";

fn argv(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| word.to_string()).collect()
}

fn code_of(error: &KernelError) -> String {
    match error {
        KernelError::Controller(message) => message.split(':').next().unwrap().to_string(),
        other => panic!("not a refusal: {other}"),
    }
}

// ------------------------------------------------------------------------------------------------------------ parse

fn parse_merge(rest: &[&str]) -> Result<DecideInput, String> {
    let mut words = argv(&[
        "integration:int-1",
        "--expect",
        "0123456789abcdef",
        "--nonce",
        "nonce-0001",
    ]);
    words.extend(argv(rest));
    parse(&words).map_err(|r| r.code.to_string())
}

#[test]
fn parse_accepts_the_merge_verb_and_nothing_else_of_an_integration() {
    let input = parse_merge(&["merge"]).unwrap();
    assert_eq!(input.kind, Kind::Integration);
    assert_eq!(input.verb, "merge");
    assert_eq!(input.target, "int-1");
    assert_eq!(input.client, "remote");
    let client = parse(&argv(&[
        "integration:int-1",
        "--expect",
        "0123456789abcdef",
        "--nonce",
        "nonce-0001",
        "--client",
        "phone",
        "merge",
    ]))
    .unwrap();
    assert_eq!(client.client, "phone");
    // free text, another verb of the kind and a flag are refused
    for rest in [
        &["merge", "now please"][..],
        &["merge", "--reason", "x"],
        &["approve"],
        &["deny"],
        &[],
    ] {
        assert_eq!(
            parse_merge(rest).unwrap_err(),
            "invalid_request",
            "{rest:?}"
        );
    }
    // a missing --expect or --nonce
    assert_eq!(
        parse(&argv(&[
            "integration:int-1",
            "--nonce",
            "nonce-0001",
            "merge"
        ]))
        .unwrap_err()
        .code,
        "invalid_request"
    );
    assert_eq!(
        parse(&argv(&[
            "integration:int-1",
            "--expect",
            "0123456789abcdef",
            "merge"
        ]))
        .unwrap_err()
        .code,
        "invalid_request"
    );
    // the verb belongs to the integration kind only
    for request in [
        "op:p-1",
        "prompt:r-1",
        "agent:dev-1",
        "message:m-1",
        "run",
        "design:p-1/ui",
    ] {
        let words = argv(&[
            request,
            "--expect",
            "0123456789abcdef",
            "--nonce",
            "nonce-0001",
            "merge",
        ]);
        assert_eq!(
            parse(&words).unwrap_err().code,
            "invalid_request",
            "{request}"
        );
    }
}

// ----------------------------------------------------------------------------------------------------------- the lists

#[test]
fn the_pending_lists_are_apart_from_the_protocol_lists_and_hold_each_name_once() {
    let merge_codes = [
        "merge_unrecorded",
        "merge_disabled",
        "invalid_commit",
        "invalid_branch",
        "no_identity",
        "run_paused",
        "busy",
        "not_reviewed",
        "not_verified",
        "branch_moved",
        "unsafe_config",
        "wrong_branch",
        "dirty_tree",
        "merge_conflict",
        "git_too_old",
        "merge_failed",
        "git_unavailable",
    ];
    assert_eq!(MERGE_ERROR_CODES, merge_codes);
    assert_eq!(PENDING_ERROR_CODES, merge_codes);
    assert_eq!(PENDING_RESULT_FIELDS, ["merge"]);
    assert!(MERGE_ERROR_CODES
        .iter()
        .all(|code| !ERROR_CODES.contains(code)));
    assert!(PENDING_RESULT_FIELDS
        .iter()
        .all(|f| !RESULT_FIELDS.contains(f)));
    // every verb and code appears exactly once in the union, so this holds after PENDING is folded into the lists
    let verbs: Vec<(Kind, &str)> = VERBS
        .iter()
        .chain(PENDING_VERBS)
        .map(|row| (row.kind, row.verb))
        .collect();
    for (index, verb) in verbs.iter().enumerate() {
        assert!(!verbs[index + 1..].contains(verb), "{verb:?} twice");
    }
    assert_eq!(
        verbs
            .iter()
            .filter(|(kind, verb)| *kind == Kind::Integration && *verb == "merge")
            .count(),
        1
    );
    let codes: Vec<&str> = ERROR_CODES
        .iter()
        .chain(PENDING_ERROR_CODES)
        .copied()
        .collect();
    for (index, code) in codes.iter().enumerate() {
        assert!(!codes[index + 1..].contains(code), "{code} twice");
    }
    for code in merge_codes {
        assert_eq!(codes.iter().filter(|c| **c == code).count(), 1, "{code}");
    }
}

// --------------------------------------------------------------------------------------------------------------- rig

fn insert_integration(rig: &Rig, id: &str, sequence: i64, state: &str, head: Option<&str>) {
    let db = rig.db();
    let project: String = db
        .query_row("SELECT project_id FROM projects", [], |r| r.get(0))
        .unwrap();
    let done = (state != "running").then_some("2026-01-01T00:00:00.000Z");
    db.execute(
        "INSERT INTO integrations(project_id, integration_id, sequence, base_sha, branch, requested_by, state, head_sha, created_at, completed_at)
         VALUES (?, ?, ?, ?, ?, 'pm-1', ?, ?, '2026-01-01T00:00:00.000Z', ?)",
        rusqlite::params![project, id, sequence, BASE, format!("capstan/integration/{id}"), state, head, done],
    )
    .unwrap();
}

fn insert_review(rig: &Rig, integration: &str, round: i64, state: &str) {
    let db = rig.db();
    let (project, agent, actor): (String, String, String) = db
        .query_row(
            "SELECT project_id, agent_id, actor_id FROM agents WHERE agent_id = 'rev-1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    let sequence: i64 = db
        .query_row(
            "SELECT COALESCE(MAX(sequence), 0) + 1 FROM reviews",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let verdict = matches!(state, "passed" | "findings").then_some("text");
    let finished = (state != "started").then_some("2026-01-01T00:00:00.000Z");
    db.execute(
        "INSERT INTO reviews(project_id, review_id, sequence, round, subject_integration_id, commit_sha, base_sha,
           requested_by_actor_id, reviewer_role, reviewer_agent_id, reviewer_actor_id, state, verdict_text, created_at, completed_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'reviewer', ?, ?, ?, ?, '2026-01-01T00:00:00.000Z', ?)",
        rusqlite::params![
            project, format!("review-{sequence}"), sequence, round, integration, HEAD, BASE, actor, agent, actor, state,
            verdict, finished
        ],
    )
    .unwrap();
}

fn insert_verify(
    rig: &Rig,
    integration: &str,
    commit: &str,
    commands_sha: &str,
    state: &str,
) -> String {
    let db = rig.db();
    let project: String = db
        .query_row("SELECT project_id FROM projects", [], |r| r.get(0))
        .unwrap();
    let sequence: i64 = db
        .query_row(
            "SELECT COALESCE(MAX(sequence), 0) + 1 FROM verify_runs",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let id = format!("verify-{sequence}");
    db.execute(
        "INSERT INTO verify_runs(project_id, run_id, sequence, subject_integration_id, commit_sha, commands_json, commands_sha,
           trigger, requested_by_actor_id, state, queued_at, started_at, finished_at, exit_code, duration_ms)
         VALUES (?, ?, ?, ?, ?, '[\"make test\"]', ?, 'manual', 'x', ?, '2026-01-01T00:00:00.000Z',
                 '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:01.000Z', 0, 1000)",
        rusqlite::params![project, id, sequence, integration, commit, commands_sha, state],
    )
    .unwrap();
    id
}

/// A merged integration with a passed review and a passed verify run: every gate is green with `facts()`.
fn green(rig: &Rig, id: &str, sequence: i64) -> String {
    insert_integration(rig, id, sequence, "merged", Some(HEAD));
    insert_review(rig, id, 1, "passed");
    insert_verify(rig, id, HEAD, COMMANDS, "passed")
}

fn facts() -> MergeFacts {
    MergeFacts {
        merge_enabled: true,
        target_branch: Some("main".into()),
        branch_tip: Some(HEAD.into()),
        verify_commands_sha: Some(COMMANDS.into()),
        already_in_head: false,
        integration_running: false,
    }
}

fn request(id: &str) -> String {
    format!("integration:{id}")
}

fn hash_of(rig: &Rig, id: &str) -> String {
    content_hash(&rig.core, &request(id))
        .unwrap()
        .expect("the integration waits")
}

fn merge_input(id: &str, expect: &str, nonce: &str) -> DecideInput {
    parse(&argv(&[
        &request(id),
        "--expect",
        expect,
        "--nonce",
        nonce,
        "merge",
    ]))
    .unwrap()
}

fn gates(rig: &Rig, id: &str, nonce: &str, facts: &MergeFacts) -> Result<MergeGate, KernelError> {
    let input = merge_input(id, &hash_of_or_dummy(rig, id), nonce);
    merge_gates(&rig.core, &rig.ctx("owner"), &input, facts)
}

fn hash_of_or_dummy(rig: &Rig, id: &str) -> String {
    content_hash(&rig.core, &request(id))
        .unwrap()
        .unwrap_or_else(|| "a".repeat(64))
}

fn gate_code(result: Result<MergeGate, KernelError>) -> String {
    code_of(&result.expect_err("a gate refuses"))
}

fn go(rig: &Rig, id: &str, nonce: &str, facts: &MergeFacts) -> MergeGate {
    gates(rig, id, nonce, facts).expect("every gate passes")
}

fn pause_run(rig: &Rig) {
    let hash = content_hash(&rig.core, "run").unwrap().unwrap();
    let input = parse(&argv(&[
        "run",
        "--expect",
        &hash,
        "--nonce",
        "pause-nonce-1",
        "pause",
    ]))
    .unwrap();
    decide(&rig.core, &rig.ctx("owner"), &input).unwrap();
}

/// An applied merge row of an integration that still waits (as a merge that was recorded before it was confirmed).
fn insert_merge_row(rig: &Rig, id: &str, hash: &str, nonce: &str) {
    let db = rig.db();
    let project: String = db
        .query_row("SELECT project_id FROM projects", [], |r| r.get(0))
        .unwrap();
    db.execute(
        "INSERT INTO remote_decisions(project_id, nonce, decision_id, request_id, content_hash, verb, args_json, client,
           outcome, result_json, message_id, decided_at)
         VALUES (?, ?, 'remote-0000000099', ?, ?, 'merge', '{}', 'remote', 'applied', '{}', NULL, '2026-01-01T00:00:00.000Z')",
        rusqlite::params![project, nonce, format!("integration:{id}"), hash],
    )
    .unwrap();
}

fn set_integration_state(rig: &Rig, id: &str, state: &str) {
    rig.db()
        .execute(
            "UPDATE integrations SET state = ? WHERE integration_id = ?",
            rusqlite::params![state, id],
        )
        .unwrap();
}

// ------------------------------------------------------------------------------------------------------------- gates

#[test]
fn decide_refuses_the_merge_verb() {
    let rig = Rig::new();
    green(&rig, "int-1", 1);
    let input = merge_input("int-1", &hash_of(&rig, "int-1"), "nonce-0001");
    let error = decide(&rig.core, &rig.ctx("owner"), &input).unwrap_err();
    assert_eq!(code_of(&error), "invalid_request");
    assert_eq!(decision_for_nonce(&rig.core, "nonce-0001").unwrap(), None);
}

#[test]
fn all_gates_green_go_with_the_head_and_the_verify_run() {
    let rig = Rig::new();
    let run = green(&rig, "int-1", 1);
    match go(&rig, "int-1", "nonce-0001", &facts()) {
        MergeGate::Go {
            integration_id,
            branch,
            head_sha,
            verify,
        } => {
            assert_eq!(integration_id, "int-1");
            assert_eq!(branch, "capstan/integration/int-1");
            assert_eq!(head_sha, HEAD);
            assert_eq!(verify, VerifyState::Passed { run_id: run });
        }
        other => panic!("{other:?}"),
    }
    let none = MergeFacts {
        verify_commands_sha: None,
        ..facts()
    };
    match go(&rig, "int-1", "nonce-0001", &none) {
        MergeGate::Go { verify, .. } => assert_eq!(verify, VerifyState::NotConfigured),
        other => panic!("{other:?}"),
    }
}

#[test]
fn each_gate_alone_gives_its_code() {
    // not_waiting: unknown id, and a confirmed one
    let rig = Rig::new();
    assert_eq!(
        gate_code(gates(&rig, "int-none", "nonce-0001", &facts())),
        "not_waiting"
    );
    green(&rig, "int-1", 1);
    set_integration_state(&rig, "int-1", "confirmed");
    assert_eq!(
        gate_code(gates(&rig, "int-1", "nonce-0001", &facts())),
        "not_waiting"
    );

    // hash_mismatch
    let rig = Rig::new();
    green(&rig, "int-1", 1);
    let wrong = format!("{}0", &hash_of(&rig, "int-1")[1..]);
    let wrong = if wrong == hash_of(&rig, "int-1") {
        "b".repeat(64)
    } else {
        wrong
    };
    let input = merge_input("int-1", &wrong, "nonce-0001");
    let result = merge_gates(&rig.core, &rig.ctx("owner"), &input, &facts());
    assert_eq!(gate_code(result), "hash_mismatch");

    // merge_disabled: off, and without a target branch
    for bad in [
        MergeFacts {
            merge_enabled: false,
            ..facts()
        },
        MergeFacts {
            target_branch: None,
            ..facts()
        },
    ] {
        assert_eq!(
            gate_code(gates(&rig, "int-1", "nonce-0001", &bad)),
            "merge_disabled"
        );
    }

    // busy: from the facts alone, and from a running integration of the ledger alone
    let busy = MergeFacts {
        integration_running: true,
        ..facts()
    };
    assert_eq!(gate_code(gates(&rig, "int-1", "nonce-0001", &busy)), "busy");
    insert_integration(&rig, "int-run", 2, "running", None);
    assert_eq!(
        gate_code(gates(&rig, "int-1", "nonce-0001", &facts())),
        "busy"
    );

    // run_paused
    let rig = Rig::new();
    green(&rig, "int-1", 1);
    pause_run(&rig);
    assert_eq!(
        gate_code(gates(&rig, "int-1", "nonce-0001", &facts())),
        "run_paused"
    );

    // not_reviewed: no review, findings, and an open review after a passed one
    for (reviews, label) in [
        (vec![], "none"),
        (vec!["findings"], "findings"),
        (vec!["passed", "started"], "open"),
    ] {
        let rig = Rig::new();
        insert_integration(&rig, "int-1", 1, "merged", Some(HEAD));
        for (round, state) in reviews.iter().enumerate() {
            insert_review(&rig, "int-1", round as i64 + 1, state);
        }
        insert_verify(&rig, "int-1", HEAD, COMMANDS, "passed");
        assert_eq!(
            gate_code(gates(&rig, "int-1", "nonce-0001", &facts())),
            "not_reviewed",
            "{label}"
        );
    }
    // a later pass after findings is reviewed
    let rig = Rig::new();
    insert_integration(&rig, "int-1", 1, "merged", Some(HEAD));
    insert_review(&rig, "int-1", 1, "findings");
    insert_review(&rig, "int-1", 2, "passed");
    insert_verify(&rig, "int-1", HEAD, COMMANDS, "passed");
    go(&rig, "int-1", "nonce-0001", &facts());

    // not_verified: no run, a failed run, a run under other commands
    for (state, commands) in [
        (None, ""),
        (Some("failed"), COMMANDS),
        (Some("passed"), "other-commands"),
    ] {
        let rig = Rig::new();
        insert_integration(&rig, "int-1", 1, "merged", Some(HEAD));
        insert_review(&rig, "int-1", 1, "passed");
        if let Some(state) = state {
            insert_verify(&rig, "int-1", HEAD, commands, state);
        }
        assert_eq!(
            gate_code(gates(&rig, "int-1", "nonce-0001", &facts())),
            "not_verified",
            "{state:?}"
        );
    }

    // branch_moved: another tip, and none
    let rig = Rig::new();
    green(&rig, "int-1", 1);
    for tip in [Some("4".repeat(40)), None] {
        let moved = MergeFacts {
            branch_tip: tip,
            ..facts()
        };
        assert_eq!(
            gate_code(gates(&rig, "int-1", "nonce-0001", &moved)),
            "branch_moved"
        );
    }
}

#[test]
fn the_earlier_gate_answers_when_two_fail() {
    // confirmed item and merge off: not_waiting
    let rig = Rig::new();
    green(&rig, "int-1", 1);
    set_integration_state(&rig, "int-1", "confirmed");
    let off = MergeFacts {
        merge_enabled: false,
        ..facts()
    };
    assert_eq!(
        gate_code(gates(&rig, "int-1", "nonce-0001", &off)),
        "not_waiting"
    );

    // an applied merge row of another nonce and a wrong hash: not_waiting
    let rig = Rig::new();
    green(&rig, "int-1", 1);
    let hash = hash_of(&rig, "int-1");
    insert_merge_row(&rig, "int-1", &hash, "nonce-0001");
    let other = merge_input("int-1", &"c".repeat(64), "nonce-0002");
    let result = merge_gates(&rig.core, &rig.ctx("owner"), &other, &facts());
    assert_eq!(gate_code(result), "not_waiting");

    // hash_mismatch + merge_disabled
    let rig = Rig::new();
    green(&rig, "int-1", 1);
    let input = merge_input("int-1", &"c".repeat(64), "nonce-0001");
    let result = merge_gates(&rig.core, &rig.ctx("owner"), &input, &off);
    assert_eq!(gate_code(result), "hash_mismatch");

    // merge_disabled + run_paused
    pause_run(&rig);
    assert_eq!(
        gate_code(gates(&rig, "int-1", "nonce-0002", &off)),
        "merge_disabled"
    );

    // run_paused + busy
    let busy = MergeFacts {
        integration_running: true,
        ..facts()
    };
    assert_eq!(
        gate_code(gates(&rig, "int-1", "nonce-0002", &busy)),
        "run_paused"
    );

    // busy + not_reviewed
    let rig = Rig::new();
    insert_integration(&rig, "int-1", 1, "merged", Some(HEAD));
    insert_verify(&rig, "int-1", HEAD, COMMANDS, "passed");
    assert_eq!(gate_code(gates(&rig, "int-1", "nonce-0001", &busy)), "busy");

    // not_reviewed + not_verified
    let rig = Rig::new();
    insert_integration(&rig, "int-1", 1, "merged", Some(HEAD));
    assert_eq!(
        gate_code(gates(&rig, "int-1", "nonce-0001", &facts())),
        "not_reviewed"
    );

    // not_verified + branch_moved
    let rig = Rig::new();
    insert_integration(&rig, "int-1", 1, "merged", Some(HEAD));
    insert_review(&rig, "int-1", 1, "passed");
    let moved = MergeFacts {
        branch_tip: Some("4".repeat(40)),
        ..facts()
    };
    assert_eq!(
        gate_code(gates(&rig, "int-1", "nonce-0001", &moved)),
        "not_verified"
    );
}

#[test]
fn already_in_head_skips_busy_and_branch_moved_only() {
    let rig = Rig::new();
    green(&rig, "int-1", 1);
    insert_integration(&rig, "int-run", 2, "running", None);
    let moved_and_busy = MergeFacts {
        already_in_head: true,
        integration_running: true,
        branch_tip: Some("4".repeat(40)),
        ..facts()
    };
    assert!(matches!(
        go(&rig, "int-1", "nonce-0001", &moved_and_busy),
        MergeGate::Go { .. }
    ));
    // without the flag both stop it
    let plain = MergeFacts {
        already_in_head: false,
        ..moved_and_busy.clone()
    };
    assert_eq!(
        gate_code(gates(&rig, "int-1", "nonce-0001", &plain)),
        "busy"
    );

    // an earlier failure still answers first
    pause_run(&rig);
    assert_eq!(
        gate_code(gates(&rig, "int-1", "nonce-0002", &moved_and_busy)),
        "run_paused"
    );

    // the other gates still apply
    let rig = Rig::new();
    green(&rig, "int-1", 1);
    let skip = MergeFacts {
        already_in_head: true,
        ..facts()
    };
    let hash = hash_of(&rig, "int-1");
    let wrong = merge_input("int-1", &"c".repeat(64), "nonce-0001");
    assert_eq!(
        gate_code(merge_gates(&rig.core, &rig.ctx("owner"), &wrong, &skip)),
        "hash_mismatch"
    );
    let off = MergeFacts {
        merge_enabled: false,
        ..skip.clone()
    };
    assert_eq!(
        gate_code(gates(&rig, "int-1", "nonce-0001", &off)),
        "merge_disabled"
    );
    let unverified = MergeFacts {
        verify_commands_sha: Some("other".into()),
        ..skip.clone()
    };
    assert_eq!(
        gate_code(gates(&rig, "int-1", "nonce-0001", &unverified)),
        "not_verified"
    );
    let stored = merge_input("int-1", &hash, "nonce-0003");
    record_merge_for(&rig, &stored);
    let again = merge_gates(&rig.core, &rig.ctx("owner"), &stored, &skip).unwrap();
    assert!(matches!(again, MergeGate::Repeat(_)));

    let rig = Rig::new();
    insert_integration(&rig, "int-1", 1, "merged", Some(HEAD));
    insert_verify(&rig, "int-1", HEAD, COMMANDS, "passed");
    assert_eq!(
        gate_code(gates(&rig, "int-1", "nonce-0001", &skip)),
        "not_reviewed"
    );
}

// ---------------------------------------------------------------------------------------------------------- the nonce

fn record_merge_for(rig: &Rig, input: &DecideInput) -> Value {
    record_merge(
        &rig.core,
        &rig.ctx("owner"),
        input,
        json!({"mergeSha": "5".repeat(40), "target": "main"}),
    )
    .unwrap()
}

#[test]
fn a_stored_nonce_repeats_for_the_same_request_and_is_reused_for_any_other() {
    let rig = Rig::new();
    green(&rig, "int-1", 1);
    green_second(&rig);
    let hash = hash_of(&rig, "int-1");
    let input = merge_input("int-1", &hash, "nonce-0001");
    let done = record_merge_for(&rig, &input);

    // every gate failing, the same call is still a repeat with the stored result
    let broken = MergeFacts {
        merge_enabled: false,
        target_branch: None,
        branch_tip: None,
        verify_commands_sha: Some("other".into()),
        already_in_head: false,
        integration_running: true,
    };
    match merge_gates(&rig.core, &rig.ctx("owner"), &input, &broken).unwrap() {
        MergeGate::Repeat(value) => {
            assert_eq!(value["repeat"], true);
            assert_eq!(value["decisionId"], done["decisionId"]);
            assert_eq!(value["merge"], done["merge"]);
        }
        other => panic!("{other:?}"),
    }

    // the nonce for another integration, id, verb or hash
    let other_id = merge_input("int-2", &hash_of(&rig, "int-2"), "nonce-0001");
    assert_eq!(
        gate_code(merge_gates(
            &rig.core,
            &rig.ctx("owner"),
            &other_id,
            &facts()
        )),
        "nonce_reused"
    );
    let other_hash = merge_input("int-1", &"c".repeat(64), "nonce-0001");
    assert_eq!(
        gate_code(merge_gates(
            &rig.core,
            &rig.ctx("owner"),
            &other_hash,
            &facts()
        )),
        "nonce_reused"
    );
    let other_kind = parse(&argv(&[
        "run",
        "--expect",
        &hash_run(&rig),
        "--nonce",
        "nonce-0001",
        "pause",
    ]))
    .unwrap();
    assert_eq!(
        gate_code(merge_gates(
            &rig.core,
            &rig.ctx("owner"),
            &other_kind,
            &facts()
        )),
        "invalid_request"
    );
    let other_verb = parse(&argv(&[
        "agent:dev-1",
        "--expect",
        &"c".repeat(12),
        "--nonce",
        "nonce-0001",
        "pause",
    ]))
    .unwrap();
    assert_eq!(
        code_of(&decide(&rig.core, &rig.ctx("owner"), &other_verb).unwrap_err()),
        "nonce_reused"
    );
}

fn green_second(rig: &Rig) {
    insert_integration(rig, "int-2", 2, "merged", Some(&"6".repeat(40)));
}

fn hash_run(rig: &Rig) -> String {
    content_hash(&rig.core, "run").unwrap().unwrap()
}

// -------------------------------------------------------------------------------------------------------- the record

fn count(rig: &Rig, sql: &str) -> i64 {
    rig.db().query_row(sql, [], |row| row.get(0)).unwrap()
}

#[test]
fn record_merge_writes_one_row_and_one_event_and_confirms() {
    let rig = Rig::new();
    green(&rig, "int-1", 1);
    let hash = hash_of(&rig, "int-1");
    let input = merge_input("int-1", &hash[..12], "nonce-0001");
    assert_eq!(decision_for_nonce(&rig.core, "nonce-0001").unwrap(), None);
    let head = rig.head();
    let result = record_merge_for(&rig, &input);
    assert_eq!(result["v"], 1);
    assert_eq!(result["outcome"], "applied");
    assert_eq!(result["kind"], "integration");
    assert_eq!(result["verb"], "merge");
    assert_eq!(result["repeat"], false);
    assert_eq!(result["requestId"], "integration:int-1");
    assert_eq!(result["contentHash"], hash);
    assert_eq!(result["merge"]["target"], "main");
    let id = result["decisionId"].as_str().unwrap().to_string();
    assert_eq!(id, "remote-0000000001");
    assert_eq!(
        decision_for_nonce(&rig.core, "nonce-0001").unwrap(),
        Some(id.clone())
    );
    assert_eq!(count(&rig, "SELECT count(*) FROM remote_decisions"), 1);
    let stored: String = rig
        .db()
        .query_row("SELECT result_json FROM remote_decisions", [], |r| r.get(0))
        .unwrap();
    assert!(stored.contains("\"merge\""));
    let events = rig.since(head);
    let decided: Vec<&Value> = events
        .iter()
        .filter(|e| e["type"] == "decision.remote")
        .collect();
    assert_eq!(decided.len(), 1, "{events:?}");
    assert_eq!(decided[0]["body"]["effects"][0]["key"], "integration:1");
    assert_eq!(
        count(&rig, "SELECT count(*) FROM integrations WHERE integration_id = 'int-1' AND state = 'confirmed'"),
        1
    );
}

#[test]
fn record_merge_counts_an_already_confirmed_integration_as_done() {
    let rig = Rig::new();
    green(&rig, "int-1", 1);
    let hash = hash_of(&rig, "int-1");
    set_integration_state(&rig, "int-1", "confirmed");
    let input = merge_input("int-1", &hash, "nonce-0001");
    let result = record_merge_for(&rig, &input);
    assert_eq!(result["contentHash"], hash);
    assert_eq!(count(&rig, "SELECT count(*) FROM remote_decisions"), 1);
    assert_eq!(
        count(
            &rig,
            "SELECT count(*) FROM integrations WHERE state = 'confirmed'"
        ),
        1
    );
}

// --------------------------------------------------------------------------------------------------------- the limit

fn forward_ops(rig: &Rig, count: usize, options: DecideOptions) -> Vec<Result<Value, KernelError>> {
    let proposal = rig.propose("git push origin main", "ship");
    let request = format!("op:{}", proposal["proposalId"].as_str().unwrap());
    let hash = content_hash(&rig.core, &request).unwrap().unwrap();
    (1..=count)
        .map(|n| {
            let input = parse(&argv(&[
                &request,
                "--expect",
                &hash[..12],
                "--nonce",
                &format!("nonce-{n:04}"),
                "approve",
            ]))
            .unwrap();
            decide_with(&rig.core, &rig.ctx("owner"), &input, options)
        })
        .collect()
}

#[test]
fn messages_per_hour_caps_the_messages_to_the_pm_and_the_default_is_thirty() {
    assert_eq!(DecideOptions::default().messages_per_hour, 30);
    let rig = Rig::new();
    let results = forward_ops(
        &rig,
        8,
        DecideOptions {
            messages_per_hour: 7,
        },
    );
    assert!(results[..7].iter().all(Result::is_ok));
    assert_eq!(code_of(results[7].as_ref().unwrap_err()), "limit_reached");
    assert_eq!(count(&rig, "SELECT count(*) FROM remote_decisions"), 7);

    // the default lets the same eight through
    let rig = Rig::new();
    let results = forward_ops(&rig, 8, DecideOptions::default());
    assert!(results.iter().all(Result::is_ok));

    // a decision made more than an hour ago no longer counts
    let rig = Rig::new();
    let options = DecideOptions {
        messages_per_hour: 2,
    };
    let results = forward_ops(&rig, 3, options);
    assert_eq!(code_of(results[2].as_ref().unwrap_err()), "limit_reached");
    rig.db()
        .execute(
            "UPDATE remote_decisions SET decided_at = '2020-01-01T00:00:00.000Z'",
            [],
        )
        .unwrap();
    let proposal = rig.propose("git push origin dev", "ship");
    let request = format!("op:{}", proposal["proposalId"].as_str().unwrap());
    let hash = content_hash(&rig.core, &request).unwrap().unwrap();
    let input = parse(&argv(&[
        &request,
        "--expect",
        &hash[..12],
        "--nonce",
        "nonce-9999",
        "approve",
    ]))
    .unwrap();
    decide_with(&rig.core, &rig.ctx("owner"), &input, options).unwrap();
}

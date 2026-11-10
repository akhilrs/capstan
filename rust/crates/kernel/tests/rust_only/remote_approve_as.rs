//! `op approve-as` (`areas::remote_decisions`): the user's choice for an Operator proposal, made in a remote client and
//! applied as the Operator role. The grants and windows it makes are the ones the PM's approval of the same choice makes.

use crate::event_feed::Rig;
use capstan_kernel::areas::remote_decisions::{
    decide_with, parse, DecideOptions, FloorCheck, OperatorDecideConfig,
};
use capstan_kernel::areas::snapshot_feed::{content_hash, waiting_page};
use capstan_kernel::errors::KernelError;
use serde_json::{json, Value};

fn argv(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| word.to_string()).collect()
}

fn clear() -> Option<FloorCheck> {
    Some(FloorCheck {
        verdict: "clear".into(),
        entry: None,
    })
}

fn config(floor_now: Option<FloorCheck>) -> DecideOptions {
    DecideOptions {
        operator: Some(OperatorDecideConfig {
            session_grant_max_minutes: 30,
            auto_window_max_minutes: 240,
            approval_floor: vec!["git-push".into()],
            window_tool_prompts: false,
            proposal_ttl_minutes: 100000.0,
            floor_now,
        }),
        ..DecideOptions::default()
    }
}

fn propose(rig: &Rig, command: &str, floor_check: Value) -> Value {
    let mut body = json!({"kind": "command", "command": command, "reason": "r", "maxPending": 20});
    if !floor_check.is_null() {
        body["floorCheck"] = floor_check;
    }
    rig.core
        .propose_operator_action(&rig.ctx("dev-1"), &body)
        .unwrap()
}

fn proposed(rig: &Rig, command: &str) -> Value {
    propose(rig, command, json!({"verdict": "clear"}))
}

fn request_of(proposal: &Value) -> String {
    format!("op:{}", proposal["proposalId"].as_str().unwrap())
}

fn hash_of(rig: &Rig, request: &str) -> String {
    content_hash(&rig.core, request).unwrap().expect("waits")
}

fn call(
    rig: &Rig,
    who: &str,
    request: &str,
    nonce: &str,
    rest: &[&str],
    options: DecideOptions,
) -> Result<Value, KernelError> {
    let hash = hash_of(rig, request);
    let mut words = argv(&[
        request,
        "--expect",
        &hash[..12],
        "--nonce",
        nonce,
        "--client",
        "My phone",
    ]);
    words.extend(argv(rest));
    let input = parse(&words).unwrap_or_else(|e| panic!("{words:?}: {e}"));
    decide_with(&rig.core, &rig.ctx(who), &input, options)
}

fn approve_as(rig: &Rig, request: &str, nonce: &str, rest: &[&str]) -> Result<Value, KernelError> {
    let mut all = vec!["approve-as"];
    all.extend(rest);
    call(rig, "owner", request, nonce, &all, config(clear()))
}

fn code_of(result: Result<Value, KernelError>) -> String {
    match result.expect_err("the call is refused") {
        KernelError::Controller(message) => message.split(':').next().unwrap().to_string(),
        other => panic!("not a refusal: {other}"),
    }
}

fn text(rig: &Rig, sql: &str) -> String {
    rig.db().query_row(sql, [], |row| row.get(0)).unwrap()
}

fn count(rig: &Rig, sql: &str) -> i64 {
    rig.db().query_row(sql, [], |row| row.get(0)).unwrap()
}

fn state(rig: &Rig, proposal: &Value) -> String {
    text(
        rig,
        &format!(
            "SELECT state FROM operator_proposals WHERE proposal_id = '{}'",
            proposal["proposalId"].as_str().unwrap()
        ),
    )
}

fn pm_bodies(rig: &Rig) -> Vec<(String, String)> {
    let db = rig.db();
    let mut statement = db
        .prepare(
            "SELECT class, body FROM messages WHERE recipient_agent_id = 'pm-1' ORDER BY sequence",
        )
        .unwrap();
    let rows = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    rows
}

// ------------------------------------------------------------------------------------------------------------ parse

fn parse_code(rest: &[&str]) -> Result<&'static str, String> {
    let mut words = argv(&[
        "op:op-1",
        "--expect",
        "0123456789ab",
        "--nonce",
        "nonce-0001",
    ]);
    words.extend(argv(rest));
    match parse(&words) {
        Ok(_) => Ok("ok"),
        Err(refusal) => Ok(refusal.code),
    }
}

#[test]
fn the_parser_takes_each_choice_and_refuses_the_rest() {
    for choice in ["once", "exact", "auto-hour", "auto-session"] {
        assert_eq!(parse_code(&["approve-as", choice]), Ok("ok"), "{choice}");
        assert_eq!(
            parse_code(&["approve-as", choice, "--prefix", "git log"]),
            Ok("invalid_request"),
            "{choice} with a prefix"
        );
    }
    assert_eq!(
        parse_code(&["approve-as", "prefix", "--prefix", "git log"]),
        Ok("ok")
    );
    assert_eq!(parse_code(&["approve-as", "prefix"]), Ok("invalid_request"));
    assert_eq!(
        parse_code(&["approve-as", "forever"]),
        Ok("invalid_request")
    );
    assert_eq!(parse_code(&["approve-as"]), Ok("invalid_request"));
    assert_eq!(
        parse_code(&["approve-as", "once", "--session", "exact"]),
        Ok("invalid_request")
    );
    assert_eq!(
        parse_code(&["approve-as", "--session"]),
        Ok("invalid_request")
    );
    // the forwarded approve keeps its terminal rule
    assert_eq!(
        parse_code(&["approve", "--session", "exact"]),
        Ok("needs_terminal")
    );
    assert_eq!(parse_code(&["approve"]), Ok("ok"));
}

// ------------------------------------------------------------------------------------------------------------ choices

#[test]
fn the_item_carries_the_choices_from_the_stored_row() {
    let rig = Rig::new();
    let checked = proposed(&rig, "git log --oneline -5");
    let unchecked = propose(&rig, "ls", Value::Null);
    let held = propose(
        &rig,
        "git status",
        json!({"verdict": "floor", "entry": "git-push"}),
    );
    let long = proposed(&rig, &"x".repeat(5000));
    let page = serde_json::to_value(waiting_page(&rig.core, None, 10, None).unwrap()).unwrap();
    let items = page["items"].as_array().unwrap();
    let find = |proposal: &Value| {
        items
            .iter()
            .find(|item| item["requestId"] == json!(request_of(proposal)))
            .unwrap()["approveChoices"]
            .clone()
    };
    let choices = find(&checked);
    assert_eq!(choices["limit"], Value::Null);
    assert_eq!(choices["exact"], true);
    assert_eq!(choices["prefixes"], json!(["git log", "git log --oneline"]));
    assert_eq!(find(&unchecked)["limit"], "not_checked");
    assert_eq!(find(&unchecked)["once"], true);
    assert_eq!(find(&held)["limit"], "floor");
    assert_eq!(find(&held)["floorEntry"], "git-push");
    assert_eq!(find(&long)["limit"], "truncated");
    // two snapshots of the same item are equal
    let again = serde_json::to_value(waiting_page(&rig.core, None, 10, None).unwrap()).unwrap();
    assert_eq!(page["items"], again["items"]);
}

#[test]
fn propose_stores_the_floor_check_and_the_generic_call_refuses_it() {
    let rig = Rig::new();
    let proposal = propose(
        &rig,
        "git status",
        json!({"verdict": "floor", "entry": "git-push"}),
    );
    let id = proposal["proposalId"].as_str().unwrap();
    assert_eq!(
        text(
            &rig,
            &format!("SELECT floor_check || '/' || floor_entry FROM operator_proposals WHERE proposal_id = '{id}'")
        ),
        "floor/git-push"
    );
    let plain = propose(&rig, "ls", Value::Null);
    let id = plain["proposalId"].as_str().unwrap();
    assert_eq!(
        count(
            &rig,
            &format!("SELECT count(*) FROM operator_proposals WHERE proposal_id = '{id}' AND floor_check IS NULL AND floor_entry IS NULL")
        ),
        1
    );
    // the dispatch used by every other caller refuses the field
    let mut args = vec![serde_json::to_value(rig.ctx("dev-1")).unwrap()];
    args.push(json!({"kind": "command", "command": "ls", "reason": "r", "maxPending": 20, "floorCheck": {"verdict": "clear"}}));
    assert!(rig.core.dispatch("proposeOperatorAction", &args).is_err());
    // a verdict that is not one of the three is refused
    let bad = rig.core.propose_operator_action(
        &rig.ctx("dev-1"),
        &json!({"kind": "command", "command": "ls", "reason": "r", "maxPending": 20, "floorCheck": {"verdict": "maybe"}}),
    );
    assert!(bad.is_err());
}

#[test]
fn a_ledger_with_proposals_migrates_and_reads_not_checked() {
    let mut rig = Rig::new();
    let proposal = propose(&rig, "ls", Value::Null);
    // roll the ledger back to before the migration, with the proposal in it
    rig.core.close();
    {
        let db = rig.db();
        db.execute_batch(
            "ALTER TABLE operator_proposals DROP COLUMN floor_entry;
             ALTER TABLE operator_proposals DROP COLUMN floor_check;
             DROP INDEX mutation_requests_by_created;
             DROP INDEX messages_by_body_head;
             DROP TRIGGER immutable_verify_runs_identity;
             DROP INDEX one_active_verify_run;
             ALTER TABLE verify_runs DROP COLUMN repo;
             DELETE FROM schema_migrations WHERE name LIKE '%ledger_planner_stats%' OR name LIKE '%operator_proposal_floor_check%' OR name LIKE '%model_overrides%' OR name LIKE '%package_landings%' OR name LIKE '%plan_archives%' OR name LIKE '%workspace_repos%';",
        )
        .unwrap();
    }
    rig.reopen("migrate");
    assert_eq!(
        count(
            &rig,
            "SELECT count(*) FROM operator_proposals WHERE floor_check IS NULL AND floor_entry IS NULL"
        ),
        1
    );
    let page = serde_json::to_value(waiting_page(&rig.core, None, 10, None).unwrap()).unwrap();
    let item = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["requestId"] == json!(request_of(&proposal)))
        .unwrap();
    assert_eq!(item["approveChoices"]["limit"], "not_checked");
    assert_eq!(item["approveChoices"]["once"], true);
}

// ------------------------------------------------------------------------------------------------------------ apply

#[test]
fn once_approves_without_a_grant() {
    let rig = Rig::new();
    let proposal = proposed(&rig, "git log --oneline");
    let request = request_of(&proposal);
    let result = approve_as(&rig, &request, "nonce-0001", &["once"]).unwrap();
    assert_eq!(result["outcome"], "applied");
    assert_eq!(result["verb"], "approve-as");
    assert_eq!(result["approval"]["choice"], "once");
    assert_eq!(result["approval"]["grantId"], Value::Null);
    assert_eq!(result["approval"]["windowId"], Value::Null);
    assert_eq!(state(&rig, &proposal), "approved");
    assert_eq!(count(&rig, "SELECT count(*) FROM operator_grants"), 0);
    // decided by the operator actor, and the record says it came from the client
    let operator = rig.ctx("owner");
    let _ = operator;
    assert_eq!(
        text(
            &rig,
            "SELECT role FROM actors WHERE actor_id = (SELECT decided_by_actor_id FROM operator_proposals)"
        ),
        "operator"
    );
    let record = rig
        .core
        .operator_proposal(proposal["proposalId"].as_str().unwrap())
        .unwrap();
    assert_eq!(record["decidedVia"]["client"], "My phone");
    assert_eq!(record["decidedVia"]["decisionId"], result["decisionId"]);
    // the decided event names the remote source
    let details = text(
        &rig,
        "SELECT payload_json FROM controller_events WHERE entity_type = 'operator_proposal' AND to_state = 'approved'",
    );
    let details: Value = serde_json::from_str(&details).unwrap();
    assert_eq!(details["details"]["source"], "remote");
    assert_eq!(details["details"]["client"], "My phone");
    assert_eq!(details["details"]["decisionId"], result["decisionId"]);
}

#[test]
fn exact_prefix_and_windows_make_the_rows_the_pm_path_makes() {
    let rig = Rig::new();
    let session = |kind: &str| json!({"kind": kind, "maxMinutes": 30});
    // exact
    let a = proposed(&rig, "git log --oneline -5");
    let r = approve_as(&rig, &request_of(&a), "nonce-0001", &["exact"]).unwrap();
    let grant = r["approval"]["grantId"].as_str().unwrap().to_string();
    assert!(r["approval"]["expiresAt"].is_string());
    let pm_exact = proposed(&rig, "git log --oneline -5");
    let by_pm = rig
        .core
        .decide_operator_proposal(
            &rig.ctx("pm-1"),
            &json!({
                "proposalId": pm_exact["proposalId"], "decision": "approve",
                "hash": pm_exact["commandSha"], "proposalTtlMinutes": 100000,
                "session": session("exact"),
            }),
        )
        .unwrap();
    let rows = |id: &str| {
        text(
            &rig,
            &format!("SELECT kind || '|' || text || '|' || command_sha || '|' || ((julianday(expires_at) - julianday(created_at)) * 1440) FROM operator_grants WHERE grant_id = '{id}'"),
        )
    };
    let mine = rows(&grant);
    let theirs = rows(by_pm["sessionGrant"]["grantId"].as_str().unwrap());
    assert_eq!(
        mine.split('|').take(3).collect::<Vec<_>>(),
        theirs.split('|').take(3).collect::<Vec<_>>()
    );
    let minutes = |row: &str| {
        row.split('|')
            .nth(3)
            .unwrap()
            .parse::<f64>()
            .unwrap()
            .round()
    };
    assert_eq!(minutes(&mine), 30.0);
    assert_eq!(minutes(&theirs), 30.0);
    // prefix
    let b = proposed(&rig, "git log --oneline -5");
    let r = approve_as(
        &rig,
        &request_of(&b),
        "nonce-0002",
        &["prefix", "--prefix", "git log"],
    )
    .unwrap();
    let grant = r["approval"]["grantId"].as_str().unwrap();
    assert_eq!(
        text(
            &rig,
            &format!("SELECT kind || '|' || text FROM operator_grants WHERE grant_id = '{grant}'")
        ),
        "prefix|git log"
    );
    // windows: hour is capped at 60, session at the configured cap
    let c = proposed(&rig, "git log --oneline -5");
    let r = approve_as(&rig, &request_of(&c), "nonce-0003", &["auto-hour"]).unwrap();
    let window = r["approval"]["windowId"].as_str().unwrap();
    assert_eq!(
        text(
            &rig,
            &format!(
                "SELECT kind || '|' || minutes FROM approval_windows WHERE window_id = '{window}'"
            )
        ),
        "hour|60"
    );
    assert_eq!(r["approval"]["grantId"], Value::Null);
    let d = proposed(&rig, "git log --oneline -5");
    let r = approve_as(&rig, &request_of(&d), "nonce-0004", &["auto-session"]).unwrap();
    let window = r["approval"]["windowId"].as_str().unwrap();
    assert_eq!(
        text(&rig, &format!("SELECT kind || '|' || minutes || '|' || created_by FROM approval_windows WHERE window_id = '{window}'")).split('|').take(2).collect::<Vec<_>>(),
        ["session", "240"]
    );
    for proposal in [&a, &b, &c, &d] {
        assert_eq!(state(&rig, proposal), "approved");
    }
    // the window and grant events carry the remote source
    let events = text(
        &rig,
        "SELECT group_concat(payload_json, ' ') FROM controller_events WHERE entity_type IN ('operator_grant', 'approval_window') AND payload_json LIKE '%My phone%'",
    );
    assert!(events.contains("operator.grant_created"), "{events}");
    assert!(events.contains("operator.window_opened"), "{events}");
    assert!(events.contains("remote"), "{events}");
}

#[test]
fn a_repeat_of_the_same_nonce_changes_nothing() {
    let rig = Rig::new();
    let proposal = proposed(&rig, "git log --oneline");
    let request = request_of(&proposal);
    let first = approve_as(&rig, &request, "nonce-0001", &["exact"]).unwrap();
    let grants = count(&rig, "SELECT count(*) FROM operator_grants");
    let rows = count(&rig, "SELECT count(*) FROM remote_decisions");
    let events = count(&rig, "SELECT count(*) FROM controller_events");
    // the item no longer waits, so the repeat is answered from its stored row
    let stored = decide_with(
        &rig.core,
        &rig.ctx("owner"),
        &parse(&argv(&[
            &request,
            "--expect",
            &first["contentHash"].as_str().unwrap()[..12],
            "--nonce",
            "nonce-0001",
            "--client",
            "My phone",
            "approve-as",
            "exact",
        ]))
        .unwrap(),
        config(clear()),
    )
    .unwrap();
    assert_eq!(stored["repeat"], true);
    assert_eq!(stored["approval"], first["approval"]);
    assert_eq!(count(&rig, "SELECT count(*) FROM operator_grants"), grants);
    assert_eq!(count(&rig, "SELECT count(*) FROM remote_decisions"), rows);
    assert_eq!(
        count(&rig, "SELECT count(*) FROM controller_events"),
        events
    );
}

// ------------------------------------------------------------------------------------------------------------ refusals

#[test]
fn what_is_not_offered_is_refused_and_changes_nothing() {
    let rig = Rig::new();
    let proposal = proposed(&rig, "git log --oneline -5");
    let request = request_of(&proposal);
    // a prefix that is not one of the offered ones (byte for byte)
    assert_eq!(
        code_of(approve_as(
            &rig,
            &request,
            "nonce-0001",
            &["prefix", "--prefix", "git"]
        )),
        "invalid_request"
    );
    assert_eq!(
        code_of(approve_as(
            &rig,
            &request,
            "nonce-0002",
            &["prefix", "--prefix", "git  log"]
        )),
        "invalid_request"
    );
    // the Operator is not enabled on this host
    assert_eq!(
        code_of(call(
            &rig,
            "owner",
            &request,
            "nonce-0003",
            &["approve-as", "once"],
            DecideOptions::default()
        )),
        "needs_terminal"
    );
    // a limited choice: the floor holds the command back now
    let held = Some(FloorCheck {
        verdict: "floor".into(),
        entry: Some("git-push".into()),
    });
    let refused = call(
        &rig,
        "owner",
        &request,
        "nonce-0004",
        &["approve-as", "exact"],
        config(held.clone()),
    );
    let message = match refused.as_ref().unwrap_err() {
        KernelError::Controller(message) => message.clone(),
        other => panic!("{other}"),
    };
    assert!(message.starts_with("needs_terminal"), "{message}");
    assert!(message.contains("git-push"), "{message}");
    // a proposal the floor never checked offers only once
    let unchecked = propose(&rig, "ls", Value::Null);
    let version = rig.core.state_version().unwrap();
    assert_eq!(
        code_of(call(
            &rig,
            "owner",
            &request_of(&unchecked),
            "nonce-0005",
            &["approve-as", "exact"],
            config(None)
        )),
        "needs_terminal"
    );
    assert_eq!(state(&rig, &proposal), "proposed");
    assert_eq!(state(&rig, &unchecked), "proposed");
    assert_eq!(count(&rig, "SELECT count(*) FROM operator_grants"), 0);
    assert_eq!(count(&rig, "SELECT count(*) FROM remote_decisions"), 0);
    assert_eq!(rig.core.state_version().unwrap(), version);
}

#[test]
fn an_expired_proposal_and_a_running_run_are_refused() {
    let mut rig = Rig::new();
    let proposal = proposed(&rig, "git log --oneline");
    let other = rig
        .core
        .propose_operator_action(
            &rig.ctx("dev-1"),
            &json!({"kind": "command", "command": "git status", "reason": "r", "maxPending": 20, "autoRule": "exact-rule"}),
        )
        .unwrap();
    assert_eq!(other["state"], "approved");
    let request = request_of(&proposal);
    // a ttl of zero minutes: the proposal has waited too long
    let mut options = config(clear());
    options.operator.as_mut().unwrap().proposal_ttl_minutes = 0.0;
    assert_eq!(
        code_of(call(
            &rig,
            "owner",
            &request,
            "nonce-0001",
            &["approve-as", "once"],
            options
        )),
        "not_waiting"
    );
    assert_eq!(state(&rig, &proposal), "proposed");
    // another proposal is running
    rig.tamper(
        &[],
        &format!(
            "UPDATE operator_proposals SET state = 'running' WHERE proposal_id = '{}'",
            other["proposalId"].as_str().unwrap()
        ),
        "running",
    );
    assert_eq!(
        code_of(approve_as(&rig, &request, "nonce-0002", &["once"])),
        "busy"
    );
    assert_eq!(state(&rig, &proposal), "proposed");
    assert_eq!(count(&rig, "SELECT count(*) FROM remote_decisions"), 0);
}

#[test]
fn only_the_operator_role_approves_as() {
    let rig = Rig::new();
    let proposal = proposed(&rig, "git log --oneline");
    let request = request_of(&proposal);
    for who in ["pm-1", "dev-2"] {
        let result = call(
            &rig,
            who,
            &request,
            "nonce-0001",
            &["approve-as", "once"],
            config(clear()),
        );
        assert!(
            matches!(result, Err(KernelError::Authorization(_))),
            "{who}: {result:?}"
        );
    }
    assert_eq!(state(&rig, &proposal), "proposed");
    assert_eq!(count(&rig, "SELECT count(*) FROM remote_decisions"), 0);
    // the operator role still cannot approve through `cstan op decide`
    let result = rig.core.decide_operator_proposal(
        &rig.ctx("owner"),
        &json!({
            "proposalId": proposal["proposalId"], "decision": "approve",
            "hash": proposal["commandSha"], "proposalTtlMinutes": 100000,
        }),
    );
    let message = result.unwrap_err().to_string();
    assert!(message.contains("approve_requires_pm"), "{message}");
}

// ------------------------------------------------------------------------------------------------------------ races

#[test]
fn the_first_decision_wins_in_both_orders() {
    let rig = Rig::new();
    // approve-as, then the PM
    let a = proposed(&rig, "git log --oneline");
    approve_as(&rig, &request_of(&a), "nonce-0001", &["once"]).unwrap();
    let late = rig.core.decide_operator_proposal(
        &rig.ctx("pm-1"),
        &json!({"proposalId": a["proposalId"], "decision": "approve", "hash": a["commandSha"], "proposalTtlMinutes": 100000}),
    );
    assert!(late.unwrap_err().to_string().contains("proposal_not_open"));
    // the PM, then approve-as
    let b = proposed(&rig, "git log --oneline");
    let request = request_of(&b);
    let hash = hash_of(&rig, &request);
    rig.decide(&b, "approve");
    let words = argv(&[
        &request,
        "--expect",
        &hash[..12],
        "--nonce",
        "nonce-0002",
        "approve-as",
        "once",
    ]);
    let result = decide_with(
        &rig.core,
        &rig.ctx("owner"),
        &parse(&words).unwrap(),
        config(clear()),
    );
    assert_eq!(code_of(result), "not_waiting");
    // a remote deny, then approve-as
    let c = proposed(&rig, "git log --oneline");
    let request = request_of(&c);
    let hash = hash_of(&rig, &request);
    call(
        &rig,
        "owner",
        &request,
        "nonce-0003",
        &["deny"],
        config(clear()),
    )
    .unwrap();
    let words = argv(&[
        &request,
        "--expect",
        &hash[..12],
        "--nonce",
        "nonce-0004",
        "approve-as",
        "once",
    ]);
    let result = decide_with(
        &rig.core,
        &rig.ctx("owner"),
        &parse(&words).unwrap(),
        config(clear()),
    );
    assert_eq!(code_of(result), "not_waiting");
    // approve-as, then a remote deny
    let d = proposed(&rig, "git log --oneline");
    let request = request_of(&d);
    let hash = hash_of(&rig, &request);
    approve_as(&rig, &request, "nonce-0005", &["once"]).unwrap();
    let words = argv(&[
        &request,
        "--expect",
        &hash[..12],
        "--nonce",
        "nonce-0006",
        "deny",
    ]);
    let result = decide_with(
        &rig.core,
        &rig.ctx("owner"),
        &parse(&words).unwrap(),
        config(clear()),
    );
    assert_eq!(code_of(result), "not_waiting");
}

#[test]
fn a_floor_entry_added_after_the_proposal_refuses_exact() {
    let rig = Rig::new();
    let proposal = proposed(&rig, "git log --oneline");
    let held = Some(FloorCheck {
        verdict: "floor".into(),
        entry: Some("git-log".into()),
    });
    let result = call(
        &rig,
        "owner",
        &request_of(&proposal),
        "nonce-0001",
        &["approve-as", "exact"],
        config(held),
    );
    match result.unwrap_err() {
        KernelError::Controller(message) => {
            assert!(message.starts_with("needs_terminal"), "{message}");
            assert!(message.contains("git-log"), "{message}");
        }
        other => panic!("{other}"),
    }
    assert_eq!(state(&rig, &proposal), "proposed");
}

// ------------------------------------------------------------------------------------------------------------ messages

#[test]
fn the_pm_is_told_once_and_open_forwards_are_superseded() {
    let rig = Rig::new();
    let proposal = proposed(&rig, "git log --oneline");
    let request = request_of(&proposal);
    let id = proposal["proposalId"].as_str().unwrap();
    // the forward of op approve: the new wording
    let forward = call(
        &rig,
        "owner",
        &request,
        "nonce-0001",
        &["approve"],
        config(clear()),
    )
    .unwrap();
    assert_eq!(forward["outcome"], "forwarded");
    let bodies = pm_bodies(&rig);
    let (class, body) = bodies.last().unwrap();
    assert_eq!(class, "action");
    assert!(
        body.starts_with(&format!(
            "The user chose approve once for {request} in a remote client; that client offers only this choice. Run: cstan op decide {id} approve --hash "
        )),
        "{body}"
    );
    assert!(body.contains("Do not ask the user again."), "{body}");
    assert!(body.contains("Untrusted remote-client data"), "{body}");
    // approve-as supersedes it and tells the PM the user already answered
    let before = pm_bodies(&rig).len();
    let applied = approve_as(&rig, &request, "nonce-0002", &["exact"]).unwrap();
    let grant = applied["approval"]["grantId"].as_str().unwrap();
    let bodies = pm_bodies(&rig);
    assert_eq!(bodies.len(), before + 2, "{bodies:?}");
    let told: Vec<&(String, String)> = bodies
        .iter()
        .filter(|(_, body)| body.contains("was approved from remote client"))
        .collect();
    assert_eq!(told.len(), 1);
    let (class, body) = told[0];
    assert_eq!(class, "info");
    assert!(
        body.starts_with(&format!(
            "Operator proposal {id} was approved from remote client My phone."
        )),
        "{body}"
    );
    assert!(body.contains("Choice: exact."), "{body}");
    assert!(body.contains(grant), "{body}");
    assert!(
        body.contains("do not ask again and do not run cstan op decide"),
        "{body}"
    );
    assert_eq!(
        count(&rig, "SELECT count(*) FROM remote_decisions WHERE outcome = 'forwarded' AND superseded_by IS NOT NULL"),
        1
    );
    assert!(bodies.iter().any(|(_, b)| b.contains("no longer valid")));
}

#[test]
fn a_remote_deny_tells_the_pm_too() {
    let rig = Rig::new();
    let proposal = proposed(&rig, "git log --oneline");
    let id = proposal["proposalId"].as_str().unwrap();
    call(
        &rig,
        "owner",
        &request_of(&proposal),
        "nonce-0001",
        &["deny"],
        config(clear()),
    )
    .unwrap();
    let bodies = pm_bodies(&rig);
    let (class, body) = bodies
        .iter()
        .find(|(_, body)| body.contains("was denied from remote client"))
        .expect("the PM is told");
    assert_eq!(class, "info");
    assert!(
        body.starts_with(&format!(
            "Operator proposal {id} was denied from remote client My phone."
        )),
        "{body}"
    );
    assert!(
        body.contains("do not ask again and do not run cstan op decide"),
        "{body}"
    );
}

//! The Rust command policy and restart coordinator against what Node decides on the same inputs
//! (rust/crates/operator/tests/parity, written by test/operator-parity-export.ts). Every fixture entry is compared; a
//! difference fails the test, so zero pending is the only passing state.

use capstan_operator::api::{GrantKind, OperatorError, ProposalKind, RestartCoordinator};
use capstan_operator::policy::*;
use capstan_operator::restart::{
    busy_snapshot, restart_notice_to_pm, restart_run_report, KnownGoodBuild,
    ProcessRestartCoordinator, RestartCoordinatorOptions,
};
use capstan_operator::restart_helper::RestartResult;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

fn fixture(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("parity")
        .join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).expect("fixture json")
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .expect("array")
        .iter()
        .map(|v| v.as_str().expect("string").to_string())
        .collect()
}

fn kind_of(name: &str) -> ProposalKind {
    match name {
        "command" => ProposalKind::Command,
        "restart" => ProposalKind::Restart,
        other => panic!("kind {other}"),
    }
}

fn text_json(result: Result<&str, TextRefusal>) -> Value {
    match result {
        Ok(_) => json!({"ok": true}),
        Err(refusal) => json!({"ok": false, "code": refusal.code()}),
    }
}

#[test]
fn the_word_lists_are_node_s() {
    let fixture = fixture("policy.json");
    assert_eq!(
        strings(&fixture["alwaysApproval"]),
        OPERATOR_ALWAYS_APPROVAL.map(String::from)
    );
    assert_eq!(
        strings(&fixture["dangerousLetters"]),
        OPERATOR_DANGEROUS_SHORT_LETTERS.map(String::from)
    );
    let expected: Vec<Value> = OPERATOR_AUTO_ALLOWLIST
        .iter()
        .map(|entry| {
            json!({
                "command": entry.command,
                "subcommand": entry.subcommand,
                "options": entry.options,
                "positionals": match entry.positionals {
                    Positionals::None => json!("none"),
                    Positionals::Paths => json!("paths"),
                    Positionals::Words(words) => json!(words),
                },
            })
        })
        .collect();
    assert_eq!(fixture["allowlist"], Value::Array(expected));
}

#[test]
fn every_command_is_classified_and_hashed_as_node_does() {
    let fixture = fixture("policy.json");
    let mut checked = 0;
    for case in fixture["commands"].as_array().unwrap() {
        let command = case["command"].as_str().unwrap();
        let label = format!("{command:?}");
        assert_eq!(
            text_json(normalize_command(command)),
            case["normalize"],
            "normalize {label}"
        );
        assert_eq!(
            text_json(normalize_reason(command)),
            case["reason"],
            "reason {label}"
        );
        let classification = classify_command(command);
        assert_eq!(
            json!(classification.simple),
            case["simple"],
            "simple {label}"
        );
        assert_eq!(
            json!(classification.tokens),
            case["tokens"],
            "tokens {label}"
        );
        assert_eq!(
            json!(classification.always_approval),
            case["alwaysApproval"],
            "approval {label}"
        );
        let tokens = tokenize(command);
        assert_eq!(json!(tokens.words), case["words"], "words {label}");
        let letters: Vec<String> = tokens.letters.iter().map(|c| c.to_string()).collect();
        assert_eq!(json!(letters), case["letters"], "letters {label}");
        assert_eq!(
            json!(capstan_config::auto_approve_rule_problem(command)),
            case["ruleProblem"],
            "rule problem {label}"
        );
        for kind in ["command", "restart"] {
            for force in [false, true] {
                let hash = command_hash(kind, command, force);
                assert_eq!(
                    json!(hash),
                    case["hashes"][format!("{kind}:{force}")],
                    "hash {label}"
                );
            }
        }
        assert_eq!(
            json!(hash_prefix(&command_hash("command", command, false))),
            case["hash12"]
        );
        checked += 1;
    }
    assert!(checked > 100, "{checked} commands");
}

#[test]
fn auto_approve_decisions_match() {
    let fixture = fixture("policy.json");
    let mut checked = 0;
    for set in fixture["auto"].as_array().unwrap() {
        let exact = strings(&set["exact"]);
        let prefix = strings(&set["prefix"]);
        for case in set["results"].as_array().unwrap() {
            let command = case["command"].as_str().unwrap();
            let verdict = auto_decision(
                kind_of(case["kind"].as_str().unwrap()),
                command,
                &exact,
                &prefix,
            );
            let got = json!({
                "kind": case["kind"], "command": command, "auto": verdict.auto,
                "rule": verdict.rule, "reason": verdict.reason,
            });
            assert_eq!(&got, case, "rules {exact:?} {prefix:?}");
            checked += 1;
        }
    }
    assert!(checked > 1000, "{checked} decisions");
}

#[test]
fn session_grants_match() {
    let fixture = fixture("policy.json");
    let mut checked = 0;
    for set in fixture["grants"].as_array().unwrap() {
        let grants: Vec<GrantRef> = set["grants"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| GrantRef {
                grant_id: g["grantId"].as_str().unwrap().into(),
                kind: match g["kind"].as_str().unwrap() {
                    "exact" => GrantKind::Exact,
                    _ => GrantKind::Prefix,
                },
                text: g["text"].as_str().unwrap().into(),
            })
            .collect();
        for case in set["results"].as_array().unwrap() {
            let command = case["command"].as_str().unwrap();
            let verdict =
                match_session_grant(kind_of(case["kind"].as_str().unwrap()), command, &grants);
            let got = match verdict {
                GrantVerdict::Matched { grant_id } => {
                    json!({"kind": case["kind"], "command": command, "matched": true, "grantId": grant_id})
                }
                GrantVerdict::Unmatched { reason } => {
                    json!({"kind": case["kind"], "command": command, "matched": false, "reason": reason})
                }
            };
            assert_eq!(&got, case);
            checked += 1;
        }
    }
    assert!(checked > 500, "{checked} grant decisions");
    for case in fixture["prefixProblems"].as_array().unwrap() {
        let prefix = case["prefix"].as_str().unwrap();
        assert_eq!(
            json!(prefix_grant_problem(prefix)),
            case["problem"],
            "{prefix:?}"
        );
    }
    for case in fixture["sessionRules"].as_array().unwrap() {
        let id = case["grantId"].as_str().unwrap();
        let rule = session_rule(id);
        assert_eq!(json!(rule), case["rule"]);
        assert_eq!(json!(session_rule_grant_id(Some(&rule))), case["back"]);
    }
    for case in fixture["otherRules"].as_array().unwrap() {
        assert_eq!(
            json!(session_rule_grant_id(case["rule"].as_str())),
            case["grantId"]
        );
    }
}

#[test]
fn busy_snapshots_reports_and_notices_match() {
    let fixture = fixture("restart.json");
    for case in fixture["busy"].as_array().unwrap() {
        let in_flight = case["inFlight"].as_i64().unwrap_or(0);
        assert_eq!(
            json!(busy_snapshot(case, in_flight)),
            case["busy"],
            "{case}"
        );
    }
    for case in fixture["reports"].as_array().unwrap() {
        let result = RestartResult::from_json(&case["result"]).expect("a result");
        let report = restart_run_report(&result);
        assert_eq!(
            json!({
                "status": report.status.as_str(),
                "exitCode": report.exit_code,
                "outputTail": report.output_tail,
            }),
            case["report"],
            "{}",
            case["result"]
        );
        assert_eq!(
            json!(restart_notice_to_pm("p-1", &result)),
            case["notice"],
            "{}",
            case["result"]
        );
    }
}

#[test]
fn the_coordinator_refuses_with_nodes_texts() {
    let fixture = fixture("restart.json");
    let scratch = tempfile::tempdir().unwrap();
    let state_dir = scratch.path().join("state");
    std::fs::create_dir_all(&state_dir).unwrap();
    let notices = Arc::new(Mutex::new(Vec::<String>::new()));
    let make = |busy: Vec<String>| {
        let sink = notices.clone();
        ProcessRestartCoordinator::new(RestartCoordinatorOptions {
            state_dir: state_dir.clone(),
            project_root: scratch.path().to_path_buf(),
            binary_path: scratch.path().join("cstan"),
            node: "node".into(),
            argv: vec![],
            socket_path: state_dir.join("control.sock"),
            pid_path: state_dir.join("daemon.pid"),
            log_path: state_dir.join("daemon.log"),
            credential_file: scratch.path().join("operator.key"),
            health_timeout_seconds: 1,
            idle_wait_seconds: 0,
            busy: Box::new(move || busy.clone()),
            notify_pm: Box::new(move |body, action| {
                sink.lock().unwrap().push(format!(
                    "{}: {body}",
                    if action { "action" } else { "info" }
                ))
            }),
            request_stop: Box::new(|| {}),
            pid: None,
            timing: None,
            idle_poll_ms: None,
            now_ms: None,
            sleep: None,
            log: None,
        })
    };
    let proposal = json!({"proposalId": "p-1", "forceRestart": false});
    let outcome = |name: &str, result: Result<(), OperatorError>| -> Value {
        let notices = std::mem::take(&mut *notices.lock().unwrap());
        match result {
            Ok(()) => json!({"name": name, "refused": false, "notices": notices}),
            Err(e) => {
                json!({"name": name, "refused": true, "code": e.code, "message": e.message, "notices": notices})
            }
        }
    };
    let mut got = Vec::new();
    got.push(outcome(
        "preflight without known-good",
        make(vec![]).preflight().map(|_| ()),
    ));
    got.push(outcome(
        "run without known-good",
        make(vec![]).run(&proposal),
    ));
    std::fs::write(scratch.path().join("cstan"), "binary").unwrap();
    KnownGoodBuild::new(&state_dir)
        .snapshot(&scratch.path().join("cstan"), 1)
        .unwrap();
    got.push(outcome(
        "preflight with known-good",
        make(vec![]).preflight().map(|_| ()),
    ));
    got.push(outcome(
        "run busy",
        make(vec![
            "1 review(s) in state started".into(),
            "2 integration(s) not yet settled".into(),
        ])
        .run(&proposal),
    ));
    assert_eq!(Value::Array(got), fixture["refusals"]);
}

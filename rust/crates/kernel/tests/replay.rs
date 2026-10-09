//! The parity harness of the kernel: replays the sequences Node exported (tests/parity/*.json) and checks the plan-body,
//! text and notice helpers and the method list against the fixtures Node wrote.
//!
//! `CAPSTAN_KERNEL_PARITY_STRICT=1` makes a pending sequence (one that reached an area not ported yet) a failure.

mod common;

use capstan_kernel::helpers::*;
use capstan_kernel::plan_body::*;
use capstan_kernel::records::{AgentFindingRow, AgentReportRow, AgentRow, ReviewRow};
use common::replay::{first_difference, replay_directory, Report, Status};
use common::{crate_dir, fixture, parity_dir, read_json};
use serde_json::{json, Value};

fn strict() -> bool {
    std::env::var("CAPSTAN_KERNEL_PARITY_STRICT").is_ok_and(|v| v == "1")
}

fn summarize(reports: &[Report]) -> (usize, usize, Vec<String>) {
    let mut failures = Vec::new();
    let (mut passed, mut pending) = (0, 0);
    for report in reports {
        println!("{}", report.line());
        match &report.status {
            Status::Passed => passed += 1,
            Status::Pending(_) => {
                pending += 1;
                if strict() {
                    failures.push(report.line());
                }
            }
            Status::Failed(_) => failures.push(report.line()),
        }
    }
    println!(
        "{passed} passed, {pending} pending, {} failed",
        failures.len()
    );
    (passed, pending, failures)
}

#[test]
fn every_parity_group_replays() {
    let reports = replay_directory(&parity_dir());
    assert!(
        !reports.is_empty(),
        "no sequences found in {}",
        parity_dir().display()
    );
    let (passed, _, failures) = summarize(&reports);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert!(passed > 0, "no sequence passed");
}

#[test]
fn a_group_file_dropped_into_the_directory_is_picked_up() {
    let scratch = tempfile::tempdir().unwrap();
    let core = read_json(&parity_dir().join("core.json"));
    let first = core["sequences"][0].clone();
    for (file, group, name) in [
        ("alpha.json", "alpha", "first-copy"),
        ("omega.json", "omega", "second-copy"),
    ] {
        let mut sequence = first.clone();
        sequence["name"] = json!(name);
        let text =
            serde_json::to_string(&json!({"group": group, "format": 1, "sequences": [sequence]}))
                .unwrap();
        std::fs::write(scratch.path().join(file), text).unwrap();
    }
    std::fs::write(scratch.path().join("notes.txt"), "not a group").unwrap();
    let reports = replay_directory(scratch.path());
    let seen: Vec<String> = reports
        .iter()
        .map(|r| format!("{}/{}", r.group, r.sequence))
        .collect();
    assert_eq!(seen, ["alpha/first-copy", "omega/second-copy"]);
    assert!(
        reports.iter().all(|r| r.status == Status::Passed),
        "{reports:?}"
    );
}

#[test]
fn a_broken_expectation_is_reported_with_the_first_difference() {
    let scratch = tempfile::tempdir().unwrap();
    let core = read_json(&parity_dir().join("core.json"));
    let mut sequence = core["sequences"][0].clone();
    sequence["steps"][1]["result"] = json!("not-the-project");
    std::fs::write(
        scratch.path().join("broken.json"),
        serde_json::to_string(&json!({"group": "broken", "format": 1, "sequences": [sequence]}))
            .unwrap(),
    )
    .unwrap();
    let reports = replay_directory(scratch.path());
    match &reports[0].status {
        Status::Failed(detail) => assert!(
            detail.contains("outcome.result: expected \"not-the-project\" but got \"proj1\""),
            "{detail}"
        ),
        other => panic!("{other:?}"),
    }
}

// ----------------------------------------------------------------------------------------------- core method list

fn snake(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_uppercase() {
            out.push('_');
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

#[test]
fn every_public_controller_core_method_has_a_rust_counterpart() {
    let methods = read_json(
        &crate_dir()
            .join("tests")
            .join("common")
            .join("core-methods.json"),
    );
    let source = std::fs::read_to_string(crate_dir().join("src").join("core.rs")).unwrap();
    let defined: Vec<&str> = source
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix("pub fn "))
        .filter_map(|rest| rest.split(['(', '<']).next())
        .collect();
    let names: Vec<&str> = methods
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(names.len() >= 170, "only {} methods exported", names.len());
    let missing: Vec<String> = names
        .iter()
        .map(|n| snake(n))
        .filter(|n| !defined.contains(&n.as_str()))
        // `open` and `openReadOnly` are `Core::open` and `Core::open_read_only`.
        .collect();
    assert!(missing.is_empty(), "core.rs lacks: {missing:?}");
}

#[test]
fn every_core_method_is_dispatched_by_name() {
    // An operation the dispatcher does not know is a TypeError; one it knows but has not ported is `Unported`.
    let methods = read_json(
        &crate_dir()
            .join("tests")
            .join("common")
            .join("core-methods.json"),
    );
    let scratch = common::private_tempdir();
    let project: capstan_kernel::types::InitialProject = serde_json::from_value(json!({
        "projectId": "p1", "name": "n", "ownerCredential": "owner-credential-0123456789-abcdefghijklmnopqrstuvwxyz",
        "initialInputs": [
            {"kind": "project_config", "content": {}}, {"kind": "task_brief", "content": {}},
            {"kind": "acceptance_criteria", "content": ["a"]}, {"kind": "policy", "content": {}}, {"kind": "plan", "content": {}}]
    }))
    .unwrap();
    let core = capstan_kernel::Core::open(
        scratch.path(),
        &project,
        &capstan_kernel::KernelOptions::default(),
        Box::new(capstan_kernel::SeededEnv::new("methods")),
    )
    .unwrap();
    let skipped = ["open", "openReadOnly", "sweepFindings", "close"];
    for name in methods.as_array().unwrap().iter().filter_map(Value::as_str) {
        if skipped.contains(&name) {
            continue;
        }
        if let Err(error) = core.dispatch(name, &[]) {
            assert!(
                !error.message().starts_with("unknown operation"),
                "{name} is not dispatched"
            );
        }
    }
}

// -------------------------------------------------------------------------------------------------- plan bodies

fn plan_value(plan: &PlanBody) -> Value {
    serde_json::to_value(plan).unwrap()
}

#[test]
fn plan_bodies_parse_like_node() {
    let cases = fixture("plan-bodies/parse.json");
    let cases = cases.as_array().unwrap();
    assert!(cases.len() > 100);
    let mut codes = std::collections::BTreeSet::new();
    for case in cases {
        let input = case["input"].as_str().unwrap();
        let max = case["maxPackages"].as_u64().unwrap() as usize;
        let actual = match parse_plan_body(input, max) {
            Ok(plan) => json!({"ok": true, "plan": plan_value(&plan)}),
            Err(error) => {
                codes.insert(error.code.as_str());
                json!({"ok": false, "code": error.code.as_str(), "reason": error.reason})
            }
        };
        if let Some(diff) = first_difference(&case["result"], &actual, "result") {
            panic!("{}: {diff}", case["name"]);
        }
    }
    for code in [
        "too_large",
        "invalid_json",
        "invalid_shape",
        "too_many_packages",
        "duplicate_id",
        "unknown_dependency",
        "cycle",
        "overlap",
        "bad_order",
    ] {
        assert!(codes.contains(code), "no fixture reaches {code}");
    }
}

#[test]
fn package_naming_and_views_read_stored_bodies_like_node() {
    for case in fixture("plan-bodies/naming.json").as_array().unwrap() {
        let naming = package_naming(
            case["body"].as_str().unwrap(),
            case["packageId"].as_str().unwrap(),
        );
        let actual = serde_json::to_value(naming).unwrap();
        if let Some(diff) = first_difference(&case["result"], &actual, "result") {
            panic!("{}: {diff}", case["name"]);
        }
    }
    for case in fixture("plan-bodies/package-view.json").as_array().unwrap() {
        let view = package_of_body(
            case["body"].as_str().unwrap(),
            case["packageId"].as_str().unwrap(),
        );
        let actual = view.map_or(Value::Null, |v| v.to_value());
        if let Some(diff) = first_difference(&case["result"], &actual, "result") {
            panic!("{}: {diff}", case["name"]);
        }
    }
}

#[test]
fn work_package_messages_are_node_s_text() {
    for case in fixture("plan-bodies/work-package-message.json")
        .as_array()
        .unwrap()
    {
        let view = PackageView::from_value(&case["view"]).expect("a view");
        let unmet: Vec<String> = case["unmet"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        let message = work_package_message(
            case["planId"].as_str().unwrap(),
            case["packageId"].as_str().unwrap(),
            case["architect"].as_str().unwrap(),
            &view,
            &unmet,
        );
        assert_eq!(Value::String(message), case["result"], "{}", case["name"]);
    }
}

#[test]
fn squash_subjects_are_node_s_text() {
    let cases = fixture("plan-bodies/subjects.json");
    let cases = cases.as_array().unwrap();
    assert!(cases.len() > 400);
    for case in cases {
        let input = &case["input"];
        let subject = format_subject(
            input["type"].as_str().unwrap(),
            input["scope"].as_str(),
            input["breaking"].as_bool().unwrap_or(false),
            input["description"].as_str().unwrap(),
            case["max"].as_u64().unwrap() as usize,
        );
        assert_eq!(Value::String(subject), case["result"], "{input}");
    }
}

// -------------------------------------------------------------------------------------------------- text helpers

#[test]
fn text_helpers_match_node_on_the_corpus() {
    let text = fixture("kernel-text/text.json");
    for case in text["normalizeText"].as_array().unwrap() {
        assert_eq!(
            Value::String(normalize_text(case["input"].as_str().unwrap())),
            case["result"],
            "{}",
            case["input"]
        );
    }
    for case in text["oneLine"].as_array().unwrap() {
        let actual = one_line(
            case["input"].as_str().unwrap(),
            case["max"].as_u64().unwrap() as usize,
            case["empty"].as_str().unwrap(),
        );
        assert_eq!(
            Value::String(actual),
            case["result"],
            "{} / {}",
            case["input"],
            case["max"]
        );
    }
    for case in text["stripTerminalSequences"].as_array().unwrap() {
        assert_eq!(
            Value::String(strip_terminal_sequences(case["input"].as_str().unwrap())),
            case["result"],
            "{}",
            case["input"]
        );
    }
    for case in text["oneLineText"].as_array().unwrap() {
        let actual = one_line_text(
            case["input"].as_str().unwrap(),
            case["max"].as_u64().unwrap() as usize,
        );
        assert_eq!(
            Value::String(actual),
            case["result"],
            "{} / {}",
            case["input"],
            case["max"]
        );
    }
    for case in text["cutAtCharacters"].as_array().unwrap() {
        let actual = cut_at_characters(
            case["input"].as_str().unwrap(),
            case["limit"].as_u64().unwrap() as usize,
        );
        assert_eq!(
            Value::String(actual),
            case["result"],
            "{} / {}",
            case["input"],
            case["limit"]
        );
    }
}

fn outcome(result: Result<String, capstan_kernel::KernelError>) -> Value {
    match result {
        Ok(v) => json!({"ok": v}),
        Err(e) => json!({"error": {"name": e.name(), "message": e.message()}}),
    }
}

#[test]
fn text_validation_matches_node() {
    let validation = fixture("kernel-text/validation.json");
    for case in validation["safeText"].as_array().unwrap() {
        let actual = outcome(safe_text(
            &case["input"],
            "the field",
            case["max"].as_u64().unwrap() as usize,
            case["multiline"].as_bool().unwrap(),
        ));
        if let Some(diff) = first_difference(&case["result"], &actual, "result") {
            panic!("{}: {diff}", case["input"]);
        }
    }
    for case in validation["findingText"].as_array().unwrap() {
        let actual = outcome(finding_text(
            &case["input"],
            "evidence",
            case["max"].as_u64().unwrap() as usize,
        ));
        if let Some(diff) = first_difference(&case["result"], &actual, "result") {
            panic!("{}: {diff}", case["input"]);
        }
    }
}

fn opt(value: &Value) -> Option<String> {
    value.as_str().map(str::to_string)
}

fn report_row(v: &Value) -> AgentReportRow {
    AgentReportRow {
        report_id: v["report_id"].as_str().unwrap().into(),
        sequence: v["sequence"].as_i64().unwrap(),
        agent_id: v["agent_id"].as_str().unwrap().into(),
        generation: v["generation"].as_i64().unwrap(),
        actor_id: v["actor_id"].as_str().unwrap().into(),
        commit_sha: v["commit_sha"].as_str().unwrap().into(),
        branch: opt(&v["branch"]),
        summary: v["summary"].as_str().unwrap().into(),
        state: v["state"].as_str().unwrap().into(),
        reason: opt(&v["reason"]),
        evidence_json: v["evidence_json"].as_str().unwrap().into(),
        notified_message_id: opt(&v["notified_message_id"]),
        created_at: v["created_at"].as_str().unwrap().into(),
    }
}

fn review_row(v: &Value) -> ReviewRow {
    ReviewRow {
        review_id: v["review_id"].as_str().unwrap().into(),
        sequence: v["sequence"].as_i64().unwrap(),
        round: v["round"].as_i64().unwrap(),
        subject_report_id: opt(&v["subject_report_id"]),
        subject_integration_id: opt(&v["subject_integration_id"]),
        subject_plan_id: opt(&v["subject_plan_id"]),
        subject_plan_revision: v["subject_plan_revision"].as_i64(),
        commit_sha: v["commit_sha"].as_str().unwrap().into(),
        base_sha: v["base_sha"].as_str().unwrap().into(),
        author_agent_id: opt(&v["author_agent_id"]),
        author_actor_id: opt(&v["author_actor_id"]),
        requested_by_actor_id: v["requested_by_actor_id"].as_str().unwrap().into(),
        reviewer_role: v["reviewer_role"].as_str().unwrap().into(),
        reviewer_agent_id: v["reviewer_agent_id"].as_str().unwrap().into(),
        reviewer_actor_id: v["reviewer_actor_id"].as_str().unwrap().into(),
        state: v["state"].as_str().unwrap().into(),
        verdict_text: opt(&v["verdict_text"]),
        failure_reason: opt(&v["failure_reason"]),
        notified_message_id: opt(&v["notified_message_id"]),
        created_at: v["created_at"].as_str().unwrap().into(),
        completed_at: opt(&v["completed_at"]),
    }
}

fn finding_row(v: &Value) -> AgentFindingRow {
    AgentFindingRow {
        finding_id: v["finding_id"].as_str().unwrap().into(),
        sequence: v["sequence"].as_i64().unwrap(),
        target_agent_id: v["target_agent_id"].as_str().unwrap().into(),
        raised_by_agent_id: v["raised_by_agent_id"].as_str().unwrap().into(),
        raised_by_actor_id: v["raised_by_actor_id"].as_str().unwrap().into(),
        severity: v["severity"].as_str().unwrap().into(),
        evidence_text: v["evidence_text"].as_str().unwrap().into(),
        requested_correction: v["requested_correction"].as_str().unwrap().into(),
        resolution_condition: v["resolution_condition"].as_str().unwrap().into(),
        state: v["state"].as_str().unwrap().into(),
        interventions: v["interventions"].as_i64().unwrap(),
        state_reason: opt(&v["state_reason"]),
        created_at: v["created_at"].as_str().unwrap().into(),
        closed_at: opt(&v["closed_at"]),
    }
}

fn agent_row(v: &Value) -> AgentRow {
    AgentRow {
        agent_id: v["agent_id"].as_str().unwrap().into(),
        role_name: v["role_name"].as_str().unwrap().into(),
        kind: v["kind"].as_str().unwrap().into(),
        seat_id: v["seat_id"].as_str().unwrap().into(),
        actor_id: v["actor_id"].as_str().unwrap().into(),
        generation: v["generation"].as_i64().unwrap(),
        state: v["state"].as_str().unwrap().into(),
        last_activity_at: v["last_activity_at"].as_str().unwrap().into(),
    }
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap().to_string())
        .collect()
}

fn check(case: &Value, actual: String, what: &str) {
    assert_eq!(
        Value::String(actual),
        case["result"],
        "{what}: {}",
        clip_case(case)
    );
}

fn clip_case(case: &Value) -> String {
    let text = case.to_string();
    text.chars().take(300).collect()
}

#[test]
fn notice_builders_match_node() {
    let n = fixture("kernel-text/notices.json");
    for c in n["reportNotice"].as_array().unwrap() {
        check(
            c,
            report_notice(&report_row(&c["row"]), c["role"].as_str().unwrap()),
            "reportNotice",
        );
    }
    for c in n["reviewTask"].as_array().unwrap() {
        let authors: Vec<AgentReportRow> = c["authors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| report_row(&a["report"]))
            .collect();
        let refs: Vec<&AgentReportRow> = authors.iter().collect();
        check(c, review_task(&review_row(&c["row"]), &refs), "reviewTask");
    }
    for c in n["reviewNotice"].as_array().unwrap() {
        check(
            c,
            review_notice(&review_row(&c["row"]), &strings(&c["authors"])),
            "reviewNotice",
        );
    }
    for c in n["planNeedsAttentionNotice"].as_array().unwrap() {
        check(
            c,
            plan_needs_attention_notice(c["planId"].as_str().unwrap()),
            "planNeedsAttentionNotice",
        );
    }
    for c in n["planSignedOffNotice"].as_array().unwrap() {
        let a = &c["args"];
        check(
            c,
            plan_signed_off_notice(
                a[0].as_str().unwrap(),
                a[1].as_str().unwrap(),
                a[2].as_str().unwrap(),
                a[3].as_str(),
                a[4].as_str().unwrap(),
                a[5].as_bool().unwrap(),
                &strings(&a[6]),
            ),
            "planSignedOffNotice",
        );
    }
    for c in n["planApprovedNotice"].as_array().unwrap() {
        let a = &c["args"];
        check(
            c,
            plan_approved_notice(
                a[0].as_str().unwrap(),
                a[1].as_str().unwrap(),
                a[2].as_str(),
            )
            .unwrap(),
            "planApprovedNotice",
        );
    }
    for c in n["cancelledReason"].as_array().unwrap() {
        let a = &c["args"];
        check(
            c,
            cancelled_reason(
                a[0].as_str().unwrap(),
                a[1].as_str().unwrap(),
                a[2].as_str(),
            ),
            "cancelledReason",
        );
    }
    for c in n["findingTask"].as_array().unwrap() {
        check(
            c,
            finding_task(
                &finding_row(&c["finding"]),
                c["attempt"].as_i64().unwrap(),
                c["evidence"].as_str().unwrap(),
            ),
            "findingTask",
        );
    }
    for c in n["findingNoticeBody"].as_array().unwrap() {
        check(
            c,
            finding_notice_body(
                &finding_row(&c["finding"]),
                c["event"].as_str().unwrap(),
                c["lastCheck"].as_str(),
            ),
            "findingNoticeBody",
        );
    }
    for c in n["packageReviewedNotice"].as_array().unwrap() {
        let a = &c["args"];
        check(
            c,
            package_reviewed_notice(
                a[0].as_str().unwrap(),
                a[1].as_str().unwrap(),
                a[2].as_str().unwrap(),
                a[3].as_str().unwrap(),
            ),
            "packageReviewedNotice",
        );
    }
    for c in n["planCancelledNotice"].as_array().unwrap() {
        let a = &c["args"];
        check(
            c,
            plan_cancelled_notice(a[0].as_str().unwrap(), a[1].as_str()),
            "planCancelledNotice",
        );
    }
    for c in n["integrationConflictNotice"].as_array().unwrap() {
        let a = &c["args"];
        check(
            c,
            integration_conflict_notice(
                a[0].as_str().unwrap(),
                a[1].as_str().unwrap(),
                &strings(&a[2]),
                a[3].as_u64().unwrap() as usize,
            ),
            "integrationConflictNotice",
        );
    }
    for c in n["deliveryProblemNotice"].as_array().unwrap() {
        let a = &c["args"];
        check(
            c,
            delivery_problem_notice(
                a[0].as_str().unwrap(),
                a[1].as_str().unwrap(),
                a[2].as_str().unwrap(),
                a[3].as_str(),
                a[4].as_str().unwrap(),
            ),
            "deliveryProblemNotice",
        );
    }
    for c in n["agentStuckNotice"].as_array().unwrap() {
        let a = &c["args"];
        check(
            c,
            agent_stuck_notice(
                a[0].as_str().unwrap(),
                a[1].as_str().unwrap(),
                a[2].as_bool().unwrap(),
            ),
            "agentStuckNotice",
        );
    }
    for c in n["lostNotice"].as_array().unwrap() {
        check(
            c,
            lost_notice(
                &agent_row(&c["agent"]),
                c["reason"].as_str().unwrap(),
                c["branch"].as_str(),
                &strings(&c["ids"]),
                c["paused"].as_bool().unwrap(),
            ),
            "lostNotice",
        );
    }
}

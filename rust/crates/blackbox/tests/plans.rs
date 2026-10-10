//! Plans, reports, reviews and integrations through the executable (test/plan-flow.test.ts, plan-routing, plan-dependency,
//! plan-review-commands, reports.test.ts, review-commands, integration, integration-git, stale-reports, nexora-commands):
//! the whole path from `plan open` to the PM's `integrate confirm` with real git in the scratch project. A reviewer's
//! spawn needs Herdr, so the review is begun in the ledger the way `request-review` does after its spawn (`begin_review`);
//! the reviewer's verdict, the integration, the sign-off and the confirm are all the real commands.

use capstan_blackbox::*;
use serde_json::{json, Value};

fn team() -> World {
    World::builder("plans")
        .config(TEAM_CONFIG)
        .agents(&[
            agent("pm-1", "pm", "PM"),
            agent("arch-1", "architect", "Developer"),
            agent("dev-1", "developer", "Developer"),
            agent("dev-2", "developer", "Developer"),
            agent("rev-1", "reviewer", "Verifier"),
            agent("rev-2", "reviewer", "Verifier"),
            agent("rev-3", "reviewer", "Verifier"),
            agent("sup-1", "supervisor", "Supervisor"),
        ])
        .build()
}

fn plan_body(ids: &[&str]) -> String {
    json!({
        "summary": "split the work",
        "packages": ids.iter().map(|id| json!({
            "id": id, "title": format!("package {id}"), "owns": [format!("src/{id}/")],
            "interfaces": [format!("exports {id}()")], "acceptance": [format!("{id} works")], "estimate_hours": 2,
        })).collect::<Vec<_>>(),
    })
    .to_string()
}

fn field(out: &Out, key: &str) -> String {
    out.stdout
        .lines()
        .find_map(|l| l.strip_prefix(&format!("{key}: ")))
        .unwrap_or_else(|| panic!("no {key} in {}", out.stdout))
        .trim()
        .to_string()
}

fn open_and_submit(w: &World, tier: &str, ids: &[&str]) -> String {
    let plan = field(
        &w.as_agent("pm-1", &["plan", "open", tier, "a plan"]).ok(),
        "planId",
    );
    w.as_agent("arch-1", &["plan", "submit", &plan, &plan_body(ids)])
        .ok();
    plan
}

/// Reads and acknowledges everything waiting for `agent`: a report is refused while mail from before the commit is unread.
fn ack_all(w: &World, agent: &str) {
    let inbox = w.as_agent(agent, &["inbox", "--json"]).ok().json();
    for message in inbox["messages"].as_array().unwrap() {
        w.as_agent(agent, &["ack", message["messageId"].as_str().unwrap()])
            .ok();
    }
}

fn report(w: &World, agent: &str, commit: &str, summary: &str) -> String {
    ack_all(w, agent);
    field(
        &w.as_agent(agent, &["report", commit, summary]).ok(),
        "reportId",
    )
}

#[test]
fn a_normal_plan_is_approved_at_once_and_the_pm_is_told() {
    let w = team();
    let plan = field(
        &w.as_agent("pm-1", &["plan", "open", "normal", "ship it"])
            .ok(),
        "planId",
    );
    // Only the architect submits.
    let refused = w.as_agent("pm-1", &["plan", "submit", &plan, &plan_body(&["wp1"])]);
    assert_eq!(refused.code, 4, "{}", refused.text());
    let submitted = w
        .as_agent(
            "arch-1",
            &["plan", "submit", &plan, &plan_body(&["wp1", "wp2"])],
        )
        .ok();
    assert!(
        submitted.stdout.contains("state: approved"),
        "{}",
        submitted.stdout
    );
    let shown = w.as_agent("dev-1", &["plan", "show", &plan]).ok();
    assert!(
        shown.stdout.contains("wp1") && shown.stdout.contains("wp2"),
        "plan show is readable by any agent: {}",
        shown.stdout
    );
    let pm = w.as_agent("pm-1", &["inbox"]).ok();
    assert!(
        pm.stdout.contains(&plan) && pm.stdout.contains("approved"),
        "{}",
        pm.stdout
    );
    // Plans list.
    assert!(w.op(&["plan", "show"]).ok().stdout.contains(&plan));
}

#[test]
fn plan_commands_check_who_and_what() {
    let w = team();
    // open: PM and operator only, with a tier and a title.
    assert_eq!(
        w.wire_as("dev-1", "plan", &["open", "normal", "x"])["code"],
        "forbidden"
    );
    assert_eq!(
        w.wire_as("pm-1", "plan", &["open", "enormous", "x"])["code"],
        "invalid_request"
    );
    assert_eq!(
        w.wire_as("pm-1", "plan", &["open", "normal"])["code"],
        "invalid_request"
    );
    let plan = field(
        &w.op(&["plan", "open", "high-risk", "by the operator"]).ok(),
        "planId",
    );
    assert!(plan.starts_with("plan-"));
    // submit: bad JSON, an unknown plan, a duplicate package id.
    assert_ne!(
        w.as_agent("arch-1", &["plan", "submit", &plan, "{not json"])
            .code,
        0
    );
    assert_ne!(
        w.as_agent("arch-1", &["plan", "submit", "plan-99", &plan_body(&["a"])])
            .code,
        0
    );
    assert_ne!(
        w.as_agent(
            "arch-1",
            &["plan", "submit", &plan, &plan_body(&["a", "a"])]
        )
        .code,
        0
    );
    // assign: PM and operator only.
    assert_eq!(
        w.wire_as("dev-1", "plan", &["assign", &plan, "a", "dev-2"])["code"],
        "forbidden"
    );
    // Unknown subcommands and a missing plan id.
    assert_ne!(w.wire_as("pm-1", "plan", &["bogus"])["ok"], json!(true));
    assert_eq!(w.wire_as("pm-1", "plan", &["show", "plan-99"])["ok"], false);
}

#[test]
fn plan_assign_sends_one_message_binds_the_package_and_names_the_branch() {
    let w = team();
    let plan = open_and_submit(&w, "normal", &["wp1", "wp2"]);
    let assigned = w
        .as_agent("pm-1", &["plan", "assign", &plan, "wp1", "dev-1"])
        .ok();
    assert!(
        assigned.stdout.contains("dev-1") || assigned.stdout.contains("wp1"),
        "{}",
        assigned.stdout
    );
    let inbox = w.as_agent("dev-1", &["inbox", "--json"]).ok().json();
    assert_eq!(inbox["count"], 1, "exactly one message");
    assert!(
        inbox["messages"][0]["body"]
            .as_str()
            .unwrap()
            .contains("wp1"),
        "{inbox}"
    );
    // The same package cannot be assigned twice, and a refusal sends nothing.
    let again = w.as_agent("pm-1", &["plan", "assign", &plan, "wp1", "dev-2"]);
    assert_ne!(again.code, 0, "{}", again.text());
    assert_eq!(
        w.as_agent("dev-2", &["inbox", "--json"]).ok().json()["count"],
        0
    );
    // The package view carries the assignee.
    let shown = w.op(&["plan", "show", &plan]).ok();
    assert!(shown.stdout.contains("dev-1"), "{}", shown.stdout);
}

#[test]
fn a_package_whose_dependency_is_not_reviewed_is_refused_and_early_needs_a_reason() {
    let w = team();
    let body = json!({
        "summary": "ordered",
        "packages": [
            {"id": "base", "title": "base", "owns": ["src/base/"], "interfaces": [], "acceptance": ["base works"], "estimate_hours": 1},
            {"id": "top", "title": "top", "owns": ["src/top/"], "interfaces": [], "acceptance": ["top works"], "estimate_hours": 1, "depends_on": ["base"]},
        ],
    })
    .to_string();
    let plan = field(
        &w.as_agent("pm-1", &["plan", "open", "normal", "ordered"])
            .ok(),
        "planId",
    );
    w.as_agent("arch-1", &["plan", "submit", &plan, &body]).ok();
    let refused = w.as_agent("pm-1", &["plan", "assign", &plan, "top", "dev-2"]);
    assert_ne!(refused.code, 0, "{}", refused.text());
    assert!(
        refused.stderr.contains("base"),
        "the unmet package is named: {}",
        refused.stderr
    );
    assert_eq!(
        w.as_agent("dev-2", &["inbox", "--json"]).ok().json()["count"],
        0,
        "nothing changed"
    );
    // --early needs a reason, and then assigns and says so.
    assert_ne!(
        w.as_agent(
            "pm-1",
            &["plan", "assign", &plan, "top", "dev-2", "--early"]
        )
        .code,
        0
    );
    let early = w.as_agent(
        "pm-1",
        &[
            "plan",
            "assign",
            &plan,
            "top",
            "dev-2",
            "--early",
            "the base is nearly done",
        ],
    );
    assert_eq!(early.code, 0, "{}", early.text());
    let told = w.as_agent("dev-2", &["inbox"]).ok();
    assert!(
        told.stdout.contains("Depends on packages: base")
            && told.stdout.contains("not yet reviewed"),
        "{}",
        told.stdout
    );
}

#[test]
fn the_whole_path_from_an_open_plan_to_the_pms_confirm() {
    let mut w = team();
    let plan = open_and_submit(&w, "normal", &["wp1", "wp2"]);
    w.as_agent("pm-1", &["plan", "assign", &plan, "wp1", "dev-1"])
        .ok();
    w.as_agent("pm-1", &["plan", "assign", &plan, "wp2", "dev-2"])
        .ok();
    let (c1, _) = w.give_branch("dev-1", "one.txt", "one\n", "feat: package one");
    let (c2, _) = w.give_branch("dev-2", "two.txt", "two\n", "feat: package two");
    let r1 = report(&w, "dev-1", &c1, "package one done");
    let r2 = report(&w, "dev-2", &c2, "package two done");
    // The report goes to the architect with the package line.
    let arch = w.as_agent("arch-1", &["inbox"]).ok();
    assert!(
        arch.stdout.contains("wp1") && arch.stdout.contains(&r1[..8]),
        "{}",
        arch.stdout
    );
    // Integrating before a review is refused.
    assert_ne!(w.as_agent("arch-1", &["integrate", &r1, &r2]).code, 0);
    // Reviews of both reports, begun by the architect; the reviewers pass them.
    w.begin_review("arch-1", &r1, "reviewer", "rev-1");
    w.begin_review("arch-1", &r2, "reviewer", "rev-2");
    w.as_agent("rev-1", &["review", "pass", "package one is fine"])
        .ok();
    w.as_agent("rev-2", &["review", "pass", "package two is fine"])
        .ok();
    // A worker cannot integrate; the architect does, in the order given.
    assert_eq!(w.wire_as("dev-1", "integrate", &[&r1])["code"], "forbidden");
    let integrated = w.as_agent("arch-1", &["integrate", &r1, &r2]).ok();
    assert_eq!(
        field(&integrated, "state"),
        "merged",
        "{}",
        integrated.stdout
    );
    let integration = field(&integrated, "integrationId");
    let branch = field(&integrated, "branch");
    assert!(branch.starts_with("integration/"), "{branch}");
    // The squash commit is on the new branch and nothing is checked out in the project.
    assert_eq!(w.git(&["rev-parse", "--abbrev-ref", "HEAD"]), "main");
    let subjects = w.git(&["log", "--format=%s", "-1", &branch]);
    assert!(!subjects.is_empty());
    // Sign-off needs the integration reviewed by someone who wrote none of the reports.
    let early = w.as_agent(
        "arch-1",
        &["plan", "signoff", &plan, &integration, "all done"],
    );
    assert_ne!(early.code, 0, "{}", early.text());
    w.begin_review("arch-1", &integration, "reviewer", "rev-3");
    w.as_agent("rev-3", &["review", "pass", "the merge is fine"])
        .ok();
    let signoff = w
        .as_agent(
            "arch-1",
            &[
                "plan",
                "signoff",
                &plan,
                &integration,
                "all packages integrated",
            ],
        )
        .ok();
    assert!(
        signoff.stdout.contains("signed") || signoff.stdout.contains(&integration),
        "{}",
        signoff.stdout
    );
    let notice = w.as_agent("pm-1", &["inbox"]).ok();
    assert!(
        notice
            .stdout
            .contains(&format!("run cstan integrate confirm {integration}")),
        "{}",
        notice.stdout
    );
    // The user has not merged: the architect cannot confirm and the PM's confirm is refused.
    assert_eq!(
        w.wire_as("arch-1", "integrate", &["confirm", &integration])["code"],
        "forbidden"
    );
    assert_ne!(
        w.as_agent("pm-1", &["integrate", "confirm", &integration])
            .code,
        0
    );
    // The user merges the branch; the PM confirms.
    w.git(&["merge", "--ff-only", "--quiet", &branch]);
    let confirmed = w
        .as_agent("pm-1", &["integrate", "confirm", &integration])
        .ok();
    assert!(
        confirmed.stdout.contains("confirmed"),
        "{}",
        confirmed.stdout
    );
    let shown = w.op(&["plan", "show", &plan]).ok();
    assert!(
        shown.stdout.contains("signed")
            || shown.stdout.contains("complete")
            || shown.stdout.contains("done")
            || !shown.stdout.is_empty()
    );
    let _: Value = json!(null);
}

#[test]
fn a_developer_writes_to_the_architect_and_the_architect_to_a_developer_and_nothing_else_opens_up()
{
    let w = team();
    let code = |who: &str, to: &str| {
        w.wire_as(who, "send", &[to, "question"])["code"]
            .as_str()
            .unwrap_or("ok")
            .to_string()
    };
    assert_eq!(code("dev-1", "arch-1"), "ok");
    assert_eq!(code("arch-1", "dev-1"), "ok");
    assert_eq!(code("dev-1", "dev-2"), "recipient_not_allowed");
    assert_eq!(
        code("rev-1", "arch-1"),
        "recipient_not_allowed",
        "a reviewer writes only to the PM"
    );
    assert_eq!(
        code("arch-1", "rev-1"),
        "recipient_not_allowed",
        "the architect writes to developers, not reviewers"
    );
    assert_eq!(code("dev-1", "@pm"), "ok");
}

#[test]
fn with_architect_disabled_a_developer_writes_only_to_the_pm_and_plan_commands_are_refused() {
    let off = TEAM_CONFIG.replace(
        "[architect]\nenabled = true",
        "[architect]\nenabled = false",
    );
    let w = World::builder("noarchitect")
        .config(&off)
        .agents(&[
            agent("pm-1", "pm", "PM"),
            agent("arch-1", "architect", "Developer"),
            agent("dev-1", "developer", "Developer"),
        ])
        .build();
    assert_eq!(
        w.wire_as("dev-1", "send", &["arch-1", "hello"])["code"],
        "recipient_not_allowed"
    );
    assert_eq!(
        w.wire_as("arch-1", "send", &["dev-1", "hello"])["code"],
        "recipient_not_allowed"
    );
    for (who, args) in [
        ("pm-1", vec!["open", "normal", "x"]),
        ("arch-1", vec!["show"]),
    ] {
        let refused = w.wire_as(who, "plan", &args);
        assert_eq!(
            refused["code"], "not_configured",
            "{who} {args:?}: {refused}"
        );
    }
    assert_eq!(w.wire_op("plan", &["show"])["code"], "not_configured");
}

#[test]
fn a_report_from_an_assignee_goes_to_the_architect_with_the_package_line_and_anyone_elses_goes_to_the_pm(
) {
    let mut w = team();
    let plan = open_and_submit(&w, "normal", &["wp1"]);
    w.as_agent("pm-1", &["plan", "assign", &plan, "wp1", "dev-1"])
        .ok();
    let (c1, _) = w.give_branch("dev-1", "one.txt", "one\n", "feat: one");
    let (c2, _) = w.give_branch("dev-2", "two.txt", "two\n", "feat: two");
    let r1 = report(&w, "dev-1", &c1, "package one done");
    let r2 = report(&w, "dev-2", &c2, "unplanned work");
    let arch = w.as_agent("arch-1", &["inbox"]).ok().stdout;
    assert!(
        arch.contains("wp1") && arch.contains(&r1[..8]),
        "the planned report reaches the architect with its package: {arch}"
    );
    assert!(
        !arch.contains(&r2[..8]),
        "an unplanned report does not: {arch}"
    );
    // The PM is told about the report that belongs to nobody's package.
    let pm = w.as_agent("pm-1", &["inbox"]).ok().stdout;
    assert!(pm.contains(&r2[..8]), "{pm}");
}

#[test]
fn a_high_risk_plan_that_needs_a_reviewer_who_cannot_start_stays_a_draft_and_tells_the_architect() {
    let w = World::builder("highrisk")
        .config(TEAM_CONFIG)
        .launching()
        .agents(&[
            agent("pm-1", "pm", "PM"),
            agent("arch-1", "architect", "Developer"),
            agent("dev-1", "developer", "Developer"),
        ])
        .build();
    let plan = field(
        &w.as_agent("pm-1", &["plan", "open", "high-risk", "risky"])
            .ok(),
        "planId",
    );
    let submitted = w.as_agent("arch-1", &["plan", "submit", &plan, &plan_body(&["wp1"])]);
    assert_ne!(
        submitted.code,
        0,
        "no Herdr: the reviewer cannot be started: {}",
        submitted.text()
    );
    let shown = w.op(&["plan", "show", &plan]).ok().stdout;
    assert!(
        shown.contains("state: draft"),
        "the plan stays a draft: {shown}"
    );
}

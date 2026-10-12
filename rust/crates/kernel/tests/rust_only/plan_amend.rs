//! Plan amendments, classified plan reviews and design-only round counting: the architect amends a plan after a review whose
//! findings were all mechanical, a plan review answers in finding lines, and only design rounds count against the plan's limit.

use crate::fixture::{context, step, Fixture, OWNER};
use capstan_kernel::helpers::amended_plan_body_json;
use capstan_kernel::plan_body::parse_plan_body;
use capstan_kernel::KernelError;
use serde_json::{json, Value};

const BODY: &str = r#"{"summary":"s","packages":[
  {"id":"api","title":"Api","owns":["src/api"],"acceptance":["x"],"estimate_hours":2,"interfaces":["i"]},
  {"id":"ui","title":"Ui","owns":["src/ui"],"acceptance":["y"],"estimate_hours":3,"depends_on":["api"]}],
  "risks":[],"integration_order":["api","ui"]}"#;
const BASE: &str = "0123456789abcdef0123456789abcdef01234567";
const AMEND: &str =
    r#"{"reason":"ui also writes docs","packages":[{"id":"ui","add_owns":["docs/ui"]}]}"#;
const MECHANICAL: &str =
    "## ui\n- [ui] major owns: ui also writes docs/ui\n- [ui] minor depends_on: ui needs api";
const DESIGN: &str =
    "- [api] blocker interface: api has no error contract\n- [ui] minor owns: docs/ui";

struct Rig {
    f: Fixture,
    architect: String,
    reviewer: String,
}

fn create_agent(f: &Fixture, agent: &str, role: &str, kind: &str) -> String {
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
    actor["credential"].as_str().unwrap().to_string()
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
    let architect = create_agent(&f, "arch-1", "architect", "Developer");
    let reviewer = create_agent(&f, "rev-1", "reviewer", "Verifier");
    Rig {
        f,
        architect,
        reviewer,
    }
}

fn stored_body() -> String {
    serde_json::to_string(&parse_plan_body(BODY, 8).unwrap()).unwrap()
}

impl Rig {
    fn core(&self) -> &capstan_kernel::Core {
        self.f.core()
    }

    fn open(&self, tier: &str, limit: Option<i64>) -> String {
        let mut input = json!({"tier": tier, "title": "T"});
        if let Some(limit) = limit {
            input["maxReviewRounds"] = json!(limit);
        }
        let plan = self
            .core()
            .open_plan(&context(self.core(), OWNER), &input)
            .unwrap();
        plan["planId"].as_str().unwrap().to_string()
    }

    fn submit(&self, plan: &str, review: bool) -> Value {
        self.core()
            .submit_plan(
                &context(self.core(), &self.architect),
                &json!({"planId": plan, "bodyJson": stored_body(), "baseSha": BASE, "review": review}),
            )
            .unwrap()
    }

    /// Begins the review of a plan in review and answers it; returns the review id.
    fn review(&self, plan: &str, verdict: &str, text: &str) -> Result<String, KernelError> {
        let begun = self.core().begin_review(
            &context(self.core(), &self.architect),
            &json!({"subjectId": plan, "reviewerRole": "reviewer", "reviewerAgentId": "rev-1"}),
        )?;
        let review_id = begun["reviewId"].as_str().unwrap().to_string();
        self.core().complete_review(
            &context(self.core(), &self.reviewer),
            &json!({"verdict": verdict, "text": text}),
        )?;
        Ok(review_id)
    }

    fn amend_as(
        &self,
        credential: &str,
        plan: &str,
        amendment: &str,
    ) -> Result<Value, KernelError> {
        let body = amended_plan_body_json(&self.current_body(plan), amendment)
            .unwrap_or_else(|_| stored_body());
        self.core().amend_plan(
            &context(self.core(), credential),
            &json!({"planId": plan, "amendmentJson": amendment, "bodyJson": body, "baseSha": BASE}),
        )
    }

    fn amend(&self, plan: &str) -> Result<Value, KernelError> {
        self.amend_as(&self.architect, plan, AMEND)
    }

    /// An amendment that adds a path no earlier one added.
    fn amend_again(&self, plan: &str, path: &str) -> Result<Value, KernelError> {
        let amendment =
            format!(r#"{{"reason":"more","packages":[{{"id":"ui","add_owns":["{path}"]}}]}}"#);
        self.amend_as(&self.architect, plan, &amendment)
    }

    fn current_body(&self, plan: &str) -> String {
        let record = self.core().plan_record(OWNER, plan).unwrap();
        record["revision"]["bodyJson"].as_str().unwrap().to_string()
    }

    fn plan(&self, plan: &str) -> Value {
        self.core().plan_record(OWNER, plan).unwrap()["plan"].clone()
    }

    fn status(&self, plan: &str) -> Value {
        self.core().plan_review_status(OWNER, plan).unwrap()
    }

    fn rows(&self, sql: &str) -> Vec<String> {
        let conn = rusqlite::Connection::open_with_flags(
            self.f.database(),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let mut statement = conn.prepare(sql).unwrap();
        statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    fn pm_messages_like(&self, text: &str) -> Vec<String> {
        self.rows(&format!(
            "SELECT body FROM messages WHERE recipient_agent_id = 'pm-1' AND body LIKE '%{text}%'"
        ))
    }
}

fn refused(result: Result<Value, KernelError>) -> String {
    match result {
        Err(error) => error.message(),
        Ok(_) => panic!("the call was accepted"),
    }
}

#[test]
fn a_normal_plan_with_mechanical_findings_is_amended_to_approved() {
    let rig = rig();
    let plan = rig.open("normal", None);
    rig.submit(&plan, true);
    let review_id = rig.review(&plan, "findings", MECHANICAL).unwrap();
    assert_eq!(rig.plan(&plan)["state"], "draft");
    let reviews = rig.core().reviews(OWNER, Some(5)).unwrap();
    assert_eq!(reviews[0]["findingsClass"], "mechanical");

    let amended = rig.amend(&plan).unwrap();
    assert_eq!(amended["state"], "approved");
    assert_eq!(amended["currentRevision"], 2);
    assert_eq!(amended["approvedRevision"], 2);

    let record = rig.core().plan_record(OWNER, &plan).unwrap();
    assert_eq!(record["revision"]["revision"], 2);
    assert_eq!(record["revision"]["amendsReviewId"], json!(review_id));
    assert_eq!(
        record["revision"]["amendment"]["reason"],
        "ui also writes docs"
    );
    let body: Value =
        serde_json::from_str(record["revision"]["bodyJson"].as_str().unwrap()).unwrap();
    assert_eq!(body["packages"][1]["owns"], json!(["src/ui", "docs/ui"]));
    let packages: Vec<&str> = record["packages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["packageId"].as_str().unwrap())
        .collect();
    assert_eq!(packages, ["api", "ui"]);

    let events = rig.rows(
        "SELECT payload_json FROM controller_events WHERE payload_json LIKE '%\"action\":\"plan.amend\"%'",
    );
    assert_eq!(events.len(), 1);
    assert!(
        events[0].contains(&format!("\"amendsReviewId\":\"{review_id}\"")),
        "{}",
        events[0]
    );
    assert!(events[0].contains("\"revision\":2"), "{}", events[0]);

    let notices = rig.pm_messages_like("approved");
    assert!(
        notices.iter().any(|n| n.contains(&format!(
            "Approved at revision 2 by amendment of revision 1 after review {review_id} (mechanical findings)"
        ))),
        "{notices:?}"
    );
}

#[test]
fn an_amendment_is_refused_unless_the_latest_review_asked_for_it() {
    let rig = rig();
    let plan = rig.open("normal", None);
    rig.submit(&plan, true);

    // A plan whose review has not finished has nothing to amend after.
    assert!(
        refused(rig.amend(&plan)).contains("amend_refused"),
        "in review"
    );

    // Design findings.
    rig.review(&plan, "findings", DESIGN).unwrap();
    assert_eq!(
        rig.core().reviews(OWNER, Some(1)).unwrap()[0]["findingsClass"],
        "design"
    );
    let message = refused(rig.amend(&plan));
    assert!(
        message.contains("amend_refused") && message.contains("mechanical"),
        "{message}"
    );
    assert_eq!(rig.plan(&plan)["currentRevision"], 1);

    // No finished review at all: the review was abandoned before it began.
    let other = rig.open("normal", None);
    rig.submit(&other, true);
    rig.core()
        .abandon_plan_review(
            &context(rig.core(), OWNER),
            &json!({"planId": other, "reason": "no reviewer"}),
        )
        .unwrap();
    let message = refused(rig.amend(&other));
    assert!(message.contains("no finished review"), "{message}");
    assert_eq!(rig.plan(&other)["currentRevision"], 1);
}

#[test]
fn only_the_plans_architect_amends_and_only_a_draft() {
    let rig = rig();
    let developer = rig.f.developer_credential.clone();
    let plan = rig.open("normal", None);
    rig.submit(&plan, true);
    rig.review(&plan, "findings", MECHANICAL).unwrap();

    let message = refused(rig.amend_as(&developer, &plan, AMEND));
    assert!(
        message.contains("amend_refused") && message.contains("architect"),
        "{message}"
    );
    let message = refused(rig.amend_as(OWNER, &plan, AMEND));
    assert!(message.contains("amend_refused"), "{message}");
    assert_eq!(rig.plan(&plan)["currentRevision"], 1);

    rig.amend(&plan).unwrap();
    // Approved now: a second amendment after the same review is refused and writes nothing.
    let message = refused(rig.amend(&plan));
    assert!(
        message.contains("only a draft plan is amended"),
        "{message}"
    );
    assert_eq!(rig.plan(&plan)["currentRevision"], 2);

    // A cancelled plan.
    let cancelled = rig.open("normal", None);
    rig.submit(&cancelled, true);
    rig.review(&cancelled, "findings", MECHANICAL).unwrap();
    rig.core()
        .cancel_plan(&context(rig.core(), OWNER), &json!({"planId": cancelled}))
        .unwrap();
    let message = refused(rig.amend(&cancelled));
    assert!(message.contains("amend_refused"), "{message}");
    assert_eq!(rig.plan(&cancelled)["currentRevision"], 1);
}

#[test]
fn an_amendment_cannot_touch_interfaces_or_acceptance_or_tamper_with_the_body() {
    let rig = rig();
    let plan = rig.open("normal", None);
    rig.submit(&plan, true);
    rig.review(&plan, "findings", MECHANICAL).unwrap();
    for edit in [
        r#"{"reason":"r","packages":[{"id":"ui","add_owns":["docs/ui"],"interfaces":["new"]}]}"#,
        r#"{"reason":"r","packages":[{"id":"ui","add_owns":["docs/ui"],"acceptance":["new"]}]}"#,
        r#"{"reason":"r","title":"new","packages":[{"id":"ui","add_owns":["docs/ui"]}]}"#,
    ] {
        let body = stored_body();
        let result = rig.core().amend_plan(
            &context(rig.core(), &rig.architect),
            &json!({"planId": plan, "amendmentJson": edit, "bodyJson": body, "baseSha": BASE}),
        );
        let message = refused(result);
        assert!(message.starts_with("amend_not_allowed:"), "{message}");
    }
    assert!(amended_plan_body_json(
        &rig.current_body(&plan),
        r#"{"reason":"r","packages":[{"id":"ui","add_owns":["docs/ui"],"interfaces":["x"]}]}"#
    )
    .unwrap_err()
    .starts_with("amend_not_allowed:"));
    // A body that is not the amendment applied is refused.
    let result = rig.core().amend_plan(
        &context(rig.core(), &rig.architect),
        &json!({"planId": plan, "amendmentJson": AMEND, "bodyJson": stored_body(), "baseSha": BASE}),
    );
    assert!(refused(result).contains("not the current revision with the amendment applied"));
    // An amendment that breaks the plan rules.
    let result = rig.amend_as(
        &rig.architect,
        &plan,
        r#"{"reason":"r","packages":[{"id":"nope","add_owns":["docs/ui"]}]}"#,
    );
    assert!(refused(result).starts_with("invalid_plan:"));
    assert_eq!(rig.plan(&plan)["currentRevision"], 1);
    assert_eq!(rig.plan(&plan)["state"], "draft");
}

#[test]
fn a_high_risk_plan_amended_goes_back_to_review_and_the_brief_lists_the_amendment() {
    let rig = rig();
    let plan = rig.open("high_risk", None);
    rig.submit(&plan, true);
    let review_id = rig.review(&plan, "findings", MECHANICAL).unwrap();
    let amended = rig.amend(&plan).unwrap();
    assert_eq!(amended["state"], "in_review");
    assert_eq!(amended["approvedRevision"], Value::Null);
    assert_eq!(amended["currentRevision"], 2);

    let begun = rig
        .core()
        .begin_review(
            &context(rig.core(), &rig.architect),
            &json!({"subjectId": plan, "reviewerRole": "reviewer", "reviewerAgentId": "rev-1"}),
        )
        .unwrap();
    assert_eq!(begun["planRevision"], 2);
    let briefs = rig.rows(
        "SELECT body FROM messages WHERE recipient_agent_id = 'rev-1' AND body LIKE '%revision 2%' AND body LIKE 'Review request%'",
    );
    assert_eq!(briefs.len(), 1);
    let brief = &briefs[0];
    for needed in [
        format!("Revision 2 is an amendment of revision 1 made after review {review_id} (mechanical findings only)"),
        "package ui: added owns: \"docs/ui\"".to_string(),
        "The architect's reason, not verified: \"ui also writes docs\"".to_string(),
        "Give one exhaustive findings list".to_string(),
        "- [<package-id>|plan] <blocker|major|minor> <owns|depends_on|integration_order|interface|acceptance|risk|design>: <text>".to_string(),
    ] {
        assert!(brief.contains(&needed), "{needed}\n{brief}");
    }
    // Passing the amended revision approves it, with the amendment line in the notice.
    rig.core()
        .complete_review(
            &context(rig.core(), &rig.reviewer),
            &json!({"verdict": "pass", "text": "fine"}),
        )
        .unwrap();
    assert_eq!(rig.plan(&plan)["state"], "approved");
    assert!(rig
        .pm_messages_like("by amendment of revision 1")
        .iter()
        .any(|n| n.contains(&format!("after review {review_id}"))));
}

#[test]
fn the_review_trigger_refuses_a_second_amendment_of_the_same_review() {
    let rig = rig();
    let plan = rig.open("high_risk", None);
    rig.submit(&plan, true);
    rig.review(&plan, "findings", MECHANICAL).unwrap();
    rig.amend(&plan).unwrap();
    // The plan is in review again: the kernel refuses another amendment, and so does the ledger.
    assert!(refused(rig.amend(&plan)).contains("only a draft plan is amended"));
    rig.core()
        .abandon_plan_review(
            &context(rig.core(), OWNER),
            &json!({"planId": plan, "reason": "x"}),
        )
        .unwrap();
    let message = refused(rig.amend(&plan));
    assert!(
        message.contains("answered revision 1, not the current revision 2"),
        "{message}"
    );
    assert_eq!(rig.plan(&plan)["currentRevision"], 2);
}

#[test]
fn only_design_rounds_count_against_the_limit_and_the_total_stops_the_plan_at_twice_it() {
    let rig = rig();
    // Two mechanical rounds, each answered by an amendment, are not needs-attention.
    let plan = rig.open("high_risk", Some(2));
    rig.submit(&plan, true);
    rig.review(&plan, "findings", MECHANICAL).unwrap();
    rig.amend(&plan).unwrap();
    rig.review(&plan, "findings", MECHANICAL).unwrap();
    assert_eq!(rig.status(&plan)["designRounds"], 0);
    assert_eq!(rig.status(&plan)["totalRounds"], 2);
    assert_eq!(rig.status(&plan)["limit"], 2);
    assert_eq!(rig.status(&plan)["exhausted"], false);
    assert!(rig.pm_messages_like("needs attention").is_empty());
    assert_eq!(rig.core().plan_review_rounds(OWNER, &plan).unwrap(), 0);

    // Total finished rounds at 4 stop the plan.
    rig.amend_again(&plan, &format!("docs/n{}", line!()))
        .unwrap();
    rig.review(&plan, "findings", MECHANICAL).unwrap();
    rig.amend_again(&plan, &format!("docs/n{}", line!()))
        .unwrap();
    rig.review(&plan, "findings", MECHANICAL).unwrap();
    assert_eq!(rig.status(&plan)["totalRounds"], 4);
    assert_eq!(rig.status(&plan)["exhausted"], true);
    let notices = rig.pm_messages_like("needs attention");
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(notices[0].contains("limit 2"), "{notices:?}");
    let message = refused(rig.amend(&plan));
    assert!(message.contains("used its review rounds"), "{message}");
    let again = rig.core().begin_review(
        &context(rig.core(), &rig.architect),
        &json!({"subjectId": plan, "reviewerRole": "reviewer", "reviewerAgentId": "rev-1"}),
    );
    assert!(again.is_err());

    // Two design rounds do.
    let plan = rig.open("high_risk", Some(2));
    rig.submit(&plan, true);
    rig.review(&plan, "findings", DESIGN).unwrap();
    assert_eq!(rig.status(&plan)["exhausted"], false);
    rig.submit(&plan, true);
    rig.review(&plan, "findings", DESIGN).unwrap();
    assert_eq!(rig.status(&plan)["designRounds"], 2);
    assert_eq!(rig.status(&plan)["exhausted"], true);
    assert_eq!(rig.pm_messages_like("needs attention").len(), 2);
}

#[test]
fn a_plan_without_a_stored_limit_uses_five() {
    let rig = rig();
    let plan = rig.open("normal", None);
    assert_eq!(rig.plan(&plan)["maxReviewRounds"], Value::Null);
    assert_eq!(rig.status(&plan)["limit"], 5);
    let limited = rig.open("normal", Some(3));
    assert_eq!(rig.plan(&limited)["maxReviewRounds"], 3);
    assert_eq!(rig.status(&limited)["limit"], 3);
    let result = rig.core().open_plan(
        &context(rig.core(), OWNER),
        &json!({"tier": "normal", "title": "T", "maxReviewRounds": 21}),
    );
    assert!(result.is_err());
}

#[test]
fn a_plan_findings_answer_needs_finding_lines_and_stores_its_class() {
    let rig = rig();
    let plan = rig.open("normal", None);
    rig.submit(&plan, true);
    let begun = rig
        .core()
        .begin_review(
            &context(rig.core(), &rig.architect),
            &json!({"subjectId": plan, "reviewerRole": "reviewer", "reviewerAgentId": "rev-1"}),
        )
        .unwrap();
    let review_id = begun["reviewId"].as_str().unwrap().to_string();
    for text in [
        "Package ui has no interface. Split it.",
        "- [ui] huge owns: too big",
        "- [ui] major whatever: nope",
        "- [Not A Package] major owns: x",
    ] {
        let result = rig.core().complete_review(
            &context(rig.core(), &rig.reviewer),
            &json!({"verdict": "findings", "text": text}),
        );
        let message = refused(result);
        assert!(
            message.contains("- [<package-id>|plan] <blocker|major|minor>"),
            "{message}"
        );
        let reviews = rig.core().reviews(OWNER, Some(1)).unwrap();
        assert_eq!(reviews[0]["reviewId"], json!(review_id));
        assert_eq!(reviews[0]["state"], "started");
        assert_eq!(rig.plan(&plan)["state"], "in_review");
    }
    // A pass needs no lines.
    rig.core()
        .complete_review(
            &context(rig.core(), &rig.reviewer),
            &json!({"verdict": "findings", "text": MECHANICAL}),
        )
        .unwrap();
    let reviews = rig.core().reviews(OWNER, Some(1)).unwrap();
    assert_eq!(reviews[0]["state"], "findings");
    assert_eq!(reviews[0]["findingsClass"], "mechanical");

    let other = rig.open("normal", None);
    rig.submit(&other, true);
    let begun = rig
        .core()
        .begin_review(
            &context(rig.core(), &rig.architect),
            &json!({"subjectId": other, "reviewerRole": "reviewer", "reviewerAgentId": "rev-1"}),
        )
        .unwrap();
    assert_eq!(begun["state"], "started");
    let passed = rig
        .core()
        .complete_review(
            &context(rig.core(), &rig.reviewer),
            &json!({"verdict": "pass", "text": "looks fine, no lines at all"}),
        )
        .unwrap();
    assert_eq!(passed["state"], "passed");
    assert!(passed.get("findingsClass").is_none());
}

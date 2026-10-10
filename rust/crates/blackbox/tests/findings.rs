//! Findings and observing (test/findings.test.ts, finding-commands.test.ts): a Supervisor raises a finding about a worker, the
//! worker acknowledges the correction, the Supervisor checks it, a second round and an escalation; the rules of who may,
//! the rate limit, and `observe` without a launcher.

use capstan_blackbox::*;
use serde_json::Value;

fn finding_id(out: &Out) -> String {
    out.stdout
        .lines()
        .find_map(|l| l.strip_prefix("findingId: "))
        .unwrap_or_else(|| panic!("no findingId in {}", out.stdout))
        .to_string()
}

fn raise(w: &World, target: &str, severity: &str) -> Out {
    w.as_agent(
        "sup-1",
        &[
            "finding",
            target,
            severity,
            "the evidence",
            "the correction",
            "done when it passes",
        ],
    )
}

#[test]
fn a_supervisor_raises_a_finding_and_the_delivery_reaches_the_worker_and_the_pm_is_told() {
    let w = World::standard("raise");
    let raised = raise(&w, "dev-1", "high").ok();
    assert!(
        raised.stdout.contains("targetAgentId: dev-1")
            && raised.stdout.contains("severity: high")
            && raised.stdout.contains("state: open"),
        "{}",
        raised.stdout
    );
    assert!(
        raised.stdout.contains("interventions: 1"),
        "{}",
        raised.stdout
    );
    // The worker's inbox holds the correction, quoted with the supervisor's words.
    let inbox = w.as_agent("dev-1", &["inbox"]).ok();
    assert!(
        inbox.stdout.contains("the correction") && inbox.stdout.contains("done when it passes"),
        "{}",
        inbox.stdout
    );
    // The PM has a notice about the finding.
    let pm = w.as_agent("pm-1", &["inbox"]).ok();
    assert!(
        pm.stdout.contains("dev-1") && pm.stdout.to_lowercase().contains("finding"),
        "{}",
        pm.stdout
    );
    // The operator's status lists it with its state.
    let status = w.wire_op("status", &[])["result"].clone();
    let listed = &status["agentFindings"][0];
    assert_eq!(listed["targetAgentId"], "dev-1", "{status}");
    assert_eq!(listed["state"], "open");
}

#[test]
fn only_an_active_supervisor_raises_and_only_about_another_active_worker() {
    let w = World::standard("raisewho");
    for (who, args, fragment) in [
        (
            "pm-1",
            vec!["finding", "dev-1", "high", "e", "c", "d"],
            "only a Supervisor",
        ),
        (
            "dev-1",
            vec!["finding", "dev-2", "high", "e", "c", "d"],
            "only a Supervisor",
        ),
    ] {
        let out = w.as_agent(who, &args);
        assert_eq!(out.code, 4, "{}", out.text());
        assert!(out.stderr.contains(fragment), "{}", out.stderr);
    }
    // The operator is not an agent.
    assert_ne!(w.op(&["finding", "dev-1", "high", "e", "c", "d"]).code, 0);
    // Not about the PM, a Supervisor, itself or a stranger; the argument count and the severity are checked.
    for target in ["pm-1", "sup-1", "ghost-1"] {
        assert_ne!(raise(&w, target, "high").code, 0, "{target}");
    }
    assert_eq!(
        w.wire_as("sup-1", "finding", &["dev-1", "high", "e", "c"])["code"],
        "invalid_request"
    );
    assert_ne!(raise(&w, "dev-1", "catastrophic").code, 0);
    // Empty or invisible text is refused, not cut.
    assert_ne!(
        w.as_agent("sup-1", &["finding", "dev-1", "high", "  ", "c", "d"])
            .code,
        0
    );
    assert_ne!(
        w.as_agent("sup-1", &["finding", "dev-1", "high", "e", "\u{200b}", "d"])
            .code,
        0
    );
    // One open finding per target.
    raise(&w, "dev-1", "low").ok();
    let second = raise(&w, "dev-1", "high");
    assert_ne!(second.code, 0, "{}", second.text());
}

#[test]
fn a_check_waits_for_the_ack_and_only_the_raising_supervisor_checks() {
    let w = World::standard("check");
    let id = finding_id(&raise(&w, "dev-1", "medium").ok());
    // Not acknowledged yet: no check.
    let early = w.as_agent(
        "sup-1",
        &["finding", "check", &id, "resolved", "looks fine"],
    );
    assert_ne!(early.code, 0, "{}", early.text());
    // The worker reads and acknowledges the correction.
    let inbox = w.as_agent("dev-1", &["inbox", "--json"]).ok().json();
    let message = inbox["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["body"].as_str().unwrap().contains("the correction"))
        .unwrap();
    w.as_agent("dev-1", &["ack", message["messageId"].as_str().unwrap()])
        .ok();
    // Only the raiser checks.
    assert_ne!(
        w.as_agent("pm-1", &["finding", "check", &id, "resolved", "pm says so"])
            .code,
        0
    );
    assert_ne!(
        w.as_agent("dev-1", &["finding", "check", &id, "resolved", "self"])
            .code,
        0
    );
    let checked = w
        .as_agent(
            "sup-1",
            &["finding", "check", &id, "resolved", "verified the fix"],
        )
        .ok();
    assert!(
        checked.stdout.contains("state: resolved"),
        "{}",
        checked.stdout
    );
    let status = w.wire_op("status", &[])["result"].clone();
    let open: Vec<&Value> = status["agentFindings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["state"] == "open")
        .collect();
    assert!(open.is_empty(), "a resolved finding is closed: {status}");
}

#[test]
fn an_unresolved_check_sends_a_second_correction_and_a_second_unresolved_check_escalates() {
    let w = World::standard("escalate");
    let id = finding_id(&raise(&w, "dev-1", "high").ok());
    let ack_all = |agent: &str| {
        let inbox = w.as_agent(agent, &["inbox", "--json"]).ok().json();
        for message in inbox["messages"].as_array().unwrap() {
            w.as_agent(agent, &["ack", message["messageId"].as_str().unwrap()])
                .ok();
        }
    };
    ack_all("dev-1");
    let first = w
        .as_agent(
            "sup-1",
            &["finding", "check", &id, "unresolved", "still broken"],
        )
        .ok();
    assert!(
        first.stdout.contains("interventions: 2"),
        "{}",
        first.stdout
    );
    ack_all("dev-1");
    let second = w
        .as_agent(
            "sup-1",
            &[
                "finding",
                "check",
                &id,
                "unresolved",
                "still broken after two rounds",
            ],
        )
        .ok();
    assert!(second.stdout.contains("escalated"), "{}", second.stdout);
    // A third intervention is never made.
    let third = w.as_agent("sup-1", &["finding", "check", &id, "unresolved", "again"]);
    assert_ne!(third.code, 0, "{}", third.text());
    let pm = w.as_agent("pm-1", &["inbox"]).ok();
    assert!(
        pm.stdout.to_lowercase().contains("escalat"),
        "{}",
        pm.stdout
    );
}

#[test]
fn ten_finding_attempts_a_minute_are_allowed_per_supervisor_and_the_eleventh_is_refused_for_the_rate(
) {
    let w = World::standard("ratelimit");
    for attempt in 0..10 {
        // Each attempt is refused for its target, but it counts.
        let out = w.as_agent("sup-1", &["finding", "ghost-1", "high", "e", "c", "d"]);
        assert_ne!(out.code, 0, "attempt {attempt}");
        assert!(
            !out.stderr.contains("rate"),
            "attempt {attempt}: {}",
            out.stderr
        );
    }
    let eleventh = w.as_agent("sup-1", &["finding", "dev-1", "high", "e", "c", "d"]);
    assert_ne!(eleventh.code, 0);
    assert!(eleventh.stderr.contains("rate"), "{}", eleventh.stderr);
}

#[test]
fn observe_is_for_the_pm_and_a_supervisor_and_needs_a_launcher() {
    let w = World::standard("observe");
    for who in ["dev-1", "rev-1"] {
        let out = w.as_agent(who, &["observe", "dev-2"]);
        assert_eq!(out.code, 4, "{who}: {}", out.text());
        assert!(out.stderr.contains("forbidden"), "{who}: {}", out.stderr);
    }
    // Lines are validated first; then, with no launcher, the answer says so.
    assert_eq!(
        w.wire_as("pm-1", "observe", &["dev-1", "0"])["code"],
        "invalid_request"
    );
    assert_eq!(
        w.wire_as("pm-1", "observe", &["dev-1", "many"])["code"],
        "invalid_request"
    );
    for who in ["pm-1", "sup-1"] {
        let out = w.as_agent(who, &["observe", "dev-1", "20"]);
        assert_eq!(out.code, 4, "{who}: {}", out.text());
        assert!(
            out.stderr.contains("not_configured"),
            "{who}: {}",
            out.stderr
        );
    }
    // An agent cannot observe itself.
    assert_ne!(w.as_agent("sup-1", &["observe", "sup-1"]).code, 0);
}

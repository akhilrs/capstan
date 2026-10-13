//! A plan's warm reviewer through the executable: the second package's `request-review` reuses the reviewer of the first
//! (its worktree moves to the new commit, nothing is spawned) and the plan's signoff releases it. The first review is
//! begun in the ledger the way `request-review` does after its spawn, with the plan as the warm plan.

use capstan_blackbox::*;
use serde_json::{json, Value};

fn field(out: &Out, key: &str) -> String {
    out.stdout
        .lines()
        .find_map(|l| l.strip_prefix(&format!("{key}: ")))
        .unwrap_or_else(|| panic!("no {key} in {}", out.stdout))
        .trim()
        .to_string()
}

fn body() -> String {
    let package = |id: &str| {
        json!({
            "id": id, "title": format!("package {id}"), "owns": [format!("src/{id}/")],
            "interfaces": [format!("exports {id}()")], "acceptance": [format!("{id} works")], "estimate_hours": 2,
        })
    };
    json!({"summary": "split", "packages": [package("wp1"), package("wp2")]}).to_string()
}

fn ack_all(w: &World, agent: &str) {
    let inbox = w.as_agent(agent, &["inbox", "--json"]).ok().json();
    for message in inbox["messages"].as_array().unwrap() {
        w.as_agent(agent, &["ack", message["messageId"].as_str().unwrap()])
            .ok();
    }
}

fn warm_rows(w: &mut World) -> Vec<Value> {
    w.while_stopped(|core, _, owner| {
        core.warm_reviewers(owner)
            .unwrap()
            .as_array()
            .unwrap()
            .clone()
    })
}

#[test]
fn the_second_package_reuses_the_reviewer_and_signoff_releases_it() {
    let mut w = World::builder("warm")
        .config(TEAM_CONFIG)
        .launching()
        .agents(&[
            agent("pm-1", "pm", "PM"),
            agent("arch-1", "architect", "Developer"),
            agent("dev-1", "developer", "Developer"),
            agent("dev-2", "developer", "Developer"),
            agent("rev-1", "reviewer", "Verifier"),
            agent("rev-3", "reviewer", "Verifier"),
        ])
        .build();
    let plan = field(
        &w.as_agent("pm-1", &["plan", "open", "normal", "a plan"])
            .ok(),
        "planId",
    );
    w.as_agent("arch-1", &["plan", "submit", &plan, &body()])
        .ok();
    w.as_agent("pm-1", &["plan", "assign", &plan, "wp1", "dev-1"])
        .ok();
    w.as_agent("pm-1", &["plan", "assign", &plan, "wp2", "dev-2"])
        .ok();
    let (c1, _) = w.give_branch("dev-1", "one.txt", "one\n", "feat: package one");
    let (c2, _) = w.give_branch("dev-2", "two.txt", "two\n", "feat: package two");
    // The reviewer sits in a worktree of its own, as a spawned one does.
    w.give_branch("rev-1", "review.txt", "r\n", "chore: reviewer worktree");
    ack_all(&w, "dev-1");
    let r1 = field(
        &w.as_agent("dev-1", &["report", &c1, "one"]).ok(),
        "reportId",
    );
    ack_all(&w, "dev-2");
    let r2 = field(
        &w.as_agent("dev-2", &["report", &c2, "two"]).ok(),
        "reportId",
    );

    // The first review, as request-review leaves it after its spawn: the plan's warm reviewer is rev-1.
    let credential = w.token("pm-1").to_string();
    let (subject, plan_id) = (r1.clone(), plan.clone());
    w.while_stopped(move |core, ctx, _| {
        core.begin_review(
            &ctx(&credential),
            &json!({"subjectId": subject, "reviewerRole": "reviewer", "reviewerAgentId": "rev-1", "warmPlanId": plan_id}),
        )
        .expect("the first review begins");
    });
    w.as_agent("rev-1", &["review", "pass", "package one is fine"])
        .ok();
    assert_eq!(warm_rows(&mut w).len(), 1);

    // The second package: the same reviewer, moved to the new commit; no reviewer is spawned (no Herdr exists here).
    let requested = w.as_agent("pm-1", &["request-review", &r2]).ok();
    assert_eq!(field(&requested, "reviewerAgentId"), "rev-1");
    assert_eq!(field(&requested, "reviewerReused"), "true");
    let brief = w.as_agent("rev-1", &["inbox"]).ok().stdout;
    assert!(brief.contains(&c2) && brief.contains("Scope:"), "{brief}");
    w.as_agent("rev-1", &["review", "pass", "package two is fine"])
        .ok();
    assert_eq!(warm_rows(&mut w)[0]["reviewsCount"], 2);

    // Integrate, review the integration with another reviewer, and sign off: the warm reviewer is released.
    let merged = w.as_agent("arch-1", &["integrate", &r1, &r2]).ok();
    let integration = field(&merged, "integrationId");
    w.begin_review("arch-1", &integration, "reviewer", "rev-3");
    w.as_agent("rev-3", &["review", "pass", "the merge is fine"])
        .ok();
    w.as_agent("arch-1", &["plan", "signoff", &plan, &integration, "done"])
        .ok();
    assert!(warm_rows(&mut w).is_empty());
}

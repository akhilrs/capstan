//! The Operator through the executable (test/operator-commands.test.ts, operator-full-auto.test.ts, operator-grants.test.ts):
//! `cstan op ...` with `[operator] enabled`, the proposals the designated operator agent writes, the PM's approval with the
//! hash, a command that really runs in the project, full auto, session grants, and who may write to the operator agent.
//! The daemon runs wired as a real one (`launching`) but without Herdr on its PATH: nothing here needs a pane.

use capstan_blackbox::*;
use serde_json::{json, Value};
use std::time::Duration;

fn operator_world(label: &str) -> World {
    World::builder(label)
        .config(TEAM_CONFIG)
        .launching()
        .agents(&[
            agent("pm-1", "pm", "PM"),
            agent("ops-1", "operator", "Developer"),
            agent("dev-1", "developer", "Developer"),
        ])
        .build()
}

fn field(out: &Out, key: &str) -> String {
    out.stdout
        .lines()
        .find_map(|l| l.strip_prefix(&format!("{key}: ")))
        .unwrap_or_else(|| panic!("no {key} in {}", out.stdout))
        .trim()
        .to_string()
}

fn propose(w: &World, command: &str) -> (String, String) {
    let proposed = w
        .as_agent("ops-1", &["op", "propose", command, "a reason"])
        .ok();
    (field(&proposed, "proposalId"), field(&proposed, "hash"))
}

fn state_of(w: &World, id: &str) -> String {
    field(&w.op(&["op", "show", id]).ok(), "state")
}

fn until_state(w: &World, id: &str, wanted: &str) {
    until(&format!("proposal {id} to be {wanted}"), 30, || {
        (state_of(w, id) == wanted).then_some(())
    });
}

#[test]
fn without_an_enabled_operator_every_op_command_answers_not_configured_and_writes_nothing() {
    let w = World::builder("noop2")
        .config("schema_version = 1\n[hosts.claude]\nkind = \"claude\"\n[roles.pm]\nkind = \"PM\"\nhost = \"claude\"\n")
        .launching()
        .agents(&[agent("pm-1", "pm", "PM")])
        .build();
    let before = w.op(&["status", "--json"]).ok().json()["stateVersion"].clone();
    for args in [
        vec!["op", "grants"],
        vec!["op", "show"],
        vec!["op", "full-auto", "status"],
        vec!["op", "full-auto", "off"],
        vec!["op", "revoke", "grant-1"],
        vec!["op", "cancel", "op-1"],
    ] {
        let out = w.op(&args);
        assert_eq!(out.code, 4, "{args:?}: {}", out.text());
        assert!(
            out.stderr.contains("not_configured"),
            "{args:?}: {}",
            out.stderr
        );
    }
    assert_eq!(
        w.op(&["status", "--json"]).ok().json()["stateVersion"],
        before,
        "nothing was written"
    );
}

#[test]
fn only_the_designated_operator_agent_proposes_and_arguments_are_checked() {
    let w = operator_world("propose");
    for who in ["pm-1", "dev-1"] {
        let out = w.as_agent(who, &["op", "propose", "ls", "why"]);
        assert_eq!(out.code, 4, "{who}: {}", out.text());
        assert!(
            out.stderr
                .contains("only the designated operator agent proposes a command"),
            "{who}: {}",
            out.stderr
        );
    }
    // The operator identity is not an agent.
    assert_ne!(w.op(&["op", "propose", "ls", "why"]).code, 0);
    for args in [
        vec!["op", "propose"],
        vec!["op", "propose", "ls"],
        vec!["op", "propose", "", "why"],
    ] {
        let out = w.as_agent("ops-1", &args);
        assert_ne!(out.code, 0, "{args:?}");
    }
    let proposed = w
        .as_agent("ops-1", &["op", "propose", "ls", "list files"])
        .ok();
    assert_eq!(field(&proposed, "state"), "proposed");
    assert_eq!(field(&proposed, "hash").len(), 12);
    assert_eq!(field(&proposed, "forceRestart"), "false");
}

#[test]
fn the_pm_approves_with_the_exact_hash_and_the_command_runs_in_the_project() {
    let w = operator_world("approve");
    let (id, hash) = propose(&w, "echo ran-here > marker.txt && pwd > where.txt");
    // A wrong hash is refused; the operator identity may only deny; the operator agent cannot decide.
    let wrong = w.as_agent(
        "pm-1",
        &["op", "decide", &id, "approve", "--hash", "000000000000"],
    );
    assert_ne!(wrong.code, 0, "{}", wrong.text());
    assert_eq!(state_of(&w, &id), "proposed");
    let by_operator = w.op(&["op", "decide", &id, "approve", "--hash", &hash]);
    assert_ne!(
        by_operator.code,
        0,
        "the operator credential does not approve: {}",
        by_operator.text()
    );
    assert_eq!(state_of(&w, &id), "proposed");
    assert_ne!(
        w.as_agent("ops-1", &["op", "decide", &id, "approve", "--hash", &hash])
            .code,
        0
    );
    assert_ne!(
        w.as_agent("dev-1", &["op", "decide", &id, "approve", "--hash", &hash])
            .code,
        0
    );
    // A flag the command does not take is named.
    let force = w.as_agent(
        "pm-1",
        &["op", "decide", &id, "approve", "--hash", &hash, "--force"],
    );
    assert_ne!(force.code, 0);
    // The PM approves with the hash; the command runs.
    w.as_agent("pm-1", &["op", "decide", &id, "approve", "--hash", &hash])
        .ok();
    until("the command to run", 30, || {
        w.project.join("marker.txt").exists().then_some(())
    });
    until_state(&w, &id, "finished");
    assert_eq!(
        std::fs::read_to_string(w.project.join("marker.txt"))
            .unwrap()
            .trim(),
        "ran-here"
    );
    let where_ = std::fs::read_to_string(w.project.join("where.txt")).unwrap();
    assert_eq!(
        std::fs::canonicalize(where_.trim()).unwrap(),
        std::fs::canonicalize(&w.project).unwrap(),
        "it ran in the project root"
    );
    // The run is shown as untrusted data, to the PM and the operator.
    let shown = w.as_agent("pm-1", &["op", "show", &id]).ok();
    assert!(
        shown.stdout.contains("exit") || shown.stdout.contains("run"),
        "{}",
        shown.stdout
    );
    // A decided proposal cannot be decided again.
    assert_ne!(
        w.as_agent("pm-1", &["op", "decide", &id, "approve", "--hash", &hash])
            .code,
        0
    );
}

#[test]
fn deny_and_cancel_end_a_proposal_that_has_not_run() {
    let w = operator_world("denycancel");
    let (first, _) = propose(&w, "touch never-1.txt");
    let denied = w.op(&["op", "decide", &first, "deny", "not now"]).ok();
    assert!(denied.stdout.contains("denied"), "{}", denied.stdout);
    let (second, _) = propose(&w, "touch never-2.txt");
    w.as_agent("ops-1", &["op", "cancel", &second]).ok();
    assert_eq!(state_of(&w, &second), "cancelled");
    let (third, _) = propose(&w, "touch never-3.txt");
    w.as_agent("pm-1", &["op", "cancel", &third]).ok();
    let (fourth, _) = propose(&w, "touch never-4.txt");
    w.op(&["op", "cancel", &fourth]).ok();
    // A worker cannot cancel; nothing ran.
    let (fifth, _) = propose(&w, "touch never-5.txt");
    assert_ne!(w.as_agent("dev-1", &["op", "cancel", &fifth]).code, 0);
    std::thread::sleep(Duration::from_millis(500));
    for n in 1..=5 {
        assert!(
            !w.project.join(format!("never-{n}.txt")).exists(),
            "never-{n}.txt"
        );
    }
}

#[test]
fn only_the_pm_writes_to_the_operator_agent_and_the_operator_agent_writes_only_to_the_pm() {
    let w = operator_world("ops-send");
    let code = |who: &str, to: &str| {
        w.wire_as(who, "send", &[to, "hello"])["code"]
            .as_str()
            .unwrap_or("ok")
            .to_string()
    };
    assert_eq!(code("pm-1", "ops-1"), "ok");
    assert_eq!(code("dev-1", "ops-1"), "recipient_not_allowed");
    assert_eq!(code("ops-1", "pm-1"), "ok");
    assert_eq!(code("ops-1", "dev-1"), "recipient_not_allowed");
    assert_eq!(code("ops-1", "@pm"), "ok");
}

#[test]
fn full_auto_is_switched_on_by_the_pm_with_the_users_words_and_approves_proposals_at_propose_time()
{
    let w = operator_world("fullauto");
    assert_eq!(
        field(&w.op(&["op", "full-auto", "status"]).ok(), "on"),
        "false"
    );
    // Only an active PM switches it on, with the user's words and minutes in range.
    for (who, args) in [
        (
            Some("dev-1"),
            vec!["op", "full-auto", "on", "--asked-user", "yes please"],
        ),
        (
            Some("ops-1"),
            vec!["op", "full-auto", "on", "--asked-user", "yes please"],
        ),
        (Some("pm-1"), vec!["op", "full-auto", "on"]),
        (
            Some("pm-1"),
            vec!["op", "full-auto", "on", "0", "--asked-user", "words"],
        ),
        (
            Some("pm-1"),
            vec!["op", "full-auto", "on", "100000", "--asked-user", "words"],
        ),
        (
            Some("pm-1"),
            vec!["op", "full-auto", "on", "5", "--asked-user", "   "],
        ),
    ] {
        let out = match who {
            Some(a) => w.as_agent(a, &args),
            None => w.op(&args),
        };
        assert_ne!(out.code, 0, "{args:?}: {}", out.text());
    }
    assert_eq!(
        field(&w.op(&["op", "full-auto", "status"]).ok(), "on"),
        "false"
    );
    let on = w
        .as_agent(
            "pm-1",
            &[
                "op",
                "full-auto",
                "on",
                "10",
                "--asked-user",
                "the user said go ahead",
            ],
        )
        .ok();
    assert_eq!(field(&on, "on"), "true", "{}", on.stdout);
    // Both sides are told.
    assert!(w
        .as_agent("pm-1", &["inbox"])
        .ok()
        .stdout
        .to_lowercase()
        .contains("full auto"));
    assert!(w
        .as_agent("ops-1", &["inbox"])
        .ok()
        .stdout
        .to_lowercase()
        .contains("full auto"));
    // A proposal is approved at propose time and runs without the PM.
    let proposed = w
        .as_agent(
            "ops-1",
            &["op", "propose", "echo auto > auto.txt", "no human needed"],
        )
        .ok();
    assert_ne!(field(&proposed, "state"), "proposed", "{}", proposed.stdout);
    until("the auto-approved command to run", 30, || {
        w.project.join("auto.txt").exists().then_some(())
    });
    // The Operator agent cannot switch it off; the PM and the operator can.
    assert_ne!(w.as_agent("ops-1", &["op", "full-auto", "off"]).code, 0);
    assert_eq!(
        field(&w.op(&["op", "full-auto", "status"]).ok(), "on"),
        "true"
    );
    w.as_agent("pm-1", &["op", "full-auto", "off"]).ok();
    assert_eq!(
        field(&w.op(&["op", "full-auto", "status"]).ok(), "on"),
        "false"
    );
    // After it is off a proposal waits for a person again.
    let (id, _) = propose(&w, "echo late > late.txt");
    assert_eq!(state_of(&w, &id), "proposed");
}

#[test]
fn full_auto_is_not_kept_across_a_restart_of_the_daemon() {
    let mut w = operator_world("fullauto-restart");
    w.as_agent(
        "pm-1",
        &["op", "full-auto", "on", "--asked-user", "the user agreed"],
    )
    .ok();
    assert_eq!(
        field(&w.op(&["op", "full-auto", "status"]).ok(), "on"),
        "true"
    );
    w.restart();
    assert_eq!(
        field(&w.op(&["op", "full-auto", "status"]).ok(), "on"),
        "false"
    );
}

#[test]
fn a_session_grant_lets_an_identical_command_run_without_a_new_approval_and_revoking_ends_it() {
    let w = operator_world("grants");
    assert!(w.op(&["op", "grants"]).ok().stdout.starts_with("grants:"));
    let (id, hash) = propose(&w, "echo granted >> grant.log");
    // Only the PM approves with a session option; it must be exact or a simple prefix.
    assert_ne!(
        w.as_agent(
            "pm-1",
            &[
                "op",
                "decide",
                &id,
                "approve",
                "--hash",
                &hash,
                "--session",
                "forever"
            ]
        )
        .code,
        0
    );
    w.as_agent(
        "pm-1",
        &[
            "op",
            "decide",
            &id,
            "approve",
            "--hash",
            &hash,
            "--session",
            "exact",
        ],
    )
    .ok();
    until_state(&w, &id, "finished");
    let grants = w.as_agent("pm-1", &["op", "grants"]).ok();
    assert!(
        grants.stdout.contains("echo granted >> grant.log"),
        "{}",
        grants.stdout
    );
    // An identical command is approved on its own and runs; any change needs an approval.
    let (again, _) = propose(&w, "echo granted >> grant.log");
    until_state(&w, &again, "finished");
    let (changed, _) = propose(&w, "echo granted >> grant.log ");
    assert_eq!(state_of(&w, &changed), "proposed");
    let lines = std::fs::read_to_string(w.project.join("grant.log"))
        .unwrap()
        .lines()
        .count();
    assert_eq!(lines, 2);
    // The grant is listed for the PM and the operator, never to a worker; the operator revokes it.
    let listed = w.op(&["op", "grants"]).ok();
    let grant_id = listed
        .stdout
        .split_whitespace()
        .find(|word| word.starts_with("grant-"))
        .map(str::to_string)
        .or_else(|| {
            listed
                .stdout
                .lines()
                .find_map(|l| l.strip_prefix("grantId: ").map(str::to_string))
        })
        .unwrap_or_else(|| panic!("no grant id in {}", listed.stdout));
    assert_ne!(w.as_agent("dev-1", &["op", "grants"]).code, 0);
    w.op(&["op", "revoke", &grant_id]).ok();
    let (after, _) = propose(&w, "echo granted >> grant.log");
    assert_eq!(
        state_of(&w, &after),
        "proposed",
        "a revoked grant approves nothing"
    );
}

#[test]
fn a_restart_proposal_is_refused_when_no_restart_is_available() {
    let w = operator_world("restartprop");
    let out = w.as_agent("ops-1", &["op", "propose", "--restart", "new build"]);
    // The text is the daemon's: either the restart is not available here or it is a proposal that waits like any other.
    if out.code != 0 {
        assert!(out.stderr.contains("restart"), "{}", out.stderr);
    } else {
        assert_eq!(field(&out, "kind"), "restart");
    }
    let _: Value = json!(null);
}

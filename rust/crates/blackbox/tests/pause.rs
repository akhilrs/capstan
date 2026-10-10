//! Pausing the run or one agent (test/pause.test.ts): who may, what a reason must be, what status and the CLI show, which
//! commands are refused while the run is paused and which keep working, and that a pause outlives a daemon restart.

use capstan_blackbox::*;
use serde_json::{json, Value};

fn pause_of(w: &World) -> Value {
    w.op(&["status", "--json"]).ok().json()["pause"].clone()
}

#[test]
fn the_operator_and_the_pm_pause_and_resume_a_worker_and_a_worker_may_not() {
    let w = World::standard("pausewho");
    // A reason is required, the id must exist, and doubles are refused.
    assert_eq!(w.wire_op("pause", &["dev-1"])["code"], "invalid_request");
    assert_eq!(
        w.wire_op("pause", &["ghost-1", "--reason", "x"])["code"],
        "unknown_agent"
    );
    assert_eq!(w.wire_op("pause", &["--reason"])["code"], "invalid_request");
    assert_eq!(
        w.wire_as("dev-1", "pause", &["dev-2", "--reason", "mine"])["code"],
        "forbidden"
    );
    assert_eq!(
        w.wire_as("dev-1", "resume", &["--reason", "mine"])["code"],
        "forbidden"
    );

    let paused = w.op(&["pause", "dev-1", "--reason", "looking at it"]).ok();
    assert!(
        paused.stdout.contains("state: paused") && paused.stdout.contains("looking at it"),
        "{}",
        paused.stdout
    );
    let again = w.wire_op("pause", &["dev-1", "--reason", "twice"]);
    assert_eq!(again["ok"], false, "a double pause is refused: {again}");
    let status = pause_of(&w);
    assert_eq!(status["agents"][0]["agentId"], "dev-1");
    assert_eq!(status["agents"][0]["reason"], "looking at it");
    assert!(
        status["agents"][0]["pausedAt"].is_string()
            || status["agents"][0]["since"].is_string()
            || status["agents"][0].get("at").is_some(),
        "{status}"
    );
    // The text status prints a PAUSED line for it.
    let text = w.op(&["status"]).ok();
    assert!(
        text.stdout.contains("PAUSED")
            && text.stdout.contains("dev-1")
            && text.stdout.contains("looking at it"),
        "{}",
        text.stdout
    );
    // The PM resumes it; a second resume is refused.
    let resumed = w
        .as_agent("pm-1", &["resume", "dev-1", "--reason", "all clear"])
        .ok();
    assert!(
        resumed.stdout.contains("state: active") || resumed.stdout.contains("resumed"),
        "{}",
        resumed.stdout
    );
    assert_eq!(pause_of(&w)["agents"], json!([]));
    assert_eq!(
        w.wire_op("resume", &["dev-1", "--reason", "again"])["ok"],
        false
    );
}

#[test]
fn a_run_pause_holds_new_work_and_everything_else_keeps_working() {
    let w = World::standard("runpause");
    let before = w.op(&["pause", "--reason", "freezing for a look"]).ok();
    assert!(
        before.stdout.contains("scope: run") && before.stdout.contains("state: paused"),
        "{}",
        before.stdout
    );
    assert_eq!(pause_of(&w)["run"]["reason"], "freezing for a look");
    // New work is refused with the reason.
    for (who, args) in [
        (None, vec!["spawn", "developer"]),
        (Some("pm-1"), vec!["spawn", "developer"]),
        (
            Some("pm-1"),
            vec!["request-review", "a".repeat(40).leak() as &str],
        ),
        (
            Some("pm-1"),
            vec!["integrate", "a".repeat(40).leak() as &str],
        ),
    ] {
        let out = match who {
            Some(a) => w.as_agent(a, &args),
            None => w.op(&args),
        };
        assert_ne!(out.code, 0, "{args:?} while paused: {}", out.text());
    }
    let launch = w.wire_op("launch", &[]);
    assert_eq!(launch["ok"], false);
    let spawn = w.wire_op("spawn", &["developer"]);
    assert!(
        spawn["message"]
            .as_str()
            .unwrap()
            .contains("freezing for a look")
            || spawn["code"] == "run_paused"
            || spawn["code"] == "paused",
        "{spawn}"
    );
    // These keep working while paused.
    w.as_agent("pm-1", &["send", "dev-1", "still talking"]).ok();
    assert!(
        w.as_agent("dev-1", &["inbox"])
            .ok()
            .stdout
            .contains("no messages"),
        "a paused run holds the workers' mail"
    );
    w.as_agent("dev-1", &["send", "@pm", "noted"]).ok();
    assert!(
        w.as_agent("pm-1", &["inbox"]).ok().stdout.contains("noted"),
        "the PM keeps receiving"
    );
    w.op(&["status", "--json"]).ok();
    w.op(&["inbox", "dev-1"]).ok();
    // Resume and the refused work is accepted again (here: the launcher is simply not configured).
    w.op(&["resume", "--reason", "carry on"]).ok();
    assert_eq!(pause_of(&w)["run"], Value::Null);
    assert!(
        w.as_agent("dev-1", &["inbox"])
            .ok()
            .stdout
            .contains("still talking"),
        "the held mail arrives after resume"
    );
    let spawn = w.wire_op("spawn", &["developer"]);
    assert_eq!(spawn["code"], "not_configured", "{spawn}");
}

#[test]
fn a_supervisor_cannot_raise_a_finding_against_a_paused_agent() {
    let w = World::standard("pausefinding");
    w.op(&["pause", "dev-1", "--reason", "hold"]).ok();
    let refused = w.as_agent(
        "sup-1",
        &[
            "finding",
            "dev-1",
            "high",
            "evidence",
            "correction",
            "done when",
        ],
    );
    assert_ne!(refused.code, 0, "{}", refused.text());
    assert!(refused.stderr.contains("paused"), "{}", refused.stderr);
    w.op(&["resume", "dev-1", "--reason", "go"]).ok();
    w.as_agent(
        "sup-1",
        &[
            "finding",
            "dev-1",
            "high",
            "evidence",
            "correction",
            "done when",
        ],
    )
    .ok();
}

#[test]
fn pauses_survive_a_restart_of_the_daemon_and_ending_an_agent_closes_its_pause() {
    let mut w = World::standard("pauserestart");
    w.op(&["pause", "dev-1", "--reason", "agent hold"]).ok();
    w.op(&["pause", "--reason", "run hold"]).ok();
    w.restart();
    let pause = pause_of(&w);
    assert_eq!(pause["run"]["reason"], "run hold");
    assert_eq!(pause["agents"][0]["agentId"], "dev-1");
    // Ending a paused agent closes its pause in the same step.
    w.while_stopped(|core, ctx, owner| {
        core.end_agent(&ctx(owner), "dev-1", None).unwrap();
    });
    let pause = pause_of(&w);
    assert_eq!(pause["agents"], json!([]), "{pause}");
    assert_eq!(
        pause["run"]["reason"], "run hold",
        "the run pause is untouched"
    );
}

#[test]
fn interrupt_needs_the_prompt_relay_and_pauses_nothing_without_it() {
    let w = World::standard("interrupt");
    let refused = w.wire_op("pause", &["dev-1", "--reason", "stop now", "--interrupt"]);
    assert_eq!(refused["code"], "not_configured", "{refused}");
    assert!(
        refused["message"]
            .as_str()
            .unwrap()
            .contains("nothing was paused"),
        "{refused}"
    );
    assert_eq!(pause_of(&w)["agents"], json!([]));
}

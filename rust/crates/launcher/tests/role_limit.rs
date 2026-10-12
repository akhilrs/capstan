//! `role_limit` in the spawn path and the successor-aware `replace`, against the stub Herdr, real git and a real ledger.
mod common;

use capstan_launcher::api::{LaunchState, LauncherService, ReplaceResult};
use common::engine::Scenario;
use serde_json::{json, Value};

fn launched(sc: &Scenario) {
    let result = sc.launcher.launch_pm().expect("the PM launches");
    assert_eq!(result.state, Some(LaunchState::Started));
}

fn spawn(sc: &Scenario, role: &str) -> String {
    let result = sc
        .launcher
        .spawn(role, &Default::default())
        .unwrap_or_else(|e| panic!("spawn {role}: {} {}", e.code, e.message));
    result.agent_id
}

fn refused(sc: &Scenario, role: &str) -> (String, String) {
    let error = sc
        .launcher
        .spawn(role, &Default::default())
        .expect_err("the spawn is refused");
    (error.code, error.message)
}

fn active(sc: &Scenario, role: &str) -> Vec<String> {
    let role = role.to_string();
    sc.ledger(move |core| {
        Ok(core
            .list_agents()?
            .as_array()
            .unwrap()
            .iter()
            .filter(|a| a["state"] == "active" && a["roleName"] == role.as_str())
            .map(|a| a["agentId"].as_str().unwrap().to_string())
            .collect::<Vec<_>>())
    })
}

fn starts(sc: &Scenario) -> usize {
    sc.take_calls()
        .iter()
        .filter(|c| c["m"] == "startAgent")
        .count()
}

fn replaced_by(sc: &Scenario, predecessor: &str) -> Option<String> {
    let predecessor = predecessor.to_string();
    sc.ledger(move |core| {
        let value = core.active_successor(&predecessor)?;
        Ok(value["agentId"].as_str().map(str::to_string))
    })
}

#[test]
fn a_second_supervisor_is_refused_with_role_limit() {
    let sc = Scenario::new("role-limit-one", json!({"maxWorkers": 3}));
    launched(&sc);
    let first = spawn(&sc, "supervisor");
    let (code, message) = refused(&sc, "supervisor");
    assert_eq!(code, "role_limit");
    assert_eq!(
        message,
        format!("role_limit: {first} is already active (max_active = 1)")
    );
    assert_eq!(active(&sc, "supervisor"), vec![first]);
}

#[test]
fn max_active_two_allows_a_second_supervisor_and_refuses_a_third() {
    let two = Scenario::new("role-limit-two", json!({"maxActive": {"supervisor": 2}}));
    launched(&two);
    spawn(&two, "supervisor");
    spawn(&two, "supervisor");
    let (code, message) = refused(&two, "supervisor");
    assert_eq!(code, "role_limit");
    assert!(message.ends_with("(max_active = 2)"), "{message}");
    assert_eq!(active(&two, "supervisor").len(), 2);
}

#[test]
fn replace_adopts_the_successor_an_auto_start_created_after_the_release() {
    let sc = Scenario::new("incident", json!({}));
    launched(&sc);
    let old = spawn(&sc, "supervisor");
    sc.launcher.release(&old).expect("released");
    let successor = spawn(&sc, "supervisor");
    sc.take_calls();
    let result = sc.launcher.replace(&old).expect("replace answers");
    assert_eq!(
        result,
        ReplaceResult::AlreadyReplaced {
            predecessor: old.clone(),
            agent_id: successor.clone()
        }
    );
    assert_eq!(result.to_value()["state"], "already_replaced");
    assert_eq!(starts(&sc), 0, "nothing was started");
    assert_eq!(active(&sc, "supervisor"), vec![successor.clone()]);
    // The ledger now holds the replacement: asking again gives the same answer.
    let again = sc.launcher.replace(&old).expect("replace answers again");
    assert_eq!(again.to_value()["agentId"], Value::String(successor));
}

#[test]
fn a_role_with_room_for_two_never_adopts_an_unrelated_agent() {
    let sc = Scenario::new("no-adoption", json!({"maxActive": {"supervisor": 2}}));
    launched(&sc);
    let x = spawn(&sc, "supervisor");
    let other = spawn(&sc, "supervisor");
    sc.launcher.release(&x).expect("released");
    let unrelated = spawn(&sc, "supervisor");
    assert_eq!(active(&sc, "supervisor").len(), 2);
    let error = sc.launcher.replace(&x).expect_err("the limit is reached");
    assert_eq!(error.code, "role_limit");
    assert_eq!(
        active(&sc, "supervisor").len(),
        2,
        "nothing else was released"
    );
    sc.launcher.release(&other).expect("released");
    let result = sc.launcher.replace(&x).expect("replace starts a new agent");
    let ReplaceResult::Replaced(done) = result else {
        panic!("a new agent starts");
    };
    assert_ne!(done.spawn.agent_id, unrelated);
    assert_eq!(replaced_by(&sc, &x), Some(done.spawn.agent_id.clone()));
}

#[test]
fn replacing_an_active_supervisor_releases_it_and_starts_the_successor() {
    let sc = Scenario::new("replace-active", json!({}));
    launched(&sc);
    let old = spawn(&sc, "supervisor");
    let result = sc.launcher.replace(&old).expect("replaced");
    let ReplaceResult::Replaced(done) = result else {
        panic!("replaced");
    };
    assert_eq!(active(&sc, "supervisor"), vec![done.spawn.agent_id]);
}

#[test]
fn a_spawn_racing_a_replace_never_leaves_two_active_supervisors() {
    let sc = Scenario::new("race", json!({}));
    launched(&sc);
    let old = spawn(&sc, "supervisor");
    let (replaced, spawned) = std::thread::scope(|scope| {
        let a = scope.spawn(|| sc.launcher.replace(&old));
        let b = scope.spawn(|| sc.launcher.spawn("supervisor", &Default::default()));
        (a.join().unwrap(), b.join().unwrap())
    });
    replaced.expect("the replace succeeds");
    assert_eq!(
        spawned.expect_err("the spawn is refused").code,
        "role_limit"
    );
    assert_eq!(active(&sc, "supervisor").len(), 1);
}

fn record_lost(sc: &Scenario, agent: &str) {
    let agent = agent.to_string();
    sc.ledger(move |core| {
        core.record_agent_lost(
            &Scenario::owner(core),
            &json!({"agentId": agent, "reason": "process_exited"}),
        )
    });
}

#[test]
fn replace_starts_a_new_agent_once_the_recorded_successor_has_ended() {
    let sc = Scenario::new("successor-ended", json!({}));
    launched(&sc);
    let old = spawn(&sc, "supervisor");
    sc.launcher.release(&old).expect("released");
    let successor = spawn(&sc, "supervisor");
    let adopted = sc.launcher.replace(&old).expect("replace answers");
    assert_eq!(
        adopted.to_value()["agentId"],
        Value::String(successor.clone())
    );
    sc.launcher.release(&successor).expect("the successor ends");
    assert!(active(&sc, "supervisor").is_empty());
    sc.take_calls();
    let result = sc
        .launcher
        .replace(&old)
        .expect("replace starts a new agent");
    let ReplaceResult::Replaced(done) = result else {
        panic!("a new agent starts, not already_replaced");
    };
    assert_eq!(starts(&sc), 1);
    assert_eq!(
        done.predecessor, successor,
        "the ended end of the chain is replaced"
    );
    assert_ne!(done.spawn.agent_id, successor);
    assert_eq!(active(&sc, "supervisor"), vec![done.spawn.agent_id.clone()]);
    assert_eq!(
        replaced_by(&sc, &successor),
        Some(done.spawn.agent_id.clone())
    );
    // The chain from the first agent now ends at the new one.
    assert_eq!(replaced_by(&sc, &old), Some(done.spawn.agent_id.clone()));
    let again = sc.launcher.replace(&old).expect("replace answers again");
    assert_eq!(again.to_value()["state"], "already_replaced");
    assert_eq!(
        again.to_value()["agentId"],
        Value::String(done.spawn.agent_id.clone())
    );
}

#[test]
fn replace_releases_a_lost_successor_and_starts_without_a_role_limit_deadlock() {
    let sc = Scenario::new("successor-lost", json!({}));
    launched(&sc);
    let old = spawn(&sc, "supervisor");
    sc.launcher.release(&old).expect("released");
    let successor = spawn(&sc, "supervisor");
    sc.launcher.replace(&old).expect("the link is recorded");
    record_lost(&sc, &successor);
    assert_eq!(
        replaced_by(&sc, &old),
        None,
        "a lost agent is not a successor"
    );
    assert_eq!(refused(&sc, "supervisor").0, "role_limit");
    let result = sc
        .launcher
        .replace(&old)
        .expect("replace starts a new agent");
    let ReplaceResult::Replaced(done) = result else {
        panic!("a new agent starts, not already_replaced");
    };
    assert_eq!(done.predecessor, successor);
    assert_eq!(active(&sc, "supervisor"), vec![done.spawn.agent_id.clone()]);
    assert_eq!(replaced_by(&sc, &old), Some(done.spawn.agent_id));
}

#[test]
fn replacing_the_original_and_the_chain_tip_at_once_never_starts_two_agents() {
    let sc = Scenario::new("tip-race", json!({}));
    launched(&sc);
    let old = spawn(&sc, "supervisor");
    sc.launcher.release(&old).expect("released");
    let tip = spawn(&sc, "supervisor");
    sc.launcher.replace(&old).expect("the link is recorded");
    record_lost(&sc, &tip);
    let (a, b) = std::thread::scope(|scope| {
        let a = scope.spawn(|| sc.launcher.replace(&old));
        let b = scope.spawn(|| sc.launcher.replace(&tip));
        (a.join().unwrap(), b.join().unwrap())
    });
    for result in [a, b] {
        if let Err(error) = result {
            assert_eq!(error.code, "replace_running", "{}", error.message);
        }
    }
    assert_eq!(active(&sc, "supervisor").len(), 1);
}

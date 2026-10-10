//! The launcher behaviours the frozen Node corpus never replayed (the rows `docs/test-map/live.md` listed as not ported for
//! `launcher-lifecycle`, `launcher-spawn` and `launcher-release`), stated directly against the Rust launcher: stub Herdr, real
//! git in a scratch repository and a real ledger, built by `common::engine::Scenario`. Each test names the Node test it
//! stands for. Nothing here starts Node or Herdr.
mod common;

use capstan_launcher::api::{LaunchState, LauncherError, LauncherService};
use common::engine::Scenario;
use serde_json::{json, Value};
use std::sync::Arc;

fn scenario(name: &str, spec: Value) -> Scenario {
    Scenario::new(name, spec)
}

fn code_of<T: std::fmt::Debug>(result: Result<T, LauncherError>) -> (String, String) {
    let error = result.expect_err("the operation is refused");
    (error.code, error.message)
}

/// Seat, actor and agent for `agent_id` of role `role_name`, through the ledger as a spawn does.
fn register(sc: &Scenario, agent_id: &str, role_name: &str, kind: &str) {
    let (agent_id, role_name, kind) = (
        agent_id.to_string(),
        role_name.to_string(),
        kind.to_string(),
    );
    sc.ledger(move |core| {
        let seat = format!("{agent_id}-seat");
        let step = |op: &str, args: Value| {
            let context = serde_json::to_value(Scenario::owner(core)).unwrap();
            core.dispatch(op, &[context, args])
        };
        step("createSeat", json!({"seatId": seat, "name": agent_id, "role": kind}))?;
        let actor = step("createActor", json!({"displayName": agent_id, "role": kind, "seatId": seat}))?;
        step(
            "registerAgent",
            json!({"agentId": agent_id, "roleName": role_name, "seatId": seat, "actorId": actor["actorId"]}),
        )?;
        Ok(())
    });
}

fn active_agents(sc: &Scenario) -> Vec<String> {
    sc.ledger(|core| {
        Ok(core
            .list_agents()?
            .as_array()
            .unwrap()
            .iter()
            .filter(|a| a["state"] == "active")
            .map(|a| a["agentId"].as_str().unwrap().to_string())
            .collect())
    })
}

fn launched(sc: &Scenario) {
    let result = sc.launcher.launch_pm().expect("the PM launches");
    assert_eq!(
        result.state,
        Some(LaunchState::Started),
        "{:?}",
        result.to_value()
    );
}

// ----------------------------------------------------------------------------------------------- launcher-lifecycle

/// test "two active PMs make launch refuse with pm_exists"
#[test]
fn two_active_pms_make_launch_refuse_with_pm_exists_and_restart_with_pm_ambiguous() {
    let sc = scenario("pm-exists", json!({}));
    register(&sc, "pm-1", "pm", "PM");
    register(&sc, "pm2-1", "pm2", "PM");
    assert_eq!(code_of(sc.launcher.launch_pm()).0, "pm_exists");
    assert_eq!(code_of(sc.launcher.restart_pm()).0, "pm_ambiguous");
    assert!(sc.take_calls().is_empty(), "nothing was asked of Herdr");
}

/// test "a role whose definition changed is synced again on demand, and a sync that fails is named in the refusal"
#[test]
fn a_role_whose_definition_changed_is_synced_again_on_demand_and_a_failing_sync_is_named() {
    let sc = scenario("resync", json!({}));
    let stale = |hash: &'static str| {
        sc.ledger(move |core| {
            let desired: Vec<Value> = common::engine::ROLES
                .iter()
                .map(|(name, kind)| {
                    let config_hash = if *name == "pm" {
                        hash.repeat(64)
                    } else {
                        common::hash_of(name)
                    };
                    json!({"name": name, "kind": kind, "host": "claude", "configHash": config_hash})
                })
                .collect();
            core.sync_role_definitions(&Scenario::owner(core), &Value::Array(desired))
        })
    };
    stale("f");
    // Without a way to sync, the launch says the role is not synced and leaves nothing behind.
    let (code, _) = code_of(sc.launcher.launch_pm());
    assert_eq!(code, "role_not_synced");
    assert!(active_agents(&sc).is_empty());
    // A sync that fails is named in the refusal: cut, free of control characters, with the failure code.
    *sc.world.sync_roles.lock().unwrap() = Some(Arc::new(|| {
        Err(format!(
            "the ledger refused\nthe \u{1b}[31mchange\u{1b}[0m {}",
            "x".repeat(400)
        ))
    }));
    let mut sc = sc;
    sc.reopen();
    let (code, message) = code_of(sc.launcher.launch_pm());
    assert_eq!(code, "role_not_synced");
    assert!(message.contains("the ledger refused the"), "{message}");
    assert!(message.contains("role_sync_failed"), "{message}");
    assert!(!message.chars().any(char::is_control), "{message:?}");
    assert!(message.chars().count() < 450, "{}", message.chars().count());
    // A sync that heals the roles lets the launch go on.
    let kernel = Arc::clone(&sc.world.kernel);
    *sc.world.sync_roles.lock().unwrap() = Some(Arc::new(move || {
        let desired: Vec<Value> = common::engine::ROLES
            .iter()
            .map(|(name, kind)| json!({"name": name, "kind": kind, "host": "claude", "configHash": common::hash_of(name)}))
            .collect();
        kernel
            .call(move |core| {
                core.sync_role_definitions(&Scenario::owner(core), &Value::Array(desired))
            })
            .map_err(|_| "the kernel is gone".to_string())?
            .map(|_| ())
            .map_err(|e| e.to_string())
    }));
    sc.reopen();
    launched(&sc);
}

/// test "a leftover row without a worktree path is reported as a record waiting to be cleaned up"
#[test]
fn a_leftover_row_without_a_worktree_path_is_reported_as_waiting_to_be_cleaned_up() {
    let sc = scenario("leftover-row", json!({}));
    launched(&sc);
    register(&sc, "developer-1", "developer", "Developer");
    sc.ledger(|core| {
        let context = serde_json::to_value(Scenario::owner(core)).unwrap();
        core.dispatch(
            "recordAgentPane",
            &[
                context,
                json!({"agentId": "developer-1", "workspaceId": "w9", "paneId": "w9:p1", "worktreePath": null, "branch": null, "baseSha": null}),
            ],
        )?;
        core.end_agent(&Scenario::owner(core), "developer-1", None)
    });
    let status = sc.launcher.status().to_value();
    assert_eq!(
        status["cleanupFailed"],
        json!([{"agentId": "developer-1", "reason": "a record of an ended agent is waiting to be cleaned up"}]),
        "{status}"
    );
}

/// test "a project path with a colon is refused because cstan could not be found on PATH"
#[test]
fn a_project_path_with_a_colon_is_refused() {
    let sc = scenario("colon", json!({}));
    let odd = sc.project_root.parent().unwrap().join("a:b");
    let launcher = sc.world.make_launcher_at(&sc.spec, &sc.state_dir, &odd);
    let result = launcher.launch_pm().expect("a refusal is an answer");
    assert_eq!(
        result.state,
        Some(LaunchState::Failed),
        "{:?}",
        result.to_value()
    );
    assert!(
        result.reason.as_deref().unwrap_or("").contains("colon"),
        "{:?}",
        result.reason
    );
    assert!(active_agents(&sc).is_empty());
}

/// test "a failure to report metadata is logged and never fails a start"
#[test]
fn a_failure_to_report_metadata_is_logged_and_never_fails_a_start() {
    let sc = scenario("metadata", json!({}));
    sc.world.stub.apply(&json!({
        "do": "fail", "method": "reportMetadata", "times": 20,
        "error": {"kind": "herdr", "code": "herdr_busy", "message": "herdr is busy"},
    }));
    let result = sc.launcher.launch_pm().expect("the start goes on");
    assert_eq!(
        result.state,
        Some(LaunchState::Started),
        "{:?}",
        result.to_value()
    );
    let events = sc.take_events();
    assert!(
        events.iter().any(|e| e["event"] == "describe_failed"),
        "{events:?}"
    );
}

/// test "overlapping operations are told apart: only the one that started an agent reports the missing variable"
#[test]
fn overlapping_launches_only_the_one_that_started_the_pm_reports_the_missing_variable() {
    let sc = scenario("overlap", json!({"pass": ["MISSING_ONE"], "base": {}}));
    let (first, second) = std::thread::scope(|scope| {
        let first = scope.spawn(|| sc.launcher.launch_pm().unwrap());
        let second = scope.spawn(|| sc.launcher.launch_pm().unwrap());
        (first.join().unwrap(), second.join().unwrap())
    });
    let (started, running) = if first.state == Some(LaunchState::Started) {
        (first, second)
    } else {
        (second, first)
    };
    assert_eq!(started.state, Some(LaunchState::Started));
    assert_eq!(started.missing_env, Some(vec!["MISSING_ONE".to_string()]));
    assert_eq!(
        running.state,
        Some(LaunchState::Running),
        "{:?}",
        running.to_value()
    );
    assert_eq!(running.missing_env, None);
    assert_eq!(running.warning, None);
    let events = sc.world.events.lock().unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| e["event"] == "env_pass_missing")
            .count(),
        1,
        "{events:?}"
    );
}

/// test "restart: a replace the core refuses leaves the old pane and the ledger untouched"
#[test]
fn a_restart_whose_replace_the_core_refuses_leaves_the_old_pane_and_the_ledger_untouched() {
    let sc = scenario("restart-refused", json!({}));
    assert_eq!(
        code_of(sc.launcher.restart_pm()).0,
        "no_pm",
        "a missing PM is refused"
    );
    launched(&sc);
    sc.take_calls();
    // The core refuses the generation change, as it does for a seat that still holds authority.
    sc.ledger(|core| {
        core.kernel()
            .database
            .exec("CREATE TRIGGER refuse_generation BEFORE UPDATE OF generation ON agents BEGIN SELECT RAISE(ABORT, 'the seat still holds authority'); END;")
            .unwrap();
        Ok(())
    });
    let (_, message) = code_of_any(sc.launcher.restart_pm());
    assert!(message.contains("still holds authority"), "{message}");
    assert!(sc.take_calls().is_empty(), "no pane was closed or started");
    let rows = sc.ledger(|core| core.agent_panes(common::engine::OWNER_CREDENTIAL));
    assert_eq!(rows.as_array().unwrap().len(), 1, "{rows}");
}

fn code_of_any<T: std::fmt::Debug>(result: Result<T, LauncherError>) -> (String, String) {
    code_of(result)
}

/// test "a hub made again while a PM is already running closes its empty root pane, and keeps the watch tab"
#[test]
fn a_hub_made_again_while_a_pm_runs_closes_its_empty_root_pane_and_keeps_the_watch_tab() {
    let mut sc = scenario("hub-again", json!({}));
    launched(&sc);
    // A daemon restart: the registry is empty, the watch pane cannot be adopted and its workspace has no room for a tab.
    sc.world.stub.apply(&json!({"do": "forget_registry"}));
    sc.reopen();
    sc.world.stub.apply(&json!({"do": "fail", "method": "adoptShellPane", "times": 2, "error": {"kind": "pane_gone", "message": "the watch pane is gone"}}));
    sc.world.stub.apply(&json!({"do": "fail", "method": "createTab", "error": {"kind": "herdr", "code": "workspace_not_found", "message": "no such workspace"}}));
    sc.take_calls();
    let result = sc.launcher.launch_pm().expect("the PM is running");
    assert_eq!(
        result.state,
        Some(LaunchState::Running),
        "{:?}",
        result.to_value()
    );
    assert_eq!(result.hub.map(|h| h.as_str()), Some("opened"));
    let calls = sc.take_calls();
    let method = |m: &str| calls.iter().filter(|c| c["m"] == m).collect::<Vec<_>>();
    let workspaces = method("createWorkspace");
    assert_eq!(workspaces.len(), 1, "one new project workspace: {calls:?}");
    let renamed = method("renameWorkspace");
    let new_workspace = renamed.last().unwrap()["a"][0]
        .as_str()
        .unwrap()
        .to_string();
    let closed: Vec<String> = method("closePane")
        .iter()
        .map(|c| c["a"][0].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        closed,
        [format!("{new_workspace}:p1")],
        "the empty root pane of the new workspace is closed, and nothing else"
    );
    let tabs = method("createTab");
    assert!(
        tabs.iter()
            .any(|t| t["a"][0]["workspaceId"] == new_workspace.as_str()
                && t["a"][0]["label"] == "watch"),
        "the watch tab is made in the new workspace: {tabs:?}"
    );
}

/// test "a PM start that fails before it takes the new workspace's root pane closes that pane"
#[test]
fn a_pm_start_that_fails_before_it_takes_the_root_pane_closes_that_pane() {
    let sc = scenario("prompt-fails", json!({}));
    sc.world.stub.apply(&json!({"do": "fail", "method": "writePromptFile", "error": {"kind": "herdr", "code": "io_error", "message": "disk full"}}));
    let result = sc
        .launcher
        .launch_pm()
        .expect("a failed start is an answer");
    assert_eq!(
        result.state,
        Some(LaunchState::Failed),
        "{:?}",
        result.to_value()
    );
    assert!(
        result.reason.as_deref().unwrap_or("").contains("disk full"),
        "{:?}",
        result.reason
    );
    let closed: Vec<Value> = sc
        .take_calls()
        .into_iter()
        .filter(|c| c["m"] == "closePane" && c["a"][0] == "w1:p1")
        .collect();
    assert_eq!(
        closed.len(),
        1,
        "the empty root pane is closed once: {closed:?}"
    );
    let hub = sc.ledger(|core| core.fallback_pane(common::engine::OWNER_CREDENTIAL));
    assert!(!hub.is_null(), "the watch tab stays recorded");
}

// ------------------------------------------------------------------------------------------------- launcher-spawn

fn spawn_ok(sc: &Scenario, role: &str) -> Value {
    let result = sc
        .launcher
        .spawn(role, &Default::default())
        .unwrap_or_else(|e| panic!("spawn {role}: {} {}", e.code, e.message));
    assert_eq!(result.state, "started", "{:?}", result.to_value());
    result.to_value()
}

/// test "an extra seat gets a dotted display name, so a role literally named like it still gets its own seat"
#[test]
fn an_extra_seat_gets_a_dotted_display_name_so_a_role_named_like_it_still_gets_its_own_seat() {
    let sc = scenario("dotted-seat", json!({"maxWorkers": 3}));
    launched(&sc);
    spawn_ok(&sc, "developer");
    spawn_ok(&sc, "developer");
    let taken = sc.ledger(|core| {
        Ok(core
            .dispatch(
                "createSeat",
                &[
                    serde_json::to_value(Scenario::owner(core)).unwrap(),
                    json!({"seatId": "other-seat", "name": "developer.2", "role": "Developer"}),
                ],
            )
            .map_err(|e| e.to_string()))
    });
    assert!(
        taken.is_err(),
        "the extra seat already holds the display name developer.2: {taken:?}"
    );
    let free = sc.ledger(|core| {
        core.dispatch(
            "createSeat",
            &[
                serde_json::to_value(Scenario::owner(core)).unwrap(),
                json!({"seatId": "developer-2-seat", "name": "developer-2", "role": "Developer"}),
            ],
        )
    });
    assert_eq!(free["seatId"], "developer-2-seat");
}

/// test "the Architect takes no worker place unless count_toward_worker_limit is true, and its prompt differs from a developer's"
#[test]
fn the_architect_prompt_differs_from_a_developers_and_the_pm_prompt_carries_the_plan_section() {
    let sc = scenario(
        "architect-prompt",
        json!({"maxWorkers": 1, "architect": {"counts": false}}),
    );
    launched(&sc);
    spawn_ok(&sc, "architect");
    spawn_ok(&sc, "developer");
    let (code, message) = code_of(sc.launcher.spawn("developer2", &Default::default()));
    assert_eq!(code, "worker_limit");
    assert!(
        message.contains("1 of 1 workers are active (developer-1)"),
        "{message}"
    );
    let prompts = sc.prompts();
    assert_eq!(prompts.len(), 3, "the PM, the architect and the developer");
    assert!(
        prompts.iter().any(|t| t.contains("You are the architect")),
        "the architect's own prompt"
    );
    assert!(
        prompts
            .iter()
            .any(|t| t.contains("the architect named in it can answer")),
        "a work package names who answers questions"
    );
    assert!(
        prompts[0].contains("Planned work"),
        "the PM prompt carries the plan section"
    );
    let architect = prompts
        .iter()
        .find(|t| t.contains("You are the architect"))
        .unwrap();
    let developer = prompts.last().unwrap();
    assert_ne!(architect, developer);
    assert!(!developer.contains("You are the architect"));
}

/// test "the architect counts toward the worker limit when count_toward_worker_limit is true" (the other half of the same row)
#[test]
fn an_architect_that_counts_takes_a_worker_place() {
    let sc = scenario(
        "architect-counts",
        json!({"maxWorkers": 1, "architect": {"counts": true}}),
    );
    launched(&sc);
    spawn_ok(&sc, "architect");
    assert_eq!(
        code_of(sc.launcher.spawn("developer", &Default::default())).0,
        "worker_limit"
    );
}

/// test "without the Architect no prompt mentions plans"
#[test]
fn without_the_architect_no_prompt_mentions_plans() {
    let sc = scenario("no-architect-prompt", json!({}));
    launched(&sc);
    spawn_ok(&sc, "developer");
    let prompts = sc.prompts();
    assert_eq!(prompts.len(), 2);
    for text in &prompts {
        assert!(
            !text.to_lowercase().contains("architect"),
            "no architect text"
        );
        assert!(!text.contains("cstan plan"), "no plan commands");
    }
}

const BLOCKS: [&str; 2] = ["# [researcher]", "# [roles.researcher]"];

/// The starter template with its researcher blocks uncommented (header line and the key lines that follow, up to the next
/// blank line), the way a person enables the role.
fn researcher_config() -> String {
    let mut on = false;
    capstan_config::starter::STARTER_CONFIG
        .split('\n')
        .map(|line| {
            if BLOCKS.contains(&line) {
                on = true;
            } else if on && !line.starts_with("# ") {
                on = false;
            }
            if on {
                line[2..].to_string()
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// test "the cstan allow rule the launcher appends passes the researcher rule check"
#[test]
fn the_cstan_allow_rule_the_launcher_appends_passes_the_researcher_rule_check() {
    use capstan_config::prompts::CSTAN_ALLOW_RULE;
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("roles")).unwrap();
    std::fs::write(
        root.path()
            .join(capstan_config::starter::DESIGNER_PROMPT_PATH),
        capstan_config::starter::DESIGNER_PROMPT,
    )
    .unwrap();
    let text = researcher_config();
    let plain = capstan_config::parse_config(text.as_bytes(), root.path())
        .expect("the starter with the researcher enabled loads");
    assert!(plain.researcher.enabled, "the researcher is on");
    // The same role with the rule the launcher adds to every agent's allow list still passes the researcher rule check.
    let (before, researcher_block) = text
        .split_once("[roles.researcher]")
        .expect("the researcher role");
    let allow_line = researcher_block
        .lines()
        .find(|l| l.starts_with("allow = ["))
        .expect("an allow line in the researcher role")
        .to_string();
    let widened = format!(
        "{before}[roles.researcher]{}",
        researcher_block.replacen(
            &allow_line,
            &allow_line.replacen(
                "allow = [",
                &format!("allow = [\"{CSTAN_ALLOW_RULE}\", "),
                1
            ),
            1
        )
    );
    // (The loader reads the rule without error; it is the launcher that puts it in front of every agent's start arguments.)
    capstan_config::parse_config(widened.as_bytes(), root.path())
        .expect("the rule passes the researcher check");
    // And the rule reaches the researcher's start arguments in a spawn.
    let sc = scenario(
        "researcher-rule",
        json!({"maxWorkers": 3, "researcher": {"enabled": true}}),
    );
    launched(&sc);
    spawn_ok(&sc, "researcher");
    let calls = sc.take_calls();
    let start = calls
        .iter()
        .rev()
        .find(|c| c["m"] == "startAgent")
        .expect("the researcher was started");
    assert!(
        start["a"][0]["args"].to_string().contains(CSTAN_ALLOW_RULE),
        "{start}"
    );
}

/// test "the PM keeps pm_width_percent of its tab when the first worker is placed"
#[test]
fn the_pm_keeps_pm_width_percent_of_its_tab_when_the_first_worker_is_placed() {
    let sc = scenario(
        "pm-width",
        json!({"maxWorkers": 3, "layout": {"spawn": "pane", "pmWidthPercent": 70}}),
    );
    launched(&sc);
    sc.take_calls();
    spawn_ok(&sc, "developer");
    spawn_ok(&sc, "developer2");
    let keeps: Vec<f64> = sc
        .take_calls()
        .iter()
        .filter(|c| c["m"] == "placePane")
        .map(|c| c["a"][0]["keep"].as_f64().unwrap())
        .collect();
    assert_eq!(
        keeps,
        [0.7, 0.5],
        "the first worker leaves the PM 70%; the next one splits the worker's pane in half"
    );
}

// ------------------------------------------------------------------------------------------------ launcher-release

/// test "replace and PM restart rebuild the researcher prompt and the research section"
#[test]
fn replace_and_pm_restart_rebuild_the_researcher_prompt_and_the_research_section() {
    let sc = scenario(
        "researcher-rebuild",
        json!({"maxWorkers": 3, "researcher": {"enabled": true}}),
    );
    launched(&sc);
    let old = spawn_ok(&sc, "researcher");
    let first = sc.prompts().last().unwrap().clone();
    sc.take_calls();
    let replaced = sc
        .launcher
        .replace(old["agentId"].as_str().unwrap())
        .expect("the researcher is replaced");
    assert_eq!(replaced.state(), "started", "{:?}", replaced.to_value());
    let start = sc
        .take_calls()
        .into_iter()
        .rev()
        .find(|c| c["m"] == "startAgent")
        .expect("a start");
    assert!(
        start["a"][0]["args"].to_string().contains("--mcp-config"),
        "{start}"
    );
    let rebuilt = sc.prompts().last().unwrap().clone();
    assert!(
        rebuilt.contains("You are the researcher of a Capstan delivery team"),
        "{rebuilt:.200}"
    );
    assert!(rebuilt.contains("docs/research/"));
    assert!(first.contains("Output contract") && rebuilt.contains("Output contract"));
    sc.launcher.restart_pm().expect("the PM restarts");
    let pm = sc.prompts().last().unwrap().clone();
    assert!(
        pm.contains("Web research (the Researcher is enabled"),
        "{pm:.200}"
    );
}

/// test "teardown time is not charged to the cleanup budget"
#[test]
fn teardown_time_is_not_charged_to_the_cleanup_budget() {
    let sc = scenario(
        "teardown-budget",
        json!({"maxWorkers": 3, "worktree": {"teardown": "./cleanup.sh", "teardownTimeoutSeconds": 600}}),
    );
    // A clock that a teardown moves on by five minutes, ten times the cleanup budget of thirty seconds.
    *sc.world.clock.lock().unwrap() = Some(Arc::new(std::sync::atomic::AtomicI64::new(1_000_000)));
    sc.world
        .teardown_advance_ms
        .store(5 * 60_000, std::sync::atomic::Ordering::SeqCst);
    let mut sc = sc;
    sc.reopen();
    launched(&sc);
    let spawned = spawn_ok(&sc, "developer");
    let released = sc
        .launcher
        .release(spawned["agentId"].as_str().unwrap())
        .expect("released");
    let outcome = released.to_value();
    assert_eq!(outcome["paneClosed"], true, "{outcome}");
    assert_eq!(outcome["worktreeRemoved"], true, "{outcome}");
    assert_eq!(outcome["branchKept"], false, "{outcome}");
    let commands = sc.world.commands.lock().unwrap();
    assert_eq!(
        commands.iter().filter(|c| c["kind"] == "teardown").count(),
        1,
        "{commands:?}"
    );
}

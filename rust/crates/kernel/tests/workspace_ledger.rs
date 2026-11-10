//! The workspace ledger of migration 0063 through `Core`: per-repo worktrees, report commits, verify runs per repo,
//! integration sets, cleanup repos, package repos and the repo-aware branch rename.

#[path = "rust_only/fixture.rs"]
mod fixture;

use capstan_kernel::plan_body::parse_plan_body;
use capstan_kernel::{Core, KernelError};
use fixture::{context, step, Fixture, OWNER};
use serde_json::{json, Value};

const BASE: &str = "0123456789abcdef0123456789abcdef01234567";
const OTHER_BASE: &str = "89abcdef0123456789abcdef0123456789abcdef";
const COMMIT_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const COMMIT_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const COMMANDS: &str = "cmd-sha-a";

fn db(f: &Fixture) -> rusqlite::Connection {
    rusqlite::Connection::open(f.database()).unwrap()
}

fn count(f: &Fixture, sql: &str) -> i64 {
    db(f).query_row(sql, [], |r| r.get(0)).unwrap()
}

fn refused(result: Result<Value, KernelError>) -> String {
    match result {
        Err(error) => error.message(),
        Ok(value) => panic!("the call was accepted: {value}"),
    }
}

fn worktree(core: &Core, repo: &str, branch: &str, base: &str) -> Result<Value, KernelError> {
    core.record_agent_worktree(
        &context(core, OWNER),
        &json!({"agentId": "dev-1", "generation": 1, "repo": repo, "worktreePath": format!("/wt/{repo}"), "branch": branch, "baseSha": base}),
    )
}

// ------------------------------------------------------------------------------------------------- worktrees

#[test]
fn a_worktree_is_recorded_removed_revived_and_refused_while_live() {
    let f = Fixture::new();
    let core = f.core();
    assert_eq!(
        worktree(core, "api", "feat/x", BASE).unwrap()["revived"],
        false
    );
    assert!(refused(worktree(core, "api", "feat/x", BASE)).contains("already_added"));
    worktree(core, "web", "feat/x", BASE).unwrap();
    let live = core.list_agent_worktrees("dev-1").unwrap();
    let repos: Vec<&str> = live
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["repo"].as_str().unwrap())
        .collect();
    assert_eq!(repos, ["api", "web"]);

    core.remove_agent_worktree(
        &context(core, OWNER),
        &json!({"agentId": "dev-1", "repo": "web"}),
    )
    .unwrap();
    assert_eq!(
        core.list_agent_worktrees("dev-1")
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(refused(core.remove_agent_worktree(
        &context(core, OWNER),
        &json!({"agentId": "dev-1", "repo": "web"})
    ))
    .contains("no live worktree"));

    // Added again: the removed row is revived with the new facts.
    let again = core
        .record_agent_worktree(
            &context(core, OWNER),
            &json!({"agentId": "dev-1", "generation": 1, "repo": "web", "worktreePath": "/wt/web2", "branch": "feat/y", "baseSha": OTHER_BASE}),
        )
        .unwrap();
    assert_eq!(again["revived"], true);
    assert_eq!(
        count(
            &f,
            "SELECT count(*) FROM agent_worktrees WHERE repo = 'web'"
        ),
        1
    );
    let live = core.list_agent_worktrees("dev-1").unwrap();
    assert_eq!(live[1]["worktreePath"], "/wt/web2");
    assert_eq!(live[1]["removedAt"], Value::Null);

    // Bad input and a stale generation are refused.
    assert!(refused(core.record_agent_worktree(&context(core, OWNER), &json!({"agentId": "dev-1", "generation": 2, "repo": "api", "worktreePath": "/p", "branch": "b", "baseSha": BASE})))
        .contains("not the current generation"));
    assert!(refused(worktree(core, "api", "feat/x", "xyz")).contains("baseSha"));
}

#[test]
fn a_replace_copies_the_live_worktrees_to_the_new_generation() {
    let f = Fixture::new();
    let core = f.core();
    worktree(core, "api", "feat/x", BASE).unwrap();
    worktree(core, "web", "feat/x", BASE).unwrap();
    core.remove_agent_worktree(
        &context(core, OWNER),
        &json!({"agentId": "dev-1", "repo": "web"}),
    )
    .unwrap();
    core.replace_agent_generation(&context(core, OWNER), "dev-1")
        .unwrap();
    let generation: i64 = count(&f, "SELECT generation FROM agents WHERE agent_id = 'dev-1'");
    assert_eq!(generation, 2);
    // Readers see nothing until the copy: they use the current generation.
    assert_eq!(core.list_agent_worktrees("dev-1").unwrap(), json!([]));
    let copied = core
        .copy_agent_worktrees(
            &context(core, OWNER),
            &json!({"agentId": "dev-1", "fromGeneration": 1, "toGeneration": 2}),
        )
        .unwrap();
    assert_eq!(copied["copied"], 1);
    let live = core.list_agent_worktrees("dev-1").unwrap();
    assert_eq!(live.as_array().unwrap().len(), 1);
    assert_eq!(
        (live[0]["repo"].as_str(), live[0]["generation"].as_i64()),
        (Some("api"), Some(2))
    );
    assert_eq!(
        count(
            &f,
            "SELECT count(*) FROM agent_worktrees WHERE generation = 1"
        ),
        2
    );
    assert!(refused(core.copy_agent_worktrees(
        &context(core, OWNER),
        &json!({"agentId": "dev-1", "fromGeneration": 2, "toGeneration": 2})
    ))
    .contains("greater"));
}

// ----------------------------------------------------------------------------------------- report commits

fn evidence(branch: &str, base: &str, commit: &str) -> Value {
    json!({"generation": 1, "branch": branch, "baseSha": base, "commitExists": true, "isAncestorOfTip": true,
        "isAncestorOfBase": false, "checkedAt": "2026-01-01T00:00:00.000Z", "branchTip": commit})
}

fn report(
    f: &Fixture,
    commit: &str,
    summary: &str,
    commits: Option<Value>,
) -> Result<Value, KernelError> {
    let core = f.core();
    let mut input = json!({"commitSha": commit, "summary": summary, "evidence": evidence("feat/x", BASE, commit)});
    if let Some(commits) = commits {
        input["commits"] = commits;
    }
    core.record_agent_report(&context(core, &f.developer_credential), &input)
}

fn commit(repo: &str, sha: &str, branch: &str, base: &str) -> Value {
    json!({"repo": repo, "commitSha": sha, "branch": branch, "baseSha": base})
}

#[test]
fn a_report_without_commits_is_what_it_was() {
    let f = Fixture::new();
    let core = f.core();
    step(
        core,
        "recordAgentPane",
        json!([{"agentId": "dev-1", "workspaceId": "ws-1", "paneId": "p", "worktreePath": "/wt", "branch": "feat/x", "baseSha": BASE}]),
    );
    let result = report(&f, COMMIT_A, "done", None).unwrap();
    assert_eq!(result["duplicate"], false);
    assert_eq!(result["record"]["state"], "accepted");
    assert_eq!(count(&f, "SELECT count(*) FROM report_commits"), 0);
    let id = result["record"]["reportId"].as_str().unwrap().to_string();
    assert_eq!(core.report_commits(&id).unwrap(), json!([]));
    // The same report again is the duplicate.
    assert_eq!(
        report(&f, COMMIT_A, "done", None).unwrap()["duplicate"],
        true
    );
}

#[test]
fn a_report_with_commits_writes_report_commits_and_checks_the_worktrees() {
    let f = Fixture::new();
    let core = f.core();
    worktree(core, "api", "feat/x", BASE).unwrap();
    worktree(core, "web", "feat/x", OTHER_BASE).unwrap();
    let commits = json!([
        commit("api", COMMIT_A, "feat/x", BASE),
        commit("web", COMMIT_B, "feat/x", OTHER_BASE)
    ]);
    let result = report(&f, COMMIT_A, "both repos", Some(commits.clone())).unwrap();
    assert_eq!(result["record"]["state"], "accepted", "{result}");
    // The report JSON is the same shape as without commits.
    let keys: Vec<&String> = result["record"].as_object().unwrap().keys().collect();
    assert!(!keys.iter().any(|k| k.contains("commits")), "{keys:?}");
    let id = result["record"]["reportId"].as_str().unwrap().to_string();
    let stored = core.report_commits(&id).unwrap();
    assert_eq!(stored, commits);
    assert_eq!(count(&f, "SELECT count(*) FROM report_commits"), 2);

    // report_commits rows are immutable.
    let conn = db(&f);
    assert!(conn
        .execute("UPDATE report_commits SET branch = 'z'", [])
        .is_err());
    assert!(conn.execute("DELETE FROM report_commits", []).is_err());

    // A commit on a branch or base the worktree of its repo does not have is rejected, and nothing else changes.
    let wrong = json!([
        commit(
            "api",
            "cccccccccccccccccccccccccccccccccccccccc",
            "feat/x",
            BASE
        ),
        commit("web", COMMIT_B, "feat/other", OTHER_BASE)
    ]);
    let rejected = report(
        &f,
        "cccccccccccccccccccccccccccccccccccccccc",
        "wrong",
        Some(wrong),
    )
    .unwrap();
    assert_eq!(rejected["record"]["state"], "rejected");
    assert_eq!(rejected["record"]["reason"], "agent_changed");
    // A repo the agent holds no worktree in has no branch to report on.
    let none = json!([commit(
        "docs",
        "dddddddddddddddddddddddddddddddddddddddd",
        "feat/x",
        BASE
    )]);
    let none = report(
        &f,
        "dddddddddddddddddddddddddddddddddddddddd",
        "no worktree",
        Some(none),
    )
    .unwrap();
    assert_eq!(none["record"]["reason"], "no_branch");
}

#[test]
fn bad_commits_are_a_type_error_and_write_nothing() {
    let f = Fixture::new();
    let core = f.core();
    worktree(core, "api", "feat/x", BASE).unwrap();
    worktree(core, "web", "feat/x", BASE).unwrap();
    let reports_before = count(&f, "SELECT count(*) FROM agent_reports");
    let a = commit("api", COMMIT_A, "feat/x", BASE);
    let w = commit("web", COMMIT_B, "feat/x", BASE);
    let cases = [
        json!([]),
        json!("api"),
        json!([w.clone(), a.clone()]), // not sorted
        json!([a.clone(), a.clone()]), // a repo twice
        json!([w.clone()]),            // the first is not the report's commit
        json!([commit("api", COMMIT_A, "feat/x", "short")]), // a bad sha
        json!([commit("a/b", COMMIT_A, "feat/x", BASE)]), // a bad repo name
        json!([{"repo": "api", "commitSha": COMMIT_A, "baseSha": BASE}]), // no branch
    ];
    for case in cases {
        let message = refused(report(&f, COMMIT_A, "x", Some(case.clone())));
        assert!(message.starts_with("commits "), "{case}: {message}");
    }
    assert_eq!(
        count(&f, "SELECT count(*) FROM agent_reports"),
        reports_before
    );
    assert_eq!(count(&f, "SELECT count(*) FROM report_commits"), 0);
}

// -------------------------------------------------------------------------------------------- verify runs

fn insert_report(f: &Fixture, commit: &str) -> String {
    let conn = db(f);
    let actor: String = conn
        .query_row(
            "SELECT actor_id FROM agents WHERE agent_id = 'dev-1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let sequence: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(sequence), 0) + 1 FROM agent_reports",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let id = format!("report-{sequence}");
    conn.execute(
        "INSERT INTO agent_reports(project_id, report_id, sequence, agent_id, generation, actor_id, commit_sha, branch, summary, state, evidence_json, created_at)
         VALUES ('prustonly', ?, ?, 'dev-1', 1, ?, ?, 'b', 's', 'accepted', '{\"baseSha\":\"0123456789abcdef0123456789abcdef01234567\"}', '2026-01-01T00:00:00.000Z')",
        rusqlite::params![id, sequence, actor, commit],
    )
    .unwrap();
    id
}

fn queue(f: &Fixture, subject: &str, repo: Option<&str>) -> Result<Value, KernelError> {
    let core = f.core();
    let mut input = json!({"subjectId": subject, "commands": ["make test"], "commandsSha": COMMANDS, "trigger": "manual"});
    if let Some(repo) = repo {
        input["repo"] = json!(repo);
    }
    core.queue_verify_run(&context(core, &f.developer_credential), &input)
}

#[test]
fn two_repos_may_queue_one_commit_and_one_repo_twice_is_the_existing_run() {
    let f = Fixture::new();
    let report = insert_report(&f, COMMIT_A);
    let root = queue(&f, &report, None).unwrap();
    // Repos are named once the report lists them; the root run above was queued before.
    add_report_commit(&f, &report, "api", COMMIT_A);
    add_report_commit(&f, &report, "web", COMMIT_A);
    // The record of a root run has no repo key: single-repo JSON is unchanged.
    assert!(root.get("repo").is_none(), "{root}");
    let api = queue(&f, &report, Some("api")).unwrap();
    let web = queue(&f, &report, Some("web")).unwrap();
    assert_eq!(api["repo"], "api");
    assert_ne!(api["runId"], web["runId"]);
    assert_ne!(api["runId"], root["runId"]);
    let again = queue(&f, &report, Some("api")).unwrap();
    assert_eq!(again["existing"], true);
    assert_eq!(again["runId"], api["runId"]);
    assert!(refused(queue(&f, &report, None)).contains("not a repo of the subject"));
    assert_eq!(count(&f, "SELECT count(*) FROM verify_runs"), 3);
    assert!(refused(queue(&f, &report, Some("a/b"))).contains("repo"));
    // Repo and the older identity columns cannot change.
    let conn = db(&f);
    assert!(conn
        .execute("UPDATE verify_runs SET repo = 'x'", [])
        .is_err());
    assert!(conn
        .execute("UPDATE verify_runs SET commit_sha = ?", [COMMIT_B])
        .is_err());
}

fn add_report_commit(f: &Fixture, report: &str, repo: &str, sha: &str) {
    db(f)
        .execute(
            "INSERT INTO report_commits(project_id, report_id, repo, commit_sha, branch, base_sha) VALUES ('prustonly', ?, ?, ?, 'b', ?)",
            rusqlite::params![report, repo, sha, BASE],
        )
        .unwrap();
}

#[test]
fn a_gate_is_per_repo_and_queue_refuses_an_unlisted_repo() {
    let f = Fixture::new();
    let core = f.core();
    let report = insert_report(&f, COMMIT_A);
    add_report_commit(&f, &report, "api", COMMIT_A);
    add_report_commit(&f, &report, "web", COMMIT_A);
    // A repo the report does not list, and the root of a report that lists repos, are refused.
    assert!(refused(queue(&f, &report, Some("other"))).contains("not a repo of the subject"));
    assert!(refused(queue(&f, &report, None)).contains("not a repo of the subject"));
    let api = queue(&f, &report, Some("api")).unwrap();
    queue(&f, &report, Some("web")).unwrap();
    let run_id = api["runId"].as_str().unwrap();
    core.claim_verify_run(
        &context(core, OWNER),
        &json!({"runId": run_id, "scratchPath": "/s", "logPath": "/l", "maxParallel": 4}),
    )
    .unwrap();
    core.finish_verify_run(&context(core, OWNER), &json!({"runId": run_id, "state": "passed", "exitCode": 0, "durationMs": 5, "outputTail": "ok"})).unwrap();
    // Repo A passed; repo B, at the same commit, has not.
    assert_eq!(
        core.verify_gate_in_repo("api", COMMIT_A, COMMANDS).unwrap()["code"],
        "passed"
    );
    assert_eq!(
        core.verify_gate_in_repo("web", COMMIT_A, COMMANDS).unwrap()["code"],
        "verify_running"
    );
    assert_eq!(
        core.verify_gate(COMMIT_A, COMMANDS).unwrap()["code"],
        "verify_missing"
    );
    // A subject with no repo rows keeps the root ''.
    let plain = insert_report(&f, COMMIT_B);
    assert!(queue(&f, &plain, None).is_ok());
    assert!(refused(queue(&f, &plain, Some("api"))).contains("not a repo of the subject"));
}

#[test]
fn a_report_run_for_a_repo_carries_that_repos_commit() {
    let f = Fixture::new();
    let report = insert_report(&f, COMMIT_A);
    add_report_commit(&f, &report, "api", COMMIT_A);
    add_report_commit(&f, &report, "web", COMMIT_B);
    let api = queue(&f, &report, Some("api")).unwrap();
    let web = queue(&f, &report, Some("web")).unwrap();
    assert_eq!(api["commitSha"], COMMIT_A, "{api}");
    assert_eq!(web["commitSha"], COMMIT_B, "{web}");
}

#[test]
fn a_report_with_one_commit_row_queues_and_gates_in_its_repo() {
    let f = Fixture::new();
    let core = f.core();
    let report = insert_report(&f, COMMIT_A);
    add_report_commit(&f, &report, "api", COMMIT_A);
    let run = queue(&f, &report, None).unwrap();
    assert_eq!(run["repo"], "api");
    let run_id = run["runId"].as_str().unwrap();
    core.claim_verify_run(
        &context(core, OWNER),
        &json!({"runId": run_id, "scratchPath": "/s", "logPath": "/l", "maxParallel": 4}),
    )
    .unwrap();
    core.finish_verify_run(&context(core, OWNER), &json!({"runId": run_id, "state": "passed", "exitCode": 0, "durationMs": 5, "outputTail": "ok"})).unwrap();
    let gate = core
        .verify_gate_for_subjects(std::slice::from_ref(&report), COMMANDS)
        .unwrap();
    assert_eq!(gate["ok"], true, "{gate}");
}

// ---------------------------------------------------------------------------------------- integration sets

fn integration(f: &Fixture, id: &str, sequence: i64, state: &str, head: Option<&str>) {
    db(f)
        .execute(
            "INSERT INTO integrations(project_id, integration_id, sequence, base_sha, branch, requested_by, state, head_sha, created_at, completed_at)
             VALUES ('prustonly', ?, ?, ?, ?, 'pm-1', ?, ?, '2026-01-01T00:00:00.000Z', ?)",
            rusqlite::params![
                id,
                sequence,
                BASE,
                format!("int/{id}"),
                state,
                head,
                if state == "running" { None } else { Some("2026-01-01T00:00:00.000Z") }
            ],
        )
        .unwrap();
}

fn make_set(f: &Fixture, repos: &[&str]) -> String {
    let core = f.core();
    core.create_integration_set(
        &context(core, OWNER),
        &json!({"branch": "int/set", "repos": repos}),
    )
    .unwrap()["setId"]
        .as_str()
        .unwrap()
        .to_string()
}

fn set_call(f: &Fixture, op: &str, input: Value) -> Result<Value, KernelError> {
    let core = f.core();
    let ctx = context(core, OWNER);
    match op {
        "attach" => core.attach_set_member(&ctx, &input),
        "detach" => core.detach_set_member(&ctx, &input),
        "state" => core.set_integration_state(&ctx, &input),
        "reset" => core.reset_set(&ctx, &input),
        _ => unreachable!(),
    }
}

#[test]
fn a_set_attaches_detaches_and_attaches_again() {
    let f = Fixture::new();
    let core = f.core();
    let set = make_set(&f, &["api", "web"]);
    let value = core.integration_set(&set).unwrap();
    assert_eq!(value["state"], "running");
    assert_eq!(value["members"][0]["repo"], "api");
    assert_eq!(value["members"][1]["position"], 1);
    assert_eq!(value["members"][0]["integrationId"], Value::Null);
    assert_eq!(core.integration_set("nope").unwrap(), Value::Null);
    assert_eq!(core.set_running().unwrap(), vec![set.clone()]);

    integration(&f, "i-1", 1, "running", None);
    integration(&f, "i-2", 2, "merged", Some(COMMIT_B));
    let attached = set_call(
        &f,
        "attach",
        json!({"setId": set, "repo": "api", "integrationId": "i-1"}),
    )
    .unwrap();
    assert_eq!(attached["attempt"], 1);
    assert_eq!(core.repo_of_integration("i-1").unwrap(), Some("api".into()));
    assert_eq!(core.repo_of_integration("i-2").unwrap(), None);
    assert_eq!(core.set_of_integration("i-1").unwrap(), Some(set.clone()));
    // A live integration refuses a second attach, naming the set.
    let message = refused(set_call(
        &f,
        "attach",
        json!({"setId": set, "repo": "api", "integrationId": "i-2"}),
    ));
    assert!(
        message.contains(&set) && message.contains("already has integration i-1"),
        "{message}"
    );
    // An integration belongs to one set; unknown repos, integrations and sets are refused.
    assert!(refused(set_call(
        &f,
        "attach",
        json!({"setId": set, "repo": "docs", "integrationId": "i-2"})
    ))
    .contains("no member"));
    assert!(refused(set_call(
        &f,
        "attach",
        json!({"setId": set, "repo": "web", "integrationId": "i-9"})
    ))
    .contains("does not exist"));
    assert!(refused(set_call(
        &f,
        "attach",
        json!({"setId": "nope", "repo": "web", "integrationId": "i-2"})
    ))
    .contains("nope"));
    let other = make_set(&f, &["web"]);
    assert!(refused(set_call(
        &f,
        "attach",
        json!({"setId": other, "repo": "web", "integrationId": "i-1"})
    ))
    .contains("already belongs"));

    set_call(&f, "detach", json!({"setId": set, "repo": "api"})).unwrap();
    assert!(
        refused(set_call(&f, "detach", json!({"setId": set, "repo": "api"})))
            .contains("no integration to detach")
    );
    assert_eq!(core.repo_of_integration("i-1").unwrap(), None);
    let again = set_call(
        &f,
        "attach",
        json!({"setId": set, "repo": "api", "integrationId": "i-2"}),
    )
    .unwrap();
    assert_eq!(again["attempt"], 2);
    assert!(refused(set_call(
        &f,
        "detach",
        json!({"setId": "nope", "repo": "api"})
    ))
    .contains("nope"));
}

#[test]
fn set_states_move_as_listed_and_complete_needs_every_member_merged() {
    let f = Fixture::new();
    let core = f.core();
    let set = make_set(&f, &["api", "web"]);
    assert!(refused(set_call(
        &f,
        "state",
        json!({"setId": set, "state": "bogus"})
    ))
    .contains("unknown state"));
    assert!(refused(set_call(
        &f,
        "state",
        json!({"setId": "nope", "state": "merged"})
    ))
    .contains("nope"));
    assert!(refused(set_call(
        &f,
        "state",
        json!({"setId": set, "state": "running"})
    ))
    .contains("cannot move"));
    for state in ["failed", "conflicted"] {
        assert!(
            refused(set_call(&f, "state", json!({"setId": set, "state": state})))
                .contains("needs a reason"),
            "{state}"
        );
    }

    assert!(
        refused(core.reset_set(&context(core, OWNER), &json!({"setId": "nope"}))).contains("nope")
    );
    assert!(core
        .set_complete("nope")
        .unwrap_err()
        .message()
        .contains("nope"));

    integration(&f, "i-1", 1, "merged", Some(COMMIT_A));
    integration(&f, "i-2", 2, "running", None);
    set_call(
        &f,
        "attach",
        json!({"setId": set, "repo": "api", "integrationId": "i-1"}),
    )
    .unwrap();
    assert!(!core.set_complete(&set).unwrap());
    set_call(
        &f,
        "attach",
        json!({"setId": set, "repo": "web", "integrationId": "i-2"}),
    )
    .unwrap();
    assert!(!core.set_complete(&set).unwrap());
    // A set whose only member merged is complete.
    let solo = make_set(&f, &["docs"]);
    integration(&f, "i-3", 3, "merged", Some(COMMIT_B));
    set_call(
        &f,
        "attach",
        json!({"setId": solo, "repo": "docs", "integrationId": "i-3"}),
    )
    .unwrap();
    assert!(core.set_complete(&solo).unwrap());

    set_call(
        &f,
        "state",
        json!({"setId": set, "state": "failed", "reason": "web conflicted"}),
    )
    .unwrap();
    assert_eq!(
        core.integration_set(&set).unwrap()["reason"],
        "web conflicted"
    );
    set_call(&f, "state", json!({"setId": set, "state": "merged"})).unwrap();
    set_call(
        &f,
        "state",
        json!({"setId": set, "state": "failed", "reason": "verify failed"}),
    )
    .unwrap();
    // Discarded only when every attached integration is discarded.
    assert!(refused(set_call(
        &f,
        "state",
        json!({"setId": set, "state": "discarded"})
    ))
    .contains("cannot be discarded"));
    set_call(&f, "detach", json!({"setId": set, "repo": "api"})).unwrap();
    set_call(&f, "detach", json!({"setId": set, "repo": "web"})).unwrap();
    set_call(&f, "state", json!({"setId": set, "state": "discarded"})).unwrap();
    assert!(refused(set_call(
        &f,
        "state",
        json!({"setId": set, "state": "merged"})
    ))
    .contains("cannot move"));
    assert_eq!(core.set_running().unwrap(), vec![solo]);
}

#[test]
fn reset_at_hides_older_runs_and_reviews_from_the_set_gates() {
    let f = Fixture::new();
    let core = f.core();
    let set = make_set(&f, &["api", "web"]);
    integration(&f, "i-1", 1, "merged", Some(COMMIT_A));
    integration(&f, "i-2", 2, "merged", Some(COMMIT_B));
    set_call(
        &f,
        "attach",
        json!({"setId": set, "repo": "api", "integrationId": "i-1"}),
    )
    .unwrap();
    set_call(
        &f,
        "attach",
        json!({"setId": set, "repo": "web", "integrationId": "i-2"}),
    )
    .unwrap();
    assert!(core.set_complete(&set).unwrap());

    // Each member run carries its member's integration, repo and commit (the repo comes from the set).
    let api = queue(&f, "i-1", None).unwrap();
    let web = queue(&f, "i-2", None).unwrap();
    assert_eq!(
        (
            api["repo"].as_str(),
            api["subjectId"].as_str(),
            api["commitSha"].as_str()
        ),
        (Some("api"), Some("i-1"), Some(COMMIT_A))
    );
    assert_eq!(web["repo"], "web");
    for run in [&api, &web] {
        let run_id = run["runId"].as_str().unwrap();
        assert!(core
            .claim_verify_run(
                &context(core, OWNER),
                &json!({"runId": run_id, "scratchPath": "/s", "logPath": "/l", "maxParallel": 4})
            )
            .unwrap()["claimed"]
            .as_bool()
            .unwrap());
        core.finish_verify_run(&context(core, OWNER), &json!({"runId": run_id, "state": "passed", "exitCode": 0, "durationMs": 5, "outputTail": "ok"})).unwrap();
    }
    let actor: String = db(&f)
        .query_row(
            "SELECT actor_id FROM agents WHERE agent_id = 'dev-1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    db(&f)
        .execute(
            "INSERT INTO reviews(project_id, review_id, sequence, round, subject_integration_id, commit_sha, base_sha, requested_by_actor_id, reviewer_role, reviewer_agent_id, reviewer_actor_id, state, created_at)
             VALUES ('prustonly', 'rv-1', 1, 1, 'i-1', ?, ?, ?, 'developer', 'dev-1', ?, 'started', ?)",
            rusqlite::params![COMMIT_A, BASE, actor, actor, "2026-01-01T00:00:00.000Z"],
        )
        .unwrap();

    let gate = core.set_gate(&set, COMMANDS).unwrap();
    assert_eq!(gate["ok"], true, "{gate}");
    assert_eq!(gate["reviewSubjectId"], "i-1");
    assert_eq!(gate["review"]["reviewId"], "rv-1");
    assert_eq!(gate["members"][1]["code"], "passed");

    // A redo or discard of a member resets the set: what came before no longer counts.
    std::thread::sleep(std::time::Duration::from_millis(5));
    let reset = set_call(&f, "reset", json!({"setId": set})).unwrap();
    let gate = core.set_gate(&set, COMMANDS).unwrap();
    assert_eq!(gate["resetAt"], reset["resetAt"]);
    assert_eq!(gate["ok"], false);
    assert_eq!(gate["members"][0]["code"], "verify_missing");
    assert_eq!(gate["review"], Value::Null);
}

#[test]
fn a_cleanup_without_a_repo_row_reads_as_the_root() {
    let f = Fixture::new();
    let core = f.core();
    integration(&f, "i-1", 1, "merged", Some(COMMIT_A));
    db(&f)
        .execute(
            "INSERT INTO cleanups(project_id, cleanup_id, sequence, trigger, integration_id, requested_by, mode, state, created_at)
             VALUES ('prustonly', 'c-1', 1, 'manual', 'i-1', 'pm-1', 'ask', 'offered', '2026-01-01T00:00:00.000Z')",
            [],
        )
        .unwrap();
    assert_eq!(core.cleanup_repo("c-1").unwrap(), "");
    core.record_cleanup_repo(
        &context(core, OWNER),
        &json!({"cleanupId": "c-1", "repo": "api"}),
    )
    .unwrap();
    assert_eq!(core.cleanup_repo("c-1").unwrap(), "api");
    assert!(refused(core.record_cleanup_repo(
        &context(core, OWNER),
        &json!({"cleanupId": "c-1", "repo": "web"})
    ))
    .contains("already has a repo"));
    assert!(refused(core.record_cleanup_repo(
        &context(core, OWNER),
        &json!({"cleanupId": "c-9", "repo": "web"})
    ))
    .contains("does not exist"));
}

// ------------------------------------------------------------------------------------------------ rename

fn rename(f: &Fixture, input: Value) -> Result<Value, KernelError> {
    let core = f.core();
    core.rename_agent_branch(&context(core, OWNER), &input)
}

fn branches(f: &Fixture) -> Vec<(String, String)> {
    let conn = db(f);
    let mut statement = conn
        .prepare("SELECT repo, branch FROM agent_worktrees WHERE removed_at IS NULL ORDER BY repo")
        .unwrap();
    statement
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

#[test]
fn a_rename_follows_the_worktrees_all_or_by_repo() {
    let f = Fixture::new();
    let core = f.core();
    step(
        core,
        "recordAgentPane",
        json!([{"agentId": "dev-1", "workspaceId": "ws-1", "paneId": "p", "worktreePath": "/wt", "branch": "feat/x", "baseSha": BASE}]),
    );
    worktree(core, "api", "feat/x", BASE).unwrap();
    worktree(core, "docs", "feat/x", BASE).unwrap();
    worktree(core, "web", "feat/z", BASE).unwrap();

    // A listed repo without a worktree on the old branch refuses, and nothing is renamed.
    let message = refused(rename(
        &f,
        json!({"agentId": "dev-1", "from": "feat/x", "to": "fix/x", "repos": ["api", "web"]}),
    ));
    assert_eq!(message, "repo web has no worktree on branch feat/x");
    assert_eq!(branches(&f)[0], ("api".into(), "feat/x".into()));
    let pane_branch: String = db(&f)
        .query_row(
            "SELECT branch FROM agent_panes WHERE agent_id = 'dev-1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(pane_branch, "feat/x");

    // The listed repos only.
    rename(
        &f,
        json!({"agentId": "dev-1", "from": "feat/x", "to": "fix/x", "repos": ["api"]}),
    )
    .unwrap();
    assert_eq!(
        branches(&f),
        [
            ("api".into(), "fix/x".into()),
            ("docs".into(), "feat/x".into()),
            ("web".into(), "feat/z".into())
        ]
    );

    // Without repos, every live row on the old branch. The pane is renamed either way.
    let pane_branch: String = db(&f)
        .query_row(
            "SELECT branch FROM agent_panes WHERE agent_id = 'dev-1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(pane_branch, "fix/x");
    rename(
        &f,
        json!({"agentId": "dev-1", "from": "fix/x", "to": "fix/y"}),
    )
    .unwrap();
    assert_eq!(branches(&f)[0], ("api".into(), "fix/y".into()));
    assert_eq!(branches(&f)[1].1, "feat/x");
    // A bad repos list is a type error.
    assert!(refused(rename(
        &f,
        json!({"agentId": "dev-1", "from": "fix/y", "to": "fix/z", "repos": "api"})
    ))
    .contains("repos"));
}

// ------------------------------------------------------------------------------------------ package repos

const REPOS_BODY: &str = r#"{"summary":"s","packages":[
  {"id":"api","title":"Api","repos":["core","web"],"owns":["core:src/api","web:src/api"],"acceptance":["x"],"estimate_hours":2},
  {"id":"ui","title":"Ui","repos":["web"],"owns":["web:src/ui"],"acceptance":["y"],"estimate_hours":3},
  {"id":"plain","title":"Plain","owns":["src/plain"],"acceptance":["z"],"estimate_hours":1}],
  "risks":[],"integration_order":["api","ui","plain"]}"#;

struct Plan {
    f: Fixture,
    architect: String,
    reviewer: String,
}

fn agent(f: &Fixture, id: &str, role: &str, kind: &str) -> String {
    let core = f.core();
    let seat = format!("seat-{id}");
    step(
        core,
        "createSeat",
        json!([{"seatId": seat, "name": seat, "role": kind}]),
    );
    let actor = step(
        core,
        "createActor",
        json!([{"displayName": id, "role": kind, "seatId": seat}]),
    );
    step(
        core,
        "registerAgent",
        json!([{"agentId": id, "roleName": role, "seatId": seat, "actorId": actor["actorId"]}]),
    );
    actor["credential"].as_str().unwrap().to_string()
}

fn plan_rig() -> Plan {
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
    let architect = agent(&f, "arch-1", "architect", "Developer");
    let reviewer = agent(&f, "rev-1", "reviewer", "Verifier");
    Plan {
        f,
        architect,
        reviewer,
    }
}

impl Plan {
    fn approved(&self, body: &str) -> String {
        let core = self.f.core();
        let plan = core
            .open_plan(
                &context(core, OWNER),
                &json!({"tier": "normal", "title": "T"}),
            )
            .unwrap()["planId"]
            .as_str()
            .unwrap()
            .to_string();
        let stored = serde_json::to_string(&parse_plan_body(body, 8).unwrap()).unwrap();
        core.submit_plan(
            &context(core, &self.architect),
            &json!({"planId": plan, "bodyJson": stored, "baseSha": BASE, "review": true}),
        )
        .unwrap();
        core.begin_review(
            &context(core, &self.architect),
            &json!({"subjectId": plan, "reviewerRole": "reviewer", "reviewerAgentId": "rev-1"}),
        )
        .unwrap();
        core.complete_review(
            &context(core, &self.reviewer),
            &json!({"verdict": "pass", "text": "ok"}),
        )
        .unwrap();
        plan
    }
}

#[test]
fn package_repos_reads_the_approved_revision() {
    let p = plan_rig();
    let core = p.f.core();
    let plan = p.approved(REPOS_BODY);
    assert_eq!(core.package_repos(&plan, "api").unwrap(), ["core", "web"]);
    assert_eq!(core.package_repos(&plan, "ui").unwrap(), ["web"]);
    assert!(core.package_repos(&plan, "plain").unwrap().is_empty());
    assert!(core.package_repos(&plan, "nope").unwrap().is_empty());
    assert!(core.package_repos("no-plan", "api").unwrap().is_empty());
}

#[test]
fn package_repos_survive_an_amendment_that_cannot_touch_them() {
    let p = plan_rig();
    let core = p.f.core();
    let plan = core
        .open_plan(
            &context(core, OWNER),
            &json!({"tier": "normal", "title": "T"}),
        )
        .unwrap()["planId"]
        .as_str()
        .unwrap()
        .to_string();
    let stored = serde_json::to_string(&parse_plan_body(REPOS_BODY, 8).unwrap()).unwrap();
    core.submit_plan(
        &context(core, &p.architect),
        &json!({"planId": plan, "bodyJson": stored, "baseSha": BASE, "review": true}),
    )
    .unwrap();
    core.begin_review(
        &context(core, &p.architect),
        &json!({"subjectId": plan, "reviewerRole": "reviewer", "reviewerAgentId": "rev-1"}),
    )
    .unwrap();
    core.complete_review(
        &context(core, &p.reviewer),
        &json!({"verdict": "findings", "text": "- [ui] major owns: ui also writes web:docs/ui"}),
    )
    .unwrap();
    assert!(
        core.package_repos(&plan, "ui").unwrap().is_empty(),
        "no approved revision yet"
    );

    let current = core.plan_record(OWNER, &plan).unwrap()["revision"]["bodyJson"]
        .as_str()
        .unwrap()
        .to_string();
    let amend = |amendment: &str| {
        let body = capstan_kernel::helpers::amended_plan_body_json(&current, amendment)
            .unwrap_or_else(|_| stored.clone());
        core.amend_plan(
            &context(core, &p.architect),
            &json!({"planId": plan, "amendmentJson": amendment, "bodyJson": body, "baseSha": BASE}),
        )
    };
    // The repos of a package are fixed at submit time.
    let message = refused(amend(
        r#"{"reason":"r","packages":[{"id":"ui","add_owns":["web:docs/ui"],"repos":["core"]}]}"#,
    ));
    assert!(message.contains("repos cannot be amended"), "{message}");
    assert!(capstan_kernel::helpers::amended_plan_body_json(
        &current,
        r#"{"reason":"r","repos":["x"],"packages":[]}"#
    )
    .unwrap_err()
    .contains("repos cannot be amended"));
    // An added path needs the prefix of one of the package's repos.
    assert!(capstan_kernel::helpers::amended_plan_body_json(
        &current,
        r#"{"reason":"r","packages":[{"id":"ui","add_owns":["docs/ui"]}]}"#
    )
    .is_err());
    assert!(capstan_kernel::helpers::amended_plan_body_json(
        &current,
        r#"{"reason":"r","packages":[{"id":"ui","add_owns":["core:docs/ui"]}]}"#
    )
    .is_err());
    let amended =
        amend(r#"{"reason":"r","packages":[{"id":"ui","add_owns":["web:./docs/ui/"]}]}"#).unwrap();
    assert_eq!(amended["state"], "approved");
    assert_eq!(core.package_repos(&plan, "ui").unwrap(), ["web"]);
    assert_eq!(core.package_repos(&plan, "api").unwrap(), ["core", "web"]);
    let body: Value = serde_json::from_str(
        core.plan_record(OWNER, &plan).unwrap()["revision"]["bodyJson"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        body["packages"][1]["owns"],
        json!(["web:src/ui", "web:docs/ui"])
    );
}

// ------------------------------------------------------------------------------------------ review brief

#[test]
fn a_review_brief_reads_the_gate_in_the_reports_own_repo() {
    let plan = plan_rig();
    let f = &plan.f;
    let core = f.core();
    let report = insert_report(f, COMMIT_A);
    add_report_commit(f, &report, "api", COMMIT_A);
    let run = queue(f, &report, None).unwrap();
    let run_id = run["runId"].as_str().unwrap().to_string();
    core.claim_verify_run(
        &context(core, OWNER),
        &json!({"runId": run_id, "scratchPath": "/s", "logPath": "/l", "maxParallel": 4}),
    )
    .unwrap();
    core.finish_verify_run(&context(core, OWNER), &json!({"runId": run_id, "state": "passed", "exitCode": 0, "durationMs": 5, "outputTail": "ok"})).unwrap();
    core.begin_review(
        &context(core, &f.pm_credential),
        &json!({"subjectId": report, "reviewerRole": "reviewer", "reviewerAgentId": "rev-1", "verifyCommandsSha": COMMANDS}),
    )
    .unwrap();
    let bodies: Vec<String> = db(f)
        .prepare("SELECT body FROM messages")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let brief = bodies
        .iter()
        .find(|b| b.contains("Gate "))
        .unwrap_or_else(|| panic!("no gate line in {bodies:?}"));
    assert!(
        brief.contains(&format!("Gate passed at {COMMIT_A} (run {run_id}")),
        "{brief}"
    );
    assert!(!brief.contains("No gate run is recorded"), "{brief}");
}

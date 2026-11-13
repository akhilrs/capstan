//! The per-repo data of plan-185 in the snapshot feed, the event feed and the agent list, through `Core`: report commits,
//! integration sets (merged, conflicted, dropped and redone members), per-repo verify results and worktrees. A project
//! without workspace rows keeps the JSON it always had.

#[path = "rust_only/fixture.rs"]
mod fixture;

use capstan_kernel::areas::event_feed::{feed_page, Position};
use capstan_kernel::areas::snapshot_feed::{waiting_page, WaitingItem};
use capstan_kernel::canonical::sha256;
use capstan_kernel::Core;
use fixture::{context, Fixture, OWNER};
use serde_json::{json, Value};

const BASE: &str = "0123456789abcdef0123456789abcdef01234567";
const COMMIT_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const COMMIT_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const COMMIT_C: &str = "cccccccccccccccccccccccccccccccccccccccc";

fn db(f: &Fixture) -> rusqlite::Connection {
    rusqlite::Connection::open(f.database()).unwrap()
}

/// An integration row; a conflicted one names the report that carries its conflict (seeded once as `conflict-report`).
fn integration(f: &Fixture, id: &str, sequence: i64, state: &str, head: Option<&str>) {
    let conn = db(f);
    if state == "conflicted" {
        let actor: String = conn
            .query_row(
                "SELECT actor_id FROM agents WHERE agent_id = 'dev-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO agent_reports(project_id, report_id, sequence, agent_id, generation, actor_id, commit_sha, branch, summary, state, evidence_json, created_at)
             VALUES ('prustonly', 'conflict-report', 900, 'dev-1', 1, ?, ?, 'b', 's', 'accepted', '{}', '2026-01-01T00:00:00.000Z')",
            rusqlite::params![actor, COMMIT_C],
        )
        .unwrap();
    }
    conn.execute(
        "INSERT INTO integrations(project_id, integration_id, sequence, base_sha, branch, requested_by, state, head_sha, created_at, completed_at, failure_reason,
             conflict_report_id, conflict_files_json, conflict_files_omitted)
         VALUES ('prustonly', ?, ?, ?, ?, 'pm-1', ?, ?, '2026-01-01T00:00:00.000Z', ?, ?, ?, ?, ?)",
        rusqlite::params![
            id,
            sequence,
            BASE,
            format!("int/{id}"),
            state,
            head,
            if state == "running" { None } else { Some("2026-01-01T00:00:00.000Z") },
            (state == "failed").then_some("git failed"),
            (state == "conflicted").then_some("conflict-report"),
            (state == "conflicted").then_some("[\"f.txt\"]"),
            (state == "conflicted").then_some(0),
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

fn attach(f: &Fixture, set: &str, repo: &str, integration: &str) {
    let core = f.core();
    core.attach_set_member(
        &context(core, OWNER),
        &json!({"setId": set, "repo": repo, "integrationId": integration}),
    )
    .unwrap();
}

fn detach(f: &Fixture, set: &str, repo: &str) {
    let core = f.core();
    core.detach_set_member(&context(core, OWNER), &json!({"setId": set, "repo": repo}))
        .unwrap();
}

fn run(f: &Fixture, id: &str, sequence: i64, subject: &str, repo: &str, commit: &str, state: &str) {
    let finished = (state != "running").then_some("2026-01-01T00:00:02.000Z");
    db(f)
        .execute(
            "INSERT INTO verify_runs(project_id, run_id, sequence, subject_integration_id, repo, commit_sha, commands_json, commands_sha,
                 trigger, requested_by_actor_id, state, queued_at, started_at, finished_at, exit_code)
             VALUES ('prustonly', ?, ?, ?, ?, ?, '[\"make test\"]', 'cmd', 'manual', 'a', ?, '2026-01-01T00:00:00.000Z',
                 '2026-01-01T00:00:01.000Z', ?, 0)",
            rusqlite::params![id, sequence, subject, repo, commit, state, finished],
        )
        .unwrap();
}

fn waiting(core: &Core) -> Vec<WaitingItem> {
    waiting_page(core, None, 100, Some("integration"))
        .unwrap()
        .items
}

fn item_of<'a>(items: &'a [WaitingItem], id: &str) -> &'a WaitingItem {
    items
        .iter()
        .find(|i| i.request_id == format!("integration:{id}"))
        .unwrap_or_else(|| panic!("no waiting item for {id}"))
}

/// A set of four repos: api confirmed, web merged, worker conflicted on its second attempt (the first failed and was
/// redone) and infra dropped (its integration was discarded and the set goes on).
fn four_repo_set(f: &Fixture) -> String {
    let set = make_set(f, &["api", "web", "worker", "infra"]);
    integration(f, "i-api", 1, "confirmed", Some(COMMIT_A));
    integration(f, "i-web", 2, "merged", Some(COMMIT_B));
    integration(f, "i-worker-1", 3, "failed", None);
    integration(f, "i-worker-2", 4, "conflicted", None);
    integration(f, "i-infra", 5, "discarded", None);
    attach(f, &set, "api", "i-api");
    attach(f, &set, "web", "i-web");
    attach(f, &set, "worker", "i-worker-1");
    detach(f, &set, "worker");
    attach(f, &set, "worker", "i-worker-2");
    attach(f, &set, "infra", "i-infra");
    set
}

// ------------------------------------------------------------------------------------------------------ sets

#[test]
fn a_set_member_lists_the_set_and_every_member_in_merge_order() {
    let f = Fixture::new();
    let core = f.core();
    let set = four_repo_set(&f);

    let view = core.integration_set_view("i-web").unwrap().unwrap();
    assert_eq!(view["setId"], set.as_str());
    assert_eq!(view["state"], "running");
    assert_eq!(view["repo"], "web");
    assert_eq!(view["position"], 1);
    assert_eq!(view["attempt"], 1);
    // The review subject is the first live member that has an integration: api, not web.
    assert_eq!(view["reviewSubject"], false);
    assert_eq!(view["dropped"], false);
    let members = view["members"].as_array().unwrap();
    let repos: Vec<&str> = members
        .iter()
        .map(|m| m["repo"].as_str().unwrap())
        .collect();
    assert_eq!(repos, ["api", "web", "worker", "infra"]);
    assert_eq!(
        members[0],
        json!({"repo": "api", "position": 0, "attempt": 1, "integrationId": "i-api", "state": "confirmed",
               "branch": "int/i-api", "headSha": COMMIT_A, "dropped": false})
    );
    assert_eq!(members[1]["state"], "merged");
    // The redone member is on attempt 2 and reads its second integration.
    assert_eq!(members[2]["attempt"], 2);
    assert_eq!(members[2]["integrationId"], "i-worker-2");
    assert_eq!(members[2]["state"], "conflicted");
    assert_eq!(members[2]["headSha"], Value::Null);
    // A discarded integration in a set that goes on is dropped.
    assert_eq!(members[3]["state"], "discarded");
    assert_eq!(members[3]["dropped"], true);

    let api = core.integration_set_view("i-api").unwrap().unwrap();
    assert_eq!(api["reviewSubject"], true);
    let infra = core.integration_set_view("i-infra").unwrap().unwrap();
    assert_eq!(infra["dropped"], true);
    assert_eq!(infra["reviewSubject"], false);
    // An integration of no set has no `set`; a detached one neither.
    integration(&f, "i-alone", 9, "merged", Some(COMMIT_C));
    assert_eq!(core.integration_set_view("i-alone").unwrap(), None);
    assert_eq!(core.integration_set_view("i-worker-1").unwrap(), None);
}

#[test]
fn the_status_lists_every_set_in_merge_order_and_whether_it_is_confirmed() {
    let f = Fixture::new();
    let core = f.core();
    assert_eq!(core.integration_sets_view(20).unwrap(), json!([]));
    let set = four_repo_set(&f);
    let sets = core.integration_sets_view(20).unwrap();
    assert_eq!(sets.as_array().unwrap().len(), 1);
    assert_eq!(sets[0]["setId"], set.as_str());
    assert_eq!(sets[0]["branch"], "int/set");
    assert_eq!(sets[0]["state"], "running");
    assert_eq!(sets[0]["confirmed"], false);
    let positions: Vec<i64> = sets[0]["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["position"].as_i64().unwrap())
        .collect();
    assert_eq!(positions, [0, 1, 2, 3]);

    // Every live member confirmed (the dropped one is skipped): that set is confirmed.
    let done = make_set(&f, &["docs", "site", "old"]);
    integration(&f, "d-1", 11, "confirmed", Some(COMMIT_A));
    integration(&f, "d-2", 12, "confirmed", Some(COMMIT_B));
    integration(&f, "d-3", 13, "discarded", None);
    attach(&f, &done, "docs", "d-1");
    attach(&f, &done, "site", "d-2");
    attach(&f, &done, "old", "d-3");
    let sets = core.integration_sets_view(20).unwrap();
    assert_eq!(sets.as_array().unwrap().len(), 2);
    // Newest first.
    assert_eq!(sets[0]["setId"], done.as_str());
    assert_eq!(sets[0]["confirmed"], true);
    assert_eq!(sets[1]["confirmed"], false);
    assert!(core.set_confirmed(&done).unwrap());
}

#[test]
fn a_member_that_has_no_integration_yet_reads_running() {
    let f = Fixture::new();
    let core = f.core();
    let set = make_set(&f, &["api", "web"]);
    integration(&f, "i-api", 1, "merged", Some(COMMIT_A));
    attach(&f, &set, "api", "i-api");
    let view = core.integration_set_view("i-api").unwrap().unwrap();
    assert_eq!(view["members"][1]["state"], "running");
    assert_eq!(view["members"][1]["integrationId"], Value::Null);
    assert_eq!(view["members"][1]["attempt"], 0);
    assert_eq!(view["members"][1]["dropped"], false);
}

// ------------------------------------------------------------------------------------------- waiting items

#[test]
fn waiting_set_members_carry_the_set_and_a_hash_over_it_and_other_items_are_untouched() {
    let f = Fixture::new();
    let core = f.core();
    let set = four_repo_set(&f);
    integration(&f, "i-alone", 9, "merged", Some(COMMIT_C));
    let items = waiting(core);
    // web (merged), worker (conflicted) and the integration of no set wait; api is confirmed and infra discarded.
    let ids: Vec<&str> = items.iter().map(|i| i.request_id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "integration:i-web",
            "integration:i-worker-2",
            "integration:i-alone"
        ]
    );

    let web = item_of(&items, "i-web");
    let wire = serde_json::to_value(web).unwrap();
    assert_eq!(
        wire["summary"],
        format!("merged int/i-web (repo web of set {set})")
    );
    assert_eq!(wire["set"]["setId"], set.as_str());
    assert_eq!(wire["set"]["repo"], "web");
    assert_eq!(wire["set"]["members"].as_array().unwrap().len(), 4);
    let expected = sha256(&capstan_kernel::areas::snapshot_feed::canonical(&json!({
        "id": "i-web", "state": "merged", "branch": "int/i-web", "headSha": COMMIT_B,
        "set": web.set.clone().unwrap(),
    })));
    assert_eq!(web.content_hash, expected);

    // The integration of no set serializes with the members it always had and the hash it always had.
    let alone = item_of(&items, "i-alone");
    let wire = serde_json::to_value(alone).unwrap();
    let keys: Vec<&str> = wire
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    for key in ["set", "commits"] {
        assert!(!keys.contains(&key), "{keys:?}");
    }
    assert_eq!(
        alone.content_hash,
        sha256(&capstan_kernel::areas::snapshot_feed::canonical(&json!({
            "id": "i-alone", "state": "merged", "branch": "int/i-alone", "headSha": COMMIT_C,
        })))
    );
}

#[test]
fn a_set_members_hash_follows_the_other_members_and_only_theirs() {
    let f = Fixture::new();
    let core = f.core();
    four_repo_set(&f);
    integration(&f, "i-alone", 9, "merged", Some(COMMIT_C));
    let before = waiting(core);
    db(&f)
        .execute("UPDATE integrations SET state = 'confirmed', head_sha = ? WHERE integration_id = 'i-web'", [COMMIT_B])
        .unwrap();
    // worker's item gained a changed member (web is confirmed now); the lone integration's hash did not move.
    let after = waiting(core);
    assert_ne!(
        item_of(&before, "i-worker-2").content_hash,
        item_of(&after, "i-worker-2").content_hash
    );
    assert_eq!(
        item_of(&before, "i-alone").content_hash,
        item_of(&after, "i-alone").content_hash
    );
}

#[test]
fn the_details_of_a_set_member_gain_the_set_and_the_results_of_every_repo() {
    let f = Fixture::new();
    let core = f.core();
    let set = four_repo_set(&f);
    for id in ["i-web", "i-alone"] {
        if id == "i-alone" {
            integration(&f, id, 9, "merged", Some(COMMIT_C));
        }
        db(&f)
            .execute(
                "INSERT INTO integration_details(project_id, integration_id, details_json, computed_at)
                 VALUES ('prustonly', ?, '{\"plans\":[],\"reports\":[],\"signoffs\":[]}', '2026-01-01T00:00:00.000Z')",
                [id],
            )
            .unwrap();
    }
    run(&f, "r-1", 1, "i-api", "api", COMMIT_A, "passed");
    run(&f, "r-2", 2, "i-web", "web", COMMIT_B, "failed");
    run(&f, "r-3", 3, "i-web", "web", COMMIT_B, "passed");
    run(&f, "r-4", 4, "i-web", "web", COMMIT_B, "failed");

    let items = waiting(core);
    let web = serde_json::to_value(item_of(&items, "i-web")).unwrap();
    let details = &web["integrationDetails"];
    assert_eq!(details["set"]["setId"], set.as_str());
    let by_repo = details["verify"]["byRepo"].as_array().unwrap();
    let repos: Vec<&str> = by_repo
        .iter()
        .map(|e| e["repo"].as_str().unwrap())
        .collect();
    assert_eq!(repos, ["api", "web", "worker", "infra"]);
    assert_eq!(by_repo[0]["latest"]["state"], "passed");
    assert_eq!(by_repo[0]["lastPassed"]["commitSha"], COMMIT_A);
    // web's latest finished run failed, and its last passed run is kept beside it.
    assert_eq!(by_repo[1]["latest"]["state"], "failed");
    assert_eq!(by_repo[1]["lastPassed"]["commitSha"], COMMIT_B);
    assert_eq!(by_repo[2]["latest"], Value::Null);
    assert_eq!(by_repo[2]["lastPassed"], Value::Null);
    // The member's own `verify` keeps its members.
    assert_eq!(details["verify"]["latest"]["state"], "failed");

    // The integration of no set has neither.
    let alone = serde_json::to_value(item_of(&items, "i-alone")).unwrap();
    assert!(alone["integrationDetails"].get("set").is_none());
    assert!(alone["integrationDetails"]["verify"].is_null());
}

// ------------------------------------------------------------------------------------------------ reports

fn worktree(core: &Core, repo: &str) {
    core.record_agent_worktree(
        &context(core, OWNER),
        &json!({"agentId": "dev-1", "generation": 1, "repo": repo, "worktreePath": format!("/cf/dev-1/{repo}"),
            "branch": "feat/x", "baseSha": BASE}),
    )
    .unwrap();
}

#[test]
fn a_report_event_names_the_commit_of_every_repo_and_one_without_commits_is_unchanged() {
    let f = Fixture::new();
    let core = f.core();
    worktree(core, "api");
    worktree(core, "web");
    let evidence = json!({"generation": 1, "branch": "feat/x", "baseSha": BASE, "commitExists": true, "isAncestorOfTip": true,
        "isAncestorOfBase": false, "checkedAt": "2026-01-01T00:00:00.000Z", "branchTip": COMMIT_A});
    let commits = json!([
        {"repo": "api", "commitSha": COMMIT_A, "branch": "feat/x", "baseSha": BASE},
        {"repo": "web", "commitSha": COMMIT_B, "branch": "feat/x", "baseSha": BASE},
    ]);
    let both = core
        .record_agent_report(
            &context(core, &f.developer_credential),
            &json!({"commitSha": COMMIT_A, "summary": "both repos", "evidence": evidence, "commits": commits}),
        )
        .unwrap();
    assert_eq!(both["record"]["state"], "accepted", "{both}");
    let page = feed_page(core, Position { seq: 0, k: 0 }, 500, None).unwrap();
    let recorded: Vec<&Value> = page
        .events
        .iter()
        .filter(|e| e["type"] == "report.recorded")
        .collect();
    assert_eq!(recorded.len(), 1);
    // commitSha stays the first commit; commits lists every repo.
    assert_eq!(recorded[0]["body"]["commitSha"], COMMIT_A);
    assert_eq!(recorded[0]["body"]["commits"], commits);

    // The status of the project reads the same list.
    let id = both["record"]["reportId"].as_str().unwrap();
    assert_eq!(core.report_commits(id).unwrap(), commits);

    // A report that named no commits has no `commits` member in its event.
    let other_evidence = json!({"generation": 1, "branch": "feat/x", "baseSha": BASE, "commitExists": true, "isAncestorOfTip": true,
        "isAncestorOfBase": false, "checkedAt": "2026-01-01T00:00:00.000Z", "branchTip": COMMIT_C});
    core.record_agent_report(
        &context(core, &f.developer_credential),
        &json!({"commitSha": COMMIT_C, "summary": "root", "evidence": other_evidence}),
    )
    .unwrap();
    let page = feed_page(core, Position { seq: 0, k: 0 }, 500, None).unwrap();
    let recorded: Vec<&Value> = page
        .events
        .iter()
        .filter(|e| e["type"] == "report.recorded")
        .collect();
    assert_eq!(recorded.len(), 2);
    assert!(recorded[1]["body"].get("commits").is_none());
}

// ------------------------------------------------------------------------------------------------ agents

#[test]
fn an_agent_lists_its_live_worktrees_and_change_folder_and_one_without_keeps_its_members() {
    let f = Fixture::new();
    let core = f.core();
    let before = core.list_agents().unwrap();
    for agent in before.as_array().unwrap() {
        assert!(agent.get("worktrees").is_none());
        assert!(agent.get("changeFolder").is_none());
    }
    worktree(core, "api");
    worktree(core, "web");
    core.remove_agent_worktree(
        &context(core, OWNER),
        &json!({"agentId": "dev-1", "repo": "web"}),
    )
    .unwrap();
    worktree(core, "worker");
    let agents = core.list_agents().unwrap();
    let dev = agents
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["agentId"] == "dev-1")
        .unwrap();
    assert_eq!(
        dev["worktrees"],
        json!([
            {"repo": "api", "path": "/cf/dev-1/api", "branch": "feat/x"},
            {"repo": "worker", "path": "/cf/dev-1/worker", "branch": "feat/x"},
        ])
    );
    assert_eq!(dev["changeFolder"], "/cf/dev-1");
    let pm = agents
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["agentId"] == "pm-1")
        .unwrap();
    assert!(pm.get("worktrees").is_none());
}

// ---------------------------------------------------------------------------------------- member verify and gate

#[test]
fn the_status_gives_each_member_its_counted_run_and_the_set_the_real_gate() {
    let f = Fixture::new();
    let core = f.core();
    let set = four_repo_set(&f);
    let any = |_: &str| String::new();
    // No run yet: the gate waits for every live member (infra is dropped and outside it).
    let view = core.set_verify_view(20, &any).unwrap();
    assert_eq!(view[0]["setId"], set.as_str());
    assert_eq!(
        view[0]["gate"],
        json!({"state": "waiting", "repos": ["api", "web", "worker"]})
    );
    assert_eq!(
        view[0]["verify"].as_array().unwrap().len(),
        3,
        "a dropped member is outside the gate"
    );
    assert_eq!(view[0]["verify"][0]["verify"], Value::Null);

    // web ran on its head (COMMIT_B) and failed; its earlier pass was on another commit.
    run(&f, "r-0", 1, "i-web", "web", COMMIT_A, "passed");
    run(&f, "r-1", 2, "i-api", "api", COMMIT_A, "passed");
    run(&f, "r-3", 3, "i-web", "web", COMMIT_B, "failed");
    // worker's head is unset, so no run counts for it.
    run(&f, "r-4", 4, "i-worker-2", "worker", COMMIT_C, "running");
    let view = core.set_verify_view(20, &any).unwrap();
    // A failed counted run beats a waiting repo.
    assert_eq!(
        view[0]["gate"],
        json!({"state": "failed", "repos": ["web"]})
    );
    let verify = view[0]["verify"].as_array().unwrap();
    assert_eq!(
        verify[0]["verify"],
        json!({"state": "passed", "commitSha": COMMIT_A, "lastPassed": null})
    );
    assert_eq!(
        verify[1]["verify"],
        json!({"state": "failed", "commitSha": COMMIT_B, "lastPassed": COMMIT_A})
    );
    assert_eq!(
        verify[2]["verify"],
        Value::Null,
        "a run on a commit that is not the member's head does not count"
    );

    // The commands of the repo count: a run under other commands is no run.
    let other = |repo: &str| {
        if repo == "api" {
            "other".to_string()
        } else {
            String::new()
        }
    };
    let view = core.set_verify_view(20, &other).unwrap();
    assert_eq!(view[0]["verify"][0]["verify"], Value::Null);

    // Every live member passed on its head: the gate passes.
    run(&f, "r-5", 5, "i-web", "web", COMMIT_B, "passed");
    // worker is redone on a merged integration.
    integration(&f, "i-worker-3", 6, "merged", Some(COMMIT_C));
    detach(&f, &set, "worker");
    attach(&f, &set, "worker", "i-worker-3");
    run(&f, "r-6", 6, "i-worker-3", "worker", COMMIT_C, "passed");
    let view = core.set_verify_view(20, &any).unwrap();
    assert_eq!(view[0]["gate"], json!({"state": "passed", "repos": []}));
}

#[test]
fn a_set_with_no_live_member_shows_no_gate() {
    let f = Fixture::new();
    let core = f.core();
    let set = make_set(&f, &["api", "web"]);
    integration(&f, "i-api", 1, "discarded", None);
    attach(&f, &set, "api", "i-api");
    let view = core.set_verify_view(20, &|_: &str| String::new()).unwrap();
    // api is dropped; web has no integration yet, so it is the one live member and the gate waits for it.
    assert_eq!(
        view[0]["gate"],
        json!({"state": "waiting", "repos": ["web"]})
    );
    let empty = make_set(&f, &["old"]);
    integration(&f, "i-old", 2, "discarded", None);
    attach(&f, &empty, "old", "i-old");
    let view = core.set_verify_view(20, &|_: &str| String::new()).unwrap();
    assert_eq!(view[0]["setId"], empty.as_str());
    assert_eq!(
        view[0]["gate"],
        Value::Null,
        "never `passed` for a set with nothing live"
    );
}

#[test]
fn a_verify_run_of_a_named_repo_carries_its_repo_and_one_of_a_single_repo_project_does_not() {
    let f = Fixture::new();
    let core = f.core();
    four_repo_set(&f);
    integration(&f, "i-alone", 9, "merged", Some(COMMIT_C));
    run(&f, "r-1", 1, "i-web", "web", COMMIT_B, "passed");
    run(&f, "r-2", 2, "i-alone", "", COMMIT_C, "passed");
    let named = core.verify_runs(Some("i-web"), 10).unwrap();
    assert_eq!(named["runs"][0]["repo"], "web");
    assert_eq!(core.verify_run("r-1").unwrap()["repo"], "web");
    let single = core.verify_run("r-2").unwrap();
    assert!(single.get("repo").is_none(), "{single}");
    assert!(core.verify_runs(Some("i-alone"), 10).unwrap()[0]
        .get("repo")
        .is_none());
}

// ------------------------------------------------------------------------------------------ single-repo ledgers

#[test]
fn a_single_repo_ledger_has_no_per_repo_key_in_its_feed_events_details_or_status_views() {
    let f = Fixture::new();
    let core = f.core();
    integration(&f, "i-1", 1, "merged", Some(COMMIT_A));
    db(&f)
        .execute(
            "INSERT INTO integration_details(project_id, integration_id, details_json, computed_at)
             VALUES ('prustonly', 'i-1', '{\"plans\":[],\"reports\":[],\"signoffs\":[]}', '2026-01-01T00:00:00.000Z')",
            [],
        )
        .unwrap();
    run(&f, "r-1", 1, "i-1", "", COMMIT_A, "passed");
    let evidence = json!({"generation": 1, "branch": "feat/x", "baseSha": BASE, "commitExists": true, "isAncestorOfTip": true,
        "isAncestorOfBase": false, "checkedAt": "2026-01-01T00:00:00.000Z", "branchTip": COMMIT_C});
    core.record_agent_report(
        &context(core, &f.developer_credential),
        &json!({"commitSha": COMMIT_C, "summary": "root", "evidence": evidence}),
    )
    .unwrap();
    let page = feed_page(core, Position { seq: 0, k: 0 }, 500, None).unwrap();
    let items = waiting(core);
    assert_eq!(items.len(), 1);
    let everything = [
        serde_json::to_string(&page.events).unwrap(),
        serde_json::to_string(&items).unwrap(),
        core.integration_sets_view(20).unwrap().to_string(),
        core.list_agents().unwrap().to_string(),
        core.verify_runs(None, 10).unwrap().to_string(),
        core.integration_set_view("i-1")
            .unwrap()
            .map_or(String::new(), |v| v.to_string()),
    ]
    .join("\n");
    for key in [
        "set",
        "commits",
        "worktrees",
        "changeFolder",
        "byRepo",
        "integrationSets",
        "gate",
        "repo",
    ] {
        assert!(
            !everything.contains(&format!("\"{key}\":")),
            "a single-repo ledger shows `{key}`: {everything}"
        );
    }
    assert_eq!(core.integration_sets_view(20).unwrap(), json!([]));
}

// ------------------------------------------------------------------------------------------------ read budget

#[test]
fn a_forced_first_set_member_may_pass_the_read_budget_by_the_set_reads_and_no_more() {
    use capstan_kernel::areas::snapshot_feed::waiting_page_counted;
    let f = Fixture::new();
    let core = f.core();
    let repos: Vec<String> = (0..32).map(|i| format!("r{i:02}")).collect();
    let names: Vec<&str> = repos.iter().map(String::as_str).collect();
    let set = make_set(&f, &names);
    for (i, repo) in names.iter().enumerate() {
        let id = format!("i-{repo}");
        integration(&f, &id, i as i64 + 1, "merged", Some(COMMIT_A));
        attach(&f, &set, repo, &id);
    }
    // A budget far below one set's reads: the call still examines (and returns) its first candidate, which reads the set
    // (2 + 1 + 32 reads) whatever is left of the budget; the next candidate waits for the next call.
    let budget = 8;
    let (page, reads) = waiting_page_counted(core, None, 100, Some("integration"), budget).unwrap();
    assert_eq!(
        page.items.len(),
        1,
        "one candidate per call under a tiny budget"
    );
    assert_eq!(
        page.items[0].set.as_ref().unwrap()["members"]
            .as_array()
            .unwrap()
            .len(),
        32
    );
    assert!(
        reads <= budget + 2 + 35 + 35,
        "reads {reads} for a budget of {budget}"
    );
}

// ---------------------------------------------------------------------------------------------- query plans

#[test]
fn the_set_statements_read_through_indexes_and_sort_only_the_members_of_one_set() {
    use capstan_kernel::areas::snapshot_feed::BY_REPO;
    use capstan_kernel::areas::workspace::SET_VIEW_SQL;
    let f = Fixture::new();
    let conn = db(&f);
    for (name, sql) in [("by repo", BY_REPO), ("set view", SET_VIEW_SQL)] {
        let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
        let plan: Vec<String> = stmt
            .query_map(["prustonly", "set-x"], |r| r.get::<_, String>(3))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        for line in &plan {
            // The only sort is the member order of one set (at most 32 rows); nothing else is sorted or scanned.
            assert!(
                !line.contains("TEMP B-TREE") || line.contains("ORDER BY"),
                "{name}: {plan:?}"
            );
            assert!(
                !(line.starts_with("SCAN ") && !line.contains("INDEX")),
                "{name} scans a table: {plan:?}"
            );
        }
    }
}

fn cleanup(
    f: &Fixture,
    id: &str,
    sequence: i64,
    state: &str,
    repo: Option<&str>,
    branches: &[(&str, Option<&str>, &str)],
) {
    let conn = db(f);
    conn.execute(
        "INSERT INTO cleanups(project_id, cleanup_id, sequence, trigger, integration_id, requested_by, mode, state, created_at, completed_at)
         VALUES ('prustonly', ?, ?, 'manual', NULL, 'pm-1', 'ask', ?, '2026-01-01T00:00:00.000Z', ?)",
        rusqlite::params![id, sequence, state, (state != "offered").then_some("2026-01-01T00:00:00.000Z")],
    )
    .unwrap();
    if let Some(repo) = repo {
        conn.execute(
            "INSERT INTO cleanup_repos(project_id, cleanup_id, repo) VALUES ('prustonly', ?, ?)",
            rusqlite::params![id, repo],
        )
        .unwrap();
    }
    for (branch, worktree, outcome) in branches {
        conn.execute(
            "INSERT INTO cleanup_branches(project_id, cleanup_id, branch, sha, class, worktree_path, outcome, decided_at)
             VALUES ('prustonly', ?, ?, ?, 'landed', ?, ?, ?)",
            rusqlite::params![
                id,
                branch,
                COMMIT_A,
                worktree,
                outcome,
                (*outcome != "offered").then_some("2026-01-01T00:00:00.000Z")
            ],
        )
        .unwrap();
    }
}

#[test]
fn the_open_cleanup_offers_of_a_workspace_list_their_repo_branches_and_worktrees() {
    let f = Fixture::new();
    let core = f.core();
    assert_eq!(core.cleanup_offers_view().unwrap(), json!([]));
    // A single-repo cleanup has no repo row: it is not listed.
    cleanup(
        &f,
        "c-root",
        1,
        "offered",
        None,
        &[("feat/a", None, "offered")],
    );
    assert_eq!(core.cleanup_offers_view().unwrap(), json!([]));
    cleanup(
        &f,
        "c-api",
        2,
        "offered",
        Some("api"),
        &[
            ("feat/b", Some("/w/b"), "offered"),
            ("feat/c", None, "offered"),
            ("feat/d", None, "deleted"),
        ],
    );
    cleanup(
        &f,
        "c-web",
        3,
        "offered",
        Some("web"),
        &[("feat/e", Some("/w/e"), "offered")],
    );
    // An applied or declined offer is no longer open.
    cleanup(
        &f,
        "c-old",
        4,
        "applied",
        Some("api"),
        &[("feat/f", None, "deleted")],
    );
    assert_eq!(
        core.cleanup_offers_view().unwrap(),
        json!([
            {"cleanupId": "c-web", "repo": "web", "branches": 1, "worktrees": 1},
            {"cleanupId": "c-api", "repo": "api", "branches": 2, "worktrees": 1},
        ])
    );
}

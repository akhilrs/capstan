//! The text of `cstan status`, `plan show --brief`, verify and cleanup for workspace data (plan-185): the lines follow the
//! approved design (`docs/design/plan-185-ws-design-strings.json`), and a project without workspace data prints what it
//! always did.

use capstan_wire::js;
use cstan_front::compact::{plan_brief, set_lines, status};
use cstan_front::render::{cleanup_lines, verify_status_lines};
use cstan_front::to_js;
use serde_json::{json, Value};

fn member(repo: &str, position: i64, state: &str, attempt: i64, dropped: bool) -> Value {
    let id = (state != "running").then(|| format!("i-{repo}"));
    json!({"repo": repo, "position": position, "attempt": attempt, "integrationId": id, "state": state,
        "branch": "int/set", "headSha": null, "dropped": dropped})
}

fn set(id: &str, state: &str, members: Vec<Value>) -> Value {
    json!({"setId": id, "state": state, "branch": "integration/plan-185", "confirmed": false, "members": members})
}

fn lines(set: &Value, ascii: bool) -> Vec<String> {
    set_lines(&to_js(set), ascii)
}

#[test]
fn a_set_is_one_line_with_a_strip_and_a_summary_in_every_state() {
    let building = set(
        "1c2d3e4f",
        "running",
        vec![
            member("api", 0, "running", 0, false),
            member("web", 1, "running", 0, false),
            member("worker", 2, "running", 0, false),
            member("infra", 3, "running", 0, false),
        ],
    );
    assert_eq!(
        lines(&building, false),
        ["set 1c2d3e4f integration/plan-185  ◌◌◌◌  building · 0 of 4 built"]
    );
    let partly = set(
        "2a7f0b19",
        "running",
        vec![
            member("api", 0, "merged", 1, false),
            member("web", 1, "merged", 1, false),
            member("worker", 2, "running", 0, false),
            member("infra", 3, "running", 0, false),
        ],
    );
    assert_eq!(
        lines(&partly, false),
        ["set 2a7f0b19 integration/plan-185  ○○◌◌  building · 2 of 4 built"]
    );
    let conflicted = set(
        "3b81e2c4",
        "running",
        vec![
            member("api", 0, "merged", 1, false),
            member("web", 1, "conflicted", 1, false),
            member("worker", 2, "running", 0, false),
            member("infra", 3, "running", 0, false),
        ],
    );
    assert_eq!(
        lines(&conflicted, false),
        [
            "set 3b81e2c4 integration/plan-185  ○▲◌◌  conflicted in web",
            "    web conflicted: cstan integrate redo 3b81e2c4 --repo web",
        ]
    );
    // A member that failed after a redo names its attempt; one that failed on its first attempt does not.
    let failed = set(
        "4c90a7d2",
        "running",
        vec![
            member("api", 0, "merged", 1, false),
            member("web", 1, "merged", 1, false),
            member("worker", 2, "failed", 2, false),
            member("infra", 3, "merged", 1, false),
        ],
    );
    assert_eq!(
        lines(&failed, false),
        [
            "set 4c90a7d2 integration/plan-185  ○○✕○  failed in worker",
            "    worker failed, attempt 2",
        ]
    );
    let first_attempt = set(
        "5e5e5e5e",
        "running",
        vec![member("api", 0, "failed", 1, false)],
    );
    assert_eq!(lines(&first_attempt, false)[1], "    api failed");
    // Every live member built: the review subject (the first live member with an integration) and the dropped repos.
    let dropped = set(
        "4b7e91c0",
        "running",
        vec![
            member("api", 0, "merged", 1, false),
            member("web", 1, "merged", 1, false),
            member("worker", 2, "merged", 1, false),
            member("infra", 3, "discarded", 1, true),
        ],
    );
    assert_eq!(
        lines(&dropped, false),
        [
            "set 4b7e91c0 integration/plan-185  ○○○–  built · none in main yet · next: api",
            "    review subject: api · dropped: infra",
        ]
    );
    let discarded = set(
        "5d13f6a8",
        "discarded",
        vec![
            member("api", 0, "discarded", 1, false),
            member("web", 1, "discarded", 1, false),
            member("worker", 2, "discarded", 1, false),
            member("infra", 3, "discarded", 1, false),
        ],
    );
    assert_eq!(
        lines(&discarded, false),
        ["set 5d13f6a8 integration/plan-185  ––––  discarded · nothing reached main"]
    );
    let in_main = set(
        "7f35b9c1",
        "merged",
        (0..4)
            .map(|i| {
                member(
                    ["api", "web", "worker", "infra"][i],
                    i as i64,
                    "confirmed",
                    1,
                    false,
                )
            })
            .collect(),
    );
    assert_eq!(
        lines(&in_main, false)[0],
        "set 7f35b9c1 integration/plan-185  ●●●●  in main · all 4 repos"
    );
    // Some in main: the next repo to merge is named.
    let decide = set(
        "8a46cad2",
        "merged",
        vec![
            member("api", 0, "confirmed", 1, false),
            member("web", 1, "merged", 1, false),
            member("worker", 2, "merged", 1, false),
            member("infra", 3, "discarded", 1, true),
        ],
    );
    assert_eq!(
        lines(&decide, false),
        [
            "set 8a46cad2 integration/plan-185  ●○○–  1 of 3 in main · next: web",
            "    review subject: api · dropped: infra",
        ]
    );
}

#[test]
fn a_strip_shows_twelve_glyphs_then_plus_n_and_ascii_replaces_the_glyphs() {
    let names: Vec<String> = (0..32).map(|i| format!("repo{i:02}")).collect();
    let members: Vec<Value> = names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let state = match i {
                0..=4 => "confirmed",
                5..=8 => "merged",
                _ => "running",
            };
            member(name, i as i64, state, 1, false)
        })
        .collect();
    let line = lines(&set("9a01c3d7", "running", members), false);
    assert_eq!(
        line,
        ["set 9a01c3d7 integration/plan-185  ●●●●●○○○○◌◌◌ +20  5 of 32 in main · next: repo05"]
    );
    // A next repo beyond the first glyphs starts the strip two before it.
    let late: Vec<Value> = (0..32)
        .map(|i| {
            member(
                &format!("repo{i:02}"),
                i,
                if i < 20 { "confirmed" } else { "running" },
                1,
                false,
            )
        })
        .collect();
    let late = lines(&set("9a01c3d8", "running", late), false);
    assert!(late[0].contains("+18 ●●◌◌◌◌◌◌◌◌◌◌ +2"), "{late:?}");
    // TERM=dumb: ASCII letters carry the states.
    let ascii = set(
        "8a46cad2",
        "merged",
        vec![
            member("api", 0, "confirmed", 1, false),
            member("web", 1, "merged", 1, false),
            member("worker", 2, "merged", 1, false),
            member("infra", 3, "discarded", 1, true),
        ],
    );
    assert_eq!(
        lines(&ascii, true)[0],
        "set 8a46cad2 integration/plan-185  #oo-  1 of 3 in main · next: web"
    );
    // A branch longer than 32 characters is cut in the middle in the set line.
    let mut long = set("1", "running", vec![member("api", 0, "running", 0, false)]);
    long["branch"] = json!("integration/plan-185-a-very-long-branch-name-indeed");
    assert!(
        lines(&long, false)[0].contains("integration/pla…anch-name-indeed"),
        "{:?}",
        lines(&long, false)
    );
}

fn text_of(result: &Value, all: bool) -> String {
    js::render(&status(&to_js(result), all, 0.0)).to_utf8_lossy()
}

#[test]
fn status_prints_the_sets_as_lines_and_a_multi_repo_report_as_one_line() {
    let result = json!({
        "integrationSets": [set("8a46cad2", "merged", vec![
            member("api", 0, "confirmed", 1, false), member("web", 1, "merged", 1, false)])],
        "integrations": [{"integrationId": "i-web", "state": "merged", "set": {"setId": "8a46cad2", "members": []}}],
        "reports": [
            {"reportId": "7f3e2a1c", "agentId": "developer-4", "state": "accepted", "commitSha": "9f3c2a1d5b7e",
             "commits": [
                {"repo": "api", "commitSha": "9f3c2a1d5b7e4a60918c2d4e6f8091a2b3c4d5e6", "branch": "feat/PM-512-refund-sync", "baseSha": "x"},
                {"repo": "web", "commitSha": "51ab07e3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9", "branch": "feat/PM-512-refund-sync", "baseSha": "y"}]},
            {"reportId": "9abf487a", "agentId": "developer-1", "state": "accepted", "commitSha": "abc"},
        ],
        "agents": [{"agentId": "developer-4", "kind": "Developer", "state": "active", "generation": 1,
            "lastActivityAt": "t", "worktrees": [{"repo": "api"}, {"repo": "web"}, {"repo": "worker"}]}],
    });
    let text = text_of(&result, true);
    assert!(
        text.contains("set 8a46cad2 integration/plan-185  ●○  1 of 2 in main · next: web"),
        "{text}"
    );
    assert!(
        text.contains("7f3e2a1c developer-4 accepted 2 repos  api 9f3c2a1 · web 51ab07e  on feat/PM-512-refund-sync"),
        "{text}"
    );
    // The member's row drops its set (the set line carries it); the single-repo report is shown as it was.
    assert!(!text.contains("members"), "{text}");
    assert!(text.contains("abc"), "{text}");
    assert!(text.contains("· 3 repos"), "{text}");
}

#[test]
fn status_without_workspace_members_prints_what_it_printed() {
    let result = json!({
        "integrations": [{"integrationId": "7c1e0f2a", "state": "running"}],
        "reports": [{"reportId": "9abf487a", "agentId": "developer-1", "state": "accepted", "commitSha": "abc"}],
        "agents": [{"agentId": "developer-1", "kind": "Developer", "state": "active", "generation": 1, "lastActivityAt": "t"}],
    });
    let text = text_of(&result, true);
    assert!(
        !text.contains("repos") && !text.contains("integrationSets"),
        "{text}"
    );
    assert!(
        text.contains("developer-1 Developer active gen 1 last t"),
        "{text}"
    );
    assert!(text.contains("integrationId: 7c1e0f2a"), "{text}");
}

#[test]
fn plan_show_brief_names_the_repos_of_a_package() {
    let result = json!({
        "plan": {"planId": "plan-185", "tier": "normal", "state": "approved", "title": "Workspace mode"},
        "packages": [
            {"packageId": "ws-design", "type": "docs", "estimateHours": 6, "dependsOn": [], "assignee": "designer-18", "progress": "working"},
            {"packageId": "ws-dash", "type": "feat", "estimateHours": 6, "dependsOn": ["ws-design", "ws-feed"], "assignee": null,
             "progress": "pending", "repos": ["capstan", "capstan-dash-fixtures"]},
        ],
    });
    let lines = plan_brief(&to_js(&result)).unwrap();
    assert_eq!(lines[1], "  ws-design docs 6h deps - designer-18 working");
    assert_eq!(
        lines[2],
        "  ws-dash feat 6h deps ws-design,ws-feed - pending repos capstan,capstan-dash-fixtures"
    );
}

#[test]
fn verify_and_cleanup_lines_name_the_repo() {
    let status = json!({"verify": {"active": [
        {"runId": "run-1", "state": "running", "subjectKind": "integration", "subjectId": "i-web", "repo": "web",
         "commitSha": "51ab07e3c4d5", "startedAt": "2026-01-01T00:00:00.000Z"},
        {"runId": "run-2", "state": "queued", "subjectKind": "integration", "subjectId": "i-1",
         "commitSha": "9f3c2a1d5b7e", "queuedAt": "2026-01-01T00:00:00.000Z"}], "recent": []}});
    let lines = verify_status_lines(Some(&to_js(&status)), 1_767_225_660_000.0);
    assert!(
        lines[1].starts_with("  run-1 running integration i-web repo web @51ab07e"),
        "{lines:?}"
    );
    // A run of a project without repos reads as it did.
    assert!(
        lines[2].starts_with("  run-2 queued integration i-1 @9f3c2a1"),
        "{lines:?}"
    );

    let offers = json!({"cleanups": [
        {"cleanupId": "3e9a71b0", "repo": "api", "candidates": 4, "worktrees": 1},
        {"cleanupId": "5c22d8f1", "repo": "web", "candidates": 2, "worktrees": 0},
        {"cleanupId": "77ab3c09", "repo": "old", "candidates": 1, "note": "the repo left the configuration"},
    ]});
    let lines = cleanup_lines(Some(&to_js(&offers)));
    assert_eq!(lines[0], "cleanup 3e9a71b0 · api: branches 4, worktrees 1");
    assert_eq!(lines[1], "cleanup 5c22d8f1 · web: branches 2, worktrees 0");
    // An answer that does not count worktrees keeps its shipped line.
    assert_eq!(lines[2], "77ab3c09 repo old: 1 candidates");
    assert_eq!(lines[3], "  the repo left the configuration");
}

#[test]
fn a_set_with_a_gate_prints_the_gate_line_and_the_per_repo_verify_line() {
    let mut web = member("web", 1, "merged", 1, false);
    web["verify"] = json!({"state": "failed", "commitSha": "c1", "lastPassed": "51ab07e3c4d5e6f7"});
    let mut api = member("api", 0, "confirmed", 1, false);
    api["verify"] = json!({"state": "passed", "commitSha": "a1", "lastPassed": "a1"});
    let mut worker = member("worker", 2, "running", 1, false);
    worker["verify"] = Value::Null;
    let infra = member("infra", 3, "discarded", 1, true);
    let mut gated = set("8a46cad2", "running", vec![api, web, worker, infra]);
    gated["gate"] = json!({"state": "failed", "repos": ["web"]});
    let out = lines(&gated, true);
    let verify: Vec<&String> = out.iter().filter(|l| l.contains("verify")).collect();
    assert_eq!(
        verify[0], "set 8a46cad2 verify  gate failed: web",
        "{out:?}"
    );
    assert_eq!(
        out.last().unwrap(),
        "    api passed · web failed · last passed 51ab07e · worker not run · infra outside the gate",
        "{out:?}"
    );
    gated["gate"] = json!({"state": "waiting", "repos": ["worker"]});
    assert!(lines(&gated, true)
        .iter()
        .any(|l| l == "set 8a46cad2 verify  gate waiting: worker"));
    gated["gate"] = json!({"state": "passed", "repos": []});
    assert!(lines(&gated, true)
        .iter()
        .any(|l| l == "set 8a46cad2 verify  gate passed: every repo's latest run passed"));
    // A host that sends no gate prints no gate line.
    let bare = set(
        "8a46cad2",
        "running",
        vec![member("api", 0, "merged", 1, false)],
    );
    assert!(lines(&bare, true).iter().all(|l| !l.contains("verify")));
}

#[test]
fn a_cleanup_of_a_repo_that_left_the_configuration_says_so() {
    let offers = json!({"cleanups": [
        {"cleanupId": "77ab3c09", "repo": "old", "candidates": 1, "worktrees": 1, "left": true, "note": "the repo left the configuration"},
    ]});
    let lines = cleanup_lines(Some(&to_js(&offers)));
    assert_eq!(lines[0], "cleanup 77ab3c09 · old: branches 1, worktrees 1");
    assert_eq!(
        lines[1],
        "  old left the configuration: worktrees only, apply them by name"
    );
    assert_eq!(lines[2], "  the repo left the configuration");
}

#[test]
fn the_gate_line_skips_a_last_pass_on_the_same_commit_and_names_dropped_repos_past_the_cap() {
    let mut web = member("web", 0, "merged", 1, false);
    web["verify"] =
        json!({"state": "failed", "commitSha": "51ab07e3c4", "lastPassed": "51ab07e3c4"});
    let mut members = vec![web];
    for i in 1..14 {
        let mut m = member(&format!("r{i:02}"), i, "merged", 1, false);
        m["verify"] = json!({"state": "passed", "commitSha": "a1", "lastPassed": null});
        members.push(m);
    }
    members.push(member("old", 14, "discarded", 1, true));
    let mut gated = set("8a46cad2", "running", members);
    gated["gate"] = json!({"state": "failed", "repos": ["web"]});
    let out = lines(&gated, true);
    let last = out.last().unwrap();
    assert!(last.starts_with("    web failed · r01 passed"), "{last}");
    assert!(!last.contains("last passed"), "{last}");
    assert!(last.ends_with(" · +3 · old outside the gate"), "{last}");
}
